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
    stroke: Option<(f32, f32, f32)>,
    /// The toolbar's eraser picked.
    eraser: bool,
    /// The pen's barrel button held.
    barrel: bool,
    /// A finger on a toolbar button.
    pressed: Option<(TouchSlot, Button)>,
    icons: Vec<Option<MemoryRenderBuffer>>,
    pen_icon: Option<MemoryRenderBuffer>,
    button_bg: MemoryRenderBuffer,
    button_on: MemoryRenderBuffer,
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
            eraser: false,
            barrel: false,
            pressed: None,
            icons: vec![icon("edit-clear-symbolic"), icon("edit-delete-symbolic"), icon("document-save-symbolic")],
            pen_icon: icon("document-edit-symbolic"),
            button_bg: crate::grid::rounded(BUTTON, BUTTON, BUTTON / 2.0, [40, 40, 44, 230]),
            button_on: crate::grid::rounded(BUTTON, BUTTON, BUTTON / 2.0, crate::layout::ACCENT),
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

    /// The pen's tip down: a toolbar button, or a stroke begun.
    pub fn pen_down(&mut self, pos: Point<f64, Logical>, pressure: f64, eraser_end: bool) {
        if let Some(b) = Self::button_at(pos) {
            self.button(b);
            return;
        }
        let (x, y) = self.local(pos);
        let erase = eraser_end || self.barrel || self.eraser;
        let w = if erase { ERASER } else { THIN + (THICK - THIN) * pressure as f32 };
        self.stroke = Some((x, y, w));
        self.segment(x, y, x, y, w, erase);
    }

    pub fn pen_motion(&mut self, pos: Point<f64, Logical>, pressure: f64, eraser_end: bool) {
        let Some((lx, ly, lw)) = self.stroke else { return };
        let (x, y) = self.local(pos);
        let erase = eraser_end || self.barrel || self.eraser;
        let w = if erase { ERASER } else { THIN + (THICK - THIN) * pressure as f32 };
        // The width eased from the last point's, so a line does not step.
        let w = lw * 0.5 + w * 0.5;
        self.segment(lx, ly, x, y, w, erase);
        self.stroke = Some((x, y, w));
    }

    pub fn pen_up(&mut self) {
        self.stroke = None;
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

    /// One segment of a stroke into the sheet's memory; only its bounds are
    /// damage.
    fn segment(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, width: f32, erase: bool) {
        let p = layout::panels()[1];
        let (w, h) = ((p.size.w * SCALE) as u32, (p.size.h * SCALE) as u32);
        let canvas = self.canvas();
        let mut ctx = canvas.render();
        let _ = ctx.draw(|mem| {
            let Some(mut pm) = tiny_skia::PixmapMut::from_bytes(mem, w, h) else { return Ok::<_, ()>(vec![]) };
            let mut pb = tiny_skia::PathBuilder::new();
            pb.move_to(x0, y0);
            // A dot for a tap: a zero-length line with round caps.
            pb.line_to(x1 + if x0 == x1 && y0 == y1 { 0.01 } else { 0.0 }, y1);
            let Some(path) = pb.finish() else { return Ok(vec![]) };
            let mut paint = tiny_skia::Paint::default();
            paint.anti_alias = true;
            let c = if erase { PAPER } else { INK };
            paint.set_color_rgba8(c[0], c[1], c[2], c[3]);
            let stroke = tiny_skia::Stroke { width, line_cap: tiny_skia::LineCap::Round, line_join: tiny_skia::LineJoin::Round, ..Default::default() };
            pm.stroke_path(&path, &paint, &stroke, tiny_skia::Transform::identity(), None);
            let r = width / 2.0 + 2.0;
            let (ax, ay) = ((x0.min(x1) - r).max(0.0) as i32, (y0.min(y1) - r).max(0.0) as i32);
            let (bx, by) = ((x0.max(x1) + r).min(w as f32) as i32, (y0.max(y1) + r).min(h as f32) as i32);
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
        self.icons
            .iter()
            .flatten()
            .chain(self.pen_icon.iter())
            .chain([&self.button_bg, &self.button_on])
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
            put(&mut out, if lit { &self.button_on } else { &self.button_bg }, r.loc.x, r.loc.y);
        }
        put(&mut out, canvas, panel.loc.x as f64, 0.0);
        out
    }
}
