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
//! The halves are drops of mercury (dock.frag), the mercury dock: as they
//! near they melt into one, as drops do, and part again with a thread
//! between them; going under the hinge a half is squashed flat, as jelly
//! through a slot; moving it stretches along its way, and after a move it
//! wobbles to rest. They are water: clear, a light ring along the edge,
//! light gathered inside along the bottom, a small sharp highlight, a soft
//! shadow; the icons seen through it a little larger (DOCK_SILVER=1: soft
//! silver instead). The icons are squashed with it.
//!
//! The dock is the screen's, not a page's: while the ribbon moves under a
//! finger (ribbon.rs), the same move follows it, between where the halves
//! stand for the pages on either side of the finger, and back if it goes
//! back.
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
const MEET_ROUND: f64 = 8.0;
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

/// The jelly: how flat a half goes under the hinge (its height, and its
/// width a little wider), and its wobble to rest after a move.
const FLATTEN: f64 = 0.72;
const WIDEN: f64 = 0.38;
/// Out from under the hinge, a drop gathers up with a bounce.
const POP: f64 = 0.14;
const WOBBLE_NS: u64 = 650_000_000;
const WOBBLE: f64 = 0.07;
const WOBBLE_PERIOD_NS: f64 = 260e6;
/// The drops: how far apart they start to melt together, px; their water.
const MELT: f64 = 26.0;
/// Water's body: a faint cool tint, mostly clear.
const BODY: [f32; 4] = [0.82, 0.9, 0.95, 0.14];
/// A drop's ends: round, half its height.
const DROP_RADIUS: f32 = 36.0;
/// How much a moving drop stretches along its way, per logical px per ms,
/// and at most; how fast the stretch follows the speed.
const STRETCH: f64 = 0.09;
const STRETCH_MAX: f64 = 0.16;
const STRETCH_FOLLOW: f64 = 0.35;
/// The icons seen through the water: a little larger.
const LENS: f64 = 1.04;
/// Alive after a touch: so long, then calming over the last part.
const ALIVE_NS: u64 = 15_000_000_000;
const CALM_NS: u64 = 2_000_000_000;

/// The halves coming in from the sides after an unlock (item's RISE_MS).
const RISE_NS: u64 = 360_000_000;

pub struct Dock {
    /// When the halves start coming in from the sides (an unlock).
    rise: Option<u64>,
    halves: Vec<Half>,
    dot: MemoryRenderBuffer,
    mode: Mode,
    /// The mode before the last Hidden, whose places Hidden dips from.
    shown: Mode,
    moving: Option<Move>,
    /// The move scrubbed by the ribbon: from, to, how far.
    scrub: Option<(Mode, Mode, f64)>,
    /// When the last move ended: the halves wobble to rest from it.
    landed: Option<u64>,
    /// Each half's x as last drawn and when, and its stretch from moving.
    last_x: std::cell::Cell<Option<(u64, [f64; 2])>>,
    stretch: std::cell::Cell<[f64; 2]>,
    /// Each half: whether it was flat under the hinge, and when it came out.
    flat: std::cell::Cell<[bool; 2]>,
    popped: std::cell::Cell<[u64; 2]>,
    /// The last touch on the screen: the drops are alive for a while after.
    pub touched: u64,
    /// The wallpaper's parallax (wallpaper.glsl), for what the drops see.
    pub shift: f64,
    /// The drops' shader (dock.frag) and its element, its uniforms as last
    /// set (set again only when they change).
    program: std::cell::RefCell<Option<smithay::backend::renderer::gles::GlesPixelProgram>>,
    drops: std::cell::RefCell<Option<(smithay::backend::renderer::gles::element::PixelShaderElement, Vec<f32>)>>,
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
        Dock { rise: None, halves, dot: crate::grid::running_dot(), mode: Mode::Both, shown: Mode::Both, moving: None, scrub: None, landed: None, last_x: Default::default(), stretch: Default::default(), flat: Default::default(), popped: Default::default(), touched: 0, shift: 0.0, program: Default::default(), drops: Default::default() }
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
        // Under the ribbon's finger: as far as it has gone, no bump.
        if let Some((from, to, k)) = self.scrub {
            return self.between(from, to, k, None, true);
        }
        let Some(m) = &self.moving else { return self.targets(self.mode) };
        let all = (frame_ns.saturating_sub(m.start_ns) as f64 / Self::duration(m) as f64).clamp(0.0, 1.0);
        let travel = MOVE_NS as f64 / Self::duration(m) as f64;
        let (k, bump) = if all >= travel && Self::bumps(m) { (1.0, Some((all - travel) / (1.0 - travel))) } else { ((all / travel).min(1.0), None) };
        self.between(m.from, m.to, k, bump, false)
    }

    /// The halves `k` of the way from `from` to `to`, eased as item's path
    /// (`linear`: as the finger has it, the contact's slowing only), and the
    /// bump after arriving.
    fn between(&self, from: Mode, to: Mode, k: f64, bump: Option<f64>, linear: bool) -> [Place; 2] {
        let (a, b) = (self.targets(from), self.targets(to));
        let joined = |mode: Mode| matches!(mode, Mode::On(_));
        let meeting = joined(from) != joined(to) && from != Mode::Hidden && to != Mode::Hidden;
        let e = if meeting {
            // The half that crosses: the one whose panel it is not.
            let on = if let Mode::On(p) = if joined(to) { to } else { from } { p } else { 0 };
            let mover = 1 - on;
            let dist = (squeeze(b[mover].x) - squeeze(a[mover].x)).abs();
            let delta = if dist > 0.0 { (CONTACT_PX / dist).min(0.5) } else { 0.0 };
            if joined(to) { contact_ease(k, delta) } else { 1.0 - contact_ease(1.0 - k, delta) }
        } else if linear {
            k
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
        if let (Some(u), Mode::On(stayer)) = (bump, to) {
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
        if meeting || joined(to) {
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
        self.end_scrub();
        let mode = mode_for(taken);
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

    /// The ribbon under a finger: the halves `k` of the way from where they
    /// stand for the panels `from` has taken to where they stand for `to`'s.
    pub fn scrub(&mut self, from: [bool; 2], to: [bool; 2], k: f64) {
        let (from, to) = (mode_for(from), mode_for(to));
        let k = k.clamp(0.0, 1.0);
        if to != Mode::Hidden {
            self.shown = to;
        } else if from != Mode::Hidden {
            self.shown = from;
        }
        self.moving = None;
        self.mode = if k >= 0.5 { to } else { from };
        self.scrub = Some((from, to, k));
        for half in &mut self.halves {
            half.pressed = None;
        }
    }

    /// The ribbon stopped: the halves stand where it left them.
    pub fn end_scrub(&mut self) {
        if let Some((from, to, k)) = self.scrub.take() {
            self.mode = if k >= 0.5 { to } else { from };
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
                // Arrived (not dipped away): it wobbles to rest.
                if m.to != Mode::Hidden {
                    self.landed = Some(frame_ns);
                }
                self.moving = None;
                true
            }
            Some(_) => true,
            // Wobbling, still stretched from moving, gathering up after the
            // hinge, or alive after a touch.
            None => {
                self.landed.is_some_and(|t| frame_ns < t + WOBBLE_NS)
                    || self.stretch.get().iter().any(|s| s.abs() > 0.003)
                    || self.popped.get().iter().any(|&t| t != 0 && frame_ns < t + WOBBLE_NS)
                    || (self.mode != Mode::Hidden && self.life(frame_ns) > 0.0)
            }
        }
    }

    /// How alive the drops are at `frame_ns`: 1 after a touch, calming to 0.
    fn life(&self, frame_ns: u64) -> f64 {
        let since = frame_ns.saturating_sub(self.touched);
        if self.touched == 0 || since >= ALIVE_NS {
            return 0.0;
        }
        let left = (ALIVE_NS - since) as f64 / CALM_NS as f64;
        let k = left.min(1.0);
        k * k * (3.0 - 2.0 * k)
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
        if self.moving.is_some() || self.scrub.is_some() || self.mode == Mode::Hidden {
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
        let mut push = |out: &mut Vec<ShellElement>, buffer: &MemoryRenderBuffer, x: f64, y: f64, dst: Option<(f64, f64)>, src: Option<Rectangle<f64, Logical>>, alpha: f32| {
            let size = dst.map(|(w, h)| smithay::utils::Size::<i32, Logical>::from((w.round().max(1.0) as i32, h.round().max(1.0) as i32)));
            match MemoryRenderBufferRenderElement::from_buffer(renderer, (px(x), px(y)), buffer, Some(alpha), src, size, Kind::Unspecified) {
                Ok(e) => out.push(ShellElement::Text(e)),
                Err(e) => tracing::warn!("dock: {e}"),
            }
        };
        // Each half's box (its bump's squash about its anchor) and its
        // jelly: flat under the hinge, wobbling after a move.
        let jelly = self.jelly(&places, frame_ns);
        let boxes: Vec<(f64, f64, f64, f64)> = (0..2)
            .map(|h| {
                let p = places[h];
                let (w, ht) = (self.halves[h].size.0 as f64, self.halves[h].size.1 as f64);
                let x = p.anchor + (p.x - p.anchor) * p.scale;
                (x, p.y, w * p.scale, ht)
            })
            .collect();
        for (h, half) in self.halves.iter().enumerate() {
            let p = places[h];
            if p.y >= bottom || half.apps.is_empty() {
                continue;
            }
            // Squashed about its anchor (the bump), then as jelly about its
            // bottom middle.
            let (bx, by, bw, bh) = boxes[h];
            let (sx, sy, flat) = jelly[h];
            let mid = bx + bw / 2.0;
            let foot = by + bh;
            let tx = |x: f64| mid + (p.anchor + (x - p.anchor) * p.scale - mid) * sx;
            let ty = |y: f64| foot - (foot - y) * sy;
            for (i, app) in half.apps.iter().enumerate() {
                let c = self.cell(i, &p);
                // A dot under a running app, in the slab's bottom padding.
                if app.ids.iter().any(|id| running.contains(id)) {
                    let d = crate::grid::DOT;
                    push(&mut out, &self.dot, tx(c.loc.x + (c.size.w - d) / 2.0), ty(c.loc.y + c.size.h + (PAD as f64 - d) / 2.0), None, None, 1.0);
                }
                if let Some(icon) = &app.icon {
                    // A pressed icon dims under the finger; in a drop gone
                    // flat it sinks and fades.
                    let pressed = if half.pressed.is_some_and(|(q, _, _)| q == i) { 0.55 } else { 1.0 };
                    let alpha = pressed * ((1.0 - flat) * (1.0 - flat)) as f32;
                    if alpha < 0.01 {
                        continue;
                    }
                    // Through the water, a little larger, about its middle;
                    // squashed with the drop, but not flattened with it.
                    let sink = 1.0 - 0.45 * flat;
                    let (iw, ih) = (ICON as f64 * p.scale * sx * LENS * sink / (1.0 + WIDEN * flat), ICON as f64 * (sy + FLATTEN * flat) * LENS * sink);
                    let (cx, cy) = (tx(c.loc.x + ICON as f64 / 2.0), ty(c.loc.y + ICON as f64 / 2.0));
                    let src = Rectangle::new((0.0, 0.0).into(), (ICON as f64, ICON as f64).into());
                    push(&mut out, icon, cx - iw / 2.0, cy - ih / 2.0, Some((iw, ih)), Some(src), alpha);
                }
            }
        }
        // The drops under the icons.
        let squash = [(jelly[0].0, jelly[0].1), (jelly[1].0, jelly[1].1)];
        if let Some(e) = self.drops(renderer, &boxes, &squash, frame_ns) {
            out.push(ShellElement::Pixel(e));
        }
        out
    }

    /// Each half's jelly at `frame_ns`: its width and height scales -
    /// spread flat as it goes under the hinge, gathering up with a bounce
    /// out of it, wobbling after a move - and how flat it is (0 to 1).
    fn jelly(&self, places: &[Place; 2], frame_ns: u64) -> [(f64, f64, f64); 2] {
        let panels = layout::panels();
        let (h0, h1) = ((panels[0].loc.x + panels[0].size.w) as f64, panels[1].loc.x as f64);
        let wobble = self.landed.map(|t| {
            let u = frame_ns.saturating_sub(t) as f64;
            if u >= WOBBLE_NS as f64 { 0.0 } else { WOBBLE * (-u / (WOBBLE_NS as f64 / 4.0)).exp() * (u / WOBBLE_PERIOD_NS * std::f64::consts::TAU).sin() }
        });
        // Moving, a drop stretches along its way: its speed, smoothed.
        let xs = [places[0].x, places[1].x];
        let mut stretch = self.stretch.get();
        match self.last_x.get() {
            Some((t, was)) if frame_ns > t => {
                let ms = (frame_ns - t) as f64 / 1e6;
                for h in 0..2 {
                    let v = if ms < 100.0 { (xs[h] - was[h]).abs() / ms } else { 0.0 };
                    let want = (v * STRETCH).min(STRETCH_MAX);
                    stretch[h] += (want - stretch[h]) * STRETCH_FOLLOW;
                    if stretch[h].abs() < 0.002 {
                        stretch[h] = 0.0;
                    }
                }
            }
            _ => {}
        }
        if self.last_x.get().is_none_or(|(t, _)| frame_ns > t) {
            self.last_x.set(Some((frame_ns, xs)));
            self.stretch.set(stretch);
        }
        let (mut flat, mut popped) = (self.flat.get(), self.popped.get());
        let out = [0, 1].map(|h| {
            let w = self.halves[h].size.0 as f64;
            let (x0, x1) = (places[h].x, places[h].x + w);
            // How much of it is under the hinge, or near it: the slot. A
            // drop spreads flat as it nears, under it a puddle.
            let reach = 70.0;
            let under = ((x1.min(h1 + reach) - x0.max(h0 - reach)).max(0.0) / (h1 - h0 + 2.0 * reach).min(w)).min(1.0);
            let f = under * under * (3.0 - 2.0 * under);
            // Out of it: it gathers up again, with a bounce.
            if f > 0.4 {
                flat[h] = true;
            } else if flat[h] && f < 0.05 {
                flat[h] = false;
                popped[h] = frame_ns;
            }
            let pop = if popped[h] != 0 {
                let u = frame_ns.saturating_sub(popped[h]) as f64;
                if u >= WOBBLE_NS as f64 { 0.0 } else { POP * (-u / (WOBBLE_NS as f64 / 4.0)).exp() * (u / WOBBLE_PERIOD_NS * std::f64::consts::TAU).sin() }
            } else {
                0.0
            };
            let wb = wobble.unwrap_or(0.0) + pop;
            let s = (1.0 + WIDEN * f + stretch[h], 1.0 - FLATTEN * f - 0.6 * stretch[h]);
            (s.0 * (1.0 - 0.6 * wb), s.1 * (1.0 + wb), f)
        });
        self.flat.set(flat);
        self.popped.set(popped);
        out
    }

    /// The drops, through dock.frag: an element over the screen's bottom,
    /// its uniforms set again only when they change.
    fn drops(&self, renderer: &mut GlesRenderer, boxes: &[(f64, f64, f64, f64)], jelly: &[(f64, f64); 2], frame_ns: u64) -> Option<smithay::backend::renderer::gles::element::PixelShaderElement> {
        use smithay::backend::renderer::gles::element::PixelShaderElement;
        use smithay::backend::renderer::gles::{Uniform, UniformName, UniformType};
        if self.program.borrow().is_none() {
            let names = [
                UniformName::new("half0", UniformType::_4f),
                UniformName::new("half1", UniformType::_4f),
                UniformName::new("squash0", UniformType::_2f),
                UniformName::new("squash1", UniformType::_2f),
                UniformName::new("radius", UniformType::_1f),
                UniformName::new("melt", UniformType::_1f),
                UniformName::new("body", UniformType::_4f),
                UniformName::new("shine", UniformType::_1f),
                UniformName::new("metal", UniformType::_1f),
                UniformName::new("origin", UniformType::_2f),
                UniformName::new("time", UniformType::_1f),
                UniformName::new("life", UniformType::_1f),
                UniformName::new("shift", UniformType::_1f),
            ];
            let source = include_str!("dock.frag").replace("//_WALLPAPER_", include_str!("wallpaper.glsl"));
            match renderer.compile_custom_pixel_shader(&source, &names) {
                Ok(p) => *self.program.borrow_mut() = Some(p),
                Err(e) => {
                    tracing::warn!("dock: the drops' shader: {e}");
                    return None;
                }
            }
        }
        let (width, height) = layout::LAYOUT;
        // The bottom of the screen, as high as a half and a margin above.
        let top = height - (self.halves[0].size.1 + MARGIN) - 40;
        let area = Rectangle::<i32, Logical>::new((0, top).into(), (width, height - top).into());
        let y = |v: f64| (v - top as f64) as f32;
        // Apart, they melt together the more the nearer, up to MELT; once
        // they touch they are one drop (a smooth union of two that overlap
        // would swell where they meet).
        let gap = boxes[1].0 - (boxes[0].0 + boxes[0].2);
        let (b0, b1, j0, j1, melt) = if gap <= 0.0 {
            let (x0, x1) = (boxes[0].0.min(boxes[1].0), (boxes[0].0 + boxes[0].2).max(boxes[1].0 + boxes[1].2));
            let one = (x0, boxes[0].1.min(boxes[1].1), x1 - x0, boxes[0].3.max(boxes[1].3));
            let j = ((jelly[0].0 + jelly[1].0) / 2.0, jelly[0].1.max(jelly[1].1));
            (one, one, j, j, 1.0)
        } else {
            (boxes[0], boxes[1], jelly[0], jelly[1], MELT * (gap / 14.0).clamp(0.3, 1.0))
        };
        let values: Vec<f32> = vec![
            b0.0 as f32, y(b0.1), b0.2 as f32, b0.3 as f32,
            b1.0 as f32, y(b1.1), b1.2 as f32, b1.3 as f32,
            j0.0 as f32, j0.1 as f32, j1.0 as f32, j1.1 as f32,
            melt as f32,
            ((frame_ns / 1_000_000) % 1_000_000) as f32 / 1000.0,
            self.life(frame_ns) as f32,
            self.shift as f32,
            top as f32,
        ];
        let uniforms = |v: &[f32]| {
            vec![
                Uniform::new("half0", (v[0], v[1], v[2], v[3])),
                Uniform::new("half1", (v[4], v[5], v[6], v[7])),
                Uniform::new("squash0", (v[8], v[9])),
                Uniform::new("squash1", (v[10], v[11])),
                Uniform::new("radius", DROP_RADIUS),
                Uniform::new("melt", v[12]),
                Uniform::new("body", (BODY[0] * BODY[3], BODY[1] * BODY[3], BODY[2] * BODY[3], BODY[3])),
                Uniform::new("shine", 1.0f32),
                Uniform::new("metal", if std::env::var_os("DOCK_SILVER").is_some() { 1.0f32 } else { 0.0 }),
                Uniform::new("origin", (0.0f32, v[16])),
                Uniform::new("time", v[13]),
                Uniform::new("life", v[14]),
                Uniform::new("shift", v[15]),
            ]
        };
        let mut drops = self.drops.borrow_mut();
        match drops.as_mut() {
            Some((e, last)) => {
                if *last != values {
                    e.update_uniforms(uniforms(&values));
                    *last = values;
                }
            }
            None => {
                let program = self.program.borrow().clone()?;
                let e = PixelShaderElement::new(program, area, None, 1.0, uniforms(&values), Kind::Unspecified);
                *drops = Some((e, values));
            }
        }
        drops.as_ref().map(|(e, _)| e.clone())
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

/// Where the halves stand for the panels windows have.
fn mode_for(taken: [bool; 2]) -> Mode {
    match taken {
        [false, false] => Mode::Both,
        [false, true] => Mode::On(0),
        [true, false] => Mode::On(1),
        [true, true] => Mode::Hidden,
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
