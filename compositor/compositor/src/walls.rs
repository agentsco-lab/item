//! The wallpapers: item's set of pictures, and what is on each panel.
//!
//! The set is the pictures in `~/.local/share/item/walls` and the package's
//! `/usr/share/item/walls` (the user's first, by name), each with a small
//! picture beside it for choosing (`thumb-NAME.jpg`, made from the picture
//! when it is missing). Before them comes Aurora, the wallpaper drawn by the
//! compositor itself (its small picture `thumb-aurora`).
//!
//! What is on: one picture on both panels, or one on each ("each"), each
//! seen at a zoom (1 the whole picture covering its part; up to as far as
//! the picture's own pixels allow, at most MAX_ZOOM) about a point of the
//! picture. All of it is kept in `~/.config/item/wallpaper`. On a thread the
//! pictures are decoded and the wallpaper made from them - the screen's size
//! and the parallax's room more (wallpaper.glsl, output.rs), a picture
//! scaled down with a proper filter into its part - and handed to the loop,
//! which sends it to the GPU; everything else (the dock's drops, the lock
//! screen, the frost, the accent) takes that one texture as before. With a
//! picture on each panel the wallpaper keeps still (no parallax): the seam
//! between them stays in the hinge.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
use smithay::reexports::calloop::ping::Ping;
use smithay::utils::Transform;

/// The wallpaper the compositor draws itself.
pub const AURORA: &str = "aurora";

/// How far a picture may be zoomed, its pixels allowing.
pub const MAX_ZOOM: f64 = 2.0;

/// A wallpaper made: a label for the log, its RGBA rows (none: Aurora), its
/// size, and whether it fades in over the one before (a new picture) or
/// takes its place at once (the same one zoomed or moved).
pub type Picture = (String, Vec<u8>, u32, u32, bool);

/// Which part of the wallpaper.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Side {
    Both,
    Left,
    Right,
}

impl Side {
    fn index(self) -> usize {
        match self {
            Side::Both => 0,
            Side::Left => 1,
            Side::Right => 2,
        }
    }

    fn word(self) -> &'static str {
        match self {
            Side::Both => "both",
            Side::Left => "left",
            Side::Right => "right",
        }
    }
}

/// A picture as seen: its name, the zoom, the point of it (0-1 across and
/// down) at the middle of its part.
#[derive(Clone, Debug, PartialEq)]
pub struct View {
    pub name: String,
    pub zoom: f64,
    pub cx: f64,
    pub cy: f64,
}

impl View {
    fn of(name: &str) -> View {
        View { name: name.to_owned(), zoom: 1.0, cx: 0.5, cy: 0.5 }
    }
}

/// The vignette with a picture on each panel (output.rs, vignette.frag).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vignette {
    /// How dark the edges get, 0 to 1.
    pub strength: f32,
    /// How far in from the edges it reaches, 0 to 1.
    pub size: f32,
    /// A soft gradient (1) or a firmer edge (0).
    pub soft: f32,
    /// A glow in the accent instead of the dark.
    pub glow: bool,
}

impl Default for Vignette {
    fn default() -> Vignette {
        Vignette { strength: 0.55, size: 0.5, soft: 0.6, glow: false }
    }
}

/// A picture for the picker's preview (output.rs): the part, its name, its
/// RGBA, its size, and its px per px of the picture.
pub type PreviewSource = (Side, String, Vec<u8>, u32, u32, f64);

/// The most pixels a preview's picture keeps.
const PREVIEW_PX: f64 = 8.0e6;

/// Whether a picture is on each panel: then the wallpaper keeps still
/// (state.rs's parallax).
static EACH: AtomicBool = AtomicBool::new(false);

pub fn each_on() -> bool {
    EACH.load(Ordering::Relaxed)
}

/// The wallpaper's size, logical px (the screen and the parallax's room).
fn canvas() -> (f64, f64) {
    (crate::layout::LAYOUT.0 as f64 + crate::state::WALL_LEFT + crate::state::WALL_RIGHT, crate::layout::LAYOUT.1 as f64)
}

/// Where the seam between the panels' pictures is on the wallpaper, logical
/// px: the middle of the hinge.
fn seam() -> f64 {
    let [l, r] = crate::layout::panels();
    crate::state::WALL_LEFT + ((l.loc.x + l.size.w) as f64 + r.loc.x as f64) / 2.0
}

/// A part of the wallpaper, logical px: x, y, w, h.
pub fn region(side: Side) -> (f64, f64, f64, f64) {
    let (w, h) = canvas();
    match side {
        Side::Both => (0.0, 0.0, w, h),
        Side::Left => (0.0, 0.0, seam(), h),
        Side::Right => (seam(), 0.0, w - seam(), h),
    }
}

/// A decoded picture, shared between the threads that use it.
type Source = Arc<(Vec<u8>, u32, u32)>;

pub struct Walls {
    /// Where pictures are looked for: the user's, then the package's.
    dirs: Vec<PathBuf>,
    /// The set: Aurora, then the pictures by name.
    pub names: Vec<String>,
    /// The picture of the part being chosen for (the picker's ring).
    pub current: String,
    /// One on each panel.
    pub each: bool,
    /// The panel being chosen for, with one on each.
    pub target: Side,
    /// Both, left, right.
    views: [View; 3],
    /// Pictures' sizes, once decoded: how far each may zoom.
    sizes: Arc<Mutex<HashMap<String, (u32, u32)>>>,
    /// The last pictures decoded, kept for the next zoom.
    cache: Arc<Mutex<Vec<(String, Source)>>>,
    loaded: Arc<Mutex<Option<Picture>>>,
    /// The wallpaper frosted (`frost`): Some(None) for Aurora, which has none.
    frosted: Arc<Mutex<Option<Option<(Vec<u8>, u32, u32)>>>>,
    thumbs_loaded: Arc<Mutex<Option<Vec<(usize, Vec<u8>, u32, u32)>>>>,
    /// The small pictures, once read (the picker asks for them).
    pub thumbs: Vec<Option<MemoryRenderBuffer>>,
    /// The pictures taller than wide: for one on each panel; the wide ones
    /// for one on both.
    tall: std::collections::HashSet<String>,
    /// The pictures the strip shows for the mode (indices into `names`).
    pub shown: Vec<usize>,
    thumbs_asked: bool,
    /// With one on each panel, each darkening toward its edges.
    pub vignette: Vignette,
    /// Pictures for the picker's preview, waiting for the GPU.
    previews: Arc<Mutex<Vec<PreviewSource>>>,
    /// Each part's preview picture's px per px of the picture, as made.
    preview_scale: Arc<Mutex<HashMap<(usize, String), f64>>>,
    /// The wallpaper made last, so a make is not asked for twice.
    made: Option<(bool, [View; 3])>,
    /// Which make is the last asked for: one before it, finishing late,
    /// is dropped.
    generation: Arc<std::sync::atomic::AtomicU64>,
    /// The window of the preview's picture a gesture let go of showed (px
    /// of that picture), held on screen until the wallpaper made from it is
    /// on (preview_window).
    pub held: Option<(Side, String, (f64, f64, f64, f64))>,
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
/// A JPEG's size from its frame header, without decoding it.
fn jpeg_size(path: &std::path::Path) -> Option<(u32, u32)> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::io::BufReader::new(std::fs::File::open(path).ok()?);
    let mut b = [0u8; 2];
    f.read_exact(&mut b).ok()?;
    if b != [0xFF, 0xD8] {
        return None;
    }
    loop {
        f.read_exact(&mut b).ok()?;
        if b[0] != 0xFF {
            return None;
        }
        let marker = b[1];
        if marker == 0xFF || (0xD0..=0xD7).contains(&marker) || marker == 0x01 {
            continue;
        }
        f.read_exact(&mut b).ok()?;
        let len = u16::from_be_bytes(b) as i64;
        if matches!(marker, 0xC0..=0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF) {
            let mut h = [0u8; 5];
            f.read_exact(&mut h).ok()?;
            return Some((u16::from_be_bytes([h[3], h[4]]) as u32, u16::from_be_bytes([h[1], h[2]]) as u32));
        }
        f.seek(SeekFrom::Current(len - 2)).ok()?;
    }
}

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

/// The small pictures' size, px.
const THUMB_PX: (u32, u32) = (360, 202);

/// A window of `src` (x, y, w, h in its px, fractional) scaled into `dst`
/// (stride `dst_w`) at (`dx`, `dy`), `dw` x `dh` px: separable, a triangle
/// filter as wide as the scale-down (Pillow's bilinear), so a picture
/// shrunk two or three times stays sharp and does not shimmer.
#[allow(clippy::too_many_arguments)]
fn resample(src: &[u8], sw: u32, sh: u32, win: (f64, f64, f64, f64), dst: &mut [u8], dst_w: u32, dx: u32, dy: u32, dw: u32, dh: u32) {
    // Per output coordinate: the first source index and the weights.
    fn weights(size_in: u32, start: f64, len: f64, out: u32) -> Vec<(usize, Vec<f32>)> {
        let scale = len / out as f64;
        let support = scale.max(1.0);
        (0..out)
            .map(|i| {
                let centre = start + (i as f64 + 0.5) * scale;
                let lo = ((centre - support).floor() as i64).max(0);
                let hi = ((centre + support).ceil() as i64).min(size_in as i64 - 1);
                let mut w: Vec<f32> = (lo..=hi).map(|j| (1.0 - ((j as f64 + 0.5 - centre) / support).abs()).max(0.0) as f32).collect();
                let sum: f32 = w.iter().sum();
                if sum > 0.0 {
                    w.iter_mut().for_each(|v| *v /= sum);
                }
                (lo.max(0) as usize, w)
            })
            .collect()
    }
    let (wx, wy, ww, wh) = win;
    let xs = weights(sw, wx, ww, dw);
    let ys = weights(sh, wy, wh, dh);
    // The rows the output needs, scaled across.
    let (row_lo, row_hi) = (ys.iter().map(|y| y.0).min().unwrap_or(0), ys.iter().map(|y| y.0 + y.1.len()).max().unwrap_or(0).min(sh as usize));
    let mut mid = vec![0f32; (row_hi - row_lo) * dw as usize * 4];
    for r in row_lo..row_hi {
        let line = &src[r * sw as usize * 4..(r + 1) * sw as usize * 4];
        let out = &mut mid[(r - row_lo) * dw as usize * 4..(r - row_lo + 1) * dw as usize * 4];
        for (i, (first, w)) in xs.iter().enumerate() {
            let mut acc = [0f32; 4];
            for (k, wk) in w.iter().enumerate() {
                let p = &line[(first + k) * 4..(first + k) * 4 + 4];
                for c in 0..4 {
                    acc[c] += p[c] as f32 * wk;
                }
            }
            out[i * 4..i * 4 + 4].copy_from_slice(&acc);
        }
    }
    for (j, (first, w)) in ys.iter().enumerate() {
        let row = &mut dst[((dy as usize + j) * dst_w as usize + dx as usize) * 4..];
        for i in 0..dw as usize {
            let mut acc = [0f32; 4];
            for (k, wk) in w.iter().enumerate() {
                let m = ((first + k - row_lo) * dw as usize + i) * 4;
                for c in 0..4 {
                    acc[c] += mid[m + c] * wk;
                }
            }
            for c in 0..4 {
                row[i * 4 + c] = acc[c].round().clamp(0.0, 255.0) as u8;
            }
        }
    }
}

/// The window of a `sw` x `sh` picture seen in a part `rw` x `rh` px at
/// `view`: covering it, zoomed, about its point, kept inside the picture.
/// Also the zoom as far as it may go.
fn window(sw: u32, sh: u32, rw: f64, rh: f64, view: &View) -> ((f64, f64, f64, f64), f64) {
    let (sw, sh) = (sw as f64, sh as f64);
    // Px of the part per px of the picture, covering it whole.
    let cover = (rw / sw).max(rh / sh);
    let max = (1.0 / cover).clamp(1.0, MAX_ZOOM);
    let zoom = view.zoom.clamp(1.0, max);
    let (ww, wh) = (rw / (cover * zoom), rh / (cover * zoom));
    let x = (view.cx * sw - ww / 2.0).clamp(0.0, sw - ww);
    let y = (view.cy * sh - wh / 2.0).clamp(0.0, sh - wh);
    ((x, y, ww, wh), max)
}

impl Walls {
    pub fn new(wake: Ping) -> Walls {
        let dirs = vec![home().join(".local/share/item/walls"), PathBuf::from("/usr/share/item/walls")];
        let mut files: Vec<String> = Vec::new();
        for d in &dirs {
            for n in std::fs::read_dir(d).map(|d| d.flatten().filter_map(|e| e.file_name().into_string().ok()).collect::<Vec<_>>()).unwrap_or_default() {
                if (n.ends_with(".jpg") || n.ends_with(".jpeg")) && !n.starts_with("thumb-") && !files.contains(&n) {
                    files.push(n);
                }
            }
        }
        files.sort();
        let mut names = vec![AURORA.to_owned()];
        names.extend(files);
        let fallback = if names.iter().any(|n| n == DEFAULT) { DEFAULT } else { AURORA };
        let mut walls = Walls {
            dirs,
            names,
            current: String::new(),
            each: false,
            target: Side::Left,
            views: [View::of(fallback), View::of(fallback), View::of(fallback)],
            sizes: Default::default(),
            cache: Default::default(),
            loaded: Default::default(),
            frosted: Default::default(),
            thumbs_loaded: Default::default(),
            thumbs: Vec::new(),
            tall: Default::default(),
            shown: Vec::new(),
            thumbs_asked: false,
            vignette: Vignette::default(),
            previews: Default::default(),
            preview_scale: Default::default(),
            made: None,
            generation: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            held: None,
            wake,
        };
        walls.thumbs = vec![None; walls.names.len()];
        walls.tall = walls.names.iter().filter(|n| jpeg_size(&walls.path(n)).is_some_and(|(w, h)| h > w)).cloned().collect();
        walls.read_kept();
        walls.sync_current();
        tracing::info!("walls: {} pictures; {}", walls.names.len() - 1, walls.describe());
        walls.make(false, true);
        walls
    }

    /// The kept choice: "NAME" (the old form), or lines `mode each|both` and
    /// `both|left|right NAME ZOOM CX CY`.
    fn read_kept(&mut self) {
        let Ok(text) = std::fs::read_to_string(kept()) else { return };
        let known = |n: &str| self.names.iter().any(|m| m == n);
        for line in text.lines() {
            let w: Vec<&str> = line.split_whitespace().collect();
            match w.as_slice() {
                [name] if known(name) => self.views = [View::of(name), View::of(name), View::of(name)],
                ["mode", m] => self.each = *m == "each",
                ["vignette", rest @ ..] => {
                    let num = |k: usize, d: f32| rest.get(k).and_then(|v| v.parse::<f32>().ok()).unwrap_or(d).clamp(0.0, 1.0);
                    let d = Vignette::default();
                    self.vignette = Vignette { strength: num(0, d.strength), size: num(1, d.size), soft: num(2, d.soft), glow: rest.get(3) == Some(&"glow") };
                }
                [side, name, rest @ ..] if known(name) => {
                    let i = match *side {
                        "both" => 0,
                        "left" => 1,
                        "right" => 2,
                        _ => continue,
                    };
                    let num = |k: usize, d: f64| rest.get(k).and_then(|v| v.parse::<f64>().ok()).filter(|v| v.is_finite()).unwrap_or(d);
                    self.views[i] = View { name: name.to_string(), zoom: num(0, 1.0).clamp(1.0, MAX_ZOOM), cx: num(1, 0.5).clamp(0.0, 1.0), cy: num(2, 0.5).clamp(0.0, 1.0) };
                }
                _ => {}
            }
        }
        if self.each && (self.views[1].name == AURORA || self.views[2].name == AURORA) {
            self.each = false;
        }
        self.sync_current();
    }

    fn keep(&self) {
        let path = kept();
        if let Some(d) = path.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let g = &self.vignette;
        let mut text = format!("mode {}\nvignette {:.2} {:.2} {:.2} {}\n", if self.each { "each" } else { "both" }, g.strength, g.size, g.soft, if g.glow { "glow" } else { "dark" });
        for side in [Side::Both, Side::Left, Side::Right] {
            let v = &self.views[side.index()];
            text.push_str(&format!("{} {} {:.3} {:.4} {:.4}\n", side.word(), v.name, v.zoom, v.cx, v.cy));
        }
        let _ = std::fs::write(&path, text);
    }

    fn describe(&self) -> String {
        if self.each {
            format!("{} left, {} right", self.views[1].name, self.views[2].name)
        } else {
            format!("{} on both", self.views[0].name)
        }
    }

    /// The part being chosen for: both panels, or the target one.
    pub fn part(&self) -> Side {
        if self.each { self.target } else { Side::Both }
    }

    fn sync_current(&mut self) {
        self.current = self.views[self.part().index()].name.clone();
        // The strip: the tall pictures with one on each panel, the wide ones
        // (and Aurora) with one on both; all, if none fits.
        let each = self.each;
        self.shown = (0..self.names.len()).filter(|&i| if self.names[i] == AURORA { !each } else { self.tall.contains(&self.names[i]) == each }).collect();
        if self.shown.is_empty() {
            self.shown = (0..self.names.len()).collect();
        }
    }

    /// Where the one on is in the strip, if it is there.
    pub fn shown_current(&self) -> Option<usize> {
        self.shown.iter().position(|&i| self.names[i] == self.current)
    }

    /// The strip's small pictures, in its order.
    pub fn shown_thumbs(&self) -> Vec<Option<MemoryRenderBuffer>> {
        self.shown.iter().map(|&i| self.thumbs.get(i).cloned().flatten()).collect()
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dirs.iter().map(|d| d.join(name)).find(|p| p.exists()).unwrap_or_else(|| self.dirs[0].join(name))
    }

    /// `name` on the part being chosen for, kept. Aurora goes on both: the
    /// compositor draws it across the whole screen.
    pub fn choose(&mut self, name: &str) {
        if !self.names.iter().any(|n| n == name) {
            return;
        }
        if name == AURORA {
            self.each = false;
        }
        let i = self.part().index();
        self.views[i] = View::of(name);
        self.sync_current();
        tracing::info!("walls: {}", self.describe());
        self.keep();
        self.make(true, false);
    }

    /// One on each panel, or one on both. Going to each, both panels start
    /// with the picture that was on both; Aurora stays on both.
    pub fn set_each(&mut self, each: bool) {
        if each == self.each || (each && self.views[0].name == AURORA) {
            return;
        }
        self.each = each;
        if each {
            let v = self.views[0].clone();
            self.views[1] = v.clone();
            self.views[2] = v;
        }
        self.sync_current();
        tracing::info!("walls: {}", self.describe());
        self.keep();
        self.make(true, false);
    }

    /// The vignette, kept.
    pub fn set_vignette(&mut self, v: Vignette) {
        self.vignette = v;
        self.keep();
    }

    /// Pictures for the picker's preview made since the last ask.
    pub fn take_previews(&self) -> Vec<PreviewSource> {
        std::mem::take(&mut *self.previews.lock().unwrap())
    }

    /// The window of the preview's picture the zoom `z` and move `d` show on
    /// the part being chosen for (px of that picture), as the made
    /// wallpaper will be: the part and its picture's name with it.
    pub fn preview_window(&self, z: f64, d: (f64, f64)) -> Option<(Side, String, (f64, f64, f64, f64))> {
        let part = self.part();
        let (_, nv, _, _) = self.led_to(z, d)?;
        let &(sw, sh) = self.sizes.lock().unwrap().get(&nv.name)?;
        let f = *self.preview_scale.lock().unwrap().get(&(part.index(), nv.name.clone()))?;
        let (_, _, rw, rh) = region(part);
        let s = crate::layout::SCALE as f64;
        let ((x, y, w, h), _) = window(sw, sh, rw * s, rh * s, &nv);
        Some((part, nv.name, (x * f, y * f, w * f, h * f)))
    }

    /// The panel being chosen for, with one on each.
    pub fn set_target(&mut self, side: Side) {
        if side != Side::Both {
            self.target = side;
            self.sync_current();
        }
    }

    /// The picture on the part being chosen for zoomed by `z` and moved by
    /// `d` (logical px on the screen, the way the finger went), as the
    /// picker showed it; kept, and the wallpaper made again at full quality.
    /// The view the zoom `z` and move `d` lead to, kept inside the picture,
    /// and the zoom and move that show it (the picker's preview draws those,
    /// so what the fingers see is what is made).
    fn led_to(&self, z: f64, d: (f64, f64)) -> Option<(View, View, f64, (f64, f64))> {
        let part = self.part();
        let v = self.views[part.index()].clone();
        if v.name == AURORA {
            return None;
        }
        let &(sw, sh) = self.sizes.lock().unwrap().get(&v.name)?;
        let (_, _, rw, rh) = region(part);
        let s = crate::layout::SCALE as f64;
        let (rw, rh) = (rw * s, rh * s);
        let ((x, y, ww, wh), max) = window(sw, sh, rw, rh, &v);
        let zoom = (v.zoom * z).clamp(1.0, max);
        let z = zoom / v.zoom;
        // Px of the part per px of the picture, before.
        let per = rw / ww;
        let (cx, cy) = (x + ww / 2.0 - d.0 * s / (per * z), y + wh / 2.0 - d.1 * s / (per * z));
        let mut nv = View { name: v.name.clone(), zoom, cx: cx / sw as f64, cy: cy / sh as f64 };
        // Kept to where the window can be.
        let ((x2, y2, w2, h2), _) = window(sw, sh, rw, rh, &nv);
        nv.cx = (x2 + w2 / 2.0) / sw as f64;
        nv.cy = (y2 + h2 / 2.0) / sh as f64;
        // The move that shows the kept window.
        let d = ((x + ww / 2.0 - (x2 + w2 / 2.0)) * per * z / s, (y + wh / 2.0 - (y2 + h2 / 2.0)) * per * z / s);
        Some((v, nv, z, d))
    }

    /// The zoom and move the picker's fingers ask for, kept as the made
    /// wallpaper will keep them.
    pub fn preview(&self, z: f64, d: (f64, f64)) -> (f64, (f64, f64)) {
        self.led_to(z, d).map(|(_, _, z, d)| (z, d)).unwrap_or((z, d))
    }

    /// Whether a new wallpaper is being made.
    pub fn adjust(&mut self, z: f64, d: (f64, f64)) -> bool {
        let part = self.part();
        let Some((v, nv, _, _)) = self.led_to(z, d) else { return false };
        if nv == v {
            return false;
        }
        tracing::info!("walls: {} at {:.2}x, ({:.3}, {:.3})", nv.name, nv.zoom, nv.cx, nv.cy);
        self.views[part.index()] = nv;
        self.keep();
        self.make(false, false);
        true
    }

    /// How far the picture on the part being chosen for may zoom out and in
    /// from where it is (1, 1: not at all), as far as is known.
    pub fn zoom_room(&self) -> (f64, f64) {
        let v = &self.views[self.part().index()];
        if v.name == AURORA {
            return (1.0, 1.0);
        }
        let Some(&(sw, sh)) = self.sizes.lock().unwrap().get(&v.name) else { return (1.0, 1.0) };
        let (_, _, rw, rh) = region(self.part());
        let s = crate::layout::SCALE as f64;
        let (_, max) = window(sw, sh, rw * s, rh * s, v);
        (1.0 / v.zoom, (max / v.zoom).max(1.0))
    }

    /// The wallpaper made on a thread from what is on, unless it is what was
    /// made last; `fade`: in over the one before.
    fn make(&mut self, fade: bool, first: bool) {
        let what = (self.each, self.views.clone());
        if self.made.as_ref() == Some(&what) && !first {
            return;
        }
        self.made = Some(what);
        let generation = self.generation.clone();
        let mine = generation.fetch_add(1, Ordering::SeqCst) + 1;
        EACH.store(self.each, Ordering::Relaxed);
        let parts: Vec<(Side, View, PathBuf)> = if self.each {
            [Side::Left, Side::Right].iter().map(|s| (*s, self.views[s.index()].clone(), self.path(&self.views[s.index()].name))).collect()
        } else {
            vec![(Side::Both, self.views[0].clone(), self.path(&self.views[0].name))]
        };
        let label = self.describe();
        let (slot, frosted, wake, sizes, cache) = (self.loaded.clone(), self.frosted.clone(), self.wake.clone(), self.sizes.clone(), self.cache.clone());
        let (previews, preview_scale) = (self.previews.clone(), self.preview_scale.clone());
        std::thread::spawn(move || {
            let t = std::time::Instant::now();
            if parts.len() == 1 && parts[0].1.name == AURORA {
                crate::accent::set_wall([0x6f, 0xd6, 0xc4, 255]);
                *frosted.lock().unwrap() = Some(None);
                *slot.lock().unwrap() = Some((label, Vec::new(), 0, 0, fade));
                wake.ping();
                return;
            }
            let s = crate::layout::SCALE as f64;
            let (cw, ch) = canvas();
            let (cw, ch) = ((cw * s).round() as u32, (ch * s).round() as u32);
            let mut rgba = vec![0u8; cw as usize * ch as usize * 4];
            for (side, view, path) in &parts {
                let source = {
                    let hit = cache.lock().unwrap().iter().find(|(n, _)| *n == view.name).map(|(_, s)| s.clone());
                    match hit {
                        Some(s) => s,
                        None => {
                            let Some(d) = decode(path) else { continue };
                            let s: Source = Arc::new(d);
                            let mut c = cache.lock().unwrap();
                            c.retain(|(n, _)| *n != view.name);
                            c.push((view.name.clone(), s.clone()));
                            // The two on each panel, and one more.
                            while c.len() > 3 {
                                c.remove(0);
                            }
                            s
                        }
                    }
                };
                let (src, sw, sh) = (&source.0, source.1, source.2);
                sizes.lock().unwrap().insert(view.name.clone(), (sw, sh));
                // The picker's preview picture for this part: sharp to the
                // most zoom, not over PREVIEW_PX; made when the picture is new.
                let key = (side.index(), view.name.clone());
                if !preview_scale.lock().unwrap().contains_key(&key) {
                    let (_, _, rw, rh) = region(*side);
                    let cover = (rw * s / sw as f64).max(rh * s / sh as f64);
                    let f = (cover * MAX_ZOOM).min(1.0).min((PREVIEW_PX / (sw as f64 * sh as f64)).sqrt());
                    let (pw, ph) = (((sw as f64 * f).round() as u32).max(1), ((sh as f64 * f).round() as u32).max(1));
                    let mut small = vec![0u8; (pw * ph * 4) as usize];
                    resample(src, sw, sh, (0.0, 0.0, sw as f64, sh as f64), &mut small, pw, 0, 0, pw, ph);
                    let f = pw as f64 / sw as f64;
                    let mut ps = preview_scale.lock().unwrap();
                    ps.retain(|(i, _), _| *i != side.index());
                    ps.insert(key, f);
                    previews.lock().unwrap().push((*side, view.name.clone(), small, pw, ph, f));
                }
                let (rx, ry, rw, rh) = region(*side);
                let (px, py, pw, ph) = ((rx * s).round() as u32, (ry * s).round() as u32, (rw * s).round() as u32, (rh * s).round() as u32);
                let pw = pw.min(cw - px);
                let (win, _) = window(sw, sh, pw as f64, ph as f64, view);
                resample(src, sw, sh, win, &mut rgba, cw, px, py, pw, ph.min(ch - py));
            }
            if generation.load(Ordering::SeqCst) != mine {
                tracing::info!("walls: {label} made too late, dropped");
                return;
            }
            tracing::info!("walls: {label} made in {:.0} ms", t.elapsed().as_secs_f64() * 1e3);
            crate::accent::set_wall(crate::accent::from_picture(&rgba, cw, ch));
            *frosted.lock().unwrap() = Some(Some(frost(&rgba, cw, ch)));
            *slot.lock().unwrap() = Some((label, rgba, cw, ch, fade));
            wake.ping();
        });
    }

    /// The wallpaper frosted, once it is (None inside: none to frost).
    pub fn take_frost(&self) -> Option<Option<(Vec<u8>, u32, u32)>> {
        self.frosted.lock().unwrap().take()
    }

    /// A wallpaper made and waiting for the GPU (Aurora's has no rows).
    pub fn take_loaded(&self) -> Option<Picture> {
        self.loaded.lock().unwrap().take()
    }

    /// The small pictures read, on a thread, once; one missing made from its
    /// picture.
    pub fn ask_thumbs(&mut self) {
        if self.thumbs_asked {
            return;
        }
        self.thumbs_asked = true;
        let names = self.names.clone();
        let paths: Vec<(PathBuf, PathBuf)> = names.iter().map(|n| (self.path(&format!("thumb-{n}")), self.path(n))).collect();
        let (slot, wake) = (self.thumbs_loaded.clone(), self.wake.clone());
        std::thread::spawn(move || {
            let out: Vec<_> = paths
                .iter()
                .enumerate()
                .filter_map(|(i, (thumb, picture))| {
                    let small = decode(thumb).or_else(|| {
                        if names[i] == AURORA {
                            return None;
                        }
                        let (src, sw, sh) = decode(picture)?;
                        let (w, h) = THUMB_PX;
                        let mut out = vec![0u8; (w * h * 4) as usize];
                        let (win, _) = window(sw, sh, w as f64, h as f64, &View::of(""));
                        resample(&src, sw, sh, win, &mut out, w, 0, 0, w, h);
                        Some((out, w, h))
                    });
                    small.map(|(mut p, w, h)| {
                        round_corners(&mut p, w, h, THUMB_RADIUS);
                        (i, p, w, h)
                    })
                })
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
