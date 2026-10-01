//! The ribbon: everything on the phone is a page in one row, and the two
//! panels are a window two pages wide that moves along it.
//!
//! ```text
//! [system] [desk] [desk] [app] [app] ... [pen]
//!          '---- shown ----'
//! ```
//!
//! - The system screen is the first page and the pen's sheet the last; the
//!   two desks (each panel's desktop, the dock's halves and the clock on
//!   them) come after the system screen; apps come after the desks, and
//!   between them as they are opened.
//! - An app opened from a panel takes the page that panel shows, and the
//!   pages from there on move one to the right: it lands where the finger
//!   asked, and what was there is now beside it.
//! - A swipe sideways moves the row with the finger - on a desk, the system
//!   screen or the pen's sheet anywhere, on an app along the bottom edge -
//!   and let go it runs to the nearest page, or on by the finger's speed.
//! - A window across both panels (`span`) has two pages, its own and a
//!   Wide one after it: shown together it stands on both panels, and as the
//!   row moves it moves with them, half of it shown when one is.
//! - A window put away or closed leaves the row, and the row closes up; the
//!   system screen and the pen's sheet come only when asked for.
//!
//! The ribbon keeps the order and where the window onto it stands, and the
//! motion: a finger's drag, or a run from where each page was to where it
//! goes. The state applies the result when it stops (state.rs): the windows
//! shown are mapped onto their panels, the rest are out of the space.

use smithay::backend::input::TouchSlot;
use smithay::desktop::Window;

/// One page's step: a panel and the hinge, logical px.
pub const PAGE: f64 = 717.0;
/// A release faster than this (logical px per ms) goes on a page.
const FLICK: f64 = 0.35;
/// A run's longest and shortest time.
const RUN_MAX_NS: f64 = 520e6;
const RUN_MIN_NS: f64 = 260e6;
/// Past either end, the row follows the finger this much.
const RUBBER: f64 = 0.3;
/// The dots under the row, after a move.
const DOTS_NS: u64 = 900_000_000;

#[derive(Clone, PartialEq, Debug)]
pub enum Page {
    System,
    /// A panel's desktop: 0 the left one's, 1 the right one's.
    Desk(usize),
    App(Window),
    /// The second page of a window across both panels: it follows its
    /// App page, and the window is as wide as the two.
    Wide(Window),
    Pen,
}

struct Drag {
    slot: TouchSlot,
    start_x: f64,
    /// Where the window onto the row stood when the finger took it, pages.
    from: f64,
    last: (u64, f64),
    /// Smoothed, logical px per ms.
    velocity: f64,
}

/// Pages running from where they were to where they go.
struct Run {
    /// Each page's x as the run began (logical px, the left panel's left
    /// edge 0); a page new to the row has none and stands where it goes.
    from: Vec<(Page, f64)>,
    start_ns: u64,
    duration_ns: u64,
    /// The start's speed, in the run's own measure (its 0 to 1) per run.
    v0: f64,
    /// A finger's let go: the row only moves along, nothing in or out.
    scroll: bool,
}

impl Run {
    fn k(&self, frame_ns: u64) -> f64 {
        let s = (frame_ns.saturating_sub(self.start_ns) as f64 / self.duration_ns.max(1) as f64).clamp(0.0, 1.0);
        // Hermite from 0 to 1, leaving at v0, arriving at rest.
        let (s2, s3) = (s * s, s * s * s);
        (s3 - 2.0 * s2 + s) * self.v0 + (-2.0 * s3 + 3.0 * s2)
    }
}

pub struct Ribbon {
    pub pages: Vec<Page>,
    /// The page on the left panel, as things stand (or will, once a run
    /// ends).
    pub view: usize,
    drag: Option<Drag>,
    run: Option<Run>,
    /// The pages on each panel as the motion began, for what is drawn on
    /// a panel and carried with its page (the dock, the clock).
    carried: [Option<Page>; 2],
    /// Something moved and the state has not applied where it ended.
    pub unsettled: bool,
    moved_at: u64,
}

impl Ribbon {
    pub fn new() -> Ribbon {
        Ribbon { pages: vec![Page::System, Page::Desk(0), Page::Desk(1), Page::Pen], view: 1, drag: None, run: None, carried: [None, None], unsettled: false, moved_at: 0 }
    }

    fn max_view(&self) -> usize {
        self.pages.len().saturating_sub(2)
    }

    /// The page on a panel, as things stand.
    pub fn on(&self, panel: usize) -> Option<&Page> {
        self.pages.get(self.view + panel)
    }

    pub fn find(&self, window: &Window) -> Option<usize> {
        self.pages.iter().position(|p| matches!(p, Page::App(w) if w == window))
    }

    pub fn moving(&self) -> bool {
        self.drag.is_some() || self.run.is_some()
    }

    pub fn holds(&self, slot: TouchSlot) -> bool {
        self.drag.as_ref().is_some_and(|d| d.slot == slot)
    }

    /// Where the window onto the row stands, pages, at `frame_ns`.
    fn pos(&self) -> f64 {
        if let Some(d) = &self.drag {
            let raw = d.from - (d.last.1 - d.start_x) / PAGE;
            let max = self.max_view() as f64;
            return if raw < 0.0 { raw * RUBBER } else if raw > max { max + (raw - max) * RUBBER } else { raw };
        }
        self.view as f64
    }

    /// Each page's x at `frame_ns` (logical px, the left panel's left edge
    /// 0), while the row moves.
    pub fn xs(&self, frame_ns: u64) -> Vec<(Page, f64)> {
        let pos = self.pos();
        let to = |i: usize| (i as f64 - pos) * PAGE;
        match &self.run {
            Some(run) => {
                let k = run.k(frame_ns);
                self.pages
                    .iter()
                    .enumerate()
                    .map(|(i, p)| {
                        let end = to(i);
                        let start = run.from.iter().find(|(q, _)| q == p).map(|(_, x)| *x).unwrap_or(end);
                        (p.clone(), start + (end - start) * k)
                    })
                    .collect()
            }
            None => self.pages.iter().enumerate().map(|(i, p)| (p.clone(), to(i))).collect(),
        }
    }

    /// While the row moves: how far the page that stood on `panel` as it
    /// began is from there now, logical px (what is drawn on that panel and
    /// belongs to its page moves with it).
    pub fn carried(&self, panel: usize, frame_ns: u64) -> Option<f64> {
        if !self.moving() {
            return None;
        }
        let page = self.carried[panel].as_ref()?;
        let x = self.xs(frame_ns).into_iter().find(|(p, _)| p == page)?.1;
        Some(x - panel as f64 * PAGE)
    }

    /// Where the window onto the row stands at `frame_ns`, pages (the left
    /// panel's page), moving or not.
    pub fn position(&self, frame_ns: u64) -> f64 {
        if !self.moving() {
            return self.view as f64;
        }
        self.xs(frame_ns).first().map(|(_, x)| -x / PAGE).unwrap_or(self.view as f64)
    }

    /// The pages on the panels with the window onto the row at `view`.
    pub fn at(&self, view: usize) -> [Option<&Page>; 2] {
        [self.pages.get(view), self.pages.get(view + 1)]
    }

    /// While the row only moves along (a finger on it, or its let go): the
    /// page position just left of where it stands, and how far on from it
    /// (0 to 1).
    pub fn scrolling(&self, frame_ns: u64) -> Option<(usize, f64)> {
        let pos = if self.drag.is_some() {
            self.pos()
        } else if self.run.as_ref().is_some_and(|r| r.scroll) {
            -self.xs(frame_ns).first()?.1 / PAGE
        } else {
            return None;
        };
        let max = self.max_view() as f64;
        let pos = pos.clamp(0.0, max);
        let base = pos.floor().min((max - 1.0).max(0.0));
        Some((base as usize, pos - base))
    }

    /// While the row moves: the panel the desktop's clock stood on as it
    /// began, the right one's desk before the left's (dock.rs's home).
    pub fn clock_panel(&self) -> Option<usize> {
        if !self.moving() {
            return None;
        }
        [1, 0].into_iter().find(|&k| matches!(self.carried[k], Some(Page::Desk(_))))
    }

    fn begin(&mut self, frame_ns: u64) {
        if !self.moving() {
            self.carried = [self.on(0).cloned(), self.on(1).cloned()];
        }
        self.moved_at = frame_ns;
    }

    /// A finger takes the row, at x; a run under way stops under it.
    pub fn grab(&mut self, slot: TouchSlot, start_x: f64, time_us: u64) {
        let now = hybris_hwc::now_ns();
        self.begin(now);
        // Caught mid-run: where the pages are now, as a position.
        let from = match &self.run {
            Some(_) => {
                let xs = self.xs(now);
                let first = xs.iter().position(|(p, _)| self.pages.first() == Some(p)).unwrap_or(0);
                -xs[first].1 / PAGE
            }
            None => self.view as f64,
        };
        self.run = None;
        self.drag = Some(Drag { slot, start_x, from, last: (time_us, start_x), velocity: 0.0 });
        tracing::debug!("ribbon: taken at page {from:.2}");
    }

    pub fn motion(&mut self, slot: TouchSlot, x: f64, time_us: u64) {
        let Some(d) = self.drag.as_mut().filter(|d| d.slot == slot) else { return };
        let dt = time_us.saturating_sub(d.last.0) as f64 / 1000.0;
        if dt > 0.0 {
            d.velocity = d.velocity * 0.4 + (x - d.last.1) / dt * 0.6;
        }
        d.last = (time_us, x);
    }

    /// The finger lets go: the row runs to the nearest page, or on by the
    /// finger's speed.
    pub fn up(&mut self, slot: TouchSlot) {
        if !self.holds(slot) {
            return;
        }
        let now = hybris_hwc::now_ns();
        let pos = self.pos();
        let d = self.drag.take().unwrap();
        let max = self.max_view() as f64;
        let target = if d.velocity < -FLICK {
            pos.floor() + 1.0
        } else if d.velocity > FLICK {
            pos.ceil() - 1.0
        } else {
            pos.round()
        };
        // Not more than a page from where the finger took it.
        let target = target.clamp(d.from.round() - 1.0, d.from.round() + 1.0).clamp(0.0, max);
        let from: Vec<(Page, f64)> = self.pages.iter().enumerate().map(|(i, p)| (p.clone(), (i as f64 - pos) * PAGE)).collect();
        let dist = (target - pos).abs() * PAGE;
        // The finger's speed carries on, towards the target only.
        let speed = -d.velocity * (target - pos).signum();
        let ns = if speed > 0.0 && dist > 0.0 { (2.0 * dist / speed * 1e6).clamp(RUN_MIN_NS, RUN_MAX_NS) } else { RUN_MAX_NS };
        let v0 = if dist > 0.0 { (speed.max(0.0) * ns / 1e6 / dist).min(2.0) } else { 0.0 };
        self.view = target as usize;
        self.run = Some(Run { from, start_ns: now, duration_ns: ns as u64, v0, scroll: true });
        self.unsettled = true;
        tracing::info!("ribbon: to page {} of {} ({:?} | {:?})", self.view, self.pages.len(), self.on(0), self.on(1));
    }

    pub fn cancel(&mut self) {
        if let Some(slot) = self.drag.as_ref().map(|d| d.slot) {
            self.up(slot);
        }
    }

    /// The pages run from where they are drawn now to where they go: after
    /// the row changed (a page in, out, or moved).
    fn rearranged(&mut self, before: Vec<(Page, f64)>) {
        let now = hybris_hwc::now_ns();
        self.begin(now);
        self.drag = None;
        self.run = Some(Run { from: before, start_ns: now, duration_ns: 440_000_000, v0: 0.0, scroll: false });
        self.unsettled = true;
    }

    /// Keeps the window onto the row off the system screen and the pen's
    /// sheet when the row changed under it: those come when asked for.
    fn keep_view(&mut self) {
        self.view = self.view.min(self.max_view());
        if self.view == 0 && self.pages.len() > 3 && self.pages[0] == Page::System {
            self.view = 1;
        }
        if self.pages.get(self.view + 1) == Some(&Page::Pen) && self.view > 1 {
            self.view -= 1;
        }
    }

    /// A window onto a panel: its page takes that panel's place, and the
    /// pages from there on move one to the right. Not before the system
    /// screen, nor after the pen's sheet.
    pub fn open(&mut self, window: Window, panel: usize) {
        let before = self.xs(hybris_hwc::now_ns());
        if let Some(i) = self.find(&window) {
            self.pages.remove(i);
            if i < self.view {
                self.view -= 1;
            }
        }
        let i = (self.view + panel).clamp(1, self.pages.len() - 1);
        self.pages.insert(i, Page::App(window));
        // It shows where it was asked for, or as near as it can.
        self.view = i.saturating_sub(panel).min(self.max_view());
        tracing::info!("ribbon: a window on page {i}, the left panel on {}", self.view);
        self.rearranged(before);
    }

    /// Whether a window is across both panels.
    pub fn spanned(&self, window: &Window) -> bool {
        self.pages.iter().any(|p| matches!(p, Page::Wide(w) if w == window))
    }

    /// A window across both panels: a Wide page after its own, and the
    /// two shown.
    pub fn span(&mut self, window: &Window) {
        let Some(i) = self.find(window) else { return };
        if self.spanned(window) {
            return;
        }
        let before = self.xs(hybris_hwc::now_ns());
        self.pages.insert(i + 1, Page::Wide(window.clone()));
        self.view = i.min(self.max_view());
        tracing::info!("ribbon: a window across both panels, pages {i} and {}", i + 1);
        self.rearranged(before);
    }

    /// A window across both panels back onto one.
    pub fn unspan(&mut self, window: &Window, panel: usize) {
        self.drop_wide(window);
        self.open(window.clone(), panel);
    }

    /// A window's Wide page out of the row, if it has one.
    fn drop_wide(&mut self, window: &Window) {
        if let Some(i) = self.pages.iter().position(|p| matches!(p, Page::Wide(w) if w == window)) {
            self.pages.remove(i);
            if i <= self.view && self.view > 0 {
                self.view -= 1;
            }
        }
    }

    /// A window leaves the row (put away, or closed): the row closes up,
    /// the page beside it on the other panel staying where it is.
    pub fn remove(&mut self, window: &Window) {
        self.drop_wide(window);
        let Some(i) = self.find(window) else { return };
        let before = self.xs(hybris_hwc::now_ns());
        self.pages.remove(i);
        // The left panel's page went: the one on the right stays there, the
        // page before comes onto the left.
        if i == self.view && self.view > 0 {
            self.view -= 1;
        } else if i < self.view {
            self.view -= 1;
        }
        self.keep_view();
        self.rearranged(before);
    }

    /// After a frame: whether the row still moves. Once it stops, the state
    /// applies where it stands (`unsettled`).
    pub fn settle(&mut self, frame_ns: u64) -> bool {
        if let Some(run) = &self.run {
            if frame_ns >= run.start_ns + run.duration_ns {
                self.run = None;
                self.moved_at = frame_ns;
            }
        }
        self.moving() || frame_ns < self.moved_at + DOTS_NS
    }

    /// The dots: how many pages, which is on the left panel (fractional
    /// while it moves), and how much they show (fading after a move).
    pub fn dots(&self, frame_ns: u64) -> Option<(usize, f64, f32)> {
        let alpha = if self.moving() {
            1.0
        } else {
            let since = frame_ns.saturating_sub(self.moved_at);
            if since >= DOTS_NS {
                return None;
            }
            1.0 - (since as f32 / DOTS_NS as f32).powi(3)
        };
        if self.moved_at == 0 {
            return None;
        }
        let xs = self.xs(frame_ns);
        let first = xs.first().map(|(_, x)| *x).unwrap_or(0.0);
        Some((self.pages.len(), -first / PAGE, alpha))
    }
}
