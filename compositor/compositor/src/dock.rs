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
//! between them. Seen from above: by the hinge a drop spreads thin and
//! flows along the slot - up it, narrower across it - and out of it gathers
//! up with a bounce, leaving a wet trace that dries in two seconds; moving
//! it stretches along its way, and after a move it wobbles to rest. They are water: clear, a light ring along the edge,
//! light gathered inside along the bottom, a small sharp highlight, a soft
//! shadow; the icons seen through it a little larger. Touched, a drop
//! gives under the finger and springs back when let go, and comes alive -
//! a ripple along its edge, a breath - and calms again (DOCK_SILVER=1: soft
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

/// At the hinge: how far a part reaches into it (unseen, so it ends straight
/// at the edge); the tail clinging to its edge at least CLING of its height
/// wide, letting go when it would be less than LET_GO px; the wet spot it
/// leaves, SPOT wide, drying over WET_NS.
const INTO: f64 = 12.0;
const CLING: f64 = 0.42;
const LET_GO: f64 = 10.0;
const SPOT: f64 = 18.0;
const WET_NS: u64 = 1_800_000_000;
/// A jolt (letting go, the thread breaking): a bounce.
const POP: f64 = 0.14;
/// Two drops: apart they do not reach for each other (a px, for a clean
/// edge); pulled apart they keep a thread, PINCH_MELT wide, until BREAK_GAP
/// apart; met, the waist fills over MERGE_FILL_NS, the join swelling out
/// MERGE_BULGE px and back, settling over MERGE_SETTLE_NS.
const APART_MELT: f64 = 0.8;
const PINCH_MELT: f64 = 30.0;
const BREAK_GAP: f64 = 24.0;
const MERGE_FILL_NS: f64 = 45e6;
const MERGE_BULGE: f64 = 6.0;
const MERGE_SETTLE_NS: f64 = 170e6;
const MERGE_PERIOD_NS: f64 = 260e6;
const MERGE_NS: u64 = 700_000_000;
const WOBBLE_NS: u64 = 650_000_000;
const WOBBLE: f64 = 0.07;
const WOBBLE_PERIOD_NS: f64 = 260e6;
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
/// Alive while a finger is on a drop and a while after, calming over the
/// last part.
const ALIVE_NS: u64 = 4_000_000_000;
const CALM_NS: u64 = 2_500_000_000;

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
    /// Each half: which way it last went, whether its tail clings to the
    /// hinge's edge, and when it was last jolted.
    dir: std::cell::Cell<[f64; 2]>,
    clinging: std::cell::Cell<[bool; 2]>,
    popped: std::cell::Cell<[u64; 2]>,
    /// The two met (and since when), or pulled apart with a thread.
    meeting: std::cell::Cell<(bool, u64, bool)>,
    /// Whether a half is at the hinge now (for FRAMES_DOCK).
    pub at_hinge: std::cell::Cell<bool>,
    /// A wet spot: its box (screen px) and when it was left.
    trail: std::cell::Cell<Option<([f64; 4], u64)>>,
    /// When a finger was last on a drop: they are alive for a while after.
    touched: std::cell::Cell<u64>,
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
        Dock { rise: None, halves, dot: crate::grid::running_dot(), mode: Mode::Both, shown: Mode::Both, moving: None, scrub: None, landed: None, last_x: Default::default(), stretch: Default::default(), dir: Default::default(), clinging: Default::default(), popped: Default::default(), meeting: Default::default(), at_hinge: Default::default(), trail: Default::default(), touched: Default::default(), shift: 0.0, program: Default::default(), drops: Default::default() }
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
                    || self.trail.get().is_some_and(|(_, t)| frame_ns < t + WET_NS)
                    || self.meeting.get().0 && frame_ns < self.meeting.get().1 + MERGE_NS
                    || (self.mode != Mode::Hidden && self.life(frame_ns) > 0.0)
            }
        }
    }

    /// How alive the drops are at `frame_ns`: 1 after a touch, calming to 0.
    fn life(&self, frame_ns: u64) -> f64 {
        // A finger on it: alive, and the time counts from now.
        if self.halves.iter().any(|h| h.pressed.is_some()) {
            self.touched.set(frame_ns);
        }
        let touched = self.touched.get();
        let since = frame_ns.saturating_sub(touched);
        if touched == 0 || since >= ALIVE_NS {
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
            // Touched, the drops come alive.
            self.touched.set(hybris_hwc::now_ns());
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
        let now = hybris_hwc::now_ns();
        for (h, half) in self.halves.iter_mut().enumerate() {
            if let Some((i, s, at)) = half.pressed {
                if s == slot {
                    half.pressed = None;
                    // Let go: it springs back.
                    let mut popped = self.popped.get();
                    popped[h] = now;
                    self.popped.set(popped);
                    let panel = layout::panel_at(at).unwrap_or(0);
                    let app = &half.apps[i];
                    return Some(Tap::Launch(app.exec.clone(), panel, app.large.clone(), app.ids.clone()));
                }
            }
        }
        None
    }

    pub fn cancel(&mut self) {
        let now = hybris_hwc::now_ns();
        for (h, half) in self.halves.iter_mut().enumerate() {
            if half.pressed.take().is_some() {
                let mut popped = self.popped.get();
                popped[h] = now;
                self.popped.set(popped);
            }
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
        // The drops as they stand: each half's box, its squash, and what
        // the hinge does to it (Shape).
        let shape = self.shape(&places, frame_ns);
        for (h, half) in self.halves.iter().enumerate() {
            let p = places[h];
            if p.y >= bottom || half.apps.is_empty() {
                continue;
            }
            let (bx, by, bw, bh) = shape.boxes[h];
            let (sx, sy) = shape.squash[h];
            let mid = bx + bw / 2.0;
            let foot = by + bh;
            let tx = |x: f64| mid + (p.anchor + (x - p.anchor) * p.scale - mid) * sx;
            let ty = |y: f64| foot - (foot - y) * sy;
            for (i, app) in half.apps.iter().enumerate() {
                let c = self.cell(i, &p);
                let (cx, cy) = (tx(c.loc.x + ICON as f64 / 2.0), ty(c.loc.y + ICON as f64 / 2.0));
                // By the hinge an icon goes under with the water: it fades
                // as it nears the edge, and comes out again past it.
                let shown = 1.0 - hinge_cover(cx, ICON as f64 / 2.0);
                if shown <= 0.01 {
                    continue;
                }
                // A dot under a running app, in the drop's bottom padding.
                if app.ids.iter().any(|id| running.contains(id)) {
                    let d = crate::grid::DOT;
                    push(&mut out, &self.dot, cx - d / 2.0, ty(c.loc.y + c.size.h + (PAD as f64 - d) / 2.0), None, None, shown as f32);
                }
                if let Some(icon) = &app.icon {
                    // A pressed icon dims under the finger. Through the
                    // water, a little larger, about its middle.
                    let pressed = if half.pressed.is_some_and(|(q, _, _)| q == i) { 0.55 } else { 1.0 };
                    let (iw, ih) = (ICON as f64 * p.scale * LENS, ICON as f64 * LENS);
                    let src = Rectangle::new((0.0, 0.0).into(), (ICON as f64, ICON as f64).into());
                    push(&mut out, icon, cx - iw / 2.0, cy - ih / 2.0, Some((iw, ih)), Some(src), pressed * shown as f32);
                }
            }
        }
        // The drops under the icons.
        if let Some(e) = self.drops(renderer, &shape, frame_ns) {
            out.push(ShellElement::Pixel(e));
        }
        out
    }

    /// The drops at `frame_ns` (see Shape): each half's box from its place,
    /// squashed by its motion; the two met, pulled apart, or apart; a half
    /// at the hinge in two, its tail clinging to the edge it leaves until it
    /// lets go, its head coming out round on the other side.
    fn shape(&self, places: &[Place; 2], frame_ns: u64) -> Shape {
        let bottom = layout::LAYOUT.1 as f64;
        let boxes: [(f64, f64, f64, f64); 2] = [0, 1].map(|h| {
            let p = places[h];
            let (w, ht) = (self.halves[h].size.0 as f64, self.halves[h].size.1 as f64);
            (p.anchor + (p.x - p.anchor) * p.scale, p.y, w * p.scale, ht)
        });
        let shown = [0, 1].map(|h| places[h].y < bottom && !self.halves[h].apps.is_empty());

        // Moving, a drop stretches along its way: its speed, smoothed; and
        // which way it goes.
        let xs = [places[0].x, places[1].x];
        let (mut stretch, mut dir) = (self.stretch.get(), self.dir.get());
        if let Some((t, was)) = self.last_x.get().filter(|(t, _)| frame_ns > *t) {
            let ms = (frame_ns - t) as f64 / 1e6;
            for h in 0..2 {
                let dx = xs[h] - was[h];
                let v = if ms < 100.0 { dx.abs() / ms } else { 0.0 };
                stretch[h] += ((v * STRETCH).min(STRETCH_MAX) - stretch[h]) * STRETCH_FOLLOW;
                if stretch[h].abs() < 0.002 {
                    stretch[h] = 0.0;
                }
                if v > 0.02 {
                    dir[h] = dx.signum();
                }
            }
        }
        if self.last_x.get().is_none_or(|(t, _)| frame_ns > t) {
            self.last_x.set(Some((frame_ns, xs)));
            self.stretch.set(stretch);
            self.dir.set(dir);
        }

        // Wobbles: after a move, after a jolt (popped), and the finger's
        // press.
        let ring = |since: u64, amp: f64| {
            if since == 0 {
                return 0.0;
            }
            let u = frame_ns.saturating_sub(since) as f64;
            if u >= WOBBLE_NS as f64 { 0.0 } else { amp * (-u / (WOBBLE_NS as f64 / 4.0)).exp() * (u / WOBBLE_PERIOD_NS * std::f64::consts::TAU).sin() }
        };
        let landed = self.landed.map(|t| ring(t, WOBBLE)).unwrap_or(0.0);
        let mut popped = self.popped.get();
        let squash: [(f64, f64); 2] = [0, 1].map(|h| {
            let wb = landed + ring(popped[h], POP);
            let mut s = ((1.0 + stretch[h]) * (1.0 - 0.6 * wb), (1.0 - 0.6 * stretch[h]) * (1.0 + wb));
            if self.halves[h].pressed.is_some() {
                s = (s.0 * 1.035, s.1 * 0.93);
            }
            s
        });

        // Met, pulled apart, or apart: by the gap between them.
        let gap = boxes[1].0 - (boxes[0].0 + boxes[0].2);
        let (mut joined, mut since, mut pinching) = self.meeting.get();
        if shown[0] && shown[1] && gap <= 0.0 {
            if !joined {
                joined = true;
                since = frame_ns;
                pinching = false;
            }
        } else {
            if joined {
                joined = false;
                pinching = shown[0] && shown[1];
            }
            // The thread breaks: both spring back.
            if pinching && (gap > BREAK_GAP || !(shown[0] && shown[1])) {
                pinching = false;
                popped = [frame_ns, frame_ns];
            }
        }
        self.meeting.set((joined, since, pinching));
        let melt = if pinching { PINCH_MELT } else { APART_MELT };
        let one = joined.then(|| {
            let u = frame_ns.saturating_sub(since) as f64;
            let fill = 1.0 - (-u / MERGE_FILL_NS).exp();
            let bulge = MERGE_BULGE * (-u / MERGE_SETTLE_NS).exp() * (u / MERGE_PERIOD_NS * std::f64::consts::TAU).cos();
            let (x0, x1) = (boxes[0].0.min(boxes[1].0), (boxes[0].0 + boxes[0].2).max(boxes[1].0 + boxes[1].2));
            let ht = boxes[0].3.max(boxes[1].3);
            // The two as one, squashed as they are on average.
            let (sx, sy) = ((squash[0].0 + squash[1].0) / 2.0, (squash[0].1 + squash[1].1) / 2.0);
            let (w, hh) = ((x1 - x0) * sx, ht * sy);
            let foot = boxes[0].1 + boxes[0].3;
            ([(x0 + x1) / 2.0 - w / 2.0, foot - hh, w, hh], fill, bulge, boxes[0].0 + boxes[0].2)
        });

        // The hinge: a half across it is in two, each part against its
        // edge (reaching INTO the hinge, where nothing is seen, so it ends
        // straight at the edge). The tail clings to its edge, as water does,
        // until it is too little and lets go; the head comes out small and
        // round, and grows.
        let panels = layout::panels();
        let (h0, h1) = ((panels[0].loc.x + panels[0].size.w) as f64, panels[1].loc.x as f64);
        let mut drops: Vec<([f64; 4], (f64, f64))> = Vec::new();
        let mut clinging = self.clinging.get();
        for h in 0..2 {
            if !shown[h] {
                continue;
            }
            let (bx, by, bw, bh) = boxes[h];
            let (sx, sy) = squash[h];
            let (mid, foot) = (bx + bw / 2.0, by + bh);
            let (w, ht) = (bw * sx, bh * sy);
            let (x0, x1) = (mid - w / 2.0, mid + w / 2.0);
            if x1 <= h0 || x0 >= h1 {
                drops.push(([bx, by, bw, bh], (sx, sy)));
                clinging[h] = false;
                continue;
            }
            let (wl, wr) = ((h0 - x0).max(0.0), (x1 - h1).max(0.0));
            // A part as much smaller as it is narrower: a bulge out of the
            // slot, round, about the drop's middle.
            let part = |width: f64| ht * (0.3 + 0.7 * smooth(width / (0.9 * ht)));
            let middle = foot - ht / 2.0;
            // The tail: the part on the side it comes from.
            let tail_left = dir[h] > 0.0;
            let tail = if tail_left { wl } else { wr };
            let held = if tail < LET_GO { 0.0 } else { tail.max(CLING * ht) };
            if clinging[h] && held == 0.0 {
                // It let go: a wet spot where it clung, and the head jolts.
                let edge = if tail_left { h0 - SPOT } else { h1 };
                self.trail.set(Some(([edge, foot - 0.72 * ht, SPOT + 0.0, 0.58 * ht], frame_ns)));
                popped[h] = frame_ns;
            }
            clinging[h] = held > 0.0;
            let (wl, wr) = if tail_left { (held, wr) } else { (wl, held) };
            if wl > 0.5 {
                let hp = part(wl);
                drops.push(([h0 - wl, middle - hp / 2.0, wl + INTO, hp], (1.0, 1.0)));
            }
            if wr > 0.5 {
                let hp = part(wr);
                drops.push(([h1 - INTO, middle - hp / 2.0, wr + INTO, hp], (1.0, 1.0)));
            }
        }
        self.clinging.set(clinging);
        self.popped.set(popped);
        self.at_hinge.set(drops.len() > [shown[0], shown[1]].iter().filter(|s| **s).count());

        // The wet spot, drying: fainter, and shrinking towards its middle.
        let trail = match self.trail.get() {
            Some((t, at)) if frame_ns < at + WET_NS => {
                let k = (frame_ns - at) as f64 / WET_NS as f64;
                let inset = k * 0.3 * t[2].min(t[3]);
                ([t[0] + inset, t[1] + inset, t[2] - 2.0 * inset, t[3] - 2.0 * inset], (1.0 - k).powf(1.5))
            }
            _ => ([0.0; 4], 0.0),
        };
        Shape { boxes, squash, drops, melt, one, trail }
    }

    /// The drops, through dock.frag: an element over the screen's bottom,
    /// its uniforms set again only when they change.
    fn drops(&self, renderer: &mut GlesRenderer, shape: &Shape, frame_ns: u64) -> Option<smithay::backend::renderer::gles::element::PixelShaderElement> {
        use smithay::backend::renderer::gles::element::PixelShaderElement;
        use smithay::backend::renderer::gles::{Uniform, UniformName, UniformType};
        if self.program.borrow().is_none() {
            let mut names: Vec<UniformName> = ["b0", "b1", "b2", "b3", "s01", "s23", "one", "meet", "body", "trail"].into_iter().map(|n| UniformName::new(n, UniformType::_4f)).collect();
            names.extend(["radius", "melt", "shine", "metal", "time", "life", "shift", "wet"].into_iter().map(|n| UniformName::new(n, UniformType::_1f)));
            names.push(UniformName::new("origin", UniformType::_2f));
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
        // The bottom of the screen: the drops, their shadow, their breath.
        let top = height - 140;
        let area = Rectangle::<i32, Logical>::new((0, top).into(), (width, height - top).into());
        let y = |v: f64| (v - top as f64) as f32;
        let far = ([-1000.0, 0.0, 10.0, 10.0], (1.0, 1.0));
        let d: Vec<([f64; 4], (f64, f64))> = (0..4).map(|i| shape.drops.get(i).copied().unwrap_or(far)).collect();
        let (one, meet) = match shape.one {
            Some((b, fill, bulge, join)) => (b, [fill, bulge, join, 0.0]),
            None => ([-1000.0, 0.0, 10.0, 10.0], [0.0; 4]),
        };
        let mut v: Vec<f32> = Vec::new();
        for (b, _) in &d {
            v.extend([b[0] as f32, y(b[1]), b[2] as f32, b[3] as f32]);
        }
        v.extend([d[0].1 .0, d[0].1 .1, d[1].1 .0, d[1].1 .1, d[2].1 .0, d[2].1 .1, d[3].1 .0, d[3].1 .1].map(|x| x as f32));
        v.extend([one[0] as f32, y(one[1]), one[2] as f32, one[3] as f32]);
        v.extend(meet.map(|x| x as f32));
        let t = shape.trail.0;
        v.extend([t[0] as f32, y(t[1]), t[2] as f32, t[3] as f32, shape.trail.1 as f32]);
        v.extend([shape.melt as f32, ((frame_ns / 1_000_000) % 1_000_000) as f32 / 1000.0, self.life(frame_ns) as f32, self.shift as f32, top as f32]);
        let uniforms = |v: &[f32]| {
            vec![
                Uniform::new("b0", (v[0], v[1], v[2], v[3])),
                Uniform::new("b1", (v[4], v[5], v[6], v[7])),
                Uniform::new("b2", (v[8], v[9], v[10], v[11])),
                Uniform::new("b3", (v[12], v[13], v[14], v[15])),
                Uniform::new("s01", (v[16], v[17], v[18], v[19])),
                Uniform::new("s23", (v[20], v[21], v[22], v[23])),
                Uniform::new("one", (v[24], v[25], v[26], v[27])),
                Uniform::new("meet", (v[28], v[29], v[30], v[31])),
                Uniform::new("trail", (v[32], v[33], v[34], v[35])),
                Uniform::new("wet", v[36]),
                Uniform::new("melt", v[37]),
                Uniform::new("time", v[38]),
                Uniform::new("life", v[39]),
                Uniform::new("shift", v[40]),
                Uniform::new("origin", (0.0f32, v[41])),
                Uniform::new("radius", DROP_RADIUS),
                Uniform::new("body", (BODY[0] * BODY[3], BODY[1] * BODY[3], BODY[2] * BODY[3], BODY[3])),
                Uniform::new("shine", 1.0f32),
                Uniform::new("metal", if std::env::var_os("DOCK_SILVER").is_some() { 1.0f32 } else { 0.0 }),
            ]
        };
        let mut drops = self.drops.borrow_mut();
        match drops.as_mut() {
            Some((e, last)) => {
                if *last != v {
                    e.update_uniforms(uniforms(&v));
                    *last = v;
                }
            }
            None => {
                let program = self.program.borrow().clone()?;
                let e = PixelShaderElement::new(program, area, None, 1.0, uniforms(&v), Kind::Unspecified);
                *drops = Some((e, v));
            }
        }
        drops.as_ref().map(|(e, _)| e.clone())
    }
}

/// The drops at a moment (Dock::shape): each half's box (its bump's squash
/// about its anchor, before the hinge) and its squash; the drops to draw,
/// up to four (a half across the hinge is two); how much drops apart melt
/// together; the two as one once met (the box, how filled the waist is, the
/// join's swell and where); the wet spot and how wet.
struct Shape {
    boxes: [(f64, f64, f64, f64); 2],
    squash: [(f64, f64); 2],
    drops: Vec<([f64; 4], (f64, f64))>,
    melt: f64,
    one: Option<([f64; 4], f64, f64, f64)>,
    trail: ([f64; 4], f64),
}

/// Smoothstep from 0 to 1.
fn smooth(x: f64) -> f64 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

/// How much of an icon centred at `cx`, `r` wide each way, is under the
/// hinge or about to be (0 none, 1 all): it fades in the last px before.
fn hinge_cover(cx: f64, r: f64) -> f64 {
    let panels = layout::panels();
    let (h0, h1) = ((panels[0].loc.x + panels[0].size.w) as f64, panels[1].loc.x as f64);
    let d = if cx < h0 { h0 - cx } else if cx > h1 { cx - h1 } else { 0.0 };
    1.0 - smooth(d / (r + 10.0))
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
