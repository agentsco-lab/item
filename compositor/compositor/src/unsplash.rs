//! Wallpapers from Unsplash, for whoever brought a key: with `unsplash` in
//! the keyring (item/grid's Keys, or `item-secret set unsplash`), the
//! picker shows "Shuffle". Without one nothing here runs and nothing is
//! asked of the network: the package's pictures are the set.
//!
//! Shuffle asks Unsplash's "Wallpapers" topic for a page of photos in the
//! picker's shape (wide for one picture on both panels, tall for one on
//! each), keeps the dark ones by the colour Unsplash gives each photo
//! (nothing bright is even fetched), never one shown before, twelve at
//! most, and fetches their small pictures for the strip, after the set's
//! own. A tap on one fetches it at the Duo's size into the owner's set
//! (`~/.local/share/item/walls/unsplash-ID.jpg`), its credit beside it
//! (`unsplash-ID.txt`: "Photo by NAME on Unsplash" and the links), tells
//! Unsplash it was downloaded (their API's rule), and puts it on. The
//! pictures not chosen live in memory only.
//!
//! The key goes to curl in a configuration file of the user's alone (0600,
//! in the runtime directory, gone once used): never on a command line.
//! Only the API's requests carry it; the pictures come from Unsplash's
//! image host without it.

use std::collections::HashSet;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
use smithay::reexports::calloop::ping::Ping;
use smithay::utils::Transform;

/// The key's name in the keyring.
const KEY: &str = "unsplash";
/// Photos a shuffle shows.
const BATCH: usize = 12;
/// The darkest average colour kept (its luma, 0..1): the set is dark, as
/// item's own pictures are.
const DARK: f64 = 0.38;
/// A small picture's size (walls.rs's THUMB_PX).
const THUMB: (u32, u32) = (360, 202);
/// The longer side fetched for the wallpaper (the Duo's book is 2784 px
/// wide; more leaves the zoom some room).
const LONG_SIDE: u32 = 3600;
const API: &str = "https://api.unsplash.com";
/// How the app names itself in Unsplash's links (their attribution rule).
const UTM: &str = "utm_source=item&utm_medium=referral";

#[derive(Clone)]
struct Photo {
    id: String,
    author: String,
    author_url: String,
    page_url: String,
    download_location: String,
    raw: String,
}

#[derive(Default)]
struct Shared {
    /// The photos shown, and their small pictures as read (RGBA).
    photos: Vec<Photo>,
    thumbs: Vec<Option<(Vec<u8>, u32, u32)>>,
    /// Every photo shown since the picker first shuffled: not again.
    seen: HashSet<String>,
    shuffling: bool,
    /// A photo being fetched; the wallpaper's name it became, or why not.
    picking: bool,
    picked: Option<Result<String, String>>,
    error: Option<String>,
    changed: bool,
}

pub struct Gallery {
    /// Whether the keyring has the key (asked as the picker opens).
    key: Arc<std::sync::atomic::AtomicBool>,
    shared: Arc<Mutex<Shared>>,
    /// The small pictures made buffers, in the strip's order after the set.
    pub thumbs: Vec<Option<MemoryRenderBuffer>>,
    wake: Ping,
}

impl Gallery {
    pub fn new(wake: Ping) -> Gallery {
        Gallery { key: Default::default(), shared: Default::default(), thumbs: Vec::new(), wake }
    }

    /// The keyring asked whether the key is there (on a thread: the
    /// keyring is a bus call away), as the picker opens.
    pub fn check_key(&self) {
        let (key, wake) = (self.key.clone(), self.wake.clone());
        std::thread::spawn(move || {
            let has = matches!(crate::secrets::get(KEY), Ok(Some(k)) if !k.trim().is_empty());
            if key.swap(has, std::sync::atomic::Ordering::Relaxed) != has {
                wake.ping();
            }
        });
    }

    /// Shuffle offered: the key is there.
    pub fn available(&self) -> bool {
        self.key.load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn busy(&self) -> bool {
        let s = self.shared.lock().unwrap();
        s.shuffling || s.picking
    }

    /// The photos in the strip now.
    pub fn len(&self) -> usize {
        self.thumbs.len()
    }

    /// What went wrong last, once.
    pub fn take_error(&self) -> Option<String> {
        self.shared.lock().unwrap().error.take()
    }

    /// The photos gone from the strip (the mode changed: another shape).
    pub fn clear(&mut self) {
        let mut s = self.shared.lock().unwrap();
        s.photos.clear();
        s.thumbs.clear();
        self.thumbs.clear();
    }

    /// A new batch, in the shape `tall` (one picture on each panel) or wide.
    pub fn shuffle(&mut self, tall: bool) {
        {
            let mut s = self.shared.lock().unwrap();
            if s.shuffling || s.picking {
                return;
            }
            s.shuffling = true;
        }
        let (shared, wake) = (self.shared.clone(), self.wake.clone());
        std::thread::spawn(move || {
            let seen = shared.lock().unwrap().seen.clone();
            let got = batch(tall, &seen);
            let mut s = shared.lock().unwrap();
            s.shuffling = false;
            match got {
                Ok(photos) => {
                    tracing::info!("unsplash: {} photos shuffled in", photos.len());
                    s.seen.extend(photos.iter().map(|p| p.id.clone()));
                    s.thumbs = vec![None; photos.len()];
                    s.photos = photos.clone();
                    s.changed = true;
                    drop(s);
                    wake.ping();
                    // Their small pictures, each on the strip as it comes.
                    for (i, p) in photos.iter().enumerate() {
                        let url = format!("{}&w={}&h={}&fit=crop&fm=jpg&q=75", p.raw, THUMB.0, THUMB.1);
                        let small = fetch(&url, None).ok().and_then(|b| crate::walls::decode_bytes(&b));
                        let mut s = shared.lock().unwrap();
                        if s.photos.get(i).is_some_and(|q| q.id == p.id) {
                            s.thumbs[i] = small.map(|(mut px, w, h)| {
                                crate::walls::round_corners(&mut px, w, h, crate::walls::THUMB_RADIUS);
                                (px, w, h)
                            });
                            s.changed = true;
                        }
                        drop(s);
                        wake.ping();
                    }
                }
                Err(e) => {
                    tracing::warn!("unsplash: {e}");
                    s.error = Some(e);
                    s.changed = true;
                    drop(s);
                    wake.ping();
                }
            }
        });
    }

    /// The `i`th photo of the strip's batch fetched into the owner's set.
    pub fn pick(&self, i: usize, tall: bool) {
        let photo = {
            let mut s = self.shared.lock().unwrap();
            if s.picking {
                return;
            }
            let Some(p) = s.photos.get(i).cloned() else { return };
            s.picking = true;
            p
        };
        let (shared, wake) = (self.shared.clone(), self.wake.clone());
        std::thread::spawn(move || {
            let r = keep(&photo, tall);
            let mut s = shared.lock().unwrap();
            s.picking = false;
            s.picked = Some(r);
            s.changed = true;
            drop(s);
            wake.ping();
        });
    }

    /// The wallpaper a pick became (its name in the set), or why not, once.
    pub fn take_picked(&self) -> Option<Result<String, String>> {
        self.shared.lock().unwrap().picked.take()
    }

    /// What the threads brought, made buffers: whether anything changed.
    pub fn take(&mut self) -> bool {
        let mut s = self.shared.lock().unwrap();
        if !std::mem::take(&mut s.changed) {
            return false;
        }
        self.thumbs = s.thumbs.iter().map(|t| t.as_ref().map(|(px, w, h)| MemoryRenderBuffer::from_slice(px, Fourcc::Abgr8888, (*w as i32, *h as i32), 2, Transform::Normal, None))).collect();
        true
    }
}

/// A batch: a random page of the topic in the shape asked, the dark ones
/// not seen before, BATCH at most (another page or two if one gives too
/// few).
fn batch(tall: bool, seen: &HashSet<String>) -> Result<Vec<Photo>, String> {
    let orientation = if tall { "portrait" } else { "landscape" };
    let mut out: Vec<Photo> = Vec::new();
    let mut ids: HashSet<String> = HashSet::new();
    for _ in 0..3 {
        let page = 1 + std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.subsec_nanos() % 40);
        let url = format!("{API}/topics/wallpapers/photos?per_page=30&page={page}&orientation={orientation}");
        let body = fetch(&url, Some(&key()?))?;
        let list: serde_json::Value = serde_json::from_slice(&body).map_err(|e| format!("Unsplash's answer: {e}"))?;
        let Some(list) = list.as_array() else {
            return Err(format!("Unsplash said: {}", list["errors"].get(0).and_then(|e| e.as_str()).unwrap_or("no photos")));
        };
        for p in list {
            let id = p["id"].as_str().unwrap_or_default().to_owned();
            if id.is_empty() || seen.contains(&id) || ids.contains(&id) || luma(p["color"].as_str().unwrap_or("#ffffff")) > DARK {
                continue;
            }
            let s = |v: &serde_json::Value| v.as_str().unwrap_or_default().to_owned();
            ids.insert(id.clone());
            out.push(Photo {
                id,
                author: s(&p["user"]["name"]),
                author_url: s(&p["user"]["links"]["html"]),
                page_url: s(&p["links"]["html"]),
                download_location: s(&p["links"]["download_location"]),
                raw: s(&p["urls"]["raw"]),
            });
            if out.len() == BATCH {
                return Ok(out);
            }
        }
    }
    if out.is_empty() {
        return Err("no dark photos on these pages".into());
    }
    Ok(out)
}

/// A photo fetched at the Duo's size into the owner's set with its credit,
/// Unsplash told: the wallpaper's name.
fn keep(p: &Photo, tall: bool) -> Result<String, String> {
    if !p.id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') || !p.raw.starts_with("https://images.unsplash.com/") {
        return Err("not an Unsplash photo".into());
    }
    let side = if tall { "h" } else { "w" };
    let jpg = fetch(&format!("{}&{side}={LONG_SIDE}&fm=jpg&q=85", p.raw), None)?;
    if crate::walls::decode_bytes(&jpg).is_none() {
        return Err("the photo did not arrive whole".into());
    }
    let dir = std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".local/share/item/walls");
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let name = format!("unsplash-{}.jpg", p.id);
    let credit = format!("Photo by {} on Unsplash\n{}?{UTM}\n{}?{UTM}\n", p.author, p.author_url, p.page_url);
    std::fs::write(dir.join(format!("unsplash-{}.txt", p.id)), credit).map_err(|e| e.to_string())?;
    // The picture last: the set (walls.rs) sees it with its credit there.
    let tmp = dir.join(format!(".{name}.new"));
    std::fs::write(&tmp, &jpg).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, dir.join(&name)).map_err(|e| e.to_string())?;
    // Their rule: a download counted when a photo is used.
    if p.download_location.starts_with(API) {
        if let Ok(k) = key() {
            let _ = fetch(&p.download_location, Some(&k));
        }
    }
    tracing::info!("unsplash: {name} kept, by {}", p.author);
    Ok(name)
}

fn key() -> Result<String, String> {
    crate::secrets::get(KEY)?.filter(|k| !k.trim().is_empty()).map(|k| k.trim().to_owned()).ok_or_else(|| format!("no key: item-secret set {KEY}"))
}

/// A GET over https with curl; with `key`, Unsplash's API's header, the key
/// in curl's configuration file of the user's alone (0600), never on its
/// command line.
fn fetch(url: &str, key: Option<&str>) -> Result<Vec<u8>, String> {
    if !url.starts_with("https://") {
        return Err(format!("{url}: https only"));
    }
    let mut c = Command::new("curl");
    c.args(["-sS", "--fail", "-L", "-m", "60", "--proto", "=https"]);
    let rc = match key {
        Some(k) => {
            use std::io::Write;
            use std::os::unix::fs::OpenOptionsExt;
            let dir = std::env::var("XDG_RUNTIME_DIR").map_err(|_| "no XDG_RUNTIME_DIR".to_owned())?;
            let rc = format!("{dir}/item-unsplash-{}-{:?}.curlrc", std::process::id(), std::thread::current().id()).replace(['(', ')'], "");
            std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(&rc)
                .and_then(|mut f| f.write_all(format!("header = \"Authorization: Client-ID {k}\"\nheader = \"Accept-Version: v1\"\n").as_bytes()))
                .map_err(|e| format!("{rc}: {e}"))?;
            c.args(["-K", &rc]);
            Some(rc)
        }
        None => None,
    };
    let out = c.arg(url).stdin(Stdio::null()).output();
    if let Some(rc) = rc {
        let _ = std::fs::remove_file(rc);
    }
    let out = out.map_err(|e| format!("curl: {e}"))?;
    if !out.status.success() {
        let e = String::from_utf8_lossy(&out.stderr);
        return Err(format!("Unsplash did not answer ({})", e.trim().lines().last().unwrap_or("no reason")));
    }
    Ok(out.stdout)
}

/// An "#rrggbb"'s luma, 0..1.
fn luma(hex: &str) -> f64 {
    let v = u32::from_str_radix(hex.trim_start_matches('#'), 16).unwrap_or(0xffffff);
    let (r, g, b) = ((v >> 16 & 0xff) as f64, (v >> 8 & 0xff) as f64, (v & 0xff) as f64);
    (0.2126 * r + 0.7152 * g + 0.0722 * b) / 255.0
}
