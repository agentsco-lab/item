//! The lock screen's halves opening in depth: each half of the screen turns
//! about an upright axis, in perspective, and is gone edge on.
//!
//! - `saloon`: each half on hinges at its outer edge, as saloon doors, the
//!   inner edges going back first;
//! - `book`: both halves about the Duo's own hinge, folding back like a book
//!   closed away from you;
//! - `slide`: no turn, the halves slide out (lock.rs draws it);
//! - `pixels` (the default): no doors; the view pulls back from where it
//!   was touched, the table's squares smaller and smaller until they are
//!   the wallpaper's pixels - the desktop under it the same picture (as the
//!   first setup ends; lock.rs and the table draw it);
//! - `fly`: no doors; each panel's half flies up and away,
//!   growing a little smaller and fading, as a cover swiped up in the flow;
//! - `wave`: no doors; the lock screen goes in a wave from
//!   where it was touched - the reader's mark, or the PIN pad's OK - with a
//!   soft glowing edge (wave.frag), as a Pixel's unlock ripples out of its
//!   reader.
//!
//! The renderer draws flat rectangles, so a turning half is drawn as narrow
//! upright strips of a picture of it, each as tall as perspective makes its
//! distance from the eye, and each dimmed by how far back it has gone - as
//! item's phoc patch turned a window about the hinge (patches/phoc/0015:
//! 48 strips, the eye 1.6 widths away, the far edge dimmed to 0.6).
//!
//! `$XDG_RUNTIME_DIR/item-doors` ("<mode> <ms> <camera> <dim>", e.g.
//! "saloon 500 2.0 0.6"; mode "cycle" takes them all in turn) is read at
//! each unlock, to try other ways on the phone without a new build.

pub const STRIPS: usize = 48;
/// How long the squares take to become pixels, the halves to fly away,
/// and the others to go.
pub const PIXELS_NS: u64 = 1_100_000_000;
const FLY_NS: u64 = 480_000_000;
const WAVE_NS: u64 = 850_000_000;
/// How far up a flying half goes (its panel's heights), how much smaller
/// it grows.
pub const FLY_UP: f64 = 0.6;
pub const FLY_SHRINK: f64 = 0.1;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Mode {
    /// The squares become the wallpaper's pixels.
    Pixels,
    /// Each half flies up and away.
    Fly,
    /// The lock screen goes in a wave from where it was touched.
    Wave,
    Slide,
    Saloon,
    Book,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Style {
    pub mode: Mode,
    pub ns: u64,
    /// The eye's distance from the screen, in half-screen widths.
    pub camera: f64,
    /// How dark a half gets edge on.
    pub dim: f64,
}

impl Default for Style {
    fn default() -> Style {
        Style { mode: Mode::Pixels, ns: PIXELS_NS, camera: 2.0, dim: 0.6 }
    }
}

impl Style {
    /// The defaults, or what `$XDG_RUNTIME_DIR/item-doors` says.
    pub fn read() -> Style {
        let mut style = Style::default();
        let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR") else { return style };
        let Ok(text) = std::fs::read_to_string(std::path::Path::new(&dir).join("item-doors")) else { return style };
        let mut words = text.split_whitespace();
        style.mode = match words.next() {
            Some("slide") => Mode::Slide,
            Some("wave") => Mode::Wave,
            Some("fly") => Mode::Fly,
            Some("book") => Mode::Book,
            Some("saloon") => Mode::Saloon,
            // Each unlock the next of the three, to compare them.
            Some("cycle") => {
                static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
                [Mode::Pixels, Mode::Fly, Mode::Wave, Mode::Saloon, Mode::Book, Mode::Slide][NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed) % 6]
            }
            _ => Mode::Pixels,
        };
        style.ns = match style.mode {
            Mode::Pixels => PIXELS_NS,
            Mode::Fly => FLY_NS,
            _ => WAVE_NS,
        };
        if let Some(ms) = words.next().and_then(|w| w.parse::<u64>().ok()).filter(|m| (50..=3000).contains(m)) {
            style.ns = ms * 1_000_000;
        }
        if let Some(c) = words.next().and_then(|w| w.parse::<f64>().ok()).filter(|c| (0.2..=20.0).contains(c)) {
            style.camera = c;
        }
        if let Some(d) = words.next().and_then(|w| w.parse::<f64>().ok()).filter(|d| (0.0..=1.0).contains(d)) {
            style.dim = d;
        }
        style
    }
}

/// One strip of a turning half, logical px: the picture's columns
/// `src_x0..src_x1` drawn over `x0..x1`, `height` tall about the middle,
/// under black at `dim`.
#[derive(Clone, Copy, Debug)]
pub struct Strip {
    pub src_x0: f64,
    pub src_x1: f64,
    pub x0: f64,
    pub x1: f64,
    pub height: f64,
    pub dim: f64,
}

/// The strips of both halves, `k` (0..1) of the way open, for a screen
/// `width` wide whose halves meet at `middle`, `height` tall.
pub fn strips(style: &Style, k: f64, width: f64, middle: f64, height: f64) -> Vec<Strip> {
    let angle = k.clamp(0.0, 1.0) * std::f64::consts::FRAC_PI_2;
    let (cos, sin) = (angle.cos(), angle.sin());
    let mut out = Vec::with_capacity(2 * STRIPS);
    // Each half: where its axis is, which way it reaches from the axis, how
    // wide it is, where the eye looks from (the projection's centre).
    let halves: [(f64, f64, f64); 2] = match style.mode {
        Mode::Saloon => [(0.0, 1.0, middle), (width, -1.0, width - middle)],
        _ => [(middle, -1.0, middle), (middle, 1.0, width - middle)],
    };
    let centre = middle;
    for (axis, dir, w) in halves {
        let eye = style.camera * w;
        // Where a point `d` from the axis lands, and how much it shrinks.
        let at = |d: f64| {
            let x = axis + dir * d * cos;
            let s = eye / (eye + d * sin);
            (centre + (x - centre) * s, s)
        };
        for i in 0..STRIPS {
            let (d0, d1) = (w * i as f64 / STRIPS as f64, w * (i + 1) as f64 / STRIPS as f64);
            let ((xa, _), (xb, _), (_, s)) = (at(d0), at(d1), at((d0 + d1) / 2.0));
            let (sa, sb) = (axis + dir * d0, axis + dir * d1);
            out.push(Strip {
                src_x0: sa.min(sb),
                src_x1: sa.max(sb),
                x0: xa.min(xb),
                x1: xa.max(xb),
                height: height * s,
                dim: style.dim * sin * (0.35 + 0.65 * (d0 + d1) / 2.0 / w),
            });
        }
    }
    out
}
