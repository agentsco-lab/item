//! The shade: a sheet over each panel, pulled down from the panel's top edge
//! by the finger and drawn by the compositor itself, in the same pass as the
//! windows.
//!
//! A touch that starts at the top edge of a panel, or on its sheet, belongs to
//! the sheet and never reaches a client. While the finger holds it, the sheet's
//! bottom edge follows the finger; let go, it runs to open or closed by the
//! finger's speed, or by how far it was pulled. A tap on an open sheet closes
//! it. The sheet carries the time, the date and the battery, as text
//! (`text.rs`).

use smithay::backend::input::TouchSlot;
use smithay::backend::renderer::element::memory::MemoryRenderBufferRenderElement;
use smithay::backend::renderer::element::solid::SolidColorRenderElement;
use smithay::backend::renderer::element::{render_elements, Id, Kind};
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::renderer::utils::CommitCounter;
use smithay::utils::{Physical, Rectangle};

use crate::layout::{self, SCALE};
use crate::text::{Font, Label};

render_elements! {
    /// What the shell draws: rectangles and text.
    pub ShellElement<=GlesRenderer>;
    Solid=SolidColorRenderElement,
    Text=MemoryRenderBufferRenderElement<GlesRenderer>,
}

/// Touches starting this close to a panel's top pull its sheet (logical px).
const EDGE: f64 = 20.0;
/// How long the sheet may take to run open or closed, ms.
const RUN_MIN_MS: f64 = 150.0;
const RUN_MAX_MS: f64 = 350.0;
/// A release faster than this (logical px per ms) decides by its direction.
const FLING: f64 = 0.3;
/// A touch that moves less than this is a tap.
const TAP: f64 = 8.0;

const SHEET: [f32; 4] = premultiplied([0.06, 0.08, 0.12], 0.88);
const TEXT: [f32; 4] = [0.93, 0.95, 0.97, 1.0];
const DIM: [f32; 4] = [0.62, 0.66, 0.72, 1.0];
const HANDLE: [f32; 4] = [0.45, 0.48, 0.52, 1.0];

const fn premultiplied(rgb: [f32; 3], a: f32) -> [f32; 4] {
    [rgb[0] * a, rgb[1] * a, rgb[2] * a, a]
}

/// The finger holding a sheet.
struct Grab {
    slot: TouchSlot,
    /// The sheet's height less the finger's y.
    offset: f64,
    start_y: f64,
    moved: bool,
    /// The last event's time (µs) and y.
    last: (u64, f64),
    /// Smoothed, logical px per ms.
    velocity: f64,
}

/// The sheet running open or closed after a release.
///
/// It leaves at the finger's speed and slows to a stop: a cubic from `from`
/// to `to` with the release's velocity at the start and none at the end. From
/// a finger held still it starts from rest, not at full speed.
struct Run {
    from: f64,
    to: f64,
    start_ns: u64,
    duration_ns: u64,
    /// The start's velocity times the duration, logical px.
    v0t: f64,
}

impl Run {
    fn new(from: f64, to: f64, velocity: f64) -> Run {
        let dist = to - from;
        // Only a velocity towards `to` carries on.
        let v = if velocity * dist > 0.0 { velocity.abs() } else { 0.0 };
        // At most twice the distance over the duration: beyond that the
        // cubic would overshoot `to`. Fast releases run shorter.
        let ms = if v > 0.0 { (2.0 * dist.abs() / v).clamp(RUN_MIN_MS, RUN_MAX_MS) } else { RUN_MAX_MS };
        let v0t = (v * ms).min(2.0 * dist.abs()) * dist.signum();
        Run { from, to, start_ns: hybris_hwc::now_ns(), duration_ns: (ms * 1e6) as u64, v0t }
    }

    fn at(&self, frame_ns: u64) -> f64 {
        let s = (frame_ns.saturating_sub(self.start_ns) as f64 / self.duration_ns as f64).clamp(0.0, 1.0);
        // Hermite: from, to, the start's velocity, none at the end.
        let (s2, s3) = (s * s, s * s * s);
        (2.0 * s3 - 3.0 * s2 + 1.0) * self.from + (s3 - 2.0 * s2 + s) * self.v0t + (-2.0 * s3 + 3.0 * s2) * self.to
    }

    fn done(&self, frame_ns: u64) -> bool {
        frame_ns >= self.start_ns + self.duration_ns
    }
}

struct Sheet {
    /// How far down the sheet is pulled, logical px (0: closed).
    height: f64,
    grab: Option<Grab>,
    run: Option<Run>,
    /// One id for each rectangle the sheet may draw, so the damage tracker
    /// follows each from frame to frame.
    ids: Vec<Id>,
}

impl Sheet {
    fn height_at(&self, frame_ns: u64) -> f64 {
        match &self.run {
            Some(run) => run.at(frame_ns),
            None => self.height,
        }
    }
}

pub struct Shade {
    sheets: [Sheet; 2],
    /// A finger's move the screen has not shown yet: the event's time (µs,
    /// CLOCK_MONOTONIC as libinput stamps it) and when it reached us (ns).
    moved: Option<(u64, u64)>,
    fonts: Option<(Font, Font)>,
    time: Label,
    date: Label,
    battery: Label,
}

impl Shade {
    pub fn new() -> Shade {
        let sheet = || Sheet { height: 0.0, grab: None, run: None, ids: (0..40).map(|_| Id::new()).collect() };
        let light = Font::load(&["/usr/share/fonts/truetype/lato/Lato-Light.ttf", "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf"]);
        let regular = Font::load(&["/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf", "/usr/share/fonts/truetype/lato/Lato-Regular.ttf"]);
        if light.is_none() || regular.is_none() {
            tracing::warn!("shade: no fonts, no text");
        }
        let mut shade = Shade {
            sheets: [sheet(), sheet()],
            moved: None,
            fonts: light.zip(regular),
            time: Label::new(64.0, TEXT),
            date: Label::new(18.0, TEXT),
            battery: Label::new(15.0, DIM),
        };
        shade.refresh_text();
        shade
    }

    /// Brings the text up to the time and the battery; returns whether any
    /// of it changed.
    pub fn refresh_text(&mut self) -> bool {
        let Some((light, regular)) = &self.fonts else { return false };
        let now = local_time();
        let time = format!("{}:{:02}", now.tm_hour, now.tm_min);
        let date = format!("{}, {} {}", WEEKDAYS[now.tm_wday as usize], now.tm_mday, MONTHS[now.tm_mon as usize]);
        let battery = battery();
        let mut changed = self.time.set(light, &time);
        changed |= self.date.set(regular, &date);
        changed |= self.battery.set(regular, &battery);
        changed
    }

    /// Whether a sheet is out, and its text worth keeping up to date.
    pub fn visible(&self) -> bool {
        self.sheets.iter().any(|s| s.height > 0.0 || s.run.is_some() || s.grab.is_some())
    }

    /// A touch down: taken by a sheet if it starts at a panel's top edge or on
    /// an open sheet. Returns whether it was.
    pub fn down(&mut self, slot: TouchSlot, x: f64, y: f64, time_us: u64) -> bool {
        let Some(p) = layout::panel_at((x, y).into()) else { return false };
        let now = hybris_hwc::now_ns();
        let sheet = &mut self.sheets[p];
        let height = sheet.height_at(now);
        if sheet.grab.is_some() || !(y < EDGE || y < height) {
            return false;
        }
        // Caught mid-run: it stops under the finger.
        sheet.run = None;
        sheet.height = height;
        let sheet_grab = Grab { slot, offset: height - y, start_y: y, moved: false, last: (time_us, y), velocity: 0.0 };
        sheet.grab = Some(sheet_grab);
        tracing::debug!("shade {p}: grabbed at {y:.0}, height {height:.0}");
        self.refresh_text();
        true
    }

    /// Whether a touch belongs to a sheet.
    pub fn holds(&self, slot: TouchSlot) -> bool {
        self.sheets.iter().any(|s| s.grab.as_ref().is_some_and(|g| g.slot == slot))
    }

    pub fn motion(&mut self, slot: TouchSlot, y: f64, time_us: u64) {
        let Some(sheet) = self.sheets.iter_mut().find(|s| s.grab.as_ref().is_some_and(|g| g.slot == slot)) else {
            return;
        };
        let grab = sheet.grab.as_mut().unwrap();
        let dt_ms = time_us.saturating_sub(grab.last.0) as f64 / 1000.0;
        if dt_ms > 0.0 {
            let v = (y - grab.last.1) / dt_ms;
            grab.velocity = grab.velocity * 0.4 + v * 0.6;
        }
        grab.last = (time_us, y);
        grab.moved |= (y - grab.start_y).abs() > TAP;
        sheet.height = (y + grab.offset).clamp(0.0, layout::LAYOUT.1 as f64);
        self.moved.get_or_insert((time_us, hybris_hwc::now_ns()));
    }

    /// The finger lets go: the sheet runs open or closed.
    pub fn up(&mut self, slot: TouchSlot) {
        let full = layout::LAYOUT.1 as f64;
        for (p, sheet) in self.sheets.iter_mut().enumerate() {
            if !sheet.grab.as_ref().is_some_and(|g| g.slot == slot) {
                continue;
            }
            let grab = sheet.grab.take().unwrap();
            let open = if !grab.moved {
                // A tap closes an open sheet; a touch at the edge of a closed
                // one that did not pull it leaves it closed.
                false
            } else if grab.velocity.abs() > FLING {
                grab.velocity > 0.0
            } else {
                sheet.height > full / 2.0
            };
            let to = if open { full } else { 0.0 };
            tracing::debug!("shade {p}: released at {:.0}, {:.2} px/ms, to {to:.0}", sheet.height, grab.velocity);
            sheet.run = Some(Run::new(sheet.height, to, grab.velocity));
            sheet.height = to;
        }
    }

    pub fn cancel(&mut self) {
        let slots: Vec<TouchSlot> = self.sheets.iter().filter_map(|s| s.grab.as_ref().map(|g| g.slot)).collect();
        for slot in slots {
            self.up(slot);
        }
    }

    /// After a frame for `frame_ns`: ends the runs that are over. Returns
    /// whether a sheet is still running and wants the next frame.
    pub fn settle(&mut self, frame_ns: u64) -> bool {
        let mut running = false;
        for sheet in &mut self.sheets {
            if let Some(run) = &sheet.run {
                if run.done(frame_ns) {
                    sheet.run = None;
                } else {
                    running = true;
                }
            }
        }
        running
    }

    /// The finger's move a frame has now shown, if there was one.
    pub fn take_moved(&mut self) -> Option<(u64, u64)> {
        self.moved.take()
    }

    /// Uploads the labels' textures; returns how many.
    pub fn warm_up(&self, renderer: &mut GlesRenderer) -> usize {
        [&self.time, &self.date, &self.battery]
            .iter()
            .filter(|l| MemoryRenderBufferRenderElement::from_buffer(renderer, (0.0, 0.0), &l.buffer, None, None, None, Kind::Unspecified).is_ok())
            .count()
    }

    /// The sheets' state, for the log.
    pub fn describe(&self) -> String {
        let now = hybris_hwc::now_ns();
        self.sheets
            .iter()
            .map(|s| {
                let what = if s.grab.is_some() { "held" } else if s.run.is_some() { "running" } else { "still" };
                format!("{:.0} {what}", s.height_at(now))
            })
            .collect::<Vec<_>>()
            .join(" / ")
    }

    /// What the sheets draw for a frame shown at `frame_ns`, topmost first.
    pub fn elements(&self, renderer: &mut GlesRenderer, frame_ns: u64) -> Vec<ShellElement> {
        let mut out = Vec::new();
        for (sheet, panel) in self.sheets.iter().zip(layout::panels()) {
            let height = sheet.height_at(frame_ns);
            // In physical px, whole ones, so an edge does not shimmer.
            let h = (height * SCALE as f64).round() as i32;
            if h <= 0 {
                continue;
            }
            let panel = panel.to_physical(SCALE);
            // The sheet's content hangs from its bottom edge: `y` is logical
            // px from it, and a label is centred on the panel.
            for (label, y) in [(&self.time, -210), (&self.date, -118), (&self.battery, -80)] {
                if label.extent.w == 0 {
                    continue;
                }
                let x = panel.loc.x + (panel.size.w - label.extent.w * SCALE) / 2;
                let loc = (x as f64, (h + y * SCALE) as f64);
                match MemoryRenderBufferRenderElement::from_buffer(renderer, loc, &label.buffer, None, None, None, Kind::Unspecified) {
                    Ok(e) => out.push(ShellElement::Text(e)),
                    Err(e) => tracing::warn!("shade: text: {e}"),
                }
            }
            let mut ids = sheet.ids.iter();
            let handle = Rectangle::<i32, Physical>::new(
                (panel.loc.x + panel.size.w / 2 - 30 * SCALE, h - 16 * SCALE).into(),
                (60 * SCALE, 5 * SCALE).into(),
            );
            if let Some(r) = handle.intersection(Rectangle::new(panel.loc, (panel.size.w, h).into())) {
                let id = ids.next().expect("enough ids").clone();
                out.push(ShellElement::Solid(SolidColorRenderElement::new(id, r, CommitCounter::default(), HANDLE, Kind::Unspecified)));
            }
            // The sheet itself, from the panel's top to its edge, in physical
            // px: in logical ones an odd height left a line of the window
            // above it.
            let id = ids.next().expect("enough ids").clone();
            let sheet_rect = Rectangle::new(panel.loc, (panel.size.w, h).into());
            out.push(ShellElement::Solid(SolidColorRenderElement::new(id, sheet_rect, CommitCounter::default(), SHEET, Kind::Unspecified)));
        }
        out
    }
}

const WEEKDAYS: [&str; 7] = ["Воскресенье", "Понедельник", "Вторник", "Среда", "Четверг", "Пятница", "Суббота"];
const MONTHS: [&str; 12] = [
    "января", "февраля", "марта", "апреля", "мая", "июня", "июля", "августа", "сентября", "октября", "ноября", "декабря",
];

fn local_time() -> libc::tm {
    unsafe {
        let now = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&now, &mut tm);
        tm
    }
}

/// The battery's charge and whether it charges, from the kernel.
fn battery() -> String {
    let read = |f: &str| std::fs::read_to_string(format!("/sys/class/power_supply/battery/{f}")).map(|s| s.trim().to_owned());
    match (read("capacity"), read("status")) {
        (Ok(c), Ok(s)) if s == "Charging" => format!("{c} % · заряжается"),
        (Ok(c), Ok(s)) if s == "Full" => format!("{c} % · заряжен"),
        (Ok(c), _) => format!("{c} %"),
        _ => String::new(),
    }
}
