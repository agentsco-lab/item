//! Wallpapers from Unsplash, for whoever brought a key: with `unsplash` in
//! the keyring (item/grid's Keys, or `item-secret set unsplash`), the
//! picker shows "Shuffle". Without one nothing here runs and nothing is
//! asked of the network: the package's pictures are the set.
//!
//! Shuffle searches Unsplash for one of a few dark themes (THEMES: night
//! skies, mountains, cities at night, fog...) in the picker's shape (wide
//! for one picture on both panels, tall for one on each), black in
//! Unsplash's colour filter; each photo not shown before is fetched small
//! and kept only if its own pixels are dark (`dark`), twelve at most, each
//! on the strip as it passes, in Unsplash's own group after the set. A tap on one fetches it at the Duo's size into the owner's set
//! (`~/.local/share/item/walls/unsplash-ID.jpg`), its credit beside it
//! (`unsplash-ID.txt`: "Photo by NAME on Unsplash" and the links), tells
//! Unsplash it was downloaded (their API's rule), and puts it on. The
//! pictures not chosen live in memory only.
//!
//! The key goes in the API's requests' header alone (net.rs: no file, no
//! command line, no child process); the pictures come from Unsplash's
//! image host without it.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
use smithay::reexports::calloop::ping::Ping;
use smithay::utils::Transform;

/// The key's name in the keyring.
const KEY: &str = "unsplash";
/// Photos a shuffle shows.
const BATCH: usize = 12;
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

    /// What the group says in place of photos (no connection, the limit),
    /// until the next shuffle.
    pub fn note(&self) -> Option<String> {
        self.shared.lock().unwrap().error.clone()
    }

    /// The photos gone from the strip (the mode changed: another shape).
    pub fn clear(&mut self) {
        let mut s = self.shared.lock().unwrap();
        s.photos.clear();
        s.thumbs.clear();
        self.thumbs.clear();
    }

    /// A new batch, in the shape `tall` (one picture on each panel) or wide:
    /// its places shown at once, each filled as its photo passes (dark by
    /// its own pixels, not seen before), BATCH at most.
    pub fn shuffle(&mut self, tall: bool) {
        {
            let mut s = self.shared.lock().unwrap();
            if s.shuffling || s.picking {
                return;
            }
            s.shuffling = true;
            s.error = None;
            s.photos.clear();
            s.thumbs = vec![None; BATCH];
            s.changed = true;
        }
        self.wake.ping();
        let (shared, wake) = (self.shared.clone(), self.wake.clone());
        std::thread::spawn(move || {
            let seen = shared.lock().unwrap().seen.clone();
            let r = batch(tall, &seen, &mut |photo, thumb| {
                let mut s = shared.lock().unwrap();
                let at = s.photos.len();
                s.seen.insert(photo.id.clone());
                s.photos.push(photo);
                if at < s.thumbs.len() {
                    s.thumbs[at] = Some(thumb);
                }
                s.changed = true;
                drop(s);
                wake.ping();
            });
            let mut s = shared.lock().unwrap();
            s.shuffling = false;
            // The places no photo filled go.
            let n = s.photos.len();
            s.thumbs.truncate(n);
            match r {
                Ok(()) if n > 0 => tracing::info!("unsplash: {n} photos shuffled in"),
                Ok(()) => s.error = Some("no dark photos this time: shuffle again".into()),
                Err(e) => {
                    tracing::warn!("unsplash: {e}");
                    s.error = Some(e);
                }
            }
            s.changed = true;
            drop(s);
            wake.ping();
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

/// What a batch is searched for, one theme a shuffle in turn: dark
/// scenes without people, animals, food, devices or lettering in focus (as
/// the owner chose by hand for coddy-agent's gallery).
const THEMES: [&str; 10] = ["night sky stars", "dark mountains", "city at night", "foggy forest", "dark abstract", "desert night", "ocean at night", "aurora", "dark minimal architecture", "moon landscape"];

/// A batch: a theme's search, black in Unsplash's colour filter, in the
/// shape asked; each photo not seen before fetched small and kept if its
/// own pixels are dark (`dark`), told to `keep` as it passes, BATCH at
/// most, three pages at most.
fn batch(tall: bool, seen: &HashSet<String>, keep: &mut dyn FnMut(Photo, (Vec<u8>, u32, u32))) -> Result<(), String> {
    let orientation = if tall { "portrait" } else { "landscape" };
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.subsec_nanos() as usize);
    let theme = THEMES[nanos % THEMES.len()];
    let key = key()?;
    let mut kept = 0;
    let mut ids: HashSet<String> = HashSet::new();
    for page in 1..=3 {
        let page = page + nanos % 3;
        let url = format!("{API}/search/photos?query={}&color=black&orientation={orientation}&content_filter=high&per_page=30&page={page}", theme.replace(' ', "+"));
        let body = fetch(&url, Some(&key))?;
        let v: serde_json::Value = serde_json::from_slice(&body).map_err(|e| format!("Unsplash's answer: {e}"))?;
        let Some(list) = v["results"].as_array() else {
            return Err(format!("Unsplash said: {}", v["errors"].get(0).and_then(|e| e.as_str()).unwrap_or("no photos")));
        };
        if list.is_empty() {
            break;
        }
        for p in list {
            let id = p["id"].as_str().unwrap_or_default().to_owned();
            if id.is_empty() || seen.contains(&id) || ids.contains(&id) || luma(p["color"].as_str().unwrap_or("#ffffff")) > 0.45 {
                continue;
            }
            ids.insert(id.clone());
            let s = |v: &serde_json::Value| v.as_str().unwrap_or_default().to_owned();
            let photo = Photo {
                id,
                author: s(&p["user"]["name"]),
                author_url: s(&p["user"]["links"]["html"]),
                page_url: s(&p["links"]["html"]),
                download_location: s(&p["links"]["download_location"]),
                raw: s(&p["urls"]["raw"]),
            };
            let (tw, th) = if tall { crate::walls::THUMB_TALL_PX } else { THUMB };
            let url = format!("{}&w={tw}&h={th}&fit=crop&fm=jpg&q=75", photo.raw);
            let Some((mut px, w, h)) = fetch(&url, None).ok().and_then(|b| crate::walls::decode_bytes(&b)) else { continue };
            if !dark(&px) {
                continue;
            }
            crate::walls::round_corners(&mut px, w, h, crate::walls::THUMB_RADIUS);
            keep(photo, (px, w, h));
            kept += 1;
            if kept == BATCH {
                return Ok(());
            }
        }
    }
    Ok(())
}

/// Whether a small picture (RGBA) is dark: its mean luma low, and little of
/// it bright (a dark photo with a bright shop window is not).
fn dark(px: &[u8]) -> bool {
    let (mut sum, mut bright, mut n) = (0.0, 0usize, 0usize);
    for c in px.chunks_exact(4).step_by(3) {
        let l = (0.2126 * c[0] as f64 + 0.7152 * c[1] as f64 + 0.0722 * c[2] as f64) / 255.0;
        sum += l;
        if l > 0.72 {
            bright += 1;
        }
        n += 1;
    }
    n > 0 && sum / (n as f64) < 0.26 && (bright as f64) < 0.05 * n as f64
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

/// A GET over https (net.rs); with `key`, Unsplash's API's header - the
/// key in the request alone, never in a file or on a command line.
fn fetch(url: &str, key: Option<&str>) -> Result<Vec<u8>, String> {
    let auth = key.map(|k| format!("Client-ID {k}"));
    let mut headers: Vec<(&str, &str)> = Vec::new();
    if let Some(a) = &auth {
        headers.push(("Authorization", a));
        headers.push(("Accept-Version", "v1"));
    }
    let a = crate::net::get(url, &headers, std::time::Duration::from_secs(60)).map_err(|e| format!("Unsplash did not answer ({e})"))?;
    if !a.ok() {
        return Err(format!("Unsplash did not answer ({})", a.status));
    }
    Ok(a.body)
}

/// An "#rrggbb"'s luma, 0..1.
fn luma(hex: &str) -> f64 {
    let v = u32::from_str_radix(hex.trim_start_matches('#'), 16).unwrap_or(0xffffff);
    let (r, g, b) = ((v >> 16 & 0xff) as f64, (v >> 8 & 0xff) as f64, (v & 0xff) as f64);
    (0.2126 * r + 0.7152 * g + 0.0722 * b) / 255.0
}
