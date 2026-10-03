//! The pen's sheet, item's (sfduo-pen-screen): the page right of the right
//! panel, the mirror of the system screen. A swipe left on the right
//! panel's desktop brings it in with the finger; a swipe right on it takes
//! it away. While it is out, the right panel counts as taken: the dock
//! leaves it.
//!
//! It is an empty sheet, and the pen draws on it; fingers do not. The pen's
//! pressure sets the line's width. Its other end (libinput's eraser tool),
//! its barrel button held, or the eraser picked in the toolbar erases. At
//! the top right: pen or eraser, clear, and save (a PNG in
//! ~/Pictures/Sketches). What is drawn stays while the compositor runs.
//!
//! The sheet is one texture of the panel's size in memory. A stroke draws
//! into that memory, and only the rectangle it touched goes to the GPU
//! (smithay's MemoryRenderBuffer uploads its damage), so a line costs a few
//! kilobytes a frame, not the sheet's 10 MB.

use resvg::tiny_skia;
use smithay::backend::allocator::Fourcc;
use smithay::backend::input::TouchSlot;
use smithay::backend::renderer::element::memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement};
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::utils::{Buffer, Logical, Point, Rectangle, Transform};

use crate::layout::{self, SCALE};
use crate::shade::ShellElement;

const FLICK: f64 = 0.3;
const COMMIT: f64 = 0.3;
const SLIDE_NS: f64 = 220e6;
const PAPER: [u8; 4] = [250, 250, 247, 255];
const INK: [u8; 4] = [28, 28, 32, 255];
/// A line's width, physical px, at no pressure and at full.
const THIN: f32 = 2.0;
const THICK: f32 = 8.0;
const ERASER: f32 = 40.0;
/// A sample nearer the last than this (physical px) is left out.
const MIN_STEP: f32 = 1.5;
/// How far the width goes toward each new sample's (0-1).
const WIDTH_EASE: f32 = 0.35;
/// The stroke's ends: this much of its width.
const TAPER: f32 = 0.55;
/// A curve is drawn in pieces about this long (physical px).
const PIECE: f32 = 2.5;
/// The toolbar's buttons, logical px from the panel's top right.
const BUTTON: f64 = 44.0;
const BUTTON_GAP: f64 = 10.0;
const BAR_TOP: f64 = 30.0;

#[derive(Clone, Copy, PartialEq)]
enum Button {
    Tool,
    Clear,
    Save,
}

const BUTTONS: [Button; 3] = [Button::Tool, Button::Clear, Button::Save];

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

pub struct PenSheet {
    p: f64,
    grab: Option<Grab>,
    run: Option<Run>,
    canvas: Option<MemoryRenderBuffer>,
    /// The last point of the stroke being drawn (physical px on the sheet),
    /// and its width.
    /// The stroke under way: its last points (x, y, width; physical px),
    /// the newest last, and whether it erases.
    stroke: Option<(Vec<(f32, f32, f32)>, bool)>,
    /// The stroke's cost, for the log: events, pieces drawn, time drawing,
    /// the longest event, when it began.
    cost: (u32, u32, u64, u64, u64),
    /// The toolbar's eraser picked.
    eraser: bool,
    /// The pen's barrel button held.
    barrel: bool,
    /// A finger on a toolbar button.
    pressed: Option<(TouchSlot, Button)>,
    icons: Vec<Option<MemoryRenderBuffer>>,
    pen_icon: Option<MemoryRenderBuffer>,
    button_bg: MemoryRenderBuffer,
    button_on: crate::accent::Rounded,
}

impl PenSheet {
    pub fn new() -> PenSheet {
        let icon = |n: &str| crate::quick::symbolic(&format!("/usr/share/icons/Adwaita/symbolic/actions/{n}.svg"), 24);
        let mut sheet = PenSheet {
            p: 0.0,
            grab: None,
            run: None,
            canvas: None,
            stroke: None,
            cost: (0, 0, 0, 0, 0),
            eraser: false,
            barrel: false,
            pressed: None,
            icons: vec![icon("edit-clear-symbolic"), icon("edit-delete-symbolic"), icon("document-save-symbolic")],
            pen_icon: icon("document-edit-symbolic"),
            button_bg: crate::grid::rounded(BUTTON, BUTTON, BUTTON / 2.0, [40, 40, 44, 230]),
            button_on: crate::accent::Rounded::new(BUTTON, BUTTON, BUTTON / 2.0),
        };
        // Made, and warmed up, at the start: its first upload (10 MB) would
        // otherwise be the first frame of its slide.
        sheet.canvas();
        sheet
    }

    fn at(&self, frame_ns: u64) -> f64 {
        if let Some(g) = &self.grab {
            let w = layout::panels()[1].size.w as f64;
            return (g.from + (g.start_x - g.last.1) / w).clamp(0.0, 1.0);
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
    /// on the right panel, or not.
    pub fn show(&mut self, out: bool) {
        self.grab = None;
        self.run = None;
        if out {
            self.canvas();
        }
        self.p = if out { 1.0 } else { 0.0 };
        tell_pen_split(out);
    }

    pub fn out(&self) -> bool {
        self.p > 0.0 || self.grab.is_some() || self.run.is_some()
    }

    /// Fully out and still: the pen may draw.
    fn open(&self) -> bool {
        self.p >= 1.0 && self.grab.is_none() && self.run.is_none()
    }

    fn canvas(&mut self) -> &mut MemoryRenderBuffer {
        self.canvas.get_or_insert_with(|| {
            let p = layout::panels()[1];
            let (w, h) = (p.size.w * SCALE, p.size.h * SCALE);
            let mut data = vec![0u8; (w * h * 4) as usize];
            for px in data.chunks_exact_mut(4) {
                px.copy_from_slice(&PAPER);
            }
            MemoryRenderBuffer::from_slice(&data, Fourcc::Abgr8888, (w, h), SCALE, Transform::Normal, None)
        })
    }

    /// A swipe left that began on the right panel's desktop.
    pub fn grab_from(&mut self, slot: TouchSlot, start: Point<f64, Logical>, time_us: u64) {
        let from = self.at(hybris_hwc::now_ns());
        self.run = None;
        self.canvas();
        self.grab = Some(Grab { slot, start_x: start.x, from, last: (time_us, start.x), velocity: 0.0 });
    }

    fn button_rect(i: usize) -> Rectangle<f64, Logical> {
        let p = layout::panels()[1];
        let right = (p.loc.x + p.size.w) as f64 - 20.0;
        let x = right - (BUTTONS.len() - i) as f64 * (BUTTON + BUTTON_GAP) + BUTTON_GAP;
        Rectangle::new((x, BAR_TOP).into(), (BUTTON, BUTTON).into())
    }

    fn button_at(pos: Point<f64, Logical>) -> Option<Button> {
        (0..BUTTONS.len()).find(|&i| Self::button_rect(i).contains(pos)).map(|i| BUTTONS[i])
    }

    /// A finger down while the sheet is out: a toolbar button, or the swipe
    /// right that takes it away. Fingers do not draw.
    pub fn down(&mut self, slot: TouchSlot, pos: Point<f64, Logical>, time_us: u64) -> bool {
        if !self.out() || layout::panel_at(pos) != Some(1) {
            return false;
        }
        // A toolbar button; the rest of a finger's touches move the ribbon.
        let _ = time_us;
        if self.open() {
            if let Some(b) = Self::button_at(pos) {
                self.pressed = Some((slot, b));
                return true;
            }
        }
        false
    }

    pub fn holds(&self, slot: TouchSlot) -> bool {
        self.grab.as_ref().is_some_and(|g| g.slot == slot) || self.pressed.is_some_and(|(s, _)| s == slot)
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
        if let Some((_, b)) = self.pressed.take_if(|(s, _)| *s == slot) {
            self.button(b);
            return;
        }
        if !self.grab.as_ref().is_some_and(|g| g.slot == slot) {
            return;
        }
        let now = hybris_hwc::now_ns();
        // Left opens (the finger's velocity is negative), right closes.
        let v = -self.grab.as_ref().unwrap().velocity;
        let p = self.at(now);
        let open = if v > FLICK { true } else if v < -FLICK { false } else { p >= COMMIT };
        self.grab = None;
        let to = if open { 1.0 } else { 0.0 };
        self.run = Some(Run { from: p, to, start_ns: now, duration_ns: (SLIDE_NS * (to - p).abs()).max(40e6) as u64 });
        self.p = to;
    }

    pub fn cancel(&mut self) {
        self.pressed = None;
        if let Some(slot) = self.grab.as_ref().map(|g| g.slot) {
            self.up(slot);
        }
    }

    fn button(&mut self, b: Button) {
        match b {
            Button::Tool => self.eraser = !self.eraser,
            Button::Clear => {
                if let Some(c) = &mut self.canvas {
                    let mut ctx = c.render();
                    let _ = ctx.draw(|mem| {
                        for px in mem.chunks_exact_mut(4) {
                            px.copy_from_slice(&PAPER);
                        }
                        let p = layout::panels()[1];
                        Ok::<_, ()>(vec![Rectangle::<i32, Buffer>::from_size((p.size.w * SCALE, p.size.h * SCALE).into())])
                    });
                }
                tracing::info!("pen sheet: cleared");
            }
            Button::Save => self.save(),
        }
    }

    /// The sheet as a PNG in ~/Pictures/Sketches, written on a thread.
    fn save(&mut self) {
        let p = layout::panels()[1];
        let (w, h) = ((p.size.w * SCALE) as u32, (p.size.h * SCALE) as u32);
        let Some(c) = &mut self.canvas else { return };
        let mut copy = Vec::new();
        let mut ctx = c.render();
        let _ = ctx.draw(|mem| {
            copy = mem.to_vec();
            Ok::<_, ()>(vec![])
        });
        drop(ctx);
        std::thread::spawn(move || {
            let dir = format!("{}/Pictures/Sketches", std::env::var("HOME").unwrap_or_default());
            let _ = std::fs::create_dir_all(&dir);
            let t = crate::shade::local_time();
            let path = format!("{dir}/sketch-{}-{:02}-{:02}-{:02}{:02}{:02}.png", t.tm_year + 1900, t.tm_mon + 1, t.tm_mday, t.tm_hour, t.tm_min, t.tm_sec);
            let saved = tiny_skia::IntSize::from_wh(w, h)
                .and_then(|size| tiny_skia::Pixmap::from_vec(copy, size))
                .map(|px| px.save_png(&path));
            match saved {
                Some(Ok(())) => tracing::info!("pen sheet: saved {path}"),
                other => tracing::warn!("pen sheet: not saved: {:?}", other.map(|r| r.err())),
            }
        });
    }

    /// Whether the pen's events are the sheet's: it is open and the pen is
    /// over it.
    pub fn takes_pen(&self, pos: Point<f64, Logical>) -> bool {
        self.open() && layout::panel_at(pos) == Some(1)
    }

    pub fn set_barrel(&mut self, held: bool) {
        self.barrel = held;
    }

    /// The pen's tip down: a toolbar button, or a stroke begun - a dot, a
    /// little thinner than the line will be (the tip tapers in).
    pub fn pen_down(&mut self, pos: Point<f64, Logical>, pressure: f64, eraser_end: bool) {
        if let Some(b) = Self::button_at(pos) {
            self.button(b);
            return;
        }
        let (x, y) = self.local(pos);
        let erase = eraser_end || self.barrel || self.eraser;
        let w = Self::width(pressure, erase);
        self.cost = (0, 0, 0, 0, hybris_hwc::now_ns());
        self.ink(&[(x, y, w * TAPER), (x, y, w * TAPER)], erase);
        self.stroke = Some((vec![(x, y, w * TAPER)], erase));
    }

    /// The line's width at a pressure (physical px).
    fn width(pressure: f64, erase: bool) -> f32 {
        if erase { ERASER } else { THIN + (THICK - THIN) * pressure.clamp(0.0, 1.0) as f32 }
    }

    /// The pen moved: the stroke goes on as a curve - a quadratic through
    /// the midpoints of the samples, each sample its control point - its
    /// width eased toward the pressure's, so neither the line nor its width
    /// steps at the samples. A sample too near the last is left out (the
    /// tip's tremble).
    pub fn pen_motion(&mut self, pos: Point<f64, Logical>, pressure: f64, eraser_end: bool) {
        let Some((mut pts, erase)) = self.stroke.take() else { return };
        let (x, y) = self.local(pos);
        let &(lx, ly, lw) = pts.last().expect("a point");
        if (x - lx).hypot(y - ly) < MIN_STEP {
            self.stroke = Some((pts, erase));
            return;
        }
        let w = lw + (Self::width(pressure, erase || eraser_end) - lw) * WIDTH_EASE;
        pts.push((x, y, w));
        let n = pts.len();
        let mid = |a: (f32, f32, f32), b: (f32, f32, f32)| ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0, (a.2 + b.2) / 2.0);
        let line = if n == 2 {
            // The start: straight to the first midpoint.
            vec![pts[0], mid(pts[0], pts[1])]
        } else {
            let (a, b, c) = (pts[n - 3], pts[n - 2], pts[n - 1]);
            curve(mid(a, b), b, mid(b, c))
        };
        self.ink(&line, erase);
        if pts.len() > 3 {
            pts.remove(0);
        }
        self.stroke = Some((pts, erase));
    }

    /// The tip lifted: the stroke's end, from the last midpoint to the last
    /// sample, tapering out.
    pub fn pen_up(&mut self) {
        let (events, pieces, ns, worst, at) = self.cost;
        if at > 0 {
            let long = (hybris_hwc::now_ns() - at) as f64 / 1e9;
            tracing::info!("pen: a stroke: {events} events ({:.0}/s), {pieces} pieces, drawing {:.1} ms in all, {:.2} ms the longest", events as f64 / long.max(1e-3), ns as f64 / 1e6, worst as f64 / 1e6);
        }
        if let Some((pts, erase)) = self.stroke.take() {
            let n = pts.len();
            if n >= 2 {
                let (a, b) = (pts[n - 2], pts[n - 1]);
                let m = ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0, (a.2 + b.2) / 2.0);
                self.ink(&[m, (b.0, b.1, b.2 * TAPER)], erase);
            }
        }
    }

    /// For tests: a wave across the sheet, its pressure rising from 0 to 1,
    /// and its right third erased with the pen's other end.
    pub fn test_stroke(&mut self) {
        if !self.open() {
            return;
        }
        let p = layout::panels()[1];
        let at = |k: f64| Point::from((p.loc.x as f64 + 60.0 + k * (p.size.w as f64 - 120.0), 700.0 + 120.0 * (k * 12.0).sin()));
        self.pen_down(at(0.0), 0.0, false);
        for i in 1..=200 {
            let k = i as f64 / 200.0;
            self.pen_motion(at(k), k, false);
        }
        self.pen_up();
        self.pen_down(at(0.7) + Point::from((0.0, -150.0)), 1.0, true);
        self.pen_motion(at(0.7) + Point::from((0.0, 150.0)), 1.0, true);
        self.pen_up();
    }

    fn local(&self, pos: Point<f64, Logical>) -> (f32, f32) {
        let p = layout::panels()[1];
        (((pos.x - p.loc.x as f64) * SCALE as f64) as f32, (pos.y * SCALE as f64) as f32)
    }

    /// A run of a stroke into the sheet's memory - short pieces, each with
    /// its own width between its ends', round-capped so they join; only
    /// their bounds are damage.
    fn ink(&mut self, pts: &[(f32, f32, f32)], erase: bool) {
        if pts.len() < 2 {
            return;
        }
        let t = hybris_hwc::now_ns();
        self.ink_now(pts, erase);
        let d = hybris_hwc::now_ns() - t;
        let (e, p, ns, worst, at) = self.cost;
        self.cost = (e + 1, p + pts.len() as u32 - 1, ns + d, worst.max(d), at);
    }

    fn ink_now(&mut self, pts: &[(f32, f32, f32)], erase: bool) {
        let p = layout::panels()[1];
        let (w, h) = ((p.size.w * SCALE) as u32, (p.size.h * SCALE) as u32);
        let canvas = self.canvas();
        let mut ctx = canvas.render();
        let _ = ctx.draw(|mem| {
            let Some(mut pm) = tiny_skia::PixmapMut::from_bytes(mem, w, h) else { return Ok::<_, ()>(vec![]) };
            let mut paint = tiny_skia::Paint::default();
            paint.anti_alias = true;
            let c = if erase { PAPER } else { INK };
            paint.set_color_rgba8(c[0], c[1], c[2], c[3]);
            let (mut ax, mut ay, mut bx, mut by) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
            for pair in pts.windows(2) {
                let ((x0, y0, w0), (x1, y1, w1)) = (pair[0], pair[1]);
                let mut pb = tiny_skia::PathBuilder::new();
                pb.move_to(x0, y0);
                // A dot for a tap: a zero-length line with round caps.
                pb.line_to(x1 + if x0 == x1 && y0 == y1 { 0.01 } else { 0.0 }, y1);
                let Some(path) = pb.finish() else { continue };
                let width = (w0 + w1) / 2.0;
                let stroke = tiny_skia::Stroke { width, line_cap: tiny_skia::LineCap::Round, line_join: tiny_skia::LineJoin::Round, ..Default::default() };
                pm.stroke_path(&path, &paint, &stroke, tiny_skia::Transform::identity(), None);
                let r = width / 2.0 + 2.0;
                (ax, ay, bx, by) = (ax.min(x0.min(x1) - r), ay.min(y0.min(y1) - r), bx.max(x0.max(x1) + r), by.max(y0.max(y1) + r));
            }
            if ax > bx {
                return Ok(vec![]);
            }
            let (ax, ay) = (ax.max(0.0) as i32, ay.max(0.0) as i32);
            let (bx, by) = (bx.min(w as f32) as i32, by.min(h as f32) as i32);
            Ok(vec![Rectangle::<i32, Buffer>::new((ax, ay).into(), ((bx - ax).max(1), (by - ay).max(1)).into())])
        });
    }

    /// After a frame: whether it still moves.
    pub fn settle(&mut self, frame_ns: u64) -> bool {
        if let Some(r) = &self.run {
            if frame_ns >= r.start_ns + r.duration_ns {
                self.run = None;
                // One frame more: the dock learns the panel is free (or
                // taken) only on a frame.
                return true;
            }
        }
        self.grab.is_some() || self.run.is_some()
    }

    pub fn warm_up(&self, renderer: &mut GlesRenderer) -> usize {
        let on = self.button_on.get();
        self.icons
            .iter()
            .flatten()
            .chain(self.pen_icon.iter())
            .chain([&self.button_bg, &on])
            .chain(self.canvas.iter())
            .filter(|b| MemoryRenderBufferRenderElement::from_buffer(renderer, (0.0, 0.0), b, None, None, None, Kind::Unspecified).is_ok())
            .count()
    }

    pub fn elements(&self, renderer: &mut GlesRenderer, frame_ns: u64) -> Vec<ShellElement> {
        self.drawn_at(renderer, self.at(frame_ns))
    }

    /// The sheet all the way out, wherever it is: for the ribbon to carry.
    pub fn picture(&self, renderer: &mut GlesRenderer) -> Vec<ShellElement> {
        self.drawn_at(renderer, 1.0)
    }

    /// Its canvas made, for the ribbon to show it as it comes.
    pub fn ready(&mut self) {
        self.canvas();
    }

    /// The canvas, to be sent to the GPU ahead of its first frame.
    pub fn canvas_buffer(&self) -> Option<&MemoryRenderBuffer> {
        self.canvas.as_ref()
    }

    fn drawn_at(&self, renderer: &mut GlesRenderer, p: f64) -> Vec<ShellElement> {
        let Some(canvas) = &self.canvas else { return Vec::new() };
        if p <= 0.0 {
            return Vec::new();
        }
        let panel = layout::panels()[1];
        // It slides in from the right edge with the finger.
        let dx = panel.size.w as f64 * (1.0 - p);
        let s = SCALE as f64;
        let mut out = Vec::new();
        let mut put = |out: &mut Vec<ShellElement>, b: &MemoryRenderBuffer, x: f64, y: f64| {
            if let Ok(e) = MemoryRenderBufferRenderElement::from_buffer(renderer, (((x + dx) * s).round(), (y * s).round()), b, None, None, None, Kind::Unspecified) {
                out.push(ShellElement::Text(e));
            }
        };
        for (i, b) in BUTTONS.iter().enumerate() {
            let r = Self::button_rect(i);
            let icon = match b {
                Button::Tool if !self.eraser => self.pen_icon.as_ref(),
                _ => self.icons[i].as_ref(),
            };
            if let Some(ic) = icon {
                put(&mut out, ic, r.loc.x + (BUTTON - 24.0) / 2.0, r.loc.y + (BUTTON - 24.0) / 2.0);
            }
            let lit = *b == Button::Tool && self.eraser || self.pressed.is_some_and(|(_, pb)| pb == *b);
            let bg = if lit { self.button_on.get() } else { self.button_bg.clone() };
            put(&mut out, &bg, r.loc.x, r.loc.y);
        }
        put(&mut out, canvas, panel.loc.x as f64, 0.0);
        out
    }
}

/// A quadratic from `a` to `c` with `b` its control, as short pieces, the
/// width going from `a`'s to `c`'s.
fn curve(a: (f32, f32, f32), b: (f32, f32, f32), c: (f32, f32, f32)) -> Vec<(f32, f32, f32)> {
    let len = (b.0 - a.0).hypot(b.1 - a.1) + (c.0 - b.0).hypot(c.1 - b.1);
    let n = ((len / PIECE).ceil() as usize).clamp(1, 64);
    (0..=n)
        .map(|i| {
            let t = i as f32 / n as f32;
            let u = 1.0 - t;
            let x = u * u * a.0 + 2.0 * u * t * b.0 + t * t * c.0;
            let y = u * u * a.1 + 2.0 * u * t * b.1 + t * t * c.1;
            (x, y, a.2 + (c.2 - a.2) * t)
        })
        .collect()
}

/// The port's sfduo-pen-split, which owns the digitizer, told whether the
/// sheet is out: only then does it give the pen over the sheet as a tablet
/// tool (pressure, the eraser end); elsewhere the pen stays one more finger,
/// so the shell's swipes take it. As sfduo-pen-screen told it under phosh;
/// never told, it kept the pen a finger and the sheet did not draw.
fn tell_pen_split(out: bool) {
    let sock = std::os::unix::net::UnixDatagram::unbound();
    if let Ok(s) = sock {
        if s.send_to(if out { b"1" } else { b"0" }, "/run/sfduo-pen.sock").is_err() {
            tracing::debug!("pen: sfduo-pen-split not listening");
        }
    }
}
