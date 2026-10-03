//! The wallpapers: item's set of pictures, and which one is on.
//!
//! The set is `~/.local/share/item/walls/*.jpg` (and later the package's
//! `/usr/share/item/walls`), each already the screen's size and the
//! parallax's room more (3184 x 1800 px: wallpaper.glsl, output.rs), with a
//! small picture beside it for choosing (`thumb-NAME.jpg`). Before them
//! comes Aurora, the wallpaper drawn by the compositor itself (its small
//! picture `thumb-aurora`).
//!
//! The one on is kept in `~/.config/item/wallpaper` (its name). A picture is
//! decoded on a thread (some 150 ms) and handed to the loop, which sends it
//! to the GPU and fades it in over the one before (output.rs).

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
use smithay::reexports::calloop::ping::Ping;
use smithay::utils::Transform;

/// The wallpaper the compositor draws itself.
pub const AURORA: &str = "aurora";

/// A picture decoded: its name, its RGBA rows, its size.
pub type Picture = (String, Vec<u8>, u32, u32);

pub struct Walls {
    dir: PathBuf,
    /// The set: Aurora, then the pictures by name.
    pub names: Vec<String>,
    pub current: String,
    loaded: Arc<Mutex<Option<Picture>>>,
    /// The picture frosted (`frost`): Some(None) for Aurora, which has none.
    frosted: Arc<Mutex<Option<Option<(Vec<u8>, u32, u32)>>>>,
    thumbs_loaded: Arc<Mutex<Option<Vec<(usize, Vec<u8>, u32, u32)>>>>,
    /// The small pictures, once read (the picker asks for them).
    pub thumbs: Vec<Option<MemoryRenderBuffer>>,
    thumbs_asked: bool,
    wake: Ping,
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_default())
}

fn kept() -> PathBuf {
    home().join(".config/item/wallpaper")
}

/// A small picture's corners, px of it.
const THUMB_RADIUS: f64 = 26.0;

/// Rounds an RGBA picture's corners (premultiplied, as the renderer takes
/// it: the colour goes with the alpha).
fn round_corners(rgba: &mut [u8], w: u32, h: u32, r: f64) {
    let (w, h) = (w as usize, h as usize);
    for y in 0..h {
        for x in 0..w {
            let (fx, fy) = (x as f64 + 0.5, y as f64 + 0.5);
            let cx = fx.clamp(r, w as f64 - r);
            let cy = fy.clamp(r, h as f64 - r);
            let d = ((fx - cx).powi(2) + (fy - cy).powi(2)).sqrt();
            let a = (r + 0.5 - d).clamp(0.0, 1.0);
            if a < 1.0 {
                let i = (y * w + x) * 4;
                for c in 0..4 {
                    rgba[i + c] = (rgba[i + c] as f64 * a).round() as u8;
                }
            }
        }
    }
}

/// The frosted wallpaper's px: one for each FROST_STEP of the picture's.
pub const FROST_STEP: u32 = 8;

/// A picture frosted for the system screen's glass (sysscreen.rs): an
/// FROST_STEP-th of its size, blurred hard (three box passes, ~a fifth of
/// the panel across) and darkened as the desktop's cards are - opaque.
fn frost(rgba: &[u8], w: u32, h: u32) -> (Vec<u8>, u32, u32) {
    let (fw, fh) = ((w / FROST_STEP).max(1) as usize, (h / FROST_STEP).max(1) as usize);
    let step = FROST_STEP as usize;
    let mut px = vec![[0f32; 3]; fw * fh];
    for y in 0..fh {
        for x in 0..fw {
            let mut acc = [0f32; 3];
            for dy in 0..step {
                for dx in 0..step {
                    let i = ((y * step + dy) * w as usize + x * step + dx) * 4;
                    for c in 0..3 {
                        acc[c] += rgba.get(i + c).copied().unwrap_or(0) as f32;
                    }
                }
            }
            px[y * fw + x] = acc.map(|v| v / (step * step) as f32);
        }
    }
    // Box blurs, sideways then down, three times: nearly a Gaussian.
    let r = 6usize;
    for _ in 0..3 {
        for horizontal in [true, false] {
            let (n, m) = if horizontal { (fh, fw) } else { (fw, fh) };
            let at = |a: usize, b: usize| if horizontal { a * fw + b } else { b * fw + a };
            let src = px.clone();
            for a in 0..n {
                for b in 0..m {
                    let (lo, hi) = (b.saturating_sub(r), (b + r).min(m - 1));
                    let mut acc = [0f32; 3];
                    for k in lo..=hi {
                        let p = src[at(a, k)];
                        acc = [acc[0] + p[0], acc[1] + p[1], acc[2] + p[2]];
                    }
                    let cnt = (hi - lo + 1) as f32;
                    px[at(a, b)] = acc.map(|v| v / cnt);
                }
            }
        }
    }
    // Darkened toward the night's blue, matte: text reads on it.
    const DARK: [f32; 3] = [16.0, 19.0, 27.0];
    let mut out = vec![0u8; fw * fh * 4];
    for (i, p) in px.iter().enumerate() {
        for c in 0..3 {
            out[i * 4 + c] = (p[c] * 0.38 + DARK[c] * 0.62).round().clamp(0.0, 255.0) as u8;
        }
        out[i * 4 + 3] = 255;
    }
    (out, fw as u32, fh as u32)
}

/// A JPEG file, decoded to RGBA.
fn decode(path: &std::path::Path) -> Option<(Vec<u8>, u32, u32)> {
    use zune_core::colorspace::ColorSpace;
    use zune_core::options::DecoderOptions;
    let data = std::fs::read(path).ok()?;
    let options = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::RGBA);
    let mut decoder = zune_jpeg::JpegDecoder::new_with_options(&data, options);
    let pixels = match decoder.decode() {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!("walls: {}: {e:?}", path.display());
            return None;
        }
    };
    let (w, h) = decoder.dimensions()?;
    Some((pixels, w as u32, h as u32))
}

/// The picture on until one is chosen (and under the first setup).
const DEFAULT: &str = "009.jpg";

impl Walls {
    pub fn new(wake: Ping) -> Walls {
        let dir = home().join(".local/share/item/walls");
        let mut files: Vec<String> = std::fs::read_dir(&dir)
            .map(|d| d.flatten().filter_map(|e| e.file_name().into_string().ok()).filter(|n| n.ends_with(".jpg") && !n.starts_with("thumb-")).collect())
            .unwrap_or_default();
        files.sort();
        let mut names = vec![AURORA.to_owned()];
        names.extend(files);
        let current = std::fs::read_to_string(kept()).map(|s| s.trim().to_owned()).ok().filter(|n| names.contains(n)).unwrap_or_else(|| if names.iter().any(|n| n == DEFAULT) { DEFAULT.to_owned() } else { AURORA.to_owned() });
        tracing::info!("walls: {} pictures, {current} on", names.len() - 1);
        let n = names.len();
        let mut walls = Walls { dir, names, current: String::new(), loaded: Default::default(), frosted: Default::default(), thumbs_loaded: Default::default(), thumbs: vec![None; n], thumbs_asked: false, wake };
        walls.choose(&current, false);
        walls
    }

    /// `name` on: kept (if `keep`), and its picture decoded on a thread.
    pub fn choose(&mut self, name: &str, keep: bool) {
        if !self.names.iter().any(|n| n == name) {
            return;
        }
        self.current = name.to_owned();
        if keep {
            let path = kept();
            if let Some(d) = path.parent() {
                let _ = std::fs::create_dir_all(d);
            }
            let _ = std::fs::write(&path, name);
            tracing::info!("walls: {name} on");
        }
        let (slot, frosted, wake, path, name) = (self.loaded.clone(), self.frosted.clone(), self.wake.clone(), self.dir.join(name), name.to_owned());
        std::thread::spawn(move || {
            let t = std::time::Instant::now();
            let picture = if name == AURORA { Some((Vec::new(), 0, 0)) } else { decode(&path) };
            if let Some((rgba, w, h)) = picture {
                if name != AURORA {
                    tracing::info!("walls: {name} decoded in {:.0} ms", t.elapsed().as_secs_f64() * 1e3);
                }
                crate::accent::set_wall(if name == AURORA { [0x6f, 0xd6, 0xc4, 255] } else { crate::accent::from_picture(&rgba, w, h) });
                *frosted.lock().unwrap() = Some((!rgba.is_empty()).then(|| frost(&rgba, w, h)));
                *slot.lock().unwrap() = Some((name, rgba, w, h));
                wake.ping();
            }
        });
    }

    /// The picture frosted, once it is (None inside: none to frost).
    pub fn take_frost(&self) -> Option<Option<(Vec<u8>, u32, u32)>> {
        self.frosted.lock().unwrap().take()
    }

    /// A picture decoded and waiting for the GPU (Aurora's has no rows).
    pub fn take_loaded(&self) -> Option<Picture> {
        self.loaded.lock().unwrap().take()
    }

    /// The small pictures read, on a thread, once.
    pub fn ask_thumbs(&mut self) {
        if self.thumbs_asked {
            return;
        }
        self.thumbs_asked = true;
        let (slot, wake, dir, names) = (self.thumbs_loaded.clone(), self.wake.clone(), self.dir.clone(), self.names.clone());
        std::thread::spawn(move || {
            let out: Vec<_> = names
                .iter()
                .enumerate()
                .filter_map(|(i, n)| decode(&dir.join(format!("thumb-{n}"))).map(|(mut p, w, h)| {
                    round_corners(&mut p, w, h, THUMB_RADIUS);
                    (i, p, w, h)
                }))
                .collect();
            *slot.lock().unwrap() = Some(out);
            wake.ping();
        });
    }

    /// The small pictures, made buffers once read; whether any came.
    pub fn take_thumbs(&mut self) -> bool {
        let Some(list) = self.thumbs_loaded.lock().unwrap().take() else { return false };
        for (i, rgba, w, h) in list {
            self.thumbs[i] = Some(MemoryRenderBuffer::from_slice(&rgba, Fourcc::Abgr8888, (w as i32, h as i32), 2, Transform::Normal, None));
        }
        true
    }
}
