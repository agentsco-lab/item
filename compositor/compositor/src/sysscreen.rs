//! The system screen, item's (#109): the page left of the left panel, as on
//! the Surface Duo 2, on the ribbon's first page (ribbon.rs). It is a pane
//! of frosted glass: the wallpaper under it blurred hard and darkened
//! (walls.rs makes it, `set_frost`), the page's rows (sysfacts.rs) on it.
//! They are read and drawn on a thread: on the way in, every 5 s while it is
//! shown, and kept drawn while the ribbon may bring it, so a swipe never
//! waits for it. A tap on Details opens or shuts the rest.
//!
//! The page is taller than the panel: a finger moving up or down on it
//! scrolls it, and let go it runs on; a finger moving sideways moves the
//! ribbon.

use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};

use smithay::backend::allocator::Fourcc;
use smithay::backend::input::TouchSlot;
use smithay::backend::renderer::element::memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement};
use smithay::backend::renderer::element::solid::SolidColorRenderElement;
use smithay::backend::renderer::element::{Id, Kind};
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::renderer::utils::CommitCounter;
use smithay::reexports::calloop::ping::Ping;
use smithay::utils::{Logical, Physical, Point, Rectangle, Transform};

use crate::layout::{self, SCALE};
use crate::shade::{local_time, ShellElement};
use crate::sysfacts::{Job, Page};

const READ_EVERY_NS: u64 = 5_000_000_000;
/// The glass without a picture to frost (Aurora's is drawn by a shader).
const DARK: [f32; 3] = [0.07, 0.08, 0.11];
/// A touch that moves this far is a scroll or a swipe.
const MOVE: f64 = 12.0;
/// The run after a let go: how fast it slows (per ms).
const FRICTION: f64 = 0.003;

/// A finger on the page.
struct Touch {
    slot: TouchSlot,
    start: Point<f64, Logical>,
    from: f64,
    last: (u64, f64),
    velocity: f64,
    scrolling: bool,
}

/// What a finger's move on the page asks for.
pub enum TouchAsk {
    Nothing,
    /// Sideways: the ribbon, from this x.
    Ribbon(f64),
}

pub struct SystemScreen {
    /// The weather now, as its thread last read it (the desktop's clock).
    weather: crate::sysfacts::Now,
    p: f64,
    /// The ribbon may bring it: kept read and drawn.
    wanted: bool,
    page: Option<MemoryRenderBuffer>,
    /// The page's height, logical px.
    page_h: f64,
    fresh: Arc<Mutex<Option<Page>>>,
    jobs: Sender<Job>,
    last_read_ns: u64,
    minute: i32,
    scroll: f64,
    touch: Option<Touch>,
    run: Option<(f64, u64, f64)>,
    id: Id,
    /// The wallpaper frosted (walls.rs), its size in px.
    frost: Option<(MemoryRenderBuffer, (i32, i32))>,
    /// Details open, and where its row is on the page (logical y).
    details: bool,
    details_row: (f64, f64),
}

impl SystemScreen {
    /// The weather now: temperature, what it is called, its icon.
    pub fn weather(&self) -> Option<(f64, String, String)> {
        self.weather.lock().unwrap().clone()
    }

    pub fn new(wake: Ping) -> SystemScreen {
        let fresh = Arc::new(Mutex::new(None));
        let (tx, rx) = channel();
        let page = fresh.clone();
        let weather: crate::sysfacts::Now = Default::default();
        let w = weather.clone();
        std::thread::Builder::new().name("system screen".into()).spawn(move || crate::sysfacts::worker(rx, page, w, wake)).expect("system screen thread");
        SystemScreen { weather, p: 0.0, wanted: false, page: None, page_h: 0.0, fresh, jobs: tx, last_read_ns: 0, minute: -1, scroll: 0.0, touch: None, run: None, id: Id::new(), frost: None, details: false, details_row: (0.0, 0.0) }
    }

    fn read(&mut self) {
        self.last_read_ns = hybris_hwc::now_ns();
        self.minute = { let t = local_time(); t.tm_hour * 60 + t.tm_min };
        let _ = self.jobs.send(Job::Read);
    }

    /// The screen lit or dark: the load is sampled only while it is lit.
    pub fn display(&self, lit: bool) {
        let _ = self.jobs.send(Job::Display(lit));
    }

    /// Out or not at once, as the ribbon has it (ribbon.rs): its page is
    /// on the left panel, or not.
    pub fn show(&mut self, out: bool) {
        let was = self.p > 0.0;
        self.p = if out { 1.0 } else { 0.0 };
        if out && !was {
            self.read();
        }
        if !out {
            self.touch = None;
            self.run = None;
        }
        self.wanted = out;
    }

    /// The page as drawn, to be sent to the GPU ahead of its first frame.
    pub fn page_buffer(&self) -> Option<&MemoryRenderBuffer> {
        self.page.as_ref()
    }

    /// Read now, for the ribbon to show the page as it comes.
    pub fn ready(&mut self) {
        self.wanted = true;
        if hybris_hwc::now_ns().saturating_sub(self.last_read_ns) > READ_EVERY_NS || self.page.is_none() {
            self.read();
        }
    }

    /// Shown: the left panel is taken.
    pub fn out(&self) -> bool {
        self.p > 0.0
    }

    fn max_scroll(&self) -> f64 {
        (self.page_h - layout::panels()[0].size.h as f64).max(0.0)
    }

    fn scroll_at(&self, now: u64) -> f64 {
        let max = self.max_scroll();
        if let Some(t) = self.touch.as_ref().filter(|t| t.scrolling) {
            return (t.from - (t.last.1 - t.start.y)).clamp(0.0, max);
        }
        match self.run {
            Some((v, at, from)) => {
                let t = now.saturating_sub(at) as f64 / 1e6;
                (from - v / FRICTION * (1.0 - (-FRICTION * t).exp())).clamp(0.0, max)
            }
            None => self.scroll.clamp(0.0, max),
        }
    }

    /// A finger down on the page while it is shown.
    pub fn touch_down(&mut self, slot: TouchSlot, pos: Point<f64, Logical>, time_us: u64) -> bool {
        if !self.out() || layout::panel_at(pos) != Some(0) {
            return false;
        }
        let now = hybris_hwc::now_ns();
        self.scroll = self.scroll_at(now);
        self.run = None;
        self.touch = Some(Touch { slot, start: pos, from: self.scroll, last: (time_us, pos.y), velocity: 0.0, scrolling: false });
        true
    }

    pub fn holds(&self, slot: TouchSlot) -> bool {
        self.touch.as_ref().is_some_and(|t| t.slot == slot)
    }

    /// The finger moves: up or down scrolls the page; sideways is the
    /// ribbon's.
    pub fn touch_motion(&mut self, slot: TouchSlot, pos: Point<f64, Logical>, time_us: u64) -> TouchAsk {
        let Some(t) = self.touch.as_mut().filter(|t| t.slot == slot) else { return TouchAsk::Nothing };
        let (dx, dy) = (pos.x - t.start.x, pos.y - t.start.y);
        if !t.scrolling {
            if dx.abs() > MOVE && dx.abs() > dy.abs() {
                let x = t.start.x;
                self.touch = None;
                return TouchAsk::Ribbon(x);
            }
            if dy.abs() > MOVE {
                t.scrolling = true;
            }
        }
        let dt = time_us.saturating_sub(t.last.0) as f64 / 1000.0;
        if dt > 0.0 {
            t.velocity = t.velocity * 0.4 + (pos.y - t.last.1) / dt * 0.6;
        }
        t.last = (time_us, pos.y);
        TouchAsk::Nothing
    }

    /// The wallpaper frosted (walls.rs): the glass; None, the plain dark.
    pub fn set_frost(&mut self, frost: Option<(Vec<u8>, u32, u32)>) {
        self.frost = frost.map(|(rgba, w, h)| (MemoryRenderBuffer::from_slice(&rgba, Fourcc::Abgr8888, (w as i32, h as i32), 1, Transform::Normal, None), (w as i32, h as i32)));
    }

    /// Let go: a scroll runs on; a tap on Details opens or shuts it.
    pub fn touch_up(&mut self, slot: TouchSlot) {
        let Some(t) = self.touch.take_if(|t| t.slot == slot) else { return };
        if !t.scrolling {
            let y = self.scroll_at(hybris_hwc::now_ns()) + t.start.y - layout::panels()[0].loc.y as f64;
            if y >= self.details_row.0 && y < self.details_row.1 {
                self.details = !self.details;
                let _ = self.jobs.send(Job::Details(self.details));
            }
        }
        if t.scrolling {
            self.scroll = (t.from - (t.last.1 - t.start.y)).clamp(0.0, self.max_scroll());
            if t.velocity.abs() > 0.05 {
                self.run = Some((t.velocity, hybris_hwc::now_ns(), self.scroll));
            }
        }
    }

    pub fn cancel(&mut self) {
        if let Some(slot) = self.touch.as_ref().map(|t| t.slot) {
            self.touch_up(slot);
        }
    }

    /// After a frame: whether it still moves. While it is shown, read again
    /// every 5 s and at each minute.
    pub fn settle(&mut self, frame_ns: u64) -> bool {
        if let Some((v, at, _)) = self.run {
            let t = frame_ns.saturating_sub(at) as f64 / 1e6;
            if (v * (-FRICTION * t).exp()).abs() < 0.01 {
                self.scroll = self.scroll_at(frame_ns);
                self.run = None;
            }
        }
        let minute = { let t = local_time(); t.tm_hour * 60 + t.tm_min };
        if self.p > 0.0 && (frame_ns.saturating_sub(self.last_read_ns) > READ_EVERY_NS || minute != self.minute) {
            self.read();
        }
        self.run.is_some() || self.touch.as_ref().is_some_and(|t| t.scrolling)
    }

    /// The page drawn on the thread, taken; whether a new one came.
    pub fn refresh(&mut self) -> bool {
        if !self.out() && !self.wanted {
            return false;
        }
        let Some((rgba, w, h, row)) = self.fresh.lock().unwrap().take() else { return false };
        self.details_row = (row.0 as f64, row.1 as f64);
        self.page = Some(MemoryRenderBuffer::from_slice(&rgba, Fourcc::Abgr8888, (w, h), SCALE, Transform::Normal, None));
        self.page_h = (h / SCALE) as f64;
        true
    }

    pub fn elements(&self, renderer: &mut GlesRenderer, frame_ns: u64) -> Vec<ShellElement> {
        self.drawn_at(renderer, self.p, frame_ns)
    }

    /// The page all the way out, wherever it is: for the ribbon to carry.
    pub fn picture(&self, renderer: &mut GlesRenderer, frame_ns: u64) -> Vec<ShellElement> {
        self.drawn_at(renderer, 1.0, frame_ns)
    }

    fn drawn_at(&self, renderer: &mut GlesRenderer, p: f64, frame_ns: u64) -> Vec<ShellElement> {
        if p <= 0.0 {
            return Vec::new();
        }
        let panel = layout::panels()[0];
        let mut out = Vec::new();
        // The page, scrolled: the panel's height of it.
        if let Some(page) = &self.page {
            let scroll = self.scroll_at(frame_ns);
            let h = (panel.size.h as f64).min(self.page_h);
            let src = Rectangle::<f64, Logical>::new((0.0, scroll).into(), (panel.size.w as f64, h).into());
            if let Ok(e) = MemoryRenderBufferRenderElement::from_buffer(renderer, (0.0, 0.0), page, None, Some(src), Some((panel.size.w, h as i32).into()), Kind::Unspecified) {
                out.push(ShellElement::Text(e));
            }
        }
        // The glass: the wallpaper where the page stands, frosted (its
        // picture is the wallpaper's at a FROST_STEP-th of its px, the page
        // at the wallpaper's left edge).
        if let Some((frost, (fw, fh))) = &self.frost {
            let k = crate::walls::FROST_STEP as f64 / SCALE as f64;
            let src = Rectangle::<f64, Logical>::new((0.0, 0.0).into(), ((panel.size.w as f64 / k).min(*fw as f64), (panel.size.h as f64 / k).min(*fh as f64)).into());
            if let Ok(e) = MemoryRenderBufferRenderElement::from_buffer(renderer, (0.0, 0.0), frost, None, Some(src), Some(panel.size), Kind::Unspecified) {
                out.push(ShellElement::Text(e));
                return out;
            }
        }
        let rect = Rectangle::<i32, Physical>::new(panel.loc.to_physical(SCALE), panel.size.to_physical(SCALE));
        out.push(ShellElement::Solid(SolidColorRenderElement::new(self.id.clone(), rect, CommitCounter::default(), [DARK[0], DARK[1], DARK[2], 1.0], Kind::Unspecified)));
        out
    }
}

fn run(cmd: &str, args: &[&str]) -> String {
    std::process::Command::new(cmd).args(args).output().map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default()
}

/// "Good morning" and so on, by the hour.
pub fn part_of_day(hour: i32) -> &'static str {
    match hour {
        5..=11 => "Good morning",
        12..=17 => "Good afternoon",
        18..=22 => "Good evening",
        _ => "Good night",
    }
}

/// The user's first name, unless AccountsService only knows the login.
pub fn first_name() -> Option<String> {
    let uid = unsafe { libc::getuid() };
    let login = std::env::var("USER").unwrap_or_default();
    let real = run("busctl", &["--system", "get-property", "org.freedesktop.Accounts", &format!("/org/freedesktop/Accounts/User{uid}"), "org.freedesktop.Accounts.User", "RealName"]);
    real.trim()
        .strip_prefix("s ")
        .map(|v| v.trim_matches('"').split_whitespace().next().unwrap_or("").to_owned())
        .filter(|n| !n.is_empty() && !n.eq_ignore_ascii_case(&login))
}
