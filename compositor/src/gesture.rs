//! Putting a window away: a swipe up from the bottom edge of a panel that
//! has a window, item's (sfduo-dock's bottom band, phoc's minimize motion).
//!
//! item minimizes on the release and phoc plays 280 ms of motion: the window
//! shrinks to 0.3, its bottom settling 12 px above its old bottom edge, and
//! fades (alpha 1 - p²). Here the same motion follows the finger: `p` is the
//! finger's travel up over TRAVEL. Let go, the window runs the rest of the
//! way, faster than FLICK up or past COMMIT; back otherwise. Put away, it
//! leaves the space for the list of windows put away, its panel is free,
//! and the dock comes back. A tap on its app in the dock brings it back
//! with the motion reversed.

use smithay::backend::input::TouchSlot;
use smithay::desktop::Window;
use smithay::utils::{Logical, Physical, Point, Rectangle};

use crate::layout;

/// The band along a panel's bottom edge a swipe starts in (item's EDGE).
pub const EDGE: f64 = 36.0;
/// The finger's travel up that puts a window all the way away.
const TRAVEL: f64 = 320.0;
/// A release faster than this (logical px per ms) goes its way.
const FLICK: f64 = 0.3;
/// A slower release puts away past this much of the way.
const COMMIT: f64 = 0.3;
/// The whole run's time, as phoc's minimize.
const RUN_NS: f64 = 280e6;
/// The window's size put away, and how far above its old bottom it ends.
const SMALLEST: f64 = 0.3;
const RISE: f64 = 12.0;

struct Grab {
    slot: TouchSlot,
    window: Window,
    panel: usize,
    start_y: f64,
    p: f64,
    last: (u64, f64),
    velocity: f64,
}

struct Run {
    window: Window,
    panel: usize,
    from: f64,
    to: f64,
    start_ns: u64,
    duration_ns: u64,
}

impl Run {
    fn at(&self, frame_ns: u64) -> f64 {
        let k = (frame_ns.saturating_sub(self.start_ns) as f64 / self.duration_ns.max(1) as f64).clamp(0.0, 1.0);
        let e = 1.0 - (1.0 - k).powi(3);
        self.from + (self.to - self.from) * e
    }
}

/// What a finished run asks the state to do.
pub enum Done {
    /// Take the window out of the space: it is put away.
    PutAway(Window, usize),
}

/// A window crossing to the other panel (item's "an open app, called to the
/// other panel"): it stands at its new place at once and is drawn sliding
/// there from the old, 260 ms eased in and out.
struct Slide {
    window: Window,
    from_x: i32,
    to_x: i32,
    start_ns: u64,
}

const SLIDE_NS: u64 = 260_000_000;

#[derive(Default)]
pub struct Gestures {
    grab: Option<Grab>,
    runs: Vec<Run>,
    slides: Vec<Slide>,
}

impl Gestures {
    /// A touch down in a panel's bottom band, on a panel with `window` on
    /// top: the swipe begins.
    pub fn down(&mut self, slot: TouchSlot, pos: Point<f64, Logical>, time_us: u64, window: Window, panel: usize) {
        self.runs.retain(|r| r.window != window);
        self.grab = Some(Grab { slot, window, panel, start_y: pos.y, p: 0.0, last: (time_us, pos.y), velocity: 0.0 });
    }

    pub fn holds(&self, slot: TouchSlot) -> bool {
        self.grab.as_ref().is_some_and(|g| g.slot == slot)
    }

    pub fn motion(&mut self, slot: TouchSlot, y: f64, time_us: u64) {
        let Some(g) = self.grab.as_mut().filter(|g| g.slot == slot) else { return };
        let dt = time_us.saturating_sub(g.last.0) as f64 / 1000.0;
        if dt > 0.0 {
            g.velocity = g.velocity * 0.4 + (y - g.last.1) / dt * 0.6;
        }
        g.last = (time_us, y);
        g.p = ((g.start_y - y) / TRAVEL).clamp(0.0, 1.0);
    }

    /// The finger lets go: the window runs away or back.
    pub fn up(&mut self, slot: TouchSlot, now_ns: u64) {
        let Some(g) = self.grab.take_if(|g| g.slot == slot) else { return };
        let away = if g.velocity < -FLICK {
            true
        } else if g.velocity > FLICK {
            false
        } else {
            g.p > COMMIT
        };
        let to = if away { 1.0 } else { 0.0 };
        tracing::debug!("put away: released at {:.2}, {:.2} px/ms: {}", g.p, g.velocity, if away { "away" } else { "back" });
        self.run(g.window, g.panel, g.p, to, now_ns);
    }

    pub fn cancel(&mut self, now_ns: u64) {
        if let Some(g) = self.grab.take() {
            self.run(g.window, g.panel, g.p, 0.0, now_ns);
        }
    }

    fn run(&mut self, window: Window, panel: usize, from: f64, to: f64, now_ns: u64) {
        let duration_ns = (RUN_NS * (to - from).abs()).max(60e6) as u64;
        self.runs.push(Run { window, panel, from, to, start_ns: now_ns, duration_ns });
    }

    /// A window put away comes back, onto `panel`: the motion reversed.
    pub fn bring_back(&mut self, window: Window, panel: usize, now_ns: u64) {
        self.run(window, panel, 1.0, 0.0, now_ns);
    }

    /// How far away a window is at `frame_ns` (0 in place, 1 away), if it
    /// is moving.
    pub fn progress(&self, window: &Window, frame_ns: u64) -> Option<f64> {
        if let Some(g) = self.grab.as_ref().filter(|g| &g.window == window) {
            return Some(g.p);
        }
        self.runs.iter().find(|r| &r.window == window).map(|r| r.at(frame_ns))
    }

    /// Whether a window is on its way away: its panel counts as free, so the
    /// dock sets off as the window goes, not after.
    pub fn leaving(&self, window: &Window) -> bool {
        self.runs.iter().any(|r| &r.window == window && r.to > 0.5)
    }

    pub fn moving(&self) -> bool {
        self.grab.is_some() || !self.runs.is_empty() || !self.slides.is_empty()
    }

    /// A window moved from x `from_x` to `to_x` (logical px): drawn sliding.
    pub fn slide(&mut self, window: Window, from_x: i32, to_x: i32, now_ns: u64) {
        self.slides.retain(|s| s.window != window);
        self.slides.push(Slide { window, from_x, to_x, start_ns: now_ns });
    }

    /// How far a sliding window is drawn from where it stands, logical px.
    pub fn slide_offset(&self, window: &Window, frame_ns: u64) -> Option<i32> {
        let s = self.slides.iter().find(|s| &s.window == window)?;
        let k = (frame_ns.saturating_sub(s.start_ns) as f64 / SLIDE_NS as f64).clamp(0.0, 1.0);
        let e = if k < 0.5 { 4.0 * k * k * k } else { 1.0 - (-2.0 * k + 2.0).powi(3) / 2.0 };
        Some(((s.from_x - s.to_x) as f64 * (1.0 - e)).round() as i32)
    }

    /// After a frame for `frame_ns`: the runs that are over, and what they
    /// leave to do.
    pub fn settle(&mut self, frame_ns: u64) -> Vec<Done> {
        let mut done = Vec::new();
        self.slides.retain(|s| frame_ns < s.start_ns + SLIDE_NS);
        self.runs.retain(|r| {
            if frame_ns < r.start_ns + r.duration_ns {
                return true;
            }
            if r.to > 0.5 {
                done.push(Done::PutAway(r.window.clone(), r.panel));
            }
            false
        });
        done
    }
}

/// Where a window of `geo` (physical px) is drawn at progress `p`: the
/// scale, and the offset of its top left corner from where it stands.
pub fn placement(geo: Rectangle<i32, Physical>, p: f64) -> (f64, Point<i32, Physical>) {
    let s = 1.0 - (1.0 - SMALLEST) * p;
    let (w, h) = (geo.size.w as f64, geo.size.h as f64);
    let rise = RISE * p * layout::SCALE as f64;
    let dx = (w - w * s) / 2.0;
    let dy = h - h * s - rise;
    (s, (dx.round() as i32, dy.round() as i32).into())
}

/// The alpha of a window at progress `p` (phoc's 1 - p²).
pub fn alpha(p: f64) -> f32 {
    (1.0 - p * p) as f32
}
