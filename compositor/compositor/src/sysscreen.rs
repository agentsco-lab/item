//! The system screen, item's (#109): one more page left of the left panel,
//! as on the Surface Duo 2. A swipe right on the left panel's desktop brings
//! it with the finger, a swipe left on it takes it away. It is light
//! (#F2F2F2), to break the black; coming in, its background goes from the
//! desktop's black to light with the finger, and the content appears over
//! the last third. While it is out, the left panel counts as taken: the
//! dock leaves it.
//!
//! The page: the time and the date, a greeting by the time of day (with the
//! user's first name from AccountsService, when it is a name and not the
//! login), and the battery card (#115):
//! - the level and what it is doing, with UPower's estimate;
//! - the power in or out, and the temperature;
//! - the charge over the last 24 hours from UPower's history, charging in
//!   green and draining in dark grey, broken where the history has a gap.
//!
//! Nothing is read while it is closed. It is read on the way in and every
//! 5 s while it is open, on a thread; the page is drawn into one texture
//! when what it shows changes.

use std::sync::{Arc, Mutex};

use resvg::tiny_skia;
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
use crate::shade::{date_line, local_time, ShellElement};
use crate::text::Font;

const FLICK: f64 = 0.3;
const COMMIT: f64 = 0.3;
const SLIDE_NS: f64 = 220e6;
const READ_EVERY_NS: u64 = 5_000_000_000;
const LIGHT: [f32; 3] = [0.949, 0.949, 0.949];
const DESK: [f32; 3] = [0.08, 0.1, 0.14];

/// What the page shows.
#[derive(Clone, Default, PartialEq)]
struct Facts {
    level: u32,
    status: String,
    power_w: f64,
    temp_c: f64,
    to_empty_s: i64,
    to_full_s: i64,
    name: Option<String>,
    /// UPower's history: (unix time, percent, state), oldest first.
    history: Vec<(u64, f64, u32)>,
}

struct Grab {
    slot: TouchSlot,
    start_x: f64,
    from: f64,
    last: (u64, f64),
    velocity: f64,
}

struct Run {
    from: f64,
    to: f64,
    start_ns: u64,
    duration_ns: u64,
}

pub struct SystemScreen {
    p: f64,
    grab: Option<Grab>,
    run: Option<Run>,
    facts: Arc<Mutex<Option<Facts>>>,
    shown: Option<Facts>,
    minute: i32,
    page: Option<MemoryRenderBuffer>,
    last_read_ns: u64,
    wake: Ping,
    /// The ribbon may bring the page: it is kept drawn.
    wanted: bool,
    id: Id,
    fonts: Option<(Font, Font)>,
}

impl SystemScreen {
    pub fn new(wake: Ping) -> SystemScreen {
        let thin = Font::load(&["/usr/share/fonts/truetype/lato/Lato-Light.ttf", "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf"]);
        let regular = Font::load(&["/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf", "/usr/share/fonts/truetype/lato/Lato-Regular.ttf"]);
        SystemScreen {
            p: 0.0,
            grab: None,
            run: None,
            facts: Default::default(),
            shown: None,
            minute: -1,
            page: None,
            last_read_ns: 0,
            wake,
            wanted: false,
            id: Id::new(),
            fonts: thin.zip(regular),
        }
    }

    fn at(&self, frame_ns: u64) -> f64 {
        if let Some(g) = &self.grab {
            let w = layout::panels()[0].size.w as f64;
            return (g.from + (g.last.1 - g.start_x) / w).clamp(0.0, 1.0);
        }
        match &self.run {
            Some(r) => {
                let k = (frame_ns.saturating_sub(r.start_ns) as f64 / r.duration_ns.max(1) as f64).clamp(0.0, 1.0);
                r.from + (r.to - r.from) * (1.0 - (1.0 - k).powi(3))
            }
            None => self.p,
        }
    }

    /// Out or not at once, as the ribbon has it (ribbon.rs): its page is
    /// on the left panel, or not.
    pub fn show(&mut self, out: bool) {
        let was = self.p > 0.0;
        self.grab = None;
        self.run = None;
        self.p = if out { 1.0 } else { 0.0 };
        if out && !was {
            self.read();
        }
        // Not shown, the page is kept for the ribbon to carry in again.
        self.wanted = out;
    }

    /// The page as drawn, to be sent to the GPU ahead of its first frame.
    pub fn page_buffer(&self) -> Option<&MemoryRenderBuffer> {
        self.page.as_ref()
    }

    /// Read now, for the ribbon to show the page as it comes; the page is
    /// kept up to date while it may.
    pub fn ready(&mut self) {
        self.wanted = true;
        if hybris_hwc::now_ns().saturating_sub(self.last_read_ns) > READ_EVERY_NS || self.page.is_none() {
            self.read();
        }
    }

    /// Out, or on its way: the left panel is taken.
    pub fn out(&self) -> bool {
        self.p > 0.0 || self.grab.is_some() || self.run.is_some()
    }

    fn read(&mut self) {
        self.last_read_ns = hybris_hwc::now_ns();
        let (slot, wake) = (self.facts.clone(), self.wake.clone());
        std::thread::spawn(move || {
            *slot.lock().unwrap() = Some(read_facts());
            wake.ping();
        });
    }

    /// A swipe right that began on the left panel's desktop.
    pub fn grab_from(&mut self, slot: TouchSlot, start: Point<f64, Logical>, time_us: u64) {
        let from = self.at(hybris_hwc::now_ns());
        self.run = None;
        self.grab = Some(Grab { slot, start_x: start.x, from, last: (time_us, start.x), velocity: 0.0 });
        if from == 0.0 {
            self.read();
        }
    }

    /// A touch on the page while it is out: its swipe left takes it away.
    pub fn down(&mut self, slot: TouchSlot, pos: Point<f64, Logical>, time_us: u64) -> bool {
        if !self.out() || layout::panel_at(pos) != Some(0) {
            return false;
        }
        self.grab_from(slot, pos, time_us);
        true
    }

    pub fn holds(&self, slot: TouchSlot) -> bool {
        self.grab.as_ref().is_some_and(|g| g.slot == slot)
    }

    pub fn motion(&mut self, slot: TouchSlot, pos: Point<f64, Logical>, time_us: u64) {
        if let Some(g) = self.grab.as_mut().filter(|g| g.slot == slot) {
            let dt = time_us.saturating_sub(g.last.0) as f64 / 1000.0;
            if dt > 0.0 {
                g.velocity = g.velocity * 0.4 + (pos.x - g.last.1) / dt * 0.6;
            }
            g.last = (time_us, pos.x);
        }
    }

    pub fn up(&mut self, slot: TouchSlot) {
        if !self.holds(slot) {
            return;
        }
        let now = hybris_hwc::now_ns();
        let v = self.grab.as_ref().unwrap().velocity;
        let p = self.at(now);
        let open = if v > FLICK { true } else if v < -FLICK { false } else { p >= COMMIT };
        self.grab = None;
        let to = if open { 1.0 } else { 0.0 };
        self.run = Some(Run { from: p, to, start_ns: now, duration_ns: (SLIDE_NS * (to - p).abs()).max(40e6) as u64 });
        self.p = to;
    }

    pub fn cancel(&mut self) {
        if let Some(g) = self.grab.take() {
            self.grab = Some(g);
            let slot = self.grab.as_ref().unwrap().slot;
            self.up(slot);
        }
    }

    /// After a frame: whether it still moves. While it is out, the facts are
    /// read again every 5 s, and the page redrawn when they or the minute
    /// change.
    pub fn settle(&mut self, frame_ns: u64) -> bool {
        if let Some(r) = &self.run {
            if frame_ns >= r.start_ns + r.duration_ns {
                self.run = None;
                if self.p == 0.0 {
                    self.page = None;
                    self.shown = None;
                }
                // One frame more: the dock learns the panel is free only on
                // a frame.
                return true;
            }
        }
        if self.p > 0.0 && self.run.is_none() && frame_ns.saturating_sub(self.last_read_ns) > READ_EVERY_NS {
            self.read();
        }
        self.grab.is_some() || self.run.is_some()
    }

    /// The page drawn again if what it shows changed; whether it did.
    pub fn refresh(&mut self) -> bool {
        if !self.out() && !self.wanted {
            return false;
        }
        let facts = self.facts.lock().unwrap().clone();
        let now = local_time();
        let minute = now.tm_hour * 60 + now.tm_min;
        if facts.is_none() || (facts == self.shown && minute == self.minute && self.page.is_some()) {
            return false;
        }
        self.minute = minute;
        self.shown = facts.clone();
        self.page = facts.and_then(|f| self.draw(&f, &now));
        true
    }

    pub fn elements(&self, renderer: &mut GlesRenderer, frame_ns: u64) -> Vec<ShellElement> {
        self.drawn_at(renderer, self.at(frame_ns))
    }

    /// The page all the way out, wherever it is: for the ribbon to carry.
    pub fn picture(&self, renderer: &mut GlesRenderer) -> Vec<ShellElement> {
        self.drawn_at(renderer, 1.0)
    }

    fn drawn_at(&self, renderer: &mut GlesRenderer, p: f64) -> Vec<ShellElement> {
        if p <= 0.0 {
            return Vec::new();
        }
        let panel = layout::panels()[0];
        let mut out = Vec::new();
        // The content over the last third, sliding in with the page.
        let content = ((p - 0.66) / 0.34).clamp(0.0, 1.0) as f32;
        if let (Some(page), true) = (&self.page, content > 0.0) {
            let x = -(panel.size.w as f64) * (1.0 - p);
            if let Ok(e) = MemoryRenderBufferRenderElement::from_buffer(renderer, ((x * SCALE as f64).round(), 0.0), page, Some(content), None, None, Kind::Unspecified) {
                out.push(ShellElement::Text(e));
            }
        }
        // The background: from the desktop's black to light with the finger.
        let c = |i: usize| DESK[i] + (LIGHT[i] - DESK[i]) * p as f32;
        let rect = Rectangle::<i32, Physical>::new(panel.loc.to_physical(SCALE), panel.size.to_physical(SCALE));
        let commit = CommitCounter::from((p * 1000.0) as usize);
        out.push(ShellElement::Solid(SolidColorRenderElement::new(self.id.clone(), rect, commit, [c(0), c(1), c(2), 1.0], Kind::Unspecified)));
        out
    }

    /// The page, one panel's size, on transparent: the time, the date, the
    /// greeting and the battery card.
    fn draw(&self, f: &Facts, now: &libc::tm) -> Option<MemoryRenderBuffer> {
        let (thin, regular) = self.fonts.as_ref()?;
        let s = SCALE as f32;
        let panel = layout::panels()[0];
        let (pw, ph) = ((panel.size.w * SCALE) as u32, (panel.size.h * SCALE) as u32);
        let mut c = tiny_skia::Pixmap::new(pw, ph)?;
        let ink = [0.12, 0.12, 0.13, 1.0];
        let dim = [0.42, 0.42, 0.45, 1.0];
        let text = |c: &mut tiny_skia::Pixmap, font: &Font, t: &str, size: f32, color: [f32; 4], x: f32, y: f32, centred: bool| -> f32 {
            let (rgba, w, h) = font.rasterize(t, size, color);
            if let Some(px) = tiny_skia::IntSize::from_wh(w as u32, h as u32).and_then(|sz| tiny_skia::Pixmap::from_vec(rgba, sz)) {
                let x = if centred { (pw as f32 - w as f32) / 2.0 } else { x * s };
                c.draw_pixmap(x as i32, (y * s) as i32, px.as_ref(), &tiny_skia::PixmapPaint::default(), tiny_skia::Transform::identity(), None);
            }
            h as f32 / s
        };
        let h = text(&mut c, thin, &format!("{}:{:02}", now.tm_hour, now.tm_min), 72.0, ink, 0.0, 70.0, true);
        text(&mut c, regular, &date_line(now), 18.0, dim, 0.0, 70.0 + h + 2.0, true);
        let part = part_of_day(now.tm_hour);
        let greeting = match &f.name {
            Some(n) => format!("{part}, {n}"),
            None => part.to_owned(),
        };
        text(&mut c, regular, &greeting, 24.0, ink, 0.0, 230.0, true);

        // The battery card.
        let (cx, cy, cw, chh) = (30.0f32, 300.0f32, panel.size.w as f32 - 60.0, 290.0f32);
        let mut white = tiny_skia::Paint::default();
        white.set_color_rgba8(255, 255, 255, 255);
        white.anti_alias = true;
        if let Some(r) = rounded_path(cx * s, cy * s, cw * s, chh * s, 20.0 * s) {
            c.fill_path(&r, &white, tiny_skia::FillRule::Winding, tiny_skia::Transform::identity(), None);
        }
        text(&mut c, regular, "Battery", 15.0, dim, cx + 20.0, cy + 16.0, false);
        let doing = match f.status.as_str() {
            "Charging" => match hours(f.to_full_s) {
                Some(t) => format!("{}% · full in {t}", f.level),
                None => format!("{}% · charging", f.level),
            },
            // Full, or on the charger and holding (the port's "Not
            // charging" at 100 %).
            "Full" | "Not charging" => format!("{}% · charged", f.level),
            _ => match hours(f.to_empty_s) {
                Some(t) => format!("{}% · {t} left", f.level),
                None => format!("{} %", f.level),
            },
        };
        text(&mut c, regular, &doing, 22.0, ink, cx + 20.0, cy + 40.0, false);
        text(&mut c, regular, &format!("{:.1} W · {:.0} °C", f.power_w, f.temp_c), 14.0, dim, cx + 20.0, cy + 76.0, false);

        // The last 24 hours of charge.
        let (gx, gy, gw, gh) = (cx + 20.0, cy + 112.0, cw - 40.0, 150.0);
        let mut grid = tiny_skia::Paint::default();
        grid.set_color_rgba8(220, 220, 224, 255);
        let stroke = tiny_skia::Stroke { width: 1.0 * s, ..Default::default() };
        for frac in [0.0f32, 0.5, 1.0] {
            let y = (gy + gh * (1.0 - frac)) * s;
            let mut pb = tiny_skia::PathBuilder::new();
            pb.move_to(gx * s, y);
            pb.line_to((gx + gw) * s, y);
            if let Some(path) = pb.finish() {
                c.stroke_path(&path, &grid, &stroke, tiny_skia::Transform::identity(), None);
            }
        }
        let end = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let start = end.saturating_sub(86_400);
        let pts: Vec<&(u64, f64, u32)> = f.history.iter().filter(|h| h.0 >= start).collect();
        let line = tiny_skia::Stroke { width: 3.0 * s, line_cap: tiny_skia::LineCap::Round, line_join: tiny_skia::LineJoin::Round, ..Default::default() };
        for pair in pts.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            // A gap in the history (state 0, a restart) breaks the line.
            if a.2 == 0 || b.2 == 0 {
                continue;
            }
            let x = |t: u64| (gx + gw * (t - start) as f32 / 86_400.0) * s;
            let y = |v: f64| (gy + gh * (1.0 - (v as f32 / 100.0).clamp(0.0, 1.0))) * s;
            let mut paint = tiny_skia::Paint::default();
            paint.anti_alias = true;
            if b.2 == 1 || b.2 == 4 {
                paint.set_color_rgba8(0x2e, 0xc2, 0x7e, 255);
            } else {
                paint.set_color_rgba8(0x5e, 0x5c, 0x64, 255);
            }
            let mut pb = tiny_skia::PathBuilder::new();
            pb.move_to(x(a.0), y(a.1));
            pb.line_to(x(b.0), y(b.1));
            if let Some(path) = pb.finish() {
                c.stroke_path(&path, &paint, &line, tiny_skia::Transform::identity(), None);
            }
        }
        text(&mut c, regular, "24 h ago", 11.0, dim, gx, gy + gh + 4.0, false);
        text(&mut c, regular, "now", 11.0, dim, gx + gw - 36.0, gy + gh + 4.0, false);
        Some(MemoryRenderBuffer::from_slice(c.data(), Fourcc::Abgr8888, (pw as i32, ph as i32), SCALE, Transform::Normal, None))
    }
}

/// An estimate as item rounds it: to 10 minutes, and from 5 hours on to
/// whole hours. None if UPower has none.
fn hours(s: i64) -> Option<String> {
    // None, or one too long to mean anything (a phone at rest).
    if s <= 0 || s > 48 * 3600 {
        return None;
    }
    let min = ((s as f64 / 60.0) / 10.0).round() as i64 * 10;
    Some(if min >= 300 {
        format!("~{} h", (min as f64 / 60.0).round() as i64)
    } else if min >= 60 {
        format!("~{} h {} min", min / 60, min % 60)
    } else {
        format!("~{} min", min.max(10))
    })
}

fn rounded_path(x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<tiny_skia::Path> {
    let k = r * 0.5523;
    let mut pb = tiny_skia::PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    pb.cubic_to(x + w - r + k, y, x + w, y + r - k, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.cubic_to(x + w, y + h - r + k, x + w - r + k, y + h, x + w - r, y + h);
    pb.line_to(x + r, y + h);
    pb.cubic_to(x + r - k, y + h, x, y + h - r + k, x, y + h - r);
    pb.line_to(x, y + r);
    pb.cubic_to(x, y + r - k, x + r - k, y, x + r, y);
    pb.close();
    pb.finish()
}

fn run(cmd: &str, args: &[&str]) -> String {
    std::process::Command::new(cmd).args(args).output().map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default()
}

fn read_facts() -> Facts {
    let file = |f: &str| std::fs::read_to_string(format!("/sys/class/power_supply/battery/{f}")).map(|s| s.trim().to_owned()).unwrap_or_default();
    let num = |f: &str| file(f).parse::<f64>().unwrap_or(0.0);
    let prop = |name: &str| {
        let out = run("busctl", &["--system", "get-property", "org.freedesktop.UPower", "/org/freedesktop/UPower/devices/DisplayDevice", "org.freedesktop.UPower.Device", name]);
        out.split_whitespace().nth(1).and_then(|v| v.parse::<i64>().ok()).unwrap_or(0)
    };
    // History: "a(udu) N t v s t v s ...".
    let hist = run("busctl", &["--system", "call", "org.freedesktop.UPower", "/org/freedesktop/UPower/devices/battery_battery", "org.freedesktop.UPower.Device", "GetHistory", "suu", "charge", "86400", "400"]);
    let words: Vec<&str> = hist.split_whitespace().skip(2).collect();
    let mut history: Vec<(u64, f64, u32)> = words
        .chunks(3)
        .filter_map(|c| Some((c.first()?.parse().ok()?, c.get(1)?.parse().ok()?, c.get(2)?.parse().ok()?)))
        .collect();
    history.sort_by_key(|h| h.0);
    let name = first_name();
    Facts {
        level: num("capacity") as u32,
        status: file("status"),
        power_w: (num("current_now") * num("voltage_now")).abs() / 1e12,
        temp_c: num("temp") / 10.0,
        to_empty_s: prop("TimeToEmpty"),
        to_full_s: prop("TimeToFull"),
        name,
        history,
    }
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
