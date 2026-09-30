//! The shade: a sheet over each panel, pulled down from the panel's top edge
//! by the finger and drawn by the compositor itself, in the same pass as the
//! windows.
//!
//! A touch that starts at the top edge of a panel, or on its sheet, belongs to
//! the sheet and never reaches a client. While the finger holds it, the sheet's
//! bottom edge follows the finger; let go, it runs to open or closed by the
//! finger's speed, or by how far it was pulled. A tap on an open sheet closes
//! it. The sheet carries a clock, in seven segments of solid rectangles: no
//! text rendering yet.

use std::cell::Cell;

use smithay::backend::input::TouchSlot;
use smithay::backend::renderer::element::solid::SolidColorRenderElement;
use smithay::backend::renderer::element::{Id, Kind};
use smithay::backend::renderer::utils::CommitCounter;
use smithay::utils::{Logical, Physical, Rectangle};

use crate::layout::{self, SCALE};

/// Touches starting this close to a panel's top pull its sheet (logical px).
const EDGE: f64 = 20.0;
/// How long the sheet takes to run open or closed.
const RUN_NS: u64 = 250_000_000;
/// A release faster than this (logical px per ms) decides by its direction.
const FLING: f64 = 0.3;
/// A touch that moves less than this is a tap.
const TAP: f64 = 8.0;

const SHEET: [f32; 4] = premultiplied([0.06, 0.08, 0.12], 0.88);
const DIGITS: [f32; 4] = [0.92, 0.94, 0.96, 1.0];
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
struct Run {
    from: f64,
    to: f64,
    start_ns: u64,
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
            Some(run) => {
                let p = (frame_ns.saturating_sub(run.start_ns) as f64 / RUN_NS as f64).clamp(0.0, 1.0);
                let eased = 1.0 - (1.0 - p).powi(3);
                run.from + (run.to - run.from) * eased
            }
            None => self.height,
        }
    }
}

pub struct Shade {
    sheets: [Sheet; 2],
    /// A finger's move the screen has not shown yet: the event's time (µs,
    /// CLOCK_MONOTONIC as libinput stamps it) and when it reached us (ns).
    moved: Option<(u64, u64)>,
    /// The minute the clock shows.
    minute: Cell<i32>,
}

impl Shade {
    pub fn new() -> Shade {
        let sheet = || Sheet { height: 0.0, grab: None, run: None, ids: (0..40).map(|_| Id::new()).collect() };
        Shade { sheets: [sheet(), sheet()], moved: None, minute: Cell::new(-1) }
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
        sheet.grab = Some(Grab { slot, offset: height - y, start_y: y, moved: false, last: (time_us, y), velocity: 0.0 });
        tracing::debug!("shade {p}: grabbed at {y:.0}, height {height:.0}");
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
            sheet.run = Some(Run { from: sheet.height, to, start_ns: hybris_hwc::now_ns() });
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
                if frame_ns >= run.start_ns + RUN_NS {
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

    /// Whether a sheet is out and the clock on it shows another minute than
    /// the time's.
    pub fn clock_stale(&self) -> bool {
        self.sheets.iter().any(|s| s.height > 0.0 || s.run.is_some()) && local_minutes() != self.minute.get()
    }

    /// The sheets' rectangles for a frame shown at `frame_ns`, topmost first.
    pub fn elements(&self, frame_ns: u64) -> Vec<SolidColorRenderElement> {
        let minutes = local_minutes();
        self.minute.set(minutes);
        let digits = [minutes / 600, minutes / 60 % 10, minutes % 60 / 10, minutes % 10];
        let mut out = Vec::new();
        for (sheet, panel) in self.sheets.iter().zip(layout::panels()) {
            let height = sheet.height_at(frame_ns);
            // In physical px, whole ones, so an edge does not shimmer.
            let h = (height * SCALE as f64).round() as i32;
            if h <= 0 {
                continue;
            }
            let panel = panel.to_physical(SCALE);
            let mut ids = sheet.ids.iter();
            let mut push = |rect: Rectangle<i32, Logical>, color: [f32; 4], ids: &mut std::slice::Iter<Id>| {
                let id = ids.next().expect("enough ids").clone();
                // The sheet's content hangs from its bottom edge.
                let r = Rectangle::<i32, Physical>::new(
                    (panel.loc.x + rect.loc.x * SCALE, h + rect.loc.y * SCALE).into(),
                    (rect.size.w * SCALE, rect.size.h * SCALE).into(),
                );
                if let Some(r) = r.intersection(Rectangle::new(panel.loc, (panel.size.w, h).into())) {
                    out.push(SolidColorRenderElement::new(id, r, CommitCounter::default(), color, Kind::Unspecified));
                }
            };
            // The handle, then the clock, above the sheet.
            push(Rectangle::new((675 / 2 - 30, -16).into(), (60, 5).into()), HANDLE, &mut ids);
            let (w, dh, t) = (40, 72, 8);
            let top = -150;
            let left = (675 - 232) / 2;
            for (i, &d) in digits.iter().enumerate() {
                let x = left + [0, 56, 136, 192][i];
                for (s, seg) in segments(w, dh, t).iter().enumerate() {
                    if SEVEN[d as usize] & (1 << s) != 0 {
                        push(Rectangle::new((x + seg.0, top + seg.1).into(), (seg.2, seg.3).into()), DIGITS, &mut ids);
                    }
                }
            }
            for y in [dh / 3, dh * 2 / 3] {
                push(Rectangle::new((left + 116, top + y - t / 2).into(), (t, t).into()), DIGITS, &mut ids);
            }
            // The sheet itself, from the panel's top to its edge, in physical
            // px: in logical ones an odd height left a line of the window
            // above it.
            let id = ids.next().expect("enough ids").clone();
            let sheet_rect = Rectangle::new(panel.loc, (panel.size.w, h).into());
            out.push(SolidColorRenderElement::new(id, sheet_rect, CommitCounter::default(), SHEET, Kind::Unspecified));
        }
        out
    }
}

/// The segments a-g of a digit `w` by `h` with strokes `t`: x, y, w, h.
fn segments(w: i32, h: i32, t: i32) -> [(i32, i32, i32, i32); 7] {
    [
        (0, 0, w, t),
        (w - t, 0, t, h / 2),
        (w - t, h / 2, t, h / 2),
        (0, h - t, w, t),
        (0, h / 2, t, h / 2),
        (0, 0, t, h / 2),
        (0, h / 2 - t / 2, w, t),
    ]
}

/// Which of the segments a-g (bits 0-6) each digit lights.
const SEVEN: [u8; 10] = [0x3f, 0x06, 0x5b, 0x4f, 0x66, 0x6d, 0x7d, 0x07, 0x7f, 0x6f];

/// Minutes since local midnight.
fn local_minutes() -> i32 {
    unsafe {
        let now = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&now, &mut tm);
        tm.tm_hour * 60 + tm.tm_min
    }
}
