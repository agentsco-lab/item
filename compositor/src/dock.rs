//! The dock: item's two halves, drawn by the compositor, and item's way of
//! moving them (sfduo-dock's `_pane_targets`, `_pane_path`).
//!
//! - Both panels free: each half is a rounded slab at its panel's outer
//!   bottom corner, under the thumbs of two hands holding the open device.
//! - One panel taken: the two stand side by side on the free one. The half
//!   whose panel it is stays where it stood; the other comes across the
//!   hinge and stands beside it, square where they meet, its inner padding
//!   tucked under.
//! - Both taken: they dip below the bottom edge where they stand.
//!
//! A move takes 440 ms, eased in and out. The hinge is crossed as a tunnel
//! an icon long, so an icon goes all the way in before any of it comes out.
//! A tap on an icon launches the app onto the panel it was tapped on.
//!
//! The apps are item's: `~/.config/sfduo/dock.json` (`{"left": [...],
//! "right": [...]}`, desktop file names), else a default. Icons come from the
//! hicolor theme, SVG through resvg or PNG, rasterized once at start at the
//! output's scale.

use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement};
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::input::TouchSlot;
use smithay::utils::{Logical, Point, Rectangle, Transform};

use resvg::tiny_skia;

use crate::layout::{self, SCALE};
use crate::shade::ShellElement;

/// An icon's side, logical px.
const ICON: i32 = 48;
/// Between icons, and around them inside the slab.
const GAP: i32 = 14;
const PAD: i32 = 12;
/// The slab's distance from the panel's outer and bottom edges.
const MARGIN: i32 = 10;
const RADIUS: f32 = 20.0;
const SLAB: [u8; 4] = [38, 48, 43, 235];
/// How long a move takes (item's MOVE_CROSS_MS).
const MOVE_NS: u64 = 440_000_000;
/// The hinge as the halves cross it: a tunnel an icon long (item's TUNNEL_PX).
const TUNNEL: f64 = (ICON + 2) as f64;
/// The arriving half's inner padding, tucked under where the two meet
/// (item's JOIN_TUCK): their icons then stand GAP apart, as within a half.
const TUCK: f64 = (PAD - (GAP - PAD)) as f64;
/// A touch that travels this far is not a tap.
const TAP: f64 = 12.0;

const DEFAULT: [&[&str]; 2] = [
    &["org.gnome.Calls.desktop", "sm.puri.Chatty.desktop"],
    &["org.gnome.Epiphany.desktop", "org.gnome.Settings.desktop", "org.gnome.Weather.desktop"],
];

struct App {
    name: String,
    exec: String,
    /// The names its windows may give as their app id: the desktop file's
    /// name and its StartupWMClass.
    ids: Vec<String>,
    icon: Option<MemoryRenderBuffer>,
    /// The icon at the launch curtain's size.
    large: Option<MemoryRenderBuffer>,
}

/// Where the halves stand.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Mode {
    /// Each at its own panel's outer edge.
    Both,
    /// Side by side on this panel.
    On(usize),
    /// Below the bottom edge.
    Hidden,
}

struct Half {
    apps: Vec<App>,
    /// All corners round, and square on the side that meets the other half
    /// (the right side for the left half, the left for the right).
    slab: MemoryRenderBuffer,
    slab_joined: MemoryRenderBuffer,
    /// The slab's size, logical px.
    size: (i32, i32),
    /// The icon a finger is on, and the finger.
    pressed: Option<(usize, TouchSlot, Point<f64, Logical>)>,
}

/// A half's place at a moment: its slab's left edge and top, logical px;
/// how much of its inner padding is tucked; whether it is square where the
/// halves meet.
#[derive(Clone, Copy)]
struct Place {
    x: f64,
    y: f64,
    tuck: f64,
    joined: bool,
}

struct Move {
    from: Mode,
    to: Mode,
    start_ns: u64,
}

pub struct Dock {
    halves: Vec<Half>,
    mode: Mode,
    /// The mode before the last Hidden, whose places Hidden dips from.
    shown: Mode,
    moving: Option<Move>,
}

/// What a touch on the dock asks for.
pub enum Tap {
    /// Launch this command on this panel; the app's icon for the curtain,
    /// and the app ids its windows may have (a window put away comes back
    /// instead).
    Launch(String, usize, Option<MemoryRenderBuffer>, Vec<String>),
}

impl Dock {
    pub fn new() -> Dock {
        let lists = config();
        let halves = lists
            .iter()
            .enumerate()
            .map(|(p, names)| {
                let apps: Vec<App> = names.iter().filter_map(|n| app(n)).collect();
                let n = apps.len() as i32;
                let w = 2 * PAD + n * ICON + (n - 1).max(0) * GAP;
                let h = 2 * PAD + ICON;
                tracing::info!(
                    "dock {}: {}",
                    if p == 0 { "left" } else { "right" },
                    apps.iter().map(|a| format!("{}{}", a.name, if a.icon.is_some() { "" } else { " (no icon)" })).collect::<Vec<_>>().join(", ")
                );
                // The left half meets the other with its right side, the
                // right half with its left.
                let joined = if p == 0 { slab(w, h, false, true) } else { slab(w, h, true, false) };
                Half { apps, slab: slab(w, h, false, false), slab_joined: joined, size: (w, h), pressed: None }
            })
            .collect();
        Dock { halves, mode: Mode::Both, shown: Mode::Both, moving: None }
    }

    /// Each half's place in a mode (item's `_pane_targets`).
    fn targets(&self, mode: Mode) -> [Place; 2] {
        let (lw, h) = self.halves[0].size;
        let rw = self.halves[1].size.0;
        let (width, height) = (layout::LAYOUT.0 as f64, layout::LAYOUT.1 as f64);
        let y = height - (MARGIN + h) as f64;
        let place = |x: f64, tuck: f64, joined: bool| Place { x, y, tuck, joined };
        match mode {
            Mode::Both => [place(MARGIN as f64, 0.0, false), place(width - (MARGIN + rw) as f64, 0.0, false)],
            // The left half stays at the left edge; the right one arrives
            // beside it, its padding tucked under.
            Mode::On(0) => {
                let l = MARGIN as f64;
                [place(l, 0.0, true), place(l + lw as f64 - TUCK, TUCK, true)]
            }
            Mode::On(_) => {
                let end = width - MARGIN as f64;
                [place(end - (rw + lw) as f64 + TUCK, TUCK, true), place(end - rw as f64, 0.0, true)]
            }
            Mode::Hidden => {
                let mut t = self.targets(self.shown);
                for p in &mut t {
                    p.y = height + 4.0;
                }
                t
            }
        }
    }

    /// Each half's place at `frame_ns` (item's `_pane_path`).
    fn places(&self, frame_ns: u64) -> [Place; 2] {
        let Some(m) = &self.moving else { return self.targets(self.mode) };
        let k = (frame_ns.saturating_sub(m.start_ns) as f64 / MOVE_NS as f64).clamp(0.0, 1.0);
        // Cubic ease in and out: off gently, parked gently.
        let e = if k < 0.5 { 4.0 * k * k * k } else { 1.0 - (-2.0 * k + 2.0).powi(3) / 2.0 };
        let (a, b) = (self.targets(m.from), self.targets(m.to));
        let mut out = b;
        for i in 0..2 {
            out[i] = Place {
                x: lerp_x(a[i].x, b[i].x, e),
                y: a[i].y + (b[i].y - a[i].y) * e,
                tuck: 0.0,
                joined: false,
            };
        }
        // The arriving (or leaving) half tucks its inner padding only as far
        // as it would run over the other: whole and round while apart, cut
        // square in the last TUCK px, as if sliding under.
        let overlap = (out[0].x + self.halves[0].size.0 as f64 - out[1].x).clamp(0.0, TUCK);
        let tucker = if a[1].tuck > 0.0 || b[1].tuck > 0.0 { 1 } else { 0 };
        if a[tucker].tuck > 0.0 || b[tucker].tuck > 0.0 {
            out[tucker].tuck = overlap;
        }
        // Square where they touch, round while apart: the gap between the
        // left half's visible right edge and the right half's visible left
        // edge (a tucked padding is not drawn).
        let gap = out[1].x + out[1].tuck - (out[0].x + self.halves[0].size.0 as f64 - out[0].tuck);
        let touching = gap.abs() < 1.0;
        out[0].joined = touching;
        out[1].joined = touching;
        out
    }

    /// Where the halves go for the panels windows have. Returns whether they
    /// set off.
    pub fn follow(&mut self, taken: [bool; 2], now_ns: u64) -> bool {
        let mode = match taken {
            [false, false] => Mode::Both,
            [false, true] => Mode::On(0),
            [true, false] => Mode::On(1),
            [true, true] => Mode::Hidden,
        };
        if mode == self.mode {
            return false;
        }
        // A move still under way is cut short: the new one starts from the
        // old one's target, so the halves may jump (rare: panels change
        // hands less often than every 440 ms).
        let from = self.mode;
        if mode != Mode::Hidden {
            self.shown = mode;
        } else if from != Mode::Hidden {
            self.shown = from;
        }
        tracing::debug!("dock: {from:?} -> {mode:?}");
        self.moving = Some(Move { from, to: mode, start_ns: now_ns });
        self.mode = mode;
        for half in &mut self.halves {
            half.pressed = None;
        }
        true
    }

    /// After a frame for `frame_ns`: whether the halves are still moving.
    pub fn settle(&mut self, frame_ns: u64) -> bool {
        match &self.moving {
            Some(m) if frame_ns >= m.start_ns + MOVE_NS => {
                self.moving = None;
                false
            }
            Some(_) => true,
            None => false,
        }
    }

    /// Icon `i` of a half at `place`, logical px.
    fn cell(&self, i: usize, place: &Place) -> Rectangle<f64, Logical> {
        Rectangle::new(
            (place.x + (PAD + i as i32 * (ICON + GAP)) as f64, place.y + PAD as f64).into(),
            (ICON as f64, ICON as f64).into(),
        )
    }

    /// A touch down: taken if it lands on a half standing still. Returns
    /// whether it was.
    pub fn down(&mut self, slot: TouchSlot, pos: Point<f64, Logical>) -> bool {
        if self.moving.is_some() || self.mode == Mode::Hidden {
            return false;
        }
        let places = self.targets(self.mode);
        for h in 0..2 {
            let (w, ht) = self.halves[h].size;
            let p = places[h];
            let slab = Rectangle::<f64, Logical>::new((p.x + p.tuck, p.y).into(), (w as f64 - p.tuck, ht as f64).into());
            if !slab.contains(pos) {
                continue;
            }
            let icon = (0..self.halves[h].apps.len()).find(|&i| self.cell(i, &p).contains(pos));
            self.halves[h].pressed = icon.map(|i| (i, slot, pos));
            return true;
        }
        false
    }

    pub fn holds(&self, slot: TouchSlot) -> bool {
        self.halves.iter().any(|h| h.pressed.is_some_and(|(_, s, _)| s == slot))
    }

    /// A finger on an icon that moves away is no longer a tap on it.
    pub fn motion(&mut self, slot: TouchSlot, pos: Point<f64, Logical>) {
        for half in &mut self.halves {
            if let Some((_, s, at)) = half.pressed {
                if s == slot && ((pos.x - at.x).powi(2) + (pos.y - at.y).powi(2)).sqrt() > TAP {
                    half.pressed = None;
                }
            }
        }
    }

    /// The finger lets go: a tap launches the app onto the panel it was on.
    pub fn up(&mut self, slot: TouchSlot) -> Option<Tap> {
        for half in &mut self.halves {
            if let Some((i, s, at)) = half.pressed {
                if s == slot {
                    half.pressed = None;
                    let panel = layout::panel_at(at).unwrap_or(0);
                    let app = &half.apps[i];
                    return Some(Tap::Launch(app.exec.clone(), panel, app.large.clone(), app.ids.clone()));
                }
            }
        }
        None
    }

    pub fn cancel(&mut self) {
        for half in &mut self.halves {
            half.pressed = None;
        }
    }

    /// Uploads the slabs and icons; returns how many.
    pub fn warm_up(&self, renderer: &mut GlesRenderer) -> usize {
        self.halves
            .iter()
            .flat_map(|h| [&h.slab, &h.slab_joined].into_iter().chain(h.apps.iter().flat_map(|a| a.icon.iter().chain(a.large.iter()))))
            .filter(|b| MemoryRenderBufferRenderElement::from_buffer(renderer, (0.0, 0.0), b, None, None, None, Kind::Unspecified).is_ok())
            .count()
    }

    /// What the dock draws for a frame shown at `frame_ns`, topmost first.
    pub fn elements(&self, renderer: &mut GlesRenderer, frame_ns: u64) -> Vec<ShellElement> {
        let mut out = Vec::new();
        let places = self.places(frame_ns);
        let bottom = layout::LAYOUT.1 as f64;
        for (h, half) in self.halves.iter().enumerate() {
            let p = places[h];
            if p.y >= bottom || half.apps.is_empty() {
                continue;
            }
            // Whole physical px, so a slab at rest is sharp.
            let px = |v: f64| (v * SCALE as f64).round();
            let mut push = |buffer: &MemoryRenderBuffer, x: f64, y: f64, src: Option<Rectangle<f64, Logical>>, alpha: f32| {
                let size = src.map(|r| r.size.to_i32_round());
                match MemoryRenderBufferRenderElement::from_buffer(renderer, (px(x), px(y)), buffer, Some(alpha), src, size, Kind::Unspecified) {
                    Ok(e) => out.push(ShellElement::Text(e)),
                    Err(e) => tracing::warn!("dock: {e}"),
                }
            };
            for (i, app) in half.apps.iter().enumerate() {
                if let Some(icon) = &app.icon {
                    // A pressed icon dims under the finger.
                    let alpha = if half.pressed.is_some_and(|(q, _, _)| q == i) { 0.55 } else { 1.0 };
                    let c = self.cell(i, &p);
                    push(icon, c.loc.x, c.loc.y, None, alpha);
                }
            }
            // The arriving half's inner padding is cut off where the two
            // meet: the left side of the right half, the right of the left.
            let (w, ht) = half.size;
            let tuck = p.tuck.round();
            let slab = if p.joined { &half.slab_joined } else { &half.slab };
            if tuck > 0.0 {
                let (sx, dx) = if h == 1 { (tuck, p.x + tuck) } else { (0.0, p.x) };
                let src = Rectangle::new((sx, 0.0).into(), (w as f64 - tuck, ht as f64).into());
                push(slab, dx, p.y, Some(src), 1.0);
            } else {
                push(slab, p.x, p.y, None, 1.0);
            }
        }
        out
    }
}

/// From x0 to x1 at e, through the hinge as through a tunnel TUNNEL long
/// (item's `_lerp_x`): a half's left edge is moved in a space where the
/// hinge is that long, so an icon goes all the way in before any of it
/// comes out on the other panel. The hinge's columns are not on the
/// screen, so what is drawn there is not seen.
fn lerp_x(x0: f64, x1: f64, e: f64) -> f64 {
    let panels = layout::panels();
    let half = panels[0].size.w as f64;
    let gap = (panels[1].loc.x - panels[0].size.w) as f64;
    let squeeze = |x: f64| {
        if x < half {
            x
        } else if x < half + gap {
            half + (x - half) * TUNNEL / gap
        } else {
            x - gap + TUNNEL
        }
    };
    let unsqueeze = |c: f64| {
        if c < half {
            c
        } else if c < half + TUNNEL {
            half + (c - half) * gap / TUNNEL
        } else {
            c - TUNNEL + gap
        }
    };
    let (c0, c1) = (squeeze(x0), squeeze(x1));
    unsqueeze(c0 + (c1 - c0) * e)
}

/// The dock's apps: item's configuration, else the default.
fn config() -> [Vec<String>; 2] {
    let path = std::env::var("HOME").map(|h| format!("{h}/.config/sfduo/dock.json")).unwrap_or_default();
    let parsed = std::fs::read_to_string(&path).ok().and_then(|text| {
        // {"left": [...], "right": [...]}: two lists of quoted names, without
        // a JSON parser.
        let list = |key: &str| -> Option<Vec<String>> {
            let start = text.find(&format!("\"{key}\""))?;
            let open = start + text[start..].find('[')?;
            let close = open + text[open..].find(']')?;
            Some(text[open + 1..close].split(',').map(|s| s.trim().trim_matches('"').to_owned()).filter(|s| !s.is_empty()).collect())
        };
        Some([list("left")?, list("right")?])
    });
    match parsed {
        Some(lists) => {
            tracing::info!("dock: apps from {path}");
            lists
        }
        None => DEFAULT.map(|l| l.iter().map(|s| s.to_string()).collect()),
    }
}

/// An app from its desktop file: its name, its command without field codes,
/// and its icon.
fn app(desktop: &str) -> Option<App> {
    let home = std::env::var("HOME").unwrap_or_default();
    let dirs = [format!("{home}/.local/share/applications"), "/usr/local/share/applications".into(), "/usr/share/applications".into()];
    let text = dirs.iter().find_map(|d| std::fs::read_to_string(format!("{d}/{desktop}")).ok());
    let Some(text) = text else {
        tracing::warn!("dock: no {desktop}");
        return None;
    };
    // The [Desktop Entry] group only: actions come after it.
    let entry = text.split("\n[").next().unwrap_or(&text);
    let key = |k: &str| entry.lines().find_map(|l| l.strip_prefix(&format!("{k}="))).map(str::to_owned);
    let exec = key("Exec")?
        .split_whitespace()
        .filter(|w| !(w.starts_with('%') && w.len() == 2))
        .collect::<Vec<_>>()
        .join(" ");
    let name = key("Icon");
    let small = name.as_deref().and_then(|n| icon(n, ICON));
    let large = name.as_deref().and_then(|n| icon(n, crate::curtain::ICON));
    let mut ids = vec![desktop.trim_end_matches(".desktop").to_owned()];
    ids.extend(key("StartupWMClass"));
    Some(App { name: key("Name").unwrap_or_else(|| desktop.to_owned()), exec, ids, icon: small, large })
}

/// An icon from the hicolor theme, `size` logical px at the output's scale.
fn icon(name: &str, size: i32) -> Option<MemoryRenderBuffer> {
    let px = (size * SCALE) as u32;
    let base = "/usr/share/icons/hicolor";
    let pixmap = if let Ok(data) = std::fs::read(format!("{base}/scalable/apps/{name}.svg")) {
        let tree = resvg::usvg::Tree::from_data(&data, &resvg::usvg::Options::default()).ok()?;
        let size = tree.size();
        let scale = px as f32 / size.width().max(size.height());
        let mut pixmap = tiny_skia::Pixmap::new(px, px)?;
        resvg::render(&tree, tiny_skia::Transform::from_scale(scale, scale), &mut pixmap.as_mut());
        pixmap
    } else {
        // The PNG closest above the size, scaled down.
        let png = ["256x256", "192x192", "128x128", "96x96", "64x64", "48x48"]
            .iter()
            .find_map(|s| tiny_skia::Pixmap::load_png(format!("{base}/{s}/apps/{name}.png")).ok())?;
        let mut pixmap = tiny_skia::Pixmap::new(px, px)?;
        let scale = px as f32 / png.width().max(png.height()) as f32;
        pixmap.draw_pixmap(
            0,
            0,
            png.as_ref(),
            &tiny_skia::PixmapPaint { quality: tiny_skia::FilterQuality::Bicubic, ..Default::default() },
            tiny_skia::Transform::from_scale(scale, scale),
            None,
        );
        pixmap
    };
    // tiny-skia's pixels are premultiplied RGBA: R, G, B, A in memory, as
    // Abgr8888 is.
    Some(MemoryRenderBuffer::from_slice(pixmap.data(), Fourcc::Abgr8888, (px as i32, px as i32), SCALE, Transform::Normal, None))
}

/// The slab: a rectangle `w` by `h` logical px, its corners round but on
/// the sides asked square.
fn slab(w: i32, h: i32, square_left: bool, square_right: bool) -> MemoryRenderBuffer {
    let (pw, ph) = ((w * SCALE) as u32, (h * SCALE) as u32);
    let mut pixmap = tiny_skia::Pixmap::new(pw.max(1), ph.max(1)).expect("slab pixmap");
    let r = RADIUS * SCALE as f32;
    let (fw, fh) = (pw as f32, ph as f32);
    let mut pb = tiny_skia::PathBuilder::new();
    // Quarter circles as cubics; a square side's corners have none.
    let (rl, rr) = (if square_left { 0.0 } else { r }, if square_right { 0.0 } else { r });
    let (kl, kr) = (rl * 0.5523, rr * 0.5523);
    pb.move_to(rl, 0.0);
    pb.line_to(fw - rr, 0.0);
    pb.cubic_to(fw - rr + kr, 0.0, fw, rr - kr, fw, rr);
    pb.line_to(fw, fh - rr);
    pb.cubic_to(fw, fh - rr + kr, fw - rr + kr, fh, fw - rr, fh);
    pb.line_to(rl, fh);
    pb.cubic_to(rl - kl, fh, 0.0, fh - rl + kl, 0.0, fh - rl);
    pb.line_to(0.0, rl);
    pb.cubic_to(0.0, rl - kl, rl - kl, 0.0, rl, 0.0);
    pb.close();
    if let Some(path) = pb.finish() {
        let mut paint = tiny_skia::Paint::default();
        paint.set_color_rgba8(SLAB[0], SLAB[1], SLAB[2], SLAB[3]);
        paint.anti_alias = true;
        pixmap.fill_path(&path, &paint, tiny_skia::FillRule::Winding, tiny_skia::Transform::identity(), None);
    }
    MemoryRenderBuffer::from_slice(pixmap.data(), Fourcc::Abgr8888, (pw as i32, ph as i32), SCALE, Transform::Normal, None)
}
