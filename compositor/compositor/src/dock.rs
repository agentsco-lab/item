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
/// How far towards its panel's middle a half of the dock goes with the
/// phone folded like a book to 90 degrees, the halves apart: a share of the
/// way from the edge (joined on one panel, the dock goes all the way).
const BOOK_REACH: f64 = 0.3;
const RADIUS: f32 = 20.0;
/// item's slab colour, opaque: as rgba(38,48,43,0.92) came out over the
/// desktop's black - opaque, so the halves and the neck between them can
/// overlap without darker seams.
const SLAB: [u8; 4] = [37, 46, 42, 255];
/// How long a move takes (item's MOVE_CROSS_MS was 440 ms; drops of water
/// go slower).
const MOVE_NS: u64 = 640_000_000;
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

#[derive(Clone)]
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
    /// Apps running that are on neither half, after its own (each with the
    /// app id it is for), on the half of the panel they were opened onto.
    extra: Vec<(String, App)>,
    /// The width it is going to, as apps come and go (logical px).
    target_w: i32,
    /// The slab with its inner corners (the right ones for the left half,
    /// the left for the right) at each of RADII; its outer ones round.
    slabs: Vec<MemoryRenderBuffer>,
    /// The slab's size, logical px.
    size: (i32, i32),
    /// The icon a finger is on, and the finger.
    pressed: Option<(usize, TouchSlot, Point<f64, Logical>)>,
}

impl Half {
    fn len(&self) -> usize {
        self.apps.len() + self.extra.len()
    }

    /// Its icons, its own and then the running ones.
    fn items(&self) -> impl Iterator<Item = &App> {
        self.apps.iter().chain(self.extra.iter().map(|(_, a)| a))
    }

    fn item(&self, i: usize) -> &App {
        if i < self.apps.len() { &self.apps[i] } else { &self.extra[i - self.apps.len()].1 }
    }
}

/// A half's width for `n` icons, logical px.
fn half_width(n: usize) -> i32 {
    let n = n as i32;
    if n == 0 { 0 } else { 2 * PAD + n * ICON + (n - 1) * GAP }
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
/// The tail lingers as LINGER of the drop until less than LET_GO_SHARE of
/// it is left, then is drawn in over SNAP_NS.
const LINGER: f64 = 0.1;
const LET_GO_SHARE: f64 = 0.03;
const SNAP_NS: u64 = 110_000_000;
/// Nearing the hinge, within APPROACH px, a drop is held: HELD_SHORT shorter
/// along its way, HELD_FULL fuller across, at most.
const APPROACH: f64 = 26.0;
const HELD_SHORT: f64 = 0.1;
const HELD_FULL: f64 = 0.08;
const SPOT: f64 = 18.0;
const WET_NS: u64 = 2_400_000_000;
/// A jolt (letting go, the thread breaking): a bounce.
const POP: f64 = 0.14;
/// Two drops: apart they do not reach for each other (a px, for a clean
/// edge); pulled apart they keep a thread, PINCH_MELT wide, until BREAK_GAP
/// apart; met, the waist fills over MERGE_FILL_NS, the join swelling out
/// MERGE_BULGE px and back, settling over MERGE_SETTLE_NS.
const APART_MELT: f64 = 0.8;
const PINCH_MELT: f64 = 30.0;
const BREAK_GAP: f64 = 24.0;
const MERGE_FILL_NS: f64 = 90e6;
const MERGE_BULGE: f64 = 6.0;
const MERGE_SETTLE_NS: f64 = 260e6;
const MERGE_PERIOD_NS: f64 = 380e6;
const MERGE_NS: u64 = 1_000_000_000;
const WOBBLE_NS: u64 = 1_000_000_000;
const WOBBLE: f64 = 0.07;
const WOBBLE_PERIOD_NS: f64 = 380e6;
/// Water's body: a faint cool tint, mostly clear.
const BODY: [f32; 4] = [0.82, 0.9, 0.95, 0.14];
/// A drop's ends: round, half its height.
const DROP_RADIUS: f32 = 36.0;
/// How much a moving drop stretches along its way, per logical px per ms,
/// and at most; how fast the stretch follows the speed.
const STRETCH: f64 = 0.09;
const STRETCH_MAX: f64 = 0.16;
const STRETCH_FOLLOW: f64 = 0.22;
/// Carried by the hinge, a drop flows: its front edge follows where it is
/// going within FLOW_FRONT_NS, its back edge trails within FLOW_BACK_NS (a
/// tail); longer, it is thinner as much (its water kept, down to FLOW_THIN
/// of its height); stopped, the tail draws in - no bounce.
const FLOW_FRONT_NS: f64 = 30e6;
const FLOW_BACK_NS: f64 = 150e6;
const FLOW_THIN: f64 = 0.7;
/// The wet streak a flowing drop leaves behind its tail: its far end trails
/// the tail within STREAK_NS (so it is as long as the drop is quick), drying
/// as it draws in; as wet as STREAK_WET once STREAK_FULL px long.
const STREAK_NS: f64 = 420e6;
const STREAK_WET: f64 = 0.8;
const STREAK_FULL: f64 = 40.0;
/// The icons seen through the water: a little larger.
const LENS: f64 = 1.04;
/// Alive while a finger is on a drop and a while after, calming over the
/// last part.
const ALIVE_NS: u64 = 4_000_000_000;
const CALM_NS: u64 = 2_500_000_000;

/// Carrying an icon (`lift`): held this long still, an icon lifts out as
/// a drop; a half takes it within this much of its drop; let go this far
/// above the dock, it is off the dock and dries away.
pub const LIFT_NS: u64 = 500_000_000;
const TAKE_ABOVE: f64 = 80.0;
const TAKE_SIDE: f64 = 30.0;
const OFF_ABOVE: f64 = 140.0;
const CARRY_R: f64 = 32.0;
const CARRY_IN_NS: f64 = 160e6;
const DRY_NS: f64 = 380e6;
/// A half's own apps at most; the rest of a panel is for running ones.
const MAX_PER_HALF: usize = 5;

/// The halves coming in from the sides after an unlock (item's RISE_MS).
const RISE_NS: u64 = 360_000_000;

/// Born of the first setup's drop: it falls to the left panel's foot, lands
/// and spreads into the two halves as one, which then part as from a panel
/// taken (the right one going over the hinge); each half's icons come up
/// as it stands.
const BIRTH_FALL_NS: u64 = 750_000_000;
const BIRTH_SPREAD_NS: u64 = 450_000_000;
const BIRTH_ICONS_NS: u64 = 350_000_000;

pub struct Dock {
    /// When the halves start coming in from the sides (an unlock).
    rise: Option<u64>,
    /// Born of a drop (`born`): when, where it was (its middle) and how
    /// big (its radius), logical px.
    birth: Option<(u64, (f64, f64), f64)>,
    halves: Vec<Half>,
    dot: MemoryRenderBuffer,
    mode: Mode,
    /// The mode before the last Hidden, whose places Hidden dips from.
    shown: Mode,
    moving: Option<Move>,
    /// How far the phone is folded like a book (0 flat, 1 at 90 degrees):
    /// the pieces go from the screen's edges towards their panels' middles
    /// (BOOK_REACH of the way).
    book: std::cell::Cell<f64>,
    /// The move scrubbed by the ribbon: from, to, how far.
    scrub: Option<(Mode, Mode, f64)>,
    /// When the last move ended: the halves wobble to rest from it.
    landed: Option<u64>,
    /// Each half's x as last drawn and when, and its stretch from moving.
    last_x: std::cell::Cell<Option<(u64, [f64; 2])>>,
    stretch: std::cell::Cell<[f64; 2]>,
    /// Carried by the hinge: each half's edges as drawn (left, right), where
    /// it was going and when, and whether they are still drawing in.
    flow: std::cell::Cell<Option<(u64, [[f64; 2]; 2], [f64; 2])>>,
    /// Behind each flowing drop: where its wet streak ends, and which way
    /// it last went (+1 right, -1 left).
    streak: std::cell::Cell<[(f64, f64); 2]>,
    streaks: std::cell::Cell<[([f64; 4], f64); 2]>,
    flowing: std::cell::Cell<bool>,
    /// Each half: which way it last went, whether its tail clings to the
    /// hinge's edge, and when it was last jolted.
    dir: std::cell::Cell<[f64; 2]>,
    clinging: std::cell::Cell<[bool; 2]>,
    popped: std::cell::Cell<[u64; 2]>,
    /// A tail that let go: on the left edge, its size, its middle, when.
    snap: std::cell::Cell<Option<(bool, f64, f64, f64, u64)>>,
    /// The two met (and since when), or pulled apart with a thread.
    meeting: std::cell::Cell<(bool, u64, bool)>,
    /// Whether a half is at the hinge now (for FRAMES_DOCK).
    pub at_hinge: std::cell::Cell<bool>,
    /// A wet spot: its box (screen px) and when it was left.
    trail: std::cell::Cell<Option<([f64; 4], u64)>>,
    /// When a finger was last on a drop: they are alive for a while after.
    touched: std::cell::Cell<u64>,
    /// The drops' shader (dock.frag) and its element, its uniforms as last
    /// set (set again only when they change).
    program: std::cell::RefCell<Option<smithay::backend::renderer::gles::GlesTexProgram>>,
    /// The drops' element id and the uniforms it was drawn with: a new id
    /// when they change, so the frame takes it again.
    drops: std::cell::RefCell<Option<(smithay::backend::renderer::element::Id, Vec<f32>)>>,
    /// The same for a drop alone (the first setup's, `lone`), and how it
    /// flows.
    lone: std::cell::RefCell<Option<(smithay::backend::renderer::element::Id, Vec<f32>)>>,
    lone_flow: std::cell::Cell<Flow>,
    /// Running apps' dock items by app id, found once (None: no desktop
    /// file for it).
    found: std::collections::HashMap<String, Option<App>>,
    /// An icon a finger carries; one let go off the dock, drying away
    /// (where, its icon, since when).
    carry: Option<Carry>,
    drying: Option<((f64, f64), Option<MemoryRenderBuffer>, u64)>,
    /// When the finger on an icon came down, for a long press.
    pressed_at: u64,
    /// The same for groups of drops drawn freely (`group`), by slot.
    groups: std::cell::RefCell<Vec<std::cell::RefCell<Option<(smithay::backend::renderer::element::Id, Vec<f32>)>>>>,
}

/// A lone drop's flow (`Dock::lone`): where it was and when, its stretch
/// and way, its tail at the hinge, the tail let go, the wet spot, and its
/// last jolt.
#[derive(Clone, Copy, Default)]
pub struct Flow {
    last: Option<(u64, f64)>,
    stretch: f64,
    dir: f64,
    clinging: bool,
    snap: Option<(bool, f64, f64, f64, u64)>,
    trail: Option<([f64; 4], u64)>,
    popped: u64,
}

/// An icon carried by a finger (onto the dock from the grid, or along it,
/// across it, or off it): the app, the half and place
/// it came out of, the finger, since when, and the place it would go.
struct Carry {
    slot: TouchSlot,
    app: App,
    from: Option<(usize, usize)>,
    pos: Point<f64, Logical>,
    since: u64,
    over: Option<(usize, usize)>,
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
                Half { apps, extra: Vec::new(), target_w: w, slabs, size: (w, h), pressed: None }
            })
            .collect();
        Dock { rise: None, birth: None, halves, dot: crate::grid::running_dot(), mode: Mode::Both, shown: Mode::Both, moving: None, book: Default::default(), scrub: None, landed: None, last_x: Default::default(), stretch: Default::default(), flow: Default::default(), flowing: Default::default(), streak: Default::default(), streaks: Default::default(), dir: Default::default(), clinging: Default::default(), snap: Default::default(), popped: Default::default(), meeting: Default::default(), at_hinge: Default::default(), trail: Default::default(), touched: Default::default(), program: Default::default(), drops: Default::default(), lone: Default::default(), lone_flow: Default::default(), found: Default::default(), carry: None, drying: None, pressed_at: 0, groups: Default::default() }
    }

    /// Each half's place in a mode (item's `_pane_targets`).
    fn targets(&self, mode: Mode) -> [Place; 2] {
        let (lw, h) = self.halves[0].size;
        let rw = self.halves[1].size.0;
        let (width, height) = (layout::LAYOUT.0 as f64, layout::LAYOUT.1 as f64);
        let y = height - (MARGIN + h) as f64;
        let place = |x: f64, tuck: f64, joined: bool| Place { x, y, tuck, radius: if joined { 0.0 } else { RADIUS as f64 }, scale: 1.0, anchor: 0.0 };
        // Folded like a book, each piece goes from the screen's edge towards
        // its panel's middle: apart, BOOK_REACH of the way at 90 degrees;
        // joined on one panel, all the way (as in item 0.20).
        let book = self.book.get();
        let apart = book * BOOK_REACH;
        let middle = |p: usize| {
            let r = layout::panels()[p];
            r.loc.x as f64 + r.size.w as f64 / 2.0
        };
        let pair = (lw + rw) as f64 - TUCK;
        match mode {
            Mode::Both => {
                let (l, r) = (MARGIN as f64, width - (MARGIN + rw) as f64);
                let (lc, rc) = (middle(0) - lw as f64 / 2.0, middle(1) - rw as f64 / 2.0);
                [place(l + (lc - l).max(0.0) * apart, 0.0, false), place(r + (rc - r).min(0.0) * apart, 0.0, false)]
            }
            // The left half stays at the left edge; the right one arrives
            // beside it, its padding tucked under.
            Mode::On(0) => {
                let l = MARGIN as f64 + ((middle(0) - pair / 2.0) - MARGIN as f64).max(0.0) * book;
                [place(l, 0.0, true), place(l + lw as f64 - TUCK, TUCK, true)]
            }
            Mode::On(_) => {
                let edge = width - MARGIN as f64;
                let end = edge + ((middle(1) + pair / 2.0) - edge).min(0.0) * book;
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

    /// item's bump on arriving beside the other half: not for drops of
    /// water, whose meeting is its own (the waist filling, the join
    /// swelling and settling); its rebound parted them again for a moment.
    fn bumps(_m: &Move) -> bool {
        false
    }

    /// The halves off the screen's sides, coming in from `at` over RISE_NS,
    /// easing out (item's rise after an unlock, #72).
    pub fn rise(&mut self, at: u64) {
        self.rise = Some(at);
    }

    /// The halves born of a drop of `r` about `centre` (the first setup's,
    /// handed over as it goes), from `at`: it falls, lands, spreads into the
    /// halves as one, and they part to where they stand.
    pub fn born(&mut self, at: u64, centre: (f64, f64), r: f64) {
        tracing::info!("dock: born of the setup's drop");
        self.birth = Some((at, centre, r));
        self.rise = None;
        self.scrub = None;
        self.mode = Mode::Both;
        self.shown = Mode::Both;
        let landed = at + BIRTH_FALL_NS;
        self.moving = Some(Move { from: Mode::On(0), to: Mode::Both, start_ns: landed + BIRTH_SPREAD_NS });
        // It lands with a splash: a wobble from then.
        self.popped.set([landed, landed]);
    }

    /// While born: the halves as one drop falling and spreading.
    fn birth_places(&self, frame_ns: u64) -> Option<[Place; 2]> {
        let (at, (cx, cy), r) = self.birth?;
        let t = frame_ns.saturating_sub(at);
        if t >= BIRTH_FALL_NS + BIRTH_SPREAD_NS {
            return None;
        }
        let end = self.targets(Mode::On(0));
        let (w0, w1) = (self.halves[0].size.0 as f64, self.halves[1].size.0 as f64);
        let h = self.halves[0].size.1 as f64;
        // Where it lands: the middle of the two as one, at the dock's height.
        let left = end[0].x;
        let right = end[1].x + w1;
        let land = ((left + right) / 2.0, end[0].y + h / 2.0);
        // Its width as a drop: from the setup's, to the dock's height.
        let (mid, y, width) = if t < BIRTH_FALL_NS {
            let k = t as f64 / BIRTH_FALL_NS as f64;
            // Across eased in and out; down as it falls, faster and faster.
            let e = ease_in_out(k);
            let fall = k * k;
            (cx + (land.0 - cx) * e, cy + (land.1 - cy) * fall - h / 2.0, 2.0 * r + (h - 2.0 * r) * e)
        } else {
            (land.0, end[0].y, h)
        };
        // Spreading: each half from the drop's width to its own, out to
        // where it stands.
        let s = if t < BIRTH_FALL_NS { 0.0 } else { ease_out((t - BIRTH_FALL_NS) as f64 / BIRTH_SPREAD_NS as f64) };
        let half = |i: usize| {
            let w = if i == 0 { w0 } else { w1 };
            let scale = width / w + (1.0 - width / w) * s;
            // The left half grows from its left edge, the right from its
            // right: their edges from the drop's out to their places.
            let edge = if i == 0 { (mid - width / 2.0) + (end[0].x - (mid - width / 2.0)) * s } else { (mid + width / 2.0) + (end[1].x + w1 - (mid + width / 2.0)) * s };
            let x = if i == 0 { edge } else { edge - w };
            Place { x, y, tuck: end[i].tuck * s, radius: RADIUS as f64 * (1.0 - s) + end[i].radius * s, scale, anchor: edge }
        };
        Some([half(0), half(1)])
    }

    /// Each half's icons: shown, or coming up after a birth (the left as
    /// it spreads, the right as it arrives over the hinge).
    fn icons_shown(&self, h: usize, frame_ns: u64) -> f64 {
        let Some((at, ..)) = self.birth else { return 1.0 };
        let from = at + BIRTH_FALL_NS + if h == 0 { BIRTH_SPREAD_NS / 2 } else { BIRTH_SPREAD_NS + MOVE_NS * 3 / 4 };
        ease_out(frame_ns.saturating_sub(from) as f64 / BIRTH_ICONS_NS as f64)
    }

    /// Each half's place at `frame_ns`: its path, and the rise over it.
    fn places(&self, frame_ns: u64) -> [Place; 2] {
        if let Some(p) = self.birth_places(frame_ns) {
            return p;
        }
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

    /// How far the phone is folded like a book (posture.rs): the pieces'
    /// places follow it.
    pub fn set_book(&self, book: f64) {
        self.book.set(book);
    }

    /// The middle of the panel the clock stands on (the home panel), on its
    /// way with the dock: during a move from one home panel to the other it
    /// goes along, eased as the halves are (or as the ribbon's finger has
    /// it). With whether it is under way.
    pub fn home_x(&self, frame_ns: u64) -> Option<(f64, bool)> {
        let middle = |mode: Mode| {
            let p = match mode {
                Mode::Both => 1,
                Mode::On(p) => p,
                Mode::Hidden => return None,
            };
            let r = layout::panels()[p];
            Some(r.loc.x as f64 + r.size.w as f64 / 2.0)
        };
        let (from, to, k, linear) = if let Some((from, to, k)) = self.scrub {
            (from, to, k, true)
        } else if let Some(m) = &self.moving {
            let all = (frame_ns.saturating_sub(m.start_ns) as f64 / Self::duration(m) as f64).clamp(0.0, 1.0);
            let travel = MOVE_NS as f64 / Self::duration(m) as f64;
            (m.from, m.to, (all / travel).min(1.0), false)
        } else {
            return middle(self.mode).map(|x| (x, false));
        };
        match (middle(from), middle(to)) {
            (Some(a), Some(b)) if a != b => {
                let e = if linear {
                    k
                } else if k < 0.5 {
                    4.0 * k * k * k
                } else {
                    1.0 - (-2.0 * k + 2.0).powi(3) / 2.0
                };
                Some((a + (b - a) * e, k < 1.0))
            }
            (_, Some(b)) => Some((b, false)),
            (Some(a), None) => Some((a, false)),
            (None, None) => None,
        }
    }

    /// Where the halves go for the panels windows have. Returns whether they
    /// set off.
    pub fn follow(&mut self, taken: [bool; 2], now_ns: u64) -> bool {
        // Let go part of the way (the app grid, a window): the move goes on
        // from where the finger left it, at the dock's own pace - forward,
        // or back if it was let go short.
        if let Some((from, to, k)) = self.scrub {
            let target = mode_for(taken);
            let way = if target == to && k < 1.0 { Some((from, to, k)) } else if target == from && k > 0.0 { Some((to, from, 1.0 - k)) } else { None };
            if let Some((a, b, at)) = way.filter(|(a, b, _)| a != b) {
                self.scrub = None;
                let u = self.time_at(a, b, at);
                self.moving = Some(Move { from: a, to: b, start_ns: now_ns.saturating_sub((u * MOVE_NS as f64) as u64) });
                if b != Mode::Hidden {
                    self.shown = b;
                }
                self.mode = b;
                return true;
            }
        }
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

    /// How far in time (0..1 of MOVE_NS) a move from `from` to `to` has the
    /// halves where a finger had them `k` of the way: the move's easing
    /// turned back, by bisection on the half that goes furthest.
    fn time_at(&self, from: Mode, to: Mode, k: f64) -> f64 {
        let at = self.between(from, to, k, None, true);
        let (a, b) = (self.targets(from), self.targets(to));
        let h = if (b[0].x - a[0].x).abs() >= (b[1].x - a[1].x).abs() { 0 } else { 1 };
        let (x0, x1, want) = (a[h].x, b[h].x, at[h].x);
        if (x1 - x0).abs() < 0.5 {
            return k;
        }
        let (mut lo, mut hi) = (0.0, 1.0);
        for _ in 0..30 {
            let mid = (lo + hi) / 2.0;
            let x = self.between(from, to, mid, None, false)[h].x;
            if (x - x0) / (x1 - x0) < (want - x0) / (x1 - x0) {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        (lo + hi) / 2.0
    }

    /// The ribbon stopped: the halves stand where it left them.
    pub fn end_scrub(&mut self) {
        if let Some((from, to, k)) = self.scrub.take() {
            self.mode = if k >= 0.5 { to } else { from };
        }
    }

    /// After a frame for `frame_ns`: whether the halves are still moving.
    pub fn settle(&mut self, frame_ns: u64) -> bool {
        // An icon carried, or one drying away.
        if self.carry.is_some() || self.drying.as_ref().is_some_and(|(_, _, t)| (frame_ns.saturating_sub(*t) as f64) < DRY_NS) {
            for half in &mut self.halves {
                let d = half.target_w - half.size.0;
                if d != 0 {
                    let step = ((d.abs() as f64 * 0.16).ceil() as i32).max(1);
                    half.size.0 += d.signum() * step.min(d.abs());
                }
            }
            return true;
        }
        self.drying = None;
        // A half making room for a running app, or closing up after one.
        let mut widening = false;
        for half in &mut self.halves {
            let d = half.target_w - half.size.0;
            if d != 0 {
                let step = ((d.abs() as f64 * 0.16).ceil() as i32).max(1);
                half.size.0 += d.signum() * step.min(d.abs());
                widening = true;
            }
        }
        if widening {
            return true;
        }
        if let Some((at, ..)) = self.birth {
            if frame_ns >= at + BIRTH_FALL_NS + BIRTH_SPREAD_NS + MOVE_NS + BIRTH_ICONS_NS {
                self.birth = None;
            } else {
                // Its parting ends as a move does: it wobbles to rest.
                if self.moving.as_ref().is_some_and(|m| frame_ns >= m.start_ns + Self::duration(m)) {
                    self.landed = Some(frame_ns);
                    self.moving = None;
                }
                return true;
            }
        }
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
                    || self.flowing.get()
                    || self.stretch.get().iter().any(|s| s.abs() > 0.003)
                    || self.popped.get().iter().any(|&t| t != 0 && frame_ns < t + WOBBLE_NS)
                    || self.trail.get().is_some_and(|(_, t)| frame_ns < t + WET_NS)
                    || self.snap.get().is_some()
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
            let icon = (0..self.halves[h].len()).find(|&i| self.cell(i, &p).contains(pos));
            self.halves[h].pressed = icon.map(|i| (i, slot, pos));
            self.pressed_at = hybris_hwc::now_ns();
            // Touched, the drops come alive.
            self.touched.set(self.pressed_at);
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
                    let app = half.item(i);
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

    /// A finger held still on a dock icon for LIFT_NS: it lifts out as a
    /// drop and follows the finger (its own leave the half, which closes
    /// up; a running one is carried to be put on it). Returns whether one
    /// did.
    pub fn long_press(&mut self, now_ns: u64) -> bool {
        if self.carry.is_some() || now_ns < self.pressed_at + LIFT_NS {
            return false;
        }
        let Some(h) = (0..2).find(|&h| self.halves[h].pressed.is_some()) else { return false };
        let (i, slot, pos) = self.halves[h].pressed.take().unwrap();
        let half = &mut self.halves[h];
        let (app, from) = if i < half.apps.len() {
            let app = half.apps.remove(i);
            half.target_w = half_width(half.len());
            (app, Some((h, i)))
        } else {
            (half.item(i).clone(), None)
        };
        tracing::info!("dock: {} lifted", app.name);
        self.carry = Some(Carry { slot, app, from, pos, since: now_ns, over: None });
        self.touched.set(now_ns);
        crate::fingerprint::buzz("button-pressed");
        true
    }

    /// An app from the grid, its finger held still on it: carried, to be
    /// put on the dock.
    pub fn carry_new(&mut self, slot: TouchSlot, desktop: &str, pos: Point<f64, Logical>, now_ns: u64) {
        let Some(app) = app(desktop) else { return };
        tracing::info!("dock: {} carried from the grid", app.name);
        self.carry = Some(Carry { slot, app, from: None, pos, since: now_ns, over: None });
        crate::fingerprint::buzz("button-pressed");
    }

    /// Whether an icon is carried.
    pub fn carrying(&self) -> bool {
        self.carry.is_some()
    }

    pub fn carries(&self, slot: TouchSlot) -> bool {
        self.carry.as_ref().is_some_and(|c| c.slot == slot)
    }

    /// The carried icon moves: the half under it opens a place for it.
    pub fn carry_motion(&mut self, pos: Point<f64, Logical>, frame_ns: u64) {
        let places = self.places(frame_ns);
        let Some(c) = self.carry.as_mut() else { return };
        c.pos = pos;
        let mut over = None;
        for h in 0..2 {
            let p = places[h];
            let half = &self.halves[h];
            let w = half_width(half.len().max(1)) as f64;
            let (x0, x1) = (p.x - TAKE_SIDE, p.x + w + TAKE_SIDE);
            let (y0, y1) = (p.y - TAKE_ABOVE, p.y + half.size.1 as f64 + 20.0);
            if pos.x >= x0 && pos.x <= x1 && pos.y >= y0 && pos.y <= y1 && p.y < layout::LAYOUT.1 as f64 {
                let at = ((pos.x - p.x - PAD as f64 + (ICON + GAP) as f64 / 2.0) / (ICON + GAP) as f64).floor().max(0.0) as usize;
                if half.apps.len() < MAX_PER_HALF || c.from.is_some_and(|(fh, _)| fh == h) {
                    over = Some((h, at.min(half.apps.len())));
                }
            }
        }
        if over != c.over {
            if let Some((h, _)) = c.over {
                let half = &mut self.halves[h];
                half.target_w = half_width(half.len());
            }
            if let Some((h, _)) = over {
                let half = &mut self.halves[h];
                half.target_w = half_width(half.len() + 1);
                self.touched.set(frame_ns);
            }
            c.over = over;
        }
    }

    /// The carried icon let go: into the place opened for it; far above
    /// the dock, off it (it dries away); else back where it came from.
    /// Returns whether the dock's apps changed.
    pub fn carry_up(&mut self, now_ns: u64) -> bool {
        let Some(c) = self.carry.take() else { return false };
        let top = self.places(now_ns).iter().map(|p| p.y).fold(f64::MAX, f64::min);
        let changed = match (c.over, c.from) {
            (Some((h, at)), _) => {
                tracing::info!("dock: {} onto the {} half", c.app.name, if h == 0 { "left" } else { "right" });
                self.halves[h].apps.insert(at, c.app);
                let mut popped = self.popped.get();
                popped[h] = now_ns;
                self.popped.set(popped);
                true
            }
            (None, Some(_)) if c.pos.y < top - OFF_ABOVE => {
                tracing::info!("dock: {} off the dock", c.app.name);
                self.drying = Some(((c.pos.x, c.pos.y), c.app.icon.clone(), now_ns));
                true
            }
            (None, Some((h, i))) => {
                let n = self.halves[h].apps.len();
                self.halves[h].apps.insert(i.min(n), c.app);
                false
            }
            (None, None) => {
                self.drying = Some(((c.pos.x, c.pos.y), c.app.icon.clone(), now_ns));
                false
            }
        };
        for half in &mut self.halves {
            half.target_w = half_width(half.len());
            half.pressed = None;
        }
        if changed {
            // Running apps it was or is now among are worked out again.
            for half in &mut self.halves {
                half.extra.clear();
            }
            save_config(&self.halves);
        }
        changed
    }

    /// The apps running, each with the panel it was opened onto: those on
    /// neither half get an icon at the end of that panel's half, which
    /// makes room for it; gone, it closes up again.
    pub fn set_running(&mut self, running: &[(String, usize)]) {
        let own: Vec<String> = self.halves.iter().flat_map(|h| h.apps.iter().flat_map(|a| a.ids.iter().cloned())).collect();
        for h in 0..2 {
            let want: Vec<&String> = running.iter().filter(|(id, p)| *p == h && !id.is_empty() && !own.contains(id)).map(|(id, _)| id).collect();
            let have: Vec<&String> = self.halves[h].extra.iter().map(|(id, _)| id).collect();
            if want == have {
                continue;
            }
            let extra: Vec<(String, App)> = want.iter().filter_map(|id| running_app(&mut self.found, id).map(|a| ((*id).clone(), a))).collect();
            tracing::info!("dock {}: running {}", if h == 0 { "left" } else { "right" }, extra.iter().map(|(_, a)| a.name.as_str()).collect::<Vec<_>>().join(", "));
            let half = &mut self.halves[h];
            half.extra = extra;
            half.target_w = half_width(half.len());
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
    /// `wall`: the wallpaper's texture, its size (logical px) and where the
    /// screen's left edge is in it, which the drops see through themselves.
    pub fn elements(&self, renderer: &mut GlesRenderer, frame_ns: u64, running: &[String], wall: Option<(&smithay::backend::renderer::gles::GlesTexture, (f64, f64), f64)>) -> Vec<ShellElement> {
        let mut out = Vec::new();
        let carried = self.carry.as_ref().map(|c| ((c.pos.x, c.pos.y - 18.0), c.app.icon.clone(), (frame_ns.saturating_sub(c.since) as f64 / CARRY_IN_NS).min(1.0), 1.0f32));
        let dried = self.drying.as_ref().map(|((x, y), icon, t)| {
            let k = (frame_ns.saturating_sub(*t) as f64 / DRY_NS).min(1.0);
            ((*x, *y - 30.0 * k), icon.clone(), 1.0 - k, (1.0 - k) as f32)
        });
        let floating: Vec<_> = carried.into_iter().chain(dried).collect();
        let floating_drops: Vec<ShellElement> = floating
            .iter()
            .filter_map(|((x, y), _, k, alpha)| wall.and_then(|w| self.lone(renderer, (*x, *y), CARRY_R * (0.4 + 0.6 * k), frame_ns, 0.8, *alpha, (0.0, [0.0; 3]), w)))
            .map(ShellElement::Shaded)
            .collect();
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
        // The icon a finger carries, a drop of its own over everything,
        // lifting out; one let go off the dock, drying away.
        for ((x, y), icon, k, alpha) in floating.iter() {
            if let Some(icon) = icon {
                let s = ICON as f64 * (0.6 + 0.4 * k);
                push(&mut out, icon, x - s / 2.0, y - s / 2.0, Some((s, s)), Some(Rectangle::new((0.0, 0.0).into(), (ICON as f64, ICON as f64).into())), *alpha);
            }
        }
        out.extend(floating_drops);
        // The drops as they stand: each half's box, its squash, and what
        // the hinge does to it (Shape).
        let shape = self.shape(&places, frame_ns);
        for (h, half) in self.halves.iter().enumerate() {
            let p = places[h];
            if p.y >= bottom || half.len() == 0 {
                continue;
            }
            let (bx, by, bw, bh) = shape.boxes[h];
            let (sx, sy) = shape.squash[h];
            let mid = bx + bw / 2.0;
            let foot = by + bh;
            let tx = |x: f64| mid + (p.anchor + (x - p.anchor) * p.scale - mid) * sx;
            let ty = |y: f64| foot - (foot - y) * sy;
            let own = half.apps.len();
            // A place opened for an icon carried over it.
            let gap = self.carry.as_ref().and_then(|c| c.over).filter(|(oh, _)| *oh == h).map(|(_, g)| g);
            for (i, app) in half.items().enumerate() {
                let i = if gap.is_some_and(|g| i >= g) { i + 1 } else { i };
                let c = self.cell(i, &p);
                // A running app's icon comes up as the drop makes room for
                // it, and goes as it closes up.
                let room = if i < own || gap.is_some() { 1.0 } else { ((half.size.0 as f64 - (PAD + i as i32 * (ICON + GAP)) as f64) / (ICON + PAD) as f64).clamp(0.0, 1.0) };
                let (cx, cy) = (tx(c.loc.x + ICON as f64 / 2.0), ty(c.loc.y + ICON as f64 / 2.0));
                // By the hinge an icon goes under with the water: it fades
                // as it nears the edge, and comes out again past it.
                let shown = (1.0 - hinge_cover(cx, ICON as f64 / 2.0)) * self.icons_shown(h, frame_ns) * room;
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
        let life = self.life(frame_ns);
        if let Some(e) = wall.and_then(|w| self.drops(renderer, &shape, frame_ns, w, life, DROP_RADIUS, 1.0, (0.0, [0.0; 3], 0.0), 1.0, &self.drops)) {
            out.push(ShellElement::Shaded(e));
        }
        out
    }

    /// The drops at `frame_ns` (see Shape): each half's box from its place,
    /// squashed by its motion; the two met, pulled apart, or apart; a half
    /// at the hinge in two, its tail clinging to the edge it leaves until it
    /// lets go, its head coming out round on the other side.
    fn shape(&self, places: &[Place; 2], frame_ns: u64) -> Shape {
        let bottom = layout::LAYOUT.1 as f64;
        let mut boxes: [(f64, f64, f64, f64); 2] = [0, 1].map(|h| {
            let p = places[h];
            let (w, ht) = (self.halves[h].size.0 as f64, self.halves[h].size.1 as f64);
            (p.anchor + (p.x - p.anchor) * p.scale, p.y, w * p.scale, ht)
        });
        // Carried by the hinge (no move of its own): the front edge leads,
        // the back one trails, the drop thinner as it is longer; on a move
        // of its own the edges are where the move has them.
        let by_hinge = self.moving.is_none() && self.scrub.is_none() && self.birth.is_none();
        let target = [0, 1].map(|h| [boxes[h].0, boxes[h].0 + boxes[h].2]);
        let mut flowing = false;
        if by_hinge {
            if let Some((t, mut edges, was)) = self.flow.get().filter(|(t, ..)| frame_ns > *t) {
                let dt = (frame_ns - t) as f64;
                let (front, back) = (1.0 - (-dt / FLOW_FRONT_NS).exp(), 1.0 - (-dt / FLOW_BACK_NS).exp());
                for h in 0..2 {
                    let mid = (target[h][0] + target[h][1]) / 2.0;
                    let way = mid - was[h];
                    // Going right the right edge leads; left, the left one;
                    // still, both draw in at the tail's pace.
                    let (kl, kr) = if way > 0.01 { (back, front) } else if way < -0.01 { (front, back) } else { (back, back) };
                    edges[h][0] += (target[h][0] - edges[h][0]) * kl;
                    edges[h][1] += (target[h][1] - edges[h][1]) * kr;
                    let (l, r) = (edges[h][0].min(edges[h][1] - 1.0), edges[h][1]);
                    let w0 = boxes[h].2;
                    if (r - l - w0).abs() > 0.3 || (l - target[h][0]).abs() > 0.3 {
                        flowing = true;
                    }
                    let thin = (w0 / (r - l).max(1.0)).clamp(FLOW_THIN, 1.0);
                    boxes[h] = (l, boxes[h].1 + boxes[h].3 * (1.0 - thin), r - l, boxes[h].3 * thin);
                    // The wet streak: from the tail back to where the water
                    // was a moment ago, drying as that end draws in.
                    let mut st = self.streak.get();
                    if way.abs() > 0.01 {
                        st[h].1 = way.signum();
                    }
                    let tail = if st[h].1 >= 0.0 { l } else { r };
                    if st[h].0 == 0.0 {
                        st[h].0 = tail;
                    }
                    st[h].0 += (tail - st[h].0) * (1.0 - (-dt / STREAK_NS).exp());
                    let len = (tail - st[h].0).abs();
                    let mut streaks = self.streaks.get();
                    if len > 1.0 {
                        let (ht, foot) = (boxes[h].3, boxes[h].1 + boxes[h].3);
                        // Under the drop a little too, so the two meet.
                        let (a, b) = if st[h].1 >= 0.0 { (st[h].0, tail + 6.0) } else { (tail - 6.0, st[h].0) };
                        streaks[h] = ([a.min(b), foot - 0.55 * ht - 0.3 * ht, (b - a).abs(), 0.6 * ht], (len / STREAK_FULL).min(1.0) * STREAK_WET);
                        flowing = true;
                    } else {
                        streaks[h] = ([-1000.0, 0.0, 10.0, 10.0], 0.0);
                    }
                    self.streaks.set(streaks);
                    self.streak.set(st);
                }
                self.flow.set(Some((frame_ns, edges, [0, 1].map(|h| (target[h][0] + target[h][1]) / 2.0))));
            } else if self.flow.get().is_none_or(|(t, ..)| frame_ns > t) {
                self.flow.set(Some((frame_ns, target, [0, 1].map(|h| (target[h][0] + target[h][1]) / 2.0))));
            }
        } else {
            self.flow.set(None);
            self.streak.set(Default::default());
            self.streaks.set([([-1000.0, 0.0, 10.0, 10.0], 0.0); 2]);
        }
        self.flowing.set(flowing);
        let shown = [0, 1].map(|h| places[h].y < bottom && self.halves[h].len() > 0);

        // Moving, a drop stretches along its way: its speed, smoothed; and
        // which way it goes.
        let xs = [places[0].x, places[1].x];
        let (mut stretch, mut dir) = (self.stretch.get(), self.dir.get());
        // (Carried by the hinge it flows instead: see `flow` above.)
        if let Some((t, was)) = self.last_x.get().filter(|(t, _)| frame_ns > *t) {
            let ms = (frame_ns - t) as f64 / 1e6;
            for h in 0..2 {
                let dx = xs[h] - was[h];
                let v = if ms < 100.0 && !by_hinge { dx.abs() / ms } else { 0.0 };
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

        // The hinge. Nearing it, a drop meets the edge first with its front:
        // it is held a moment, shorter along its way and fuller across.
        // Across it, it is in two, each part against its edge (reaching INTO
        // the hinge, where nothing is seen, so it ends straight at the edge),
        // the water going over from one to the other: each part as much of
        // the drop as is past the hinge's middle, round while small, long
        // once it is as high as the drop. The tail clings to its edge, as
        // water does - the last of it lingers - until it is too little and
        // lets go, drawn in to the edge over SNAP_NS; the head comes out
        // small and round, and grows.
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
                // Its front this far from the edge it goes to.
                let front = if dir[h] > 0.0 && x1 <= h0 { h0 - x1 } else if dir[h] < 0.0 && x0 >= h1 { x0 - h1 } else { f64::MAX };
                let k = smooth(1.0 - front / APPROACH);
                drops.push(([bx, by, bw, bh], (sx * (1.0 - HELD_SHORT * k), sy * (1.0 + HELD_FULL * k))));
                clinging[h] = false;
                continue;
            }
            let (wl, wr) = ((h0 - x0).max(0.0), (x1 - h1).max(0.0));
            let over = if wl + wr > 0.0 { wr / (wl + wr) } else { 0.5 };
            let middle = foot - ht / 2.0;
            let area = w * ht;
            // A part of `a` of the area: round while small, then long.
            let part = |a: f64| {
                let d = a.max(0.0).sqrt();
                if d <= ht { (d, d) } else { (a / ht, ht) }
            };
            // The tail: what is left on the side it comes from; it lingers.
            let tail_left = dir[h] > 0.0;
            let (mut left, mut right) = (1.0 - over, over);
            if dir[h] != 0.0 {
                let tail = if tail_left { left } else { right };
                let held = if tail < LET_GO_SHARE { 0.0 } else { tail.max(LINGER) };
                if clinging[h] && held == 0.0 {
                    // It let go: drawn in to the edge, a wet spot where it
                    // clung, and the head jolts.
                    let (tw, th) = part(LINGER * area);
                    self.snap.set(Some((tail_left, tw, th, middle, frame_ns)));
                    let edge = if tail_left { h0 - SPOT } else { h1 };
                    self.trail.set(Some(([edge, middle - 0.3 * ht, SPOT, 0.6 * ht], frame_ns)));
                    popped[h] = frame_ns;
                }
                clinging[h] = held > 0.0;
                if tail_left {
                    left = held;
                } else {
                    right = held;
                }
            }
            let ((lw, lh), (rw, rh)) = (part(left * area), part(right * area));
            if lw > 0.5 {
                drops.push(([h0 - lw, middle - lh / 2.0, lw + INTO, lh], (1.0, 1.0)));
            }
            if rw > 0.5 {
                drops.push(([h1 - INTO, middle - rh / 2.0, rw + INTO, rh], (1.0, 1.0)));
            }
        }
        // A tail that let go, drawn in to its edge.
        if let Some((on_left, tw, th, middle, at)) = self.snap.get() {
            let k = frame_ns.saturating_sub(at) as f64 / SNAP_NS as f64;
            if k < 1.0 {
                let s = 1.0 - smooth(k);
                let (w, ht) = (tw * s, th * (0.5 + 0.5 * s));
                if on_left {
                    drops.push(([h0 - w, middle - ht / 2.0, w + INTO, ht], (1.0, 1.0)));
                } else {
                    drops.push(([h1 - INTO, middle - ht / 2.0, w + INTO, ht], (1.0, 1.0)));
                }
            } else {
                self.snap.set(None);
            }
        }
        drops.truncate(4);
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
        let streaks = self.streaks.get();
        let (trail, trail2) = if trail.1 > 0.0 { (trail, streaks[1]) } else { (streaks[0], streaks[1]) };
        Shape { boxes, squash, drops, melt, one, trail, trail2 }
    }

    /// A drop alone, round, `r` about `centre` (logical px), as alive as
    /// `life` (0 still, 1 as the dock's after a touch), holding `fill` of
    /// `colour` (premultiplied): the first setup's. It flows as the dock's
    /// halves do: stretched along its way, held at the hinge's edge, over it
    /// in two parts as much as is past its middle, the tail lingering and
    /// let go with a jolt, a wet spot drying where it clung.
    #[allow(clippy::too_many_arguments)]
    pub fn lone(&self, renderer: &mut GlesRenderer, centre: (f64, f64), r: f64, frame_ns: u64, life: f64, alpha: f32, fill: (f64, [f32; 3]), wall: (&smithay::backend::renderer::gles::GlesTexture, (f64, f64), f64)) -> Option<smithay::backend::renderer::gles::element::TextureShaderElement> {
        let mut f = self.lone_flow.get();
        if let Some((t, was)) = f.last.filter(|(t, _)| frame_ns > *t) {
            let ms = (frame_ns - t) as f64 / 1e6;
            let dx = centre.0 - was;
            let v = if ms < 100.0 { dx.abs() / ms } else { 0.0 };
            f.stretch += ((v * STRETCH).min(STRETCH_MAX) - f.stretch) * STRETCH_FOLLOW;
            if f.stretch < 0.002 {
                f.stretch = 0.0;
            }
            if v > 0.02 {
                f.dir = dx.signum();
            } else if v == 0.0 && ms < 100.0 {
                f.dir = 0.0;
            }
        }
        if f.last.is_none_or(|(t, _)| frame_ns > t) {
            f.last = Some((frame_ns, centre.0));
        }
        let wobble = if f.popped == 0 { 0.0 } else {
            let u = frame_ns.saturating_sub(f.popped) as f64;
            if u >= WOBBLE_NS as f64 { 0.0 } else { POP * (-u / (WOBBLE_NS as f64 / 4.0)).exp() * (u / WOBBLE_PERIOD_NS * std::f64::consts::TAU).sin() }
        };
        let (sx, sy) = ((1.0 + f.stretch) * (1.0 - 0.6 * wobble), (1.0 - 0.6 * f.stretch) * (1.0 + wobble));
        let d = 2.0 * r;
        let b = [centre.0 - r, centre.1 - r, d, d];
        let (w, ht) = (d * sx, d * sy);
        let (x0, x1) = (centre.0 - w / 2.0, centre.0 + w / 2.0);
        let panels = layout::panels();
        let (h0, h1) = ((panels[0].loc.x + panels[0].size.w) as f64, panels[1].loc.x as f64);
        let mut drops: Vec<([f64; 4], (f64, f64))> = Vec::new();
        if x1 <= h0 || x0 >= h1 {
            let front = if f.dir > 0.0 && x1 <= h0 { h0 - x1 } else if f.dir < 0.0 && x0 >= h1 { x0 - h1 } else { f64::MAX };
            let k = smooth(1.0 - front / APPROACH);
            drops.push((b, (sx * (1.0 - HELD_SHORT * k), sy * (1.0 + HELD_FULL * k))));
            f.clinging = false;
        } else {
            let (wl, wr) = ((h0 - x0).max(0.0), (x1 - h1).max(0.0));
            let over = if wl + wr > 0.0 { wr / (wl + wr) } else { 0.5 };
            let middle = centre.1 + d / 2.0 - ht / 2.0;
            let area = w * ht;
            let part = |a: f64| {
                let s = a.max(0.0).sqrt();
                if s <= ht { (s, s) } else { (a / ht, ht) }
            };
            let tail_left = f.dir > 0.0;
            let (mut left, mut right) = (1.0 - over, over);
            if f.dir != 0.0 {
                let tail = if tail_left { left } else { right };
                let held = if tail < LET_GO_SHARE { 0.0 } else { tail.max(LINGER) };
                if f.clinging && held == 0.0 {
                    let (tw, th) = part(LINGER * area);
                    f.snap = Some((tail_left, tw, th, middle, frame_ns));
                    let edge = if tail_left { h0 - SPOT } else { h1 };
                    f.trail = Some(([edge, middle - 0.3 * ht, SPOT, 0.6 * ht], frame_ns));
                    f.popped = frame_ns;
                }
                f.clinging = held > 0.0;
                if tail_left {
                    left = held;
                } else {
                    right = held;
                }
            }
            let ((lw, lh), (rw, rh)) = (part(left * area), part(right * area));
            if lw > 0.5 {
                drops.push(([h0 - lw, middle - lh / 2.0, lw + INTO, lh], (1.0, 1.0)));
            }
            if rw > 0.5 {
                drops.push(([h1 - INTO, middle - rh / 2.0, rw + INTO, rh], (1.0, 1.0)));
            }
        }
        if let Some((on_left, tw, th, middle, at)) = f.snap {
            let k = frame_ns.saturating_sub(at) as f64 / SNAP_NS as f64;
            if k < 1.0 {
                let s = 1.0 - smooth(k);
                let (w, ht) = (tw * s, th * (0.5 + 0.5 * s));
                drops.push((if on_left { [h0 - w, middle - ht / 2.0, w + INTO, ht] } else { [h1 - INTO, middle - ht / 2.0, w + INTO, ht] }, (1.0, 1.0)));
            } else {
                f.snap = None;
            }
        }
        drops.truncate(4);
        let trail = match f.trail {
            Some((t, at)) if frame_ns < at + WET_NS => {
                let k = (frame_ns - at) as f64 / WET_NS as f64;
                let inset = k * 0.3 * t[2].min(t[3]);
                ([t[0] + inset, t[1] + inset, t[2] - 2.0 * inset, t[3] - 2.0 * inset], (1.0 - k).powf(1.5))
            }
            _ => {
                f.trail = None;
                ([-1000.0, 0.0, 10.0, 10.0], 0.0)
            }
        };
        self.lone_flow.set(f);
        // The level of what it holds: from the drop's foot up.
        let level_y = centre.1 + r - d * sy * fill.0;
        let shape = Shape { boxes: [(b[0], b[1], b[2], b[3]); 2], squash: [(sx, sy); 2], drops, melt: APART_MELT, one: None, trail, trail2: ([-1000.0, 0.0, 10.0, 10.0], 0.0) };
        self.drops(renderer, &shape, frame_ns, wall, life, r as f32, alpha, (level_y, fill.1, fill.0), 1.0, &self.lone)
    }

    /// Up to four round drops drawn as one (melting together `melt` px
    /// where they near), each its middle, radius and squash: the first
    /// setup's PIN keys. `slot` keeps each group's element apart.
    #[allow(clippy::too_many_arguments)]
    pub fn group(&self, renderer: &mut GlesRenderer, slot: usize, drops: &[((f64, f64), f64, (f64, f64))], melt: f64, frame_ns: u64, life: f64, alpha: f32, dim: f32, wall: (&smithay::backend::renderer::gles::GlesTexture, (f64, f64), f64)) -> Option<smithay::backend::renderer::gles::element::TextureShaderElement> {
        if drops.is_empty() {
            return None;
        }
        {
            let mut groups = self.groups.borrow_mut();
            while groups.len() <= slot {
                groups.push(Default::default());
            }
        }
        let list: Vec<([f64; 4], (f64, f64))> = drops.iter().take(4).map(|&((x, y), r, sq)| ([x - r, y - r, 2.0 * r, 2.0 * r], sq)).collect();
        let r = drops.iter().map(|d| d.1).fold(0.0, f64::max);
        let b = list[0].0;
        let shape = Shape { boxes: [(b[0], b[1], b[2], b[3]); 2], squash: [(1.0, 1.0); 2], drops: list, melt, one: None, trail: ([-1000.0, 0.0, 10.0, 10.0], 0.0), trail2: ([-1000.0, 0.0, 10.0, 10.0], 0.0) };
        let groups = self.groups.borrow();
        self.drops(renderer, &shape, frame_ns, wall, life, r as f32, alpha, (0.0, [0.0; 3], 0.0), dim, &groups[slot])
    }

    /// Whether the lone drop still flows on its own (stretch, the hinge's
    /// tail, its wobble, the wet spot drying).
    pub fn lone_moving(&self, frame_ns: u64) -> bool {
        let f = self.lone_flow.get();
        f.stretch > 0.003 || f.clinging || f.snap.is_some() || f.trail.is_some_and(|(_, t)| frame_ns < t + WET_NS) || (f.popped != 0 && frame_ns < f.popped + WOBBLE_NS)
    }

    /// The drops, through dock.frag: an element over the screen's bottom,
    /// its uniforms set again only when they change.
    #[allow(clippy::too_many_arguments)]
    fn drops(&self, renderer: &mut GlesRenderer, shape: &Shape, frame_ns: u64, wall: (&smithay::backend::renderer::gles::GlesTexture, (f64, f64), f64), life: f64, radius: f32, alpha: f32, fill: (f64, [f32; 3], f64), dim: f32, cache: &std::cell::RefCell<Option<(smithay::backend::renderer::element::Id, Vec<f32>)>>) -> Option<smithay::backend::renderer::gles::element::TextureShaderElement> {
        use smithay::backend::renderer::element::texture::TextureRenderElement;
        use smithay::backend::renderer::gles::element::TextureShaderElement;
        use smithay::backend::renderer::gles::{Uniform, UniformName, UniformType};
        use smithay::backend::renderer::Renderer;
        if self.program.borrow().is_none() {
            let mut names: Vec<UniformName> = ["b0", "b1", "b2", "b3", "s01", "s23", "one", "meet", "body", "trail", "trail2"].into_iter().map(|n| UniformName::new(n, UniformType::_4f)).collect();
            names.push(UniformName::new("fill", UniformType::_4f));
            names.extend(["radius", "melt", "shine", "metal", "time", "life", "woff", "wet", "wet2", "level", "dim"].into_iter().map(|n| UniformName::new(n, UniformType::_1f)));
            names.extend(["origin", "texl", "src0"].into_iter().map(|n| UniformName::new(n, UniformType::_2f)));
            match renderer.compile_custom_texture_shader(include_str!("dock.frag"), &names) {
                Ok(p) => *self.program.borrow_mut() = Some(p),
                Err(e) => {
                    tracing::warn!("dock: the drops' shader: {e}");
                    return None;
                }
            }
        }
        let (width, height) = layout::LAYOUT;
        // Drawn over as little as it can: the drops, the wet spot, and room
        // round them for the shadow, the thread, the ripple and the breath;
        // in whole steps, so the area moves less often than the drops.
        let mut bounds: Option<(f64, f64, f64, f64)> = None;
        let mut add = |b: &[f64; 4]| {
            if b[0] < -500.0 {
                return;
            }
            let r = (b[0], b[1], b[0] + b[2], b[1] + b[3]);
            bounds = Some(match bounds {
                Some(o) => (o.0.min(r.0), o.1.min(r.1), o.2.max(r.2), o.3.max(r.3)),
                None => r,
            });
        };
        for (b, (sx, sy)) in &shape.drops {
            // Squashed about the bottom middle: as wide and high as that.
            let (w, h) = (b[2] * sx, b[3] * sy);
            add(&[b[0] + b[2] / 2.0 - w / 2.0, b[1] + b[3] - h, w, h]);
        }
        if let Some((b, ..)) = shape.one {
            add(&b);
        }
        if shape.trail.1 > 0.0 {
            add(&shape.trail.0);
        }
        if shape.trail2.1 > 0.0 {
            add(&shape.trail2.0);
        }
        let step = 32.0;
        let (x0, y0, x1, y1) = bounds.unwrap_or((0.0, height as f64 - 1.0, 1.0, height as f64));
        let pad = 28.0;
        let snap_lo = |v: f64| ((v - pad) / step).floor() * step;
        let snap_hi = |v: f64| ((v + pad) / step).ceil() * step;
        let (ax0, ay0) = (snap_lo(x0).max(0.0) as i32, snap_lo(y0).max(0.0) as i32);
        let (ax1, ay1) = (snap_hi(x1).min(width as f64) as i32, snap_hi(y1).min(height as f64) as i32);
        let area = Rectangle::<i32, Logical>::new((ax0, ay0).into(), ((ax1 - ax0).max(1), (ay1 - ay0).max(1)).into());
        let (left, top) = (ax0, ay0);
        let y = |v: f64| (v - top as f64) as f32;
        let x = |v: f64| (v - left as f64) as f32;
        let far = ([-1000.0, 0.0, 10.0, 10.0], (1.0, 1.0));
        let d: Vec<([f64; 4], (f64, f64))> = (0..4).map(|i| shape.drops.get(i).copied().unwrap_or(far)).collect();
        let (one, meet) = match shape.one {
            Some((b, fill, bulge, join)) => (b, [fill, bulge, join, 0.0]),
            None => ([-1000.0, 0.0, 10.0, 10.0], [0.0; 4]),
        };
        let mut v: Vec<f32> = Vec::new();
        for (b, _) in &d {
            v.extend([x(b[0]), y(b[1]), b[2] as f32, b[3] as f32]);
        }
        v.extend([d[0].1 .0, d[0].1 .1, d[1].1 .0, d[1].1 .1, d[2].1 .0, d[2].1 .1, d[3].1 .0, d[3].1 .1].map(|x| x as f32));
        v.extend([x(one[0]), y(one[1]), one[2] as f32, one[3] as f32]);
        v.extend([meet[0] as f32, meet[1] as f32, x(meet[2]), meet[3] as f32]);
        let t = shape.trail.0;
        v.extend([x(t[0]), y(t[1]), t[2] as f32, t[3] as f32, shape.trail.1 as f32]);
        let (texture, texl, woff) = wall;
        v.extend([shape.melt as f32, ((frame_ns / 1_000_000) % 1_000_000) as f32 / 1000.0, life as f32, woff as f32, top as f32, left as f32]);
        v.extend([texl.0 as f32, texl.1 as f32, (woff + left as f64) as f32, top as f32, alpha, radius]);
        v.extend([y(fill.0), fill.1[0], fill.1[1], fill.1[2], fill.2 as f32, dim]);
        let t2 = shape.trail2.0;
        v.extend([x(t2[0]), y(t2[1]), t2[2] as f32, t2[3] as f32, shape.trail2.1 as f32]);
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
                Uniform::new("woff", v[40]),
                Uniform::new("origin", (v[42], v[41])),
                Uniform::new("texl", (v[43], v[44])),
                Uniform::new("src0", (v[45], v[46])),
                Uniform::new("radius", radius),
                Uniform::new("fill", (v[49], v[50], v[51], v[52])),
                Uniform::new("level", v[53]),
                Uniform::new("dim", v[54]),
                Uniform::new("trail2", (v[55], v[56], v[57], v[58])),
                Uniform::new("wet2", v[59]),
                Uniform::new("body", (BODY[0] * BODY[3], BODY[1] * BODY[3], BODY[2] * BODY[3], BODY[3])),
                Uniform::new("shine", 1.0f32),
                Uniform::new("metal", if std::env::var_os("DOCK_SILVER").is_some() { 1.0f32 } else { 0.0 }),
            ]
        };
        // A new id when anything changed: the frame takes the area again.
        let mut drops = cache.borrow_mut();
        let id = match drops.as_mut() {
            Some((id, last)) if *last == v => id.clone(),
            _ => {
                let id = smithay::backend::renderer::element::Id::new();
                *drops = Some((id.clone(), v.clone()));
                id
            }
        };
        let program = self.program.borrow().clone()?;
        // The wallpaper's part under the area: the drops' shader samples the
        // texture wherever the water bends what is behind it.
        let src = Rectangle::<f64, Logical>::new((woff + left as f64, top as f64).into(), (area.size.w as f64, area.size.h as f64).into());
        let inner = TextureRenderElement::from_static_texture(
            id,
            renderer.context_id(),
            ((area.loc.x * SCALE) as f64, (area.loc.y * SCALE) as f64),
            texture.clone(),
            SCALE,
            Transform::Normal,
            (alpha < 1.0).then_some(alpha),
            Some(src),
            Some(area.size),
            None,
            Kind::Unspecified,
        );
        Some(TextureShaderElement::new(inner, program, uniforms(&v)))
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
    /// A second wet patch (the other drop's streak).
    trail2: ([f64; 4], f64),
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

/// The halves' apps to `~/.config/sfduo/dock.json`, as config() reads it.
fn save_config(halves: &[Half]) {
    let Ok(home) = std::env::var("HOME") else { return };
    let list = |h: &Half| h.apps.iter().map(|a| format!("  \"{}.desktop\"", a.ids.first().cloned().unwrap_or_default())).collect::<Vec<_>>().join(",\n");
    let text = format!("{{\n \"left\": [\n{}\n ],\n \"right\": [\n{}\n ]\n}}\n", list(&halves[0]), list(&halves[1]));
    let dir = format!("{home}/.config/sfduo");
    let _ = std::fs::create_dir_all(&dir);
    match std::fs::write(format!("{dir}/dock.json"), text) {
        Ok(()) => tracing::info!("dock: its apps kept"),
        Err(e) => tracing::warn!("dock: its apps not kept: {e}"),
    }
}

/// A running app's dock item, by the app id its window gave: the desktop
/// file of that name, or the one whose StartupWMClass it is.
fn running_app(found: &mut std::collections::HashMap<String, Option<App>>, id: &str) -> Option<App> {
    found
        .entry(id.to_owned())
        .or_insert_with(|| crate::apps::all().into_iter().find(|e| e.ids.iter().any(|i| i == id)).map(app_of))
        .clone()
}

/// A dock item from its desktop file (apps.rs), with its icons at the
/// dock's and the curtain's sizes.
fn app(desktop: &str) -> Option<App> {
    crate::apps::entry(desktop).map(app_of)
}

fn app_of(e: crate::apps::Entry) -> App {    let small = e.icon.as_deref().and_then(|n| crate::apps::icon(n, ICON));
    let large = e.icon.as_deref().and_then(|n| crate::apps::icon(n, crate::curtain::ICON));
    App { name: e.name, exec: e.exec, ids: e.ids, icon: small, large }
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

fn ease_in_out(k: f64) -> f64 {
    let k = k.clamp(0.0, 1.0);
    if k < 0.5 { 4.0 * k * k * k } else { 1.0 - (-2.0 * k + 2.0).powi(3) / 2.0 }
}

fn ease_out(k: f64) -> f64 {
    let k = k.clamp(0.0, 1.0);
    1.0 - (1.0 - k).powi(3)
}
