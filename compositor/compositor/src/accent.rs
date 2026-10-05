//! The system's accent: the colour of what acts and what is on - buttons,
//! tiles, the keyboard's Enter, the desktop's clock. Chosen where the
//! wallpaper is (picker.rs): the first choice takes it from the wallpaper
//! (its most vivid hue, brought to a light that reads on the dark), the
//! others are fixed. Kept in `~/.config/item/accent`: `auto` or `#rrggbb`.
//!
//! Whatever is drawn in it asks `version()` and draws itself again when it
//! changed (`Rounded` does it for the rounded shapes).

use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use smithay::backend::renderer::element::memory::MemoryRenderBuffer;

/// item's coral, the accent until another is chosen.
pub const CORAL: [u8; 4] = [0xf0, 0x8a, 0x7b, 255];

/// The fixed choices, after "from the wallpaper".
pub const PALETTE: [[u8; 4]; 7] = [
    CORAL,
    [0xe9, 0xdf, 0x6a, 255], // yellow
    [0x86, 0xd6, 0x8f, 255], // green
    [0x62, 0xcf, 0xc6, 255], // teal
    [0x7d, 0xaa, 0xf7, 255], // blue
    [0xb7, 0x98, 0xf0, 255], // violet
    [0xf0, 0x92, 0xbb, 255], // pink
];

fn pack(c: [u8; 4]) -> u32 {
    u32::from_be_bytes(c)
}

fn unpack(v: u32) -> [u8; 4] {
    v.to_be_bytes()
}

static COLOR: AtomicU32 = AtomicU32::new(0xf08a7bff);
static WALL: AtomicU32 = AtomicU32::new(0xf08a7bff);
static AUTO: AtomicBool = AtomicBool::new(false);
static VERSION: AtomicU64 = AtomicU64::new(1);
/// The kept file's text as last read or written here: a change to it from
/// outside (Cradle, #168) is taken in by `follow`.
static KEPT: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());

fn kept() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".config/item/accent")
}

/// The accent now.
pub fn get() -> [u8; 4] {
    unpack(COLOR.load(Ordering::Relaxed))
}

/// The accent now, as the renderer's floats (straight, not premultiplied).
pub fn get_f() -> [f32; 4] {
    get().map(|c| c as f32 / 255.0)
}

/// Bumped whenever the accent changes.
pub fn version() -> u64 {
    VERSION.load(Ordering::Relaxed)
}

/// The choice now: None from the wallpaper, else the palette's index (or
/// the palette's coral for a colour not in it).
pub fn chosen() -> Option<usize> {
    if AUTO.load(Ordering::Relaxed) {
        return None;
    }
    Some(PALETTE.iter().position(|c| *c == get()).unwrap_or(0))
}

/// The colour the wallpaper gives, for the first choice's swatch.
pub fn wall() -> [u8; 4] {
    unpack(WALL.load(Ordering::Relaxed))
}

fn apply(c: [u8; 4]) {
    if COLOR.swap(pack(c), Ordering::Relaxed) != pack(c) {
        VERSION.fetch_add(1, Ordering::Relaxed);
    }
}

/// Read the kept choice (at the start).
pub fn load() {
    let kept = std::fs::read_to_string(kept()).unwrap_or_default();
    let kept = kept.trim();
    *KEPT.lock().unwrap() = kept.to_owned();
    if kept == "auto" {
        AUTO.store(true, Ordering::Relaxed);
        apply(wall());
    } else if let Some(hex) = kept.strip_prefix('#').filter(|h| h.len() == 6) {
        if let Ok(v) = u32::from_str_radix(hex, 16) {
            AUTO.store(false, Ordering::Relaxed);
            apply(unpack((v << 8) | 0xff));
        }
    }
}

/// The kept choice read again if something else changed it (Cradle, on the
/// computer: #168); whether it did.
pub fn follow() -> bool {
    let now = std::fs::read_to_string(kept()).unwrap_or_default();
    if now.trim() == KEPT.lock().unwrap().as_str() {
        return false;
    }
    load();
    tracing::info!("accent: {:?}, changed from outside", get());
    true
}

/// A choice made: None from the wallpaper, Some(i) the palette's; kept.
pub fn choose(choice: Option<usize>) {
    let text = match choice {
        None => {
            AUTO.store(true, Ordering::Relaxed);
            apply(wall());
            "auto".to_owned()
        }
        Some(i) => {
            AUTO.store(false, Ordering::Relaxed);
            let c = PALETTE[i.min(PALETTE.len() - 1)];
            apply(c);
            format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])
        }
    };
    let path = kept();
    if let Some(d) = path.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    *KEPT.lock().unwrap() = text.clone();
    let _ = std::fs::write(&path, text);
    tracing::info!("accent: {:?}", get());
}

/// The wallpaper's colour, worked out as its picture loads: the accent too
/// when it is chosen from the wallpaper.
pub fn set_wall(c: [u8; 4]) {
    WALL.store(pack(c), Ordering::Relaxed);
    if AUTO.load(Ordering::Relaxed) {
        apply(c);
    }
    // The swatch shows it either way.
    VERSION.fetch_add(1, Ordering::Relaxed);
}

/// A picture's accent (RGBA rows): the hue of its vivid pixels, weighted by
/// how vivid, brought to a saturation and light that read on the dark; a
/// picture with nearly no colour gives the coral.
pub fn from_picture(rgba: &[u8], w: u32, h: u32) -> [u8; 4] {
    let (w, h) = (w as usize, h as usize);
    let step = ((w * h) / 40_000).max(1);
    let (mut sx, mut sy, mut weight, mut n) = (0.0f64, 0.0f64, 0.0f64, 0usize);
    let mut i = 0;
    while i < w * h {
        let p = &rgba[i * 4..i * 4 + 3];
        let (r, g, b) = (p[0] as f64 / 255.0, p[1] as f64 / 255.0, p[2] as f64 / 255.0);
        let (max, min) = (r.max(g).max(b), r.min(g).min(b));
        let (s, v) = (if max > 0.0 { (max - min) / max } else { 0.0 }, max);
        if (0.2..0.97).contains(&v) && s > 0.15 {
            let d = max - min;
            let hue = if max == r { ((g - b) / d).rem_euclid(6.0) } else if max == g { (b - r) / d + 2.0 } else { (r - g) / d + 4.0 } * 60.0;
            let wgt = s * s;
            sx += wgt * hue.to_radians().cos();
            sy += wgt * hue.to_radians().sin();
            weight += wgt;
        }
        n += 1;
        i += step;
    }
    if n == 0 || weight / (n as f64) < 0.01 {
        return CORAL;
    }
    let hue = sy.atan2(sx).to_degrees().rem_euclid(360.0);
    hsl(hue, 0.62, 0.70)
}

fn hsl(h: f64, s: f64, l: f64) -> [u8; 4] {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - ((h / 60.0).rem_euclid(2.0) - 1.0).abs());
    let m = l - c / 2.0;
    let (r, g, b) = match (h / 60.0) as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    [((r + m) * 255.0).round() as u8, ((g + m) * 255.0).round() as u8, ((b + m) * 255.0).round() as u8, 255]
}

/// A rounded shape in the accent, drawn again when the accent changes.
pub struct Rounded {
    w: f64,
    h: f64,
    r: f64,
    drawn: RefCell<(u64, MemoryRenderBuffer)>,
}

impl Rounded {
    pub fn new(w: f64, h: f64, r: f64) -> Rounded {
        Rounded { w, h, r, drawn: RefCell::new((version(), crate::grid::rounded(w, h, r, get()))) }
    }

    /// The shape in the accent now.
    pub fn get(&self) -> MemoryRenderBuffer {
        let mut d = self.drawn.borrow_mut();
        if d.0 != version() {
            *d = (version(), crate::grid::rounded(self.w, self.h, self.r, get()));
        }
        d.1.clone()
    }
}
