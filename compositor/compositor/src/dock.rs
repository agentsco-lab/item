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
/// item's slab colour, opaque: as rgba(38,48,43,0.92) came out over the
/// desktop's black - opaque, so the halves and the neck between them can
/// overlap without darker seams.
const SLAB: [u8; 4] = [37, 46, 42, 255];
/// How long a move takes (item's MOVE_CROSS_MS).
const MOVE_NS: u64 = 440_000_000;
/// The hinge as the halves cross it: a tunnel an icon long (item's TUNNEL_PX).
const TUNNEL: f64 = (ICON + 2) as f64;
/// The arriving half's inner padding, tucked under where the two meet
/// (item's JOIN_TUCK): their icons then stand GAP apart, as within a half.
const TUCK: f64 = (PAD - (GAP - PAD)) as f64;
/// The last of the way to the other half, taken slowly, at an even speed,
/// over the last part of the crossing (item's CONTACT_PX, CONTACT_SHARE).
const CONTACT_PX: f64 = 24.0;
const CONTACT_SHARE: f64 = 0.35;
/// The bump on arriving beside the other half (item's SPRING_MS, SPRING_PX,
/// SPRING_PUSH_MAX, SQUASH_MAX, STICK_PULL).
const SPRING_NS: u64 = 420_000_000;
const SPRING_PX: f64 = 20.0;
const PUSH_MAX: f64 = 8.0;
const SQUASH_MAX: f64 = 0.12;
const STICK_PULL: f64 = 1.6;
/// The gap at which the inner corners are round again (item's MEET_ROUND),
/// and at which the neck between the halves breaks (half item's MERGE_PX).
const MEET_ROUND: f64 = 8.0;
const NECK_BREAK: f64 = 14.0;
/// The inner corner's radii a slab is drawn with, one texture each.
const RADII: [f64; 6] = [0.0, 4.0, 8.0, 12.0, 16.0, 20.0];
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
    /// The slab with its inner corners (the right ones for the left half,
    /// the left for the right) at each of RADII; its outer ones round.
    slabs: Vec<MemoryRenderBuffer>,
    /// The slab's size, logical px.
    size: (i32, i32),
    /// The icon a finger is on, and the finger.
    pressed: Option<(usize, TouchSlot, Point<f64, Logical>)>,
}

/// A half's place at a moment: its slab's left edge and top, logical px;
/// how much of its inner padding is tucked; its inner corners' radius; and
/// its squash in the bump - a width scale about an x anchor.
#[derive(Clone, Copy)]
struct Place {
    x: f64,
    y: f64,
    tuck: f64,
    radius: f64,
    scale: f64,
    anchor: f64,
}

struct Move {
    from: Mode,
    to: Mode,
    start_ns: u64,
}

/// The halves coming in from the sides after an unlock (item's RISE_MS).
const RISE_NS: u64 = 360_000_000;

pub struct Dock {
    /// When the halves start coming in from the sides (an unlock).
    rise: Option<u64>,
    halves: Vec<Half>,
    /// The neck between the halves as they meet, as last drawn: its size.
    neck: std::cell::RefCell<Option<((i32, i32, i32), MemoryRenderBuffer)>>,
    dot: MemoryRenderBuffer,
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
                let slabs = RADII.iter().map(|&r| if p == 0 { slab(w, h, RADIUS as f64, r) } else { slab(w, h, r, RADIUS as f64) }).collect();
                Half { apps, slabs, size: (w, h), pressed: None }
            })
            .collect();
        Dock { rise: None, halves, neck: Default::default(), dot: crate::grid::running_dot(), mode: Mode::Both, shown: Mode::Both, moving: None }
    }

    /// Each half's place in a mode (item's `_pane_targets`).
    fn targets(&self, mode: Mode) -> [Place; 2] {
        let (lw, h) = self.halves[0].size;
        let rw = self.halves[1].size.0;
        let (width, height) = (layout::LAYOUT.0 as f64, layout::LAYOUT.1 as f64);
        let y = height - (MARGIN + h) as f64;
        let place = |x: f64, tuck: f64, joined: bool| Place { x, y, tuck, radius: if joined { 0.0 } else { RADIUS as f64 }, scale: 1.0, anchor: 0.0 };
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

    /// How long a move takes: the crossing, and the bump when the halves
    /// meet from apart.
    fn duration(m: &Move) -> u64 {
        MOVE_NS + if Self::bumps(m) { SPRING_NS } else { 0 }
    }

    fn bumps(m: &Move) -> bool {
        m.from == Mode::Both && matches!(m.to, Mode::On(_))
    }

    /// The halves off the screen's sides, coming in from `at` over RISE_NS,
    /// easing out (item's rise after an unlock, #72).
    pub fn rise(&mut self, at: u64) {
        self.rise = Some(at);
    }

    /// Each half's place at `frame_ns`: its path, and the rise over it.
    fn places(&self, frame_ns: u64) -> [Place; 2] {
        let mut p = self.path(frame_ns);
        if let Some(at) = self.rise {
            let k = (frame_ns.saturating_sub(at) as f64 / RISE_NS as f64).clamp(0.0, 1.0);
            let off = 1.0 - (1.0 - (1.0 - k).powi(3));
            let (w0, w1) = (self.halves[0].size.0 as f64, self.halves[1].size.0 as f64);
            let width = layout::LAYOUT.0 as f64;
            p[0].x -= (p[0].x + w0 + 10.0) * off;
            p[1].x += (width - p[1].x + 10.0) * off;
            p[0].anchor = p[0].x;
            p[1].anchor = p[1].x + w1;
        }
        p
    }

    /// Each half's place on its path at `frame_ns` (item's `_pane_path`, `_spring`).
    fn path(&self, frame_ns: u64) -> [Place; 2] {
        let Some(m) = &self.moving else { return self.targets(self.mode) };
        let all = (frame_ns.saturating_sub(m.start_ns) as f64 / Self::duration(m) as f64).clamp(0.0, 1.0);
        let travel = MOVE_NS as f64 / Self::duration(m) as f64;
        let (k, bump) = if all >= travel && Self::bumps(m) { (1.0, Some((all - travel) / (1.0 - travel))) } else { ((all / travel).min(1.0), None) };
        let (a, b) = (self.targets(m.from), self.targets(m.to));
        let joined = |mode: Mode| matches!(mode, Mode::On(_));
        let meeting = joined(m.from) != joined(m.to) && m.from != Mode::Hidden && m.to != Mode::Hidden;
        let e = if meeting {
            // The half that crosses: the one whose panel it is not.
            let on = if let Mode::On(p) = if joined(m.to) { m.to } else { m.from } { p } else { 0 };
            let mover = 1 - on;
            let dist = (squeeze(b[mover].x) - squeeze(a[mover].x)).abs();
            let delta = if dist > 0.0 { (CONTACT_PX / dist).min(0.5) } else { 0.0 };
            if joined(m.to) { contact_ease(k, delta) } else { 1.0 - contact_ease(1.0 - k, delta) }
        } else if k < 0.5 {
            4.0 * k * k * k
        } else {
            1.0 - (-2.0 * k + 2.0).powi(3) / 2.0
        };
        let mut out = b;
        for i in 0..2 {
            out[i] = Place { x: lerp_x(a[i].x, b[i].x, e), y: a[i].y + (b[i].y - a[i].y) * e, tuck: 0.0, radius: RADIUS as f64, scale: 1.0, anchor: 0.0 };
        }
        // The arriving (or leaving) half tucks its inner padding only as far
        // as it would run over the other: whole and round while apart, cut
        // square in the last TUCK px, as if sliding under.
        let (w0, w1) = (self.halves[0].size.0 as f64, self.halves[1].size.0 as f64);
        let overlap = (out[0].x + w0 - out[1].x).clamp(0.0, TUCK);
        let tucker = if a[1].tuck > 0.0 || b[1].tuck > 0.0 { 1 } else { 0 };
        if a[tucker].tuck > 0.0 || b[tucker].tuck > 0.0 {
            out[tucker].tuck = overlap;
        }
        // The bump: the half that arrives pushes the one it meets, which is
        // squashed against its screen edge and pushed on a little; then the
        // arriving one rebounds, a small gap opening, and settles.
        if let (Some(u), Mode::On(stayer)) = (bump, m.to) {
            let mover = 1 - stayer;
            let sign = if mover == 1 { -1.0 } else { 1.0 };
            let width = if stayer == 0 { w0 } else { w1 };
            let wave = SPRING_PX * (-1.5 * u).exp() * (std::f64::consts::TAU * u).sin();
            if wave > 0.0 {
                let squash = (wave * 0.9).min(SQUASH_MAX * width);
                let push = (wave * 0.35).min(PUSH_MAX);
                out[stayer].x += sign * push;
                out[mover].x += sign * (push + squash);
                out[stayer].scale = 1.0 - squash / width;
            } else {
                out[mover].x += sign * wave * STICK_PULL;
            }
        }
        // Squashed about its screen edge: the left half's left, the right's right.
        out[0].anchor = out[0].x;
        out[1].anchor = out[1].x + w1;
        // The inner corners by the gap between the visible edges: square
        // where they touch, round again from MEET_ROUND apart.
        if meeting || matches!(self.mode, Mode::On(_)) {
            let r = RADIUS as f64 * (self.gap(&out) / MEET_ROUND).clamp(0.0, 1.0);
            out[0].radius = r;
            out[1].radius = r;
        }
        out
    }

    /// The gap between the left half's visible right edge and the right
    /// half's visible left edge, squash and tuck counted.
    fn gap(&self, p: &[Place; 2]) -> f64 {
        let w0 = self.halves[0].size.0 as f64;
        let right0 = p[0].anchor + (p[0].x + w0 - p[0].anchor) * p[0].scale - p[0].tuck;
        let left1 = p[1].anchor + (p[1].x - p[1].anchor) * p[1].scale + p[1].tuck;
        left1 - right0
    }

    /// The panel the dock stands on alone, where the desktop's clock goes:
    /// the right one when both are free, none when neither is.
    pub fn home_panel(&self) -> Option<usize> {
        match self.mode {
            Mode::Both => Some(1),
            Mode::On(p) => Some(p),
            Mode::Hidden => None,
        }
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

    /// Hidden at once, no motion: the ribbon carried the halves off with
    /// their pages, and they rise again where the panels now let them.
    pub fn hide_now(&mut self) {
        if self.mode != Mode::Hidden {
            self.shown = self.mode;
        }
        self.mode = Mode::Hidden;
        self.moving = None;
        for half in &mut self.halves {
            half.pressed = None;
        }
    }

    /// After a frame for `frame_ns`: whether the halves are still moving.
    pub fn settle(&mut self, frame_ns: u64) -> bool {
        if let Some(at) = self.rise {
            if frame_ns >= at + RISE_NS {
                self.rise = None;
            } else {
                return true;
            }
        }
        match &self.moving {
            Some(m) if frame_ns >= m.start_ns + Self::duration(m) => {
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
            .flat_map(|h| h.slabs.iter().chain([&self.dot]).chain(h.apps.iter().flat_map(|a| a.icon.iter().chain(a.large.iter()))))
            .filter(|b| MemoryRenderBufferRenderElement::from_buffer(renderer, (0.0, 0.0), b, None, None, None, Kind::Unspecified).is_ok())
            .count()
    }

    /// What the dock draws for a frame shown at `frame_ns`, topmost first.
    pub fn elements(&self, renderer: &mut GlesRenderer, frame_ns: u64, running: &[String]) -> Vec<ShellElement> {
        let mut out = Vec::new();
        let places = self.places(frame_ns);
        let bottom = layout::LAYOUT.1 as f64;
        // Whole physical px, so a slab at rest is sharp.
        let px = |v: f64| (v * SCALE as f64).round();
        let mut push = |out: &mut Vec<ShellElement>, buffer: &MemoryRenderBuffer, x: f64, y: f64, w: Option<f64>, src: Option<Rectangle<f64, Logical>>, alpha: f32| {
            let size = w.map(|w| smithay::utils::Size::<i32, Logical>::from((w.round() as i32, src.map(|s| s.size.h).unwrap_or(0.0) as i32)));
            match MemoryRenderBufferRenderElement::from_buffer(renderer, (px(x), px(y)), buffer, Some(alpha), src, size, Kind::Unspecified) {
                Ok(e) => out.push(ShellElement::Text(e)),
                Err(e) => tracing::warn!("dock: {e}"),
            }
        };
        // The neck between the halves as they meet (phoc's smooth union in
        // item): it fills the gap and the inner corners' notches, pinching
        // in the middle as the gap grows, and breaks at NECK_BREAK.
        let gap = self.gap(&places);
        let meeting = self.moving.as_ref().is_some_and(|m| matches!(m.to, Mode::On(_)) || matches!(m.from, Mode::On(_)));
        if meeting && gap > 0.5 && gap < NECK_BREAK && places[0].y < bottom {
            let (w0, h) = (self.halves[0].size.0 as f64, self.halves[0].size.1 as f64);
            let right0 = places[0].anchor + (places[0].x + w0 - places[0].anchor) * places[0].scale - places[0].tuck;
            let o = places[0].radius * (1.0 - gap / NECK_BREAK);
            let key = ((gap * 2.0).round() as i32, (o * 2.0).round() as i32, h as i32);
            let mut neck = self.neck.borrow_mut();
            if neck.as_ref().is_none_or(|(k, _)| *k != key) {
                *neck = Some((key, neck_texture(gap, o, h)));
            }
            if let Some((_, b)) = neck.as_ref() {
                push(&mut out, b, right0 - o, places[0].y, None, None, 1.0);
            }
        }
        for (h, half) in self.halves.iter().enumerate() {
            let p = places[h];
            if p.y >= bottom || half.apps.is_empty() {
                continue;
            }
            // Squashed about its anchor: x and widths scaled.
            let tx = |x: f64| p.anchor + (x - p.anchor) * p.scale;
            for (i, app) in half.apps.iter().enumerate() {
                let c = self.cell(i, &p);
                // A dot under a running app, in the slab's bottom padding.
                if app.ids.iter().any(|id| running.contains(id)) {
                    let d = crate::grid::DOT;
                    push(&mut out, &self.dot, tx(c.loc.x + (c.size.w - d) / 2.0), c.loc.y + c.size.h + (PAD as f64 - d) / 2.0, None, None, 1.0);
                }
                if let Some(icon) = &app.icon {
                    // A pressed icon dims under the finger.
                    let alpha = if half.pressed.is_some_and(|(q, _, _)| q == i) { 0.55 } else { 1.0 };
                    let size = (p.scale < 1.0).then_some(ICON as f64 * p.scale);
                    let src = size.map(|_| Rectangle::new((0.0, 0.0).into(), (ICON as f64, ICON as f64).into()));
                    push(&mut out, icon, tx(c.loc.x), c.loc.y, size, src, alpha);
                }
            }
            // The arriving half's inner padding is cut off where the two
            // meet: the left side of the right half, the right of the left.
            let (w, ht) = (half.size.0 as f64, half.size.1 as f64);
            // A tucked half reaches 1 px under the other: no seam of the
            // background between them, the slabs being the same colour.
            let tuck = if p.tuck > 0.0 { (p.tuck - 1.0).round().max(0.0) } else { 0.0 };
            let step = (p.radius / 4.0).round().clamp(0.0, (RADII.len() - 1) as f64) as usize;
            let slab = &half.slabs[step];
            let (sx, dx) = if h == 1 { (tuck, p.x + tuck) } else { (0.0, p.x) };
            let src = Rectangle::new((sx, 0.0).into(), (w - tuck, ht).into());
            let scaled = (p.scale < 1.0 || tuck > 0.0).then_some((w - tuck) * p.scale);
            push(&mut out, slab, tx(dx), p.y, scaled, scaled.map(|_| src), 1.0);
        }
        out
    }
}

/// item's contact ease: 0..1 over k, easing in, then the last `delta` of
/// the way at an even speed over the last CONTACT_SHARE of the time - it
/// arrives moving, and slowly.
fn contact_ease(k: f64, delta: f64) -> f64 {
    let t1 = 1.0 - CONTACT_SHARE;
    let v = delta / CONTACT_SHARE;
    if k >= t1 {
        return (1.0 - delta + v * (k - t1)).min(1.0);
    }
    let s = k / t1;
    (-2.0 * s.powi(3) + 3.0 * s * s) * (1.0 - delta) + (s.powi(3) - s * s) * v * t1
}

/// The neck: `gap` wide between the halves, reaching `o` into each under
/// their inner corners, `h` high, its top and bottom dipping in the middle
/// the more the wider the gap.
fn neck_texture(gap: f64, o: f64, h: f64) -> MemoryRenderBuffer {
    let s = SCALE as f64;
    let w = gap + 2.0 * o;
    let (pw, ph) = ((w * s).ceil().max(1.0) as u32, (h * s).round() as u32);
    let mut pixmap = tiny_skia::Pixmap::new(pw, ph).expect("neck");
    let d = (h / 2.0) * (gap / NECK_BREAK).powf(1.5) * s;
    let (fo, fg, fh) = ((o * s) as f32, (gap * s) as f32, ph as f32);
    let d = d as f32;
    let mut pb = tiny_skia::PathBuilder::new();
    pb.move_to(0.0, 0.0);
    pb.line_to(fo, 0.0);
    pb.quad_to(fo + fg / 2.0, 2.0 * d, fo + fg, 0.0);
    pb.line_to(pw as f32, 0.0);
    pb.line_to(pw as f32, fh);
    pb.line_to(fo + fg, fh);
    pb.quad_to(fo + fg / 2.0, fh - 2.0 * d, fo, fh);
    pb.line_to(0.0, fh);
    pb.close();
    if let Some(path) = pb.finish() {
        let mut paint = tiny_skia::Paint::default();
        paint.set_color_rgba8(SLAB[0], SLAB[1], SLAB[2], SLAB[3]);
        paint.anti_alias = true;
        pixmap.fill_path(&path, &paint, tiny_skia::FillRule::Winding, tiny_skia::Transform::identity(), None);
    }
    MemoryRenderBuffer::from_slice(pixmap.data(), Fourcc::Abgr8888, (pw as i32, ph as i32), SCALE, Transform::Normal, None)
}

/// From x0 to x1 at e, through the hinge as through a tunnel TUNNEL long
/// (item's `_lerp_x`): a half's left edge is moved in a space where the
/// hinge is that long, so an icon goes all the way in before any of it
/// comes out on the other panel. The hinge's columns are not on the
/// screen, so what is drawn there is not seen.
/// A left edge in the space where the hinge is a tunnel TUNNEL long.
fn squeeze(x: f64) -> f64 {
    let panels = layout::panels();
    let half = panels[0].size.w as f64;
    let gap = (panels[1].loc.x - panels[0].size.w) as f64;
    if x < half {
        x
    } else if x < half + gap {
        half + (x - half) * TUNNEL / gap
    } else {
        x - gap + TUNNEL
    }
}

pub fn lerp_x(x0: f64, x1: f64, e: f64) -> f64 {
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

/// A dock item from its desktop file (apps.rs), with its icons at the
/// dock's and the curtain's sizes.
fn app(desktop: &str) -> Option<App> {
    let e = crate::apps::entry(desktop)?;
    let small = e.icon.as_deref().and_then(|n| crate::apps::icon(n, ICON));
    let large = e.icon.as_deref().and_then(|n| crate::apps::icon(n, crate::curtain::ICON));
    Some(App { name: e.name, exec: e.exec, ids: e.ids, icon: small, large })
}

/// The slab: a rectangle `w` by `h` logical px, its left corners of radius
/// `rl` and its right ones of `rr`.
fn slab(w: i32, h: i32, rl: f64, rr: f64) -> MemoryRenderBuffer {
    let (pw, ph) = ((w * SCALE) as u32, (h * SCALE) as u32);
    let mut pixmap = tiny_skia::Pixmap::new(pw.max(1), ph.max(1)).expect("slab pixmap");
    let r = RADIUS * SCALE as f32;
    let (fw, fh) = (pw as f32, ph as f32);
    let mut pb = tiny_skia::PathBuilder::new();
    // Quarter circles as cubics; a square side's corners have none.
    let (rl, rr) = ((rl * SCALE as f64) as f32, (rr * SCALE as f64) as f32);
    let _ = r;
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
