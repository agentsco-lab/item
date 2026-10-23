//! The flow (draft 46): the open windows as covers in a row on the deck,
//! the frame turned - the chosen one in the middle, the window on the
//! screen. A sideways swipe scrolls the row; let go, it runs on and settles
//! on the nearest cover, and the screen pages to that window; a tap on a
//! side cover centres it. The covers are the windows' last frames
//! (output.rs's snapshots), the side ones turned away a little (cover.frag).

use smithay::backend::input::TouchSlot;
use smithay::desktop::Window;
use smithay::utils::{Logical, Point, Rectangle};

use crate::layout;

/// The row's room on the deck: under the dock's halves, above the keyboard.
const TOP: f64 = 96.0;
const BOTTOM_PAD: f64 = 16.0;
/// A cover at most this tall (logical px); the name's line under the row.
const COVER_MAX_H: f64 = 400.0;
const NAME_H: f64 = 36.0;
/// The side covers' size against the middle one, and the step between
/// covers as a share of the middle one's width.
const SIDE_SCALE: f64 = 0.85;
const STEP: f64 = 0.7;
/// The side covers' turn (radians) and the eye's distance in cover widths
/// (the perspective; cover.frag).
pub const TILT: f64 = 0.3;
pub const EYE: f64 = 3.0;
/// The reflection's strength at the cover's foot.
pub const FADE: f32 = 0.28;
/// The covers' corners, logical px.
pub const CORNER: f64 = 14.0;

/// A cover as drawn: its bounding box on screen, its turn, the box's left
/// and right in cover widths from the plane's middle, the box's height over
/// the cover's (cover.frag's uniforms).
pub struct Cover {
    pub index: usize,
    pub rect: Rectangle<f64, Logical>,
    pub tilt: f64,
    pub xrange: (f64, f64),
    pub yscale: f64,
    /// The cover's own size (the plane's), logical px.
    pub size: (f64, f64),
    pub alpha: f32,
}

/// A plane of width 1 turned by `tilt`, seen with the eye `EYE` away: the
/// screen x of its point `s` (-0.5..0.5 along it), and the scale there.
fn project(s: f64, tilt: f64) -> (f64, f64) {
    let (c, sn) = (tilt.cos(), tilt.sin());
    let f = EYE / (EYE - s * sn);
    (s * c * f, f)
}
/// A touch that travels less than this is a tap.
const TAP: f64 = 12.0;
/// Let go: the run to the cover it settles on.
pub const RUN_NS: f64 = 380e6;
/// How far the speed carries (ms of travel counted).
const CARRY_MS: f64 = 160.0;

/// A swipe up on a cover this far closes its window.
const CLOSE_PX: f64 = 90.0;
/// The closed cover's way up and out (ns), and how far it flies.
const CLOSE_NS: f64 = 240e6;
const CLOSE_FLY: f64 = 320.0;

struct Grab {
    slot: TouchSlot,
    start_x: f64,
    start_y: f64,
    from: f64,
    last: (u64, f64),
    velocity: f64,
    moved: bool,
    /// Lifted (a swipe up): how far, logical px; the row does not scroll then.
    lift: f64,
}

struct Run {
    from: f64,
    to: f64,
    start_ns: u64,
}

#[derive(Default)]
pub struct Flow {
    /// The row's position: the cover at the middle, in covers (fractional
    /// while it moves).
    pos: f64,
    grab: Option<Grab>,
    run: Option<Run>,
    /// The windows in the row, as the ribbon has them.
    pub windows: Vec<Window>,
    /// The cover the row last settled on, for the screen to page to.
    settled: Option<usize>,
    /// A cover swiped up: its index and since when, flying out while its
    /// window closes.
    closing: Option<(usize, u64)>,
    /// The cover a run just set off to, for the screen to page alongside.
    heading: Option<usize>,
    /// The windows' names by app id, and their labels (text.rs).
    names: std::cell::RefCell<std::collections::HashMap<String, String>>,
    /// The windows' icons by app id (the curtain's size), for a cover
    /// without a frame yet.
    icons: std::cell::RefCell<std::collections::HashMap<String, Option<smithay::backend::renderer::element::memory::MemoryRenderBuffer>>>,
    labels: std::cell::RefCell<std::collections::HashMap<String, crate::text::Label>>,
    font: std::cell::OnceCell<Option<crate::text::Font>>,
}

/// A window's icon, as its desktop file has it, at the curtain's size.
pub fn icon_of(window: &Window, icons: &mut std::collections::HashMap<String, Option<smithay::backend::renderer::element::memory::MemoryRenderBuffer>>) -> Option<smithay::backend::renderer::element::memory::MemoryRenderBuffer> {
    let id = crate::state::app_id(window);
    icons
        .entry(id.clone())
        .or_insert_with(|| crate::apps::all().into_iter().find(|e| e.ids.iter().any(|i| *i == id)).and_then(|e| e.icon).and_then(|n| crate::apps::icon(&n, crate::curtain::ICON)))
        .clone()
}

/// A cover of the row: the window's name, as its desktop file has it.
pub fn name_of(window: &Window, names: &mut std::collections::HashMap<String, String>) -> String {
    let id = crate::state::app_id(window);
    names.entry(id.clone()).or_insert_with(|| crate::apps::all().into_iter().find(|e| e.ids.iter().any(|i| *i == id)).map(|e| e.name).unwrap_or(id)).clone()
}

impl Flow {
    /// The row's windows and the one the screen shows, as the ribbon has
    /// them (state.rs, when the ribbon applies).
    pub fn sync(&mut self, windows: Vec<Window>, shown: Option<usize>) {
        if let Some((i, _)) = self.closing {
            if self.windows.get(i).is_some_and(|w| !windows.contains(w)) {
                self.closing = None;
            }
        }
        self.windows = windows;
        if let Some(i) = shown {
            if self.grab.is_none() && self.run.is_none() {
                self.pos = i as f64;
            }
        }
        self.pos = self.pos.clamp(0.0, self.windows.len().saturating_sub(1) as f64);
    }

    /// Whether the row is on the deck: the frame turned, windows open.
    pub fn shown(&self) -> bool {
        layout::turned() && !self.windows.is_empty()
    }

    /// The row's room: the deck under the dock, above the keyboard's tile
    /// when it is up (`cut`, its height).
    pub fn room(cut: Option<i32>) -> Rectangle<f64, Logical> {
        let deck = layout::panels()[1];
        let top = deck.loc.y as f64 + TOP;
        let bottom = (deck.loc.y + deck.size.h) as f64 - cut.map_or(BOTTOM_PAD, |h| h as f64 + BOTTOM_PAD);
        Rectangle::new((deck.loc.x as f64, top).into(), (deck.size.w as f64, (bottom - top).max(60.0)).into())
    }

    /// The middle cover's size: the screen's shape, as tall as the room
    /// with the keyboard up allows - the same whether or not it is up (the
    /// owner: the row must not grow when the keyboard goes).
    fn cover_size(_room: Rectangle<f64, Logical>) -> (f64, f64) {
        let screen = layout::panels()[0];
        let room = Self::room(Some(crate::kbd::TILE_H));
        let h = (room.size.h - NAME_H).min(COVER_MAX_H).max(40.0);
        (h * screen.size.w as f64 / screen.size.h as f64, h)
    }

    /// The row's own band in the room: where it stands with the keyboard
    /// up, whether or not it is (the covers do not move with the keyboard).
    pub fn band(room: Rectangle<f64, Logical>) -> Rectangle<f64, Logical> {
        let fixed = Self::room(Some(crate::kbd::TILE_H));
        Rectangle::new(room.loc, (room.size.w, fixed.size.h.min(room.size.h)).into())
    }

    pub fn position(&self, now_ns: u64) -> f64 {
        match &self.run {
            Some(r) => {
                let k = (now_ns.saturating_sub(r.start_ns) as f64 / RUN_NS).clamp(0.0, 1.0);
                let e = 1.0 - (1.0 - k).powi(3);
                r.from + (r.to - r.from) * e
            }
            None => self.pos,
        }
    }

    /// Each cover as drawn at `now` in the row's `room`, the farthest
    /// first (the middle one on top): the middle flat and largest, the
    /// others turned away, overlapping.
    pub fn covers(&self, room: Rectangle<f64, Logical>, now_ns: u64) -> Vec<Cover> {
        self.covers_with(room, now_ns, false)
    }

    /// As `covers`, with one more at the end for an app on its way (the
    /// launch curtain up, its window not yet there).
    pub fn covers_with(&self, room: Rectangle<f64, Logical>, now_ns: u64, pending: bool) -> Vec<Cover> {
        let room = Self::band(room);
        let (w, h) = Self::cover_size(room);
        let pos = if pending && self.windows.is_empty() { 0.0 } else { self.position(now_ns) };
        let cx = room.loc.x + room.size.w / 2.0;
        let cy = room.loc.y + (room.size.h - NAME_H) / 2.0;
        let mut out = Vec::new();
        let n = self.windows.len() + if pending { 1 } else { 0 };
        for i in 0..n {
            let d = i as f64 - pos;
            if d.abs() > 4.0 {
                continue;
            }
            let s = 1.0 - (1.0 - SIDE_SCALE) * d.abs().min(1.0);
            let (cw, ch) = (w * s, h * s);
            // The turn grows to TILT over the first cover out, the side it
            // turns to the side it stands on; the middle's own x is where
            // the covers step along, the near edge a little further.
            let tilt = -d.clamp(-1.0, 1.0) * TILT;
            let (xl, _) = project(-0.5, tilt);
            let (xr, _) = project(0.5, tilt);
            let (x0, x1) = (xl.min(xr), xl.max(xr));
            let (_, fnear) = project(0.5 * tilt.sin().signum(), tilt);
            let centre_x = cx + d * w * STEP;
            // Lifted by the finger, or flying out while its window closes.
            let (lift, alpha) = match (&self.grab, self.closing) {
                (_, Some((ci, at))) if ci == i => {
                    let k = (now_ns.saturating_sub(at) as f64 / CLOSE_NS).clamp(0.0, 1.0);
                    let e = 1.0 - (1.0 - k).powi(2);
                    (CLOSE_PX + CLOSE_FLY * e, (1.0 - e) as f32)
                }
                (Some(g), _) if g.lift > 0.0 && (i as f64 - pos).abs() < 0.5 => (g.lift, (1.0 - (g.lift / (CLOSE_PX * 2.5)).min(0.5)) as f32),
                _ => (0.0, 1.0),
            };
            let rect = Rectangle::new((centre_x + x0 * cw, cy - lift - ch * fnear / 2.0).into(), ((x1 - x0) * cw, ch * fnear).into());
            out.push(Cover { index: i, rect, tilt, xrange: (x0, x1), yscale: fnear, size: (cw, ch), alpha });
        }
        out.sort_by(|a, b| (b.index as f64 - pos).abs().partial_cmp(&(a.index as f64 - pos).abs()).unwrap());
        out
    }

    pub fn holds(&self, slot: TouchSlot) -> bool {
        self.grab.as_ref().is_some_and(|g| g.slot == slot)
    }

    pub fn down(&mut self, slot: TouchSlot, pos: Point<f64, Logical>, time_us: u64) -> bool {
        if !self.shown() || self.grab.is_some() {
            return false;
        }
        let from = self.position(hybris_hwc::now_ns());
        self.run = None;
        self.pos = from;
        self.grab = Some(Grab { slot, start_x: pos.x, start_y: pos.y, from, last: (time_us, pos.x), velocity: 0.0, moved: false, lift: 0.0 });
        true
    }

    /// A window's icon for its cover, while it has no frame.
    pub fn icon(&self, window: &Window) -> Option<smithay::backend::renderer::element::memory::MemoryRenderBuffer> {
        icon_of(window, &mut self.icons.borrow_mut())
    }

    /// Whether the row shows while the launch curtain is up with no window
    /// yet: a pending cover at its end (output.rs).
    pub fn shown_or_pending(&self, pending: bool) -> bool {
        layout::turned() && (!self.windows.is_empty() || pending)
    }

    /// The name under the row: the middle cover's window's.
    pub fn name(&self, now_ns: u64) -> Option<(crate::text::Label, f64)> {
        let i = self.position(now_ns).round().max(0.0) as usize;
        let window = self.windows.get(i)?;
        let text = name_of(window, &mut self.names.borrow_mut());
        let font = self.font.get_or_init(|| crate::text::Font::load(&["/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf", "/usr/share/fonts/truetype/lato/Lato-Regular.ttf"]));
        let font = font.as_ref()?;
        let mut labels = self.labels.borrow_mut();
        let label = labels.entry(text.clone()).or_insert_with(|| {
            let mut l = crate::text::Label::new(15.0, [0.85, 0.87, 0.9, 1.0]);
            l.set(font, &text);
            l
        });
        // Faint while the row moves between covers.
        let d = (self.position(now_ns) - i as f64).abs();
        Some((label.clone(), (1.0 - d * 2.0).clamp(0.0, 1.0)))
    }

    pub fn motion(&mut self, slot: TouchSlot, pos: Point<f64, Logical>, time_us: u64, room: Rectangle<f64, Logical>) {
        let Some(g) = self.grab.as_mut().filter(|g| g.slot == slot) else { return };
        let (w, _) = Self::cover_size(room);
        let step = w * STEP;
        let dt = time_us.saturating_sub(g.last.0) as f64 / 1000.0;
        if dt > 0.0 {
            let v = (pos.x - g.last.1) / dt;
            g.velocity = g.velocity * 0.4 + v * 0.6;
        }
        g.last = (time_us, pos.x);
        let (dx, dy) = (pos.x - g.start_x, pos.y - g.start_y);
        // Up, more than across: the cover lifts instead of the row scrolling.
        if g.lift > 0.0 || (!g.moved && -dy > TAP && -dy > dx.abs()) {
            g.lift = (-dy).max(0.0);
            self.pos = g.from;
            return;
        }
        g.moved |= dx.abs() > TAP;
        let n = self.windows.len().saturating_sub(1) as f64;
        self.pos = (g.from - dx / step).clamp(-0.3, n + 0.3);
    }

    /// Let go: a tap on a cover centres it; a swipe runs on to the cover
    /// its speed reaches, at the nearest.
    /// The cover the row is running to, told once as the run starts (the
    /// screen pages alongside).
    pub fn take_heading(&mut self) -> Option<usize> {
        self.heading.take()
    }

    pub fn up(&mut self, slot: TouchSlot, room: Rectangle<f64, Logical>, now_ns: u64) -> Option<Window> {
        let Some(g) = self.grab.take_if(|g| g.slot == slot) else { return None };
        let n = self.windows.len().saturating_sub(1) as f64;
        // Lifted far enough: the middle cover's window closes, the cover flies.
        if g.lift > 0.0 {
            if g.lift >= CLOSE_PX && self.closing.is_none() {
                let i = self.pos.round().max(0.0) as usize;
                if let Some(w) = self.windows.get(i).cloned() {
                    self.closing = Some((i, now_ns));
                    return Some(w);
                }
            }
            return None;
        }
        let (w, _) = Self::cover_size(room);
        let step = w * STEP;
        let to = if !g.moved {
            // The cover under the finger.
            let cx = room.loc.x + room.size.w / 2.0;
            (self.pos + (g.start_x - cx) / step).round()
        } else {
            (self.pos - g.velocity * CARRY_MS / step).round()
        }
        .clamp(0.0, n);
        self.run = Some(Run { from: self.pos, to, start_ns: now_ns });
        self.heading = Some(to.round().max(0.0) as usize);
        None
    }

    pub fn cancel(&mut self) {
        self.grab = None;
    }

    /// The run settled: the cover it stopped on, once; whether a frame is
    /// still wanted.
    pub fn settle(&mut self, now_ns: u64) -> (Option<usize>, bool) {
        let Some(r) = &self.run else { return (None, false) };
        if now_ns < r.start_ns + RUN_NS as u64 {
            return (None, true);
        }
        let to = r.to;
        self.pos = to;
        self.run = None;
        let i = to.round().max(0.0) as usize;
        let changed = self.settled != Some(i);
        self.settled = Some(i);
        (changed.then_some(i), true)
    }

    pub fn moving(&self) -> bool {
        self.grab.is_some() || self.run.is_some() || self.closing.is_some()
    }
}
