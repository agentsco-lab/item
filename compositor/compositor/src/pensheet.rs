//! The pen's sheet, item's (sfduo-pen-screen; item-tracker #120): the page
//! right of the right panel, the mirror of the system screen. A swipe left
//! on the right panel's desktop brings it in; a swipe right on it takes it
//! away. While it is out, the right panel counts as taken: the dock leaves
//! it.
//!
//! The pen draws on it; fingers do not. Its pressure sets the line's width;
//! its other end (libinput's eraser tool), its barrel button held, or the
//! eraser picked erases. The controls stand in a strip on the left panel by
//! the hinge, so all of the right panel is the pen's: undo and redo, pen or
//! eraser, four inks (black, the accent, red, blue), clear, and save (a PNG
//! in ~/Pictures/Sketches). The strip pulled to the left spreads the sheet
//! over both panels, the strip at the left panel's edge; pulled back, the
//! sheet is the right panel's again.
//!
//! What is drawn is kept as strokes - the pieces each was drawn in, its ink -
//! so it can be undone and redone, and written to ~/.local/share/item/sheet/
//! strokes after each change, read back at the start: nothing is lost when
//! the compositor restarts.
//!
//! The sheet is one texture as wide as the screen in memory. A stroke draws
//! into that memory, and only the rectangle it touched goes to the GPU
//! (smithay's MemoryRenderBuffer uploads its damage), so a line costs a few
//! kilobytes a frame. The stroke's tip ahead of the pen is a small buffer
//! of its own over it (#121).

use resvg::tiny_skia;
use smithay::backend::allocator::Fourcc;
use smithay::backend::input::TouchSlot;
use smithay::backend::renderer::element::memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement};
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::utils::{Buffer, Logical, Point, Rectangle, Size, Transform};

use crate::layout::{self, SCALE};
use crate::shade::ShellElement;

const FLICK: f64 = 0.3;
const COMMIT: f64 = 0.3;
const SLIDE_NS: f64 = 220e6;
const PAPER: [u8; 4] = [250, 250, 247, 255];
/// The inks: black, the accent (taken as each stroke begins), red, blue.
const BLACK: [u8; 4] = [28, 28, 32, 255];
const RED: [u8; 4] = [0xd8, 0x3a, 0x3a, 255];
const BLUE: [u8; 4] = [0x2f, 0x6f, 0xd8, 255];
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
/// The tip drawn ahead of the pen: this far ahead in time (us), at most
/// this long and only when longer than this (physical px); the speed from
/// this much of the pen's way (us).
const PREDICT_US: f32 = 10_000.0;
const PREDICT_MAX: f32 = 40.0;
/// Turning more than this (the cosine between the speed now and before), the
/// tip is let go; straight on, kept whole.
const TURN_COS: f32 = 0.85;
const PREDICT_MIN: f32 = 3.0;
const SPEED_SPAN_US: u64 = 25_000;
/// How far the tip's reach goes toward each move's (0-1).
const REACH_EASE: f32 = 0.3;
/// The guess ahead of the pen: drawn in this many pieces, fading from this
/// much of the ink to none.
const FADE_PIECES: usize = 6;
const FADE_FROM: f32 = 0.7;
/// The tip's buffer's side (physical px): the tip fits it with room.
const TIP_PX: i32 = 256;
/// A curve is drawn in pieces about this long (physical px).
const PIECE: f32 = 2.5;
/// The strip of controls (logical px): its width, its buttons and their
/// gap, its padding, its gap from the panel's edge.
const STRIP_W: f64 = 64.0;
const BUTTON: f64 = 44.0;
const BUTTON_GAP: f64 = 8.0;
const STRIP_PAD: f64 = 10.0;
const STRIP_EDGE: f64 = 10.0;
/// A finger on the strip that moves this far sideways pulls it.
const PULL: f64 = 12.0;
const SPREAD_NS: f64 = 260e6;

#[derive(Clone, Copy, PartialEq, Debug)]
enum Button {
    Undo,
    Redo,
    Tool,
    Ink(usize),
    Clear,
    Save,
}

const BUTTONS: [Button; 9] = [Button::Undo, Button::Redo, Button::Tool, Button::Ink(0), Button::Ink(1), Button::Ink(2), Button::Ink(3), Button::Clear, Button::Save];

/// What was done to the sheet, kept to undo, redo, save and read back.
#[derive(Clone)]
enum Mark {
    /// A stroke: its ink, whether it erased, the pieces it was drawn in
    /// (x, y, width; physical px).
    Stroke { ink: [u8; 4], erase: bool, pieces: Vec<Vec<(f32, f32, f32)>> },
    Clear,
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

/// A finger on the strip: a button pressed, or the strip pulled once it
/// moved far enough sideways.
struct StripTouch {
    slot: TouchSlot,
    button: Option<Button>,
    start_x: f64,
    from: f64,
    last: (u64, f64),
    velocity: f64,
    pulling: bool,
}

pub struct PenSheet {
    p: f64,
    grab: Option<Grab>,
    run: Option<Run>,
    canvas: Option<MemoryRenderBuffer>,
    /// How far the sheet is spread over the left panel (0-1), and its run.
    spread: f64,
    spread_run: Option<Run>,
    strip_touch: Option<StripTouch>,
    /// What pen-split was last told (0 in, 1 out, 2 over both panels).
    told: Option<u8>,
    /// The marks so far and those undone (redo), and the stroke under way:
    /// its last samples (x, y, width; physical px), whether it erases, its
    /// ink, the pieces drawn so far.
    marks: Vec<Mark>,
    undone: Vec<Mark>,
    stroke: Option<(Vec<(f32, f32, f32)>, bool)>,
    stroke_ink: [u8; 4],
    stroke_pieces: Vec<Vec<(f32, f32, f32)>>,
    /// The pen's last samples with their times (x, y physical; us), for its
    /// speed; and the stroke's tip drawn ahead of them - from where the
    /// curve has got to, through the last sample, on along the way the pen
    /// is going - not in the sheet, drawn anew at each move (its top left,
    /// physical).
    recent: std::collections::VecDeque<(f32, f32, u64)>,
    tip: Option<(f32, f32)>,
    /// Erasing: where the pen is (physical), for the eraser's ring there.
    eraser_at: Option<(f32, f32)>,
    /// The tip's buffer, kept from move to move, and the part of it last
    /// drawn (x, y, w, h; physical).
    tip_buf: Option<MemoryRenderBuffer>,
    tip_rect: Option<(i32, i32, i32, i32)>,
    /// The tip's reach ahead (physical px, x and y), eased from move to
    /// move: worked out anew from each, it jumped under the pen.
    reach: std::cell::Cell<(f32, f32)>,
    /// The sheet's elements' cost while drawing, for the stroke's log:
    /// frames, total ns, the most.
    elements_cost: std::cell::Cell<(u32, u64, u64)>,
    /// The stroke's cost, for the log: events, pieces drawn, time drawing,
    /// the longest event, when it began.
    cost: (u32, u32, u64, u64, u64),
    /// The eraser picked; the ink picked.
    eraser: bool,
    ink: usize,
    /// The pen's barrel button held.
    barrel: bool,
    icons: Vec<Option<MemoryRenderBuffer>>,
    eraser_icon: Option<MemoryRenderBuffer>,
    dots: std::cell::RefCell<(u64, Vec<MemoryRenderBuffer>)>,
    ring: MemoryRenderBuffer,
    eraser_ring: MemoryRenderBuffer,
    strip_bg: MemoryRenderBuffer,
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
            spread: 0.0,
            spread_run: None,
            strip_touch: None,
            told: None,
            marks: Vec::new(),
            undone: Vec::new(),
            stroke: None,
            stroke_ink: BLACK,
            stroke_pieces: Vec::new(),
            recent: Default::default(),
            tip: None,
            eraser_at: None,
            tip_buf: None,
            tip_rect: None,
            reach: Default::default(),
            elements_cost: Default::default(),
            cost: (0, 0, 0, 0, 0),
            eraser: false,
            ink: 0,
            barrel: false,
            // By button: undo, redo, the pen (the tool), the four inks
            // (dots, drawn apart), clear, save.
            icons: vec![
                icon("edit-undo-symbolic"),
                icon("edit-redo-symbolic"),
                icon("document-edit-symbolic"),
                None,
                None,
                None,
                None,
                icon("edit-delete-symbolic"),
                icon("document-save-symbolic"),
            ],
            eraser_icon: icon("edit-clear-symbolic"),
            dots: Default::default(),
            ring: ring(BUTTON),
            eraser_ring: eraser_ring(),
            strip_bg: crate::grid::rounded(STRIP_W, Self::strip_rect(0.0).size.h, STRIP_W / 2.0, [26, 28, 34, 235]),
            button_on: crate::accent::Rounded::new(BUTTON, BUTTON, BUTTON / 2.0),
        };
        // Made, read back and warmed up at the start: its first upload would
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
            Some(r) => run_at(r, frame_ns),
            None => self.p,
        }
    }

    /// How far spread over the left panel now (0-1).
    fn spread_at(&self, frame_ns: u64) -> f64 {
        if let Some(t) = self.strip_touch.as_ref().filter(|t| t.pulling) {
            return (t.from + (t.start_x - t.last.1) / Self::spread_span()).clamp(0.0, 1.0);
        }
        match &self.spread_run {
            Some(r) => run_at(r, frame_ns),
            None => self.spread,
        }
    }

    /// How far the strip travels between its two places (logical px).
    fn spread_span() -> f64 {
        let left = layout::panels()[0];
        left.size.w as f64 - STRIP_W - 2.0 * STRIP_EDGE
    }

    /// Spread over both panels (the dock leaves the left one).
    pub fn spread(&self) -> bool {
        self.out() && (self.spread > 0.0 || self.spread_run.is_some() || self.strip_touch.as_ref().is_some_and(|t| t.pulling))
    }

    /// pen-split told where the pen is the sheet's (`tell_pen_split`).
    fn tell(&mut self) {
        let level = if self.p <= 0.0 { 0 } else if self.spread >= 1.0 { 2 } else { 1 };
        if self.told != Some(level) {
            self.told = Some(level);
            tell_pen_split(level);
        }
    }

    /// Out or not at once, as the ribbon has it (ribbon.rs): its page is
    /// on the right panel, or not. Gone, it is the right panel's again.
    pub fn show(&mut self, out: bool) {
        self.grab = None;
        self.run = None;
        if out {
            self.canvas();
        } else {
            self.spread = 0.0;
            self.spread_run = None;
            self.strip_touch = None;
        }
        self.p = if out { 1.0 } else { 0.0 };
        self.tell();
    }

    pub fn out(&self) -> bool {
        self.p > 0.0 || self.grab.is_some() || self.run.is_some()
    }

    /// Fully out and still: the pen may draw.
    fn open(&self) -> bool {
        self.p >= 1.0 && self.grab.is_none() && self.run.is_none()
    }

    fn canvas(&mut self) -> &mut MemoryRenderBuffer {
        if self.canvas.is_none() {
            let (w, h) = (layout::LAYOUT.0 * SCALE, layout::LAYOUT.1 * SCALE);
            let mut data = vec![0u8; (w * h * 4) as usize];
            for px in data.chunks_exact_mut(4) {
                px.copy_from_slice(&PAPER);
            }
            self.canvas = Some(MemoryRenderBuffer::from_slice(&data, Fourcc::Abgr8888, (w, h), SCALE, Transform::Normal, None));
            // What was drawn before, read back and drawn again.
            self.marks = load();
            if !self.marks.is_empty() {
                tracing::info!("pen sheet: {} marks read back", self.marks.len());
                self.redraw();
            }
        }
        self.canvas.as_mut().expect("made")
    }

    /// A swipe left that began on the right panel's desktop.
    pub fn grab_from(&mut self, slot: TouchSlot, start: Point<f64, Logical>, time_us: u64) {
        let from = self.at(hybris_hwc::now_ns());
        self.run = None;
        self.canvas();
        self.grab = Some(Grab { slot, start_x: start.x, from, last: (time_us, start.x), velocity: 0.0 });
    }

    /// The strip's rectangle at a spread (logical, the screen's).
    fn strip_rect(spread: f64) -> Rectangle<f64, Logical> {
        let left = layout::panels()[0];
        let h = BUTTONS.len() as f64 * (BUTTON + BUTTON_GAP) - BUTTON_GAP + 2.0 * STRIP_PAD;
        let closed = (left.loc.x + left.size.w) as f64 - STRIP_W - STRIP_EDGE;
        let x = closed - Self::spread_span() * spread;
        Rectangle::new((x, (left.size.h as f64 - h) / 2.0).into(), (STRIP_W, h).into())
    }

    fn button_rect(spread: f64, i: usize) -> Rectangle<f64, Logical> {
        let s = Self::strip_rect(spread);
        let y = s.loc.y + STRIP_PAD + i as f64 * (BUTTON + BUTTON_GAP);
        Rectangle::new((s.loc.x + (STRIP_W - BUTTON) / 2.0, y).into(), (BUTTON, BUTTON).into())
    }

    fn button_at(&self, pos: Point<f64, Logical>) -> Option<Button> {
        (0..BUTTONS.len()).find(|&i| Self::button_rect(self.spread, i).contains(pos)).map(|i| BUTTONS[i])
    }

    /// A finger down while the sheet is out: on the strip, a button or the
    /// strip pulled; on the sheet spread over the left panel, nothing (the
    /// fingers do not draw, nor reach the desktop under it). False: the
    /// finger is not the sheet's (on the right panel it moves the ribbon).
    pub fn down(&mut self, slot: TouchSlot, pos: Point<f64, Logical>, time_us: u64) -> bool {
        if !self.open() {
            return false;
        }
        if Self::strip_rect(self.spread).contains(pos) {
            self.spread_run = None;
            self.strip_touch = Some(StripTouch { slot, button: self.button_at(pos), start_x: pos.x, from: self.spread, last: (time_us, pos.x), velocity: 0.0, pulling: false });
            return true;
        }
        self.spread > 0.0 && layout::panel_at(pos) == Some(0)
    }

    pub fn holds(&self, slot: TouchSlot) -> bool {
        self.grab.as_ref().is_some_and(|g| g.slot == slot) || self.strip_touch.as_ref().is_some_and(|t| t.slot == slot)
    }

    pub fn motion(&mut self, slot: TouchSlot, pos: Point<f64, Logical>, time_us: u64) {
        if let Some(t) = self.strip_touch.as_mut().filter(|t| t.slot == slot) {
            let dt = time_us.saturating_sub(t.last.0) as f64 / 1000.0;
            if dt > 0.0 {
                t.velocity = t.velocity * 0.4 + (pos.x - t.last.1) / dt * 0.6;
            }
            t.last = (time_us, pos.x);
            if !t.pulling && (pos.x - t.start_x).abs() > PULL {
                t.pulling = true;
                t.button = None;
            }
            return;
        }
        if let Some(g) = self.grab.as_mut().filter(|g| g.slot == slot) {
            let dt = time_us.saturating_sub(g.last.0) as f64 / 1000.0;
            if dt > 0.0 {
                g.velocity = g.velocity * 0.4 + (pos.x - g.last.1) / dt * 0.6;
            }
            g.last = (time_us, pos.x);
        }
    }

    pub fn up(&mut self, slot: TouchSlot) {
        if let Some(t) = self.strip_touch.take_if(|t| t.slot == slot) {
            if let Some(b) = t.button {
                self.button(b);
                return;
            }
            if t.pulling {
                // Pulled left spreads it, right gathers it back.
                let now = hybris_hwc::now_ns();
                let at = (t.from + (t.start_x - t.last.1) / Self::spread_span()).clamp(0.0, 1.0);
                let v = -t.velocity;
                let to = if v > FLICK { 1.0 } else if v < -FLICK { 0.0 } else if at >= 0.5 { 1.0 } else { 0.0 };
                self.spread_run = Some(Run { from: at, to, start_ns: now, duration_ns: (SPREAD_NS * (to - at).abs()).max(60e6) as u64 });
                self.spread = to;
                tracing::info!("pen sheet: {}", if to >= 1.0 { "over both panels" } else { "the right panel's" });
            }
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
        if self.strip_touch.as_ref().is_some_and(|t| t.pulling) {
            let slot = self.strip_touch.as_ref().map(|t| t.slot).expect("touch");
            self.up(slot);
        }
        self.strip_touch = None;
        if let Some(slot) = self.grab.as_ref().map(|g| g.slot) {
            self.up(slot);
        }
    }

    fn button(&mut self, b: Button) {
        tracing::info!("pen sheet: {b:?}");
        match b {
            Button::Undo => {
                if let Some(m) = self.marks.pop() {
                    self.undone.push(m);
                    self.redraw();
                    self.keep();
                }
            }
            Button::Redo => {
                if let Some(m) = self.undone.pop() {
                    self.draw_mark(&m);
                    self.marks.push(m);
                    self.keep();
                }
            }
            Button::Tool => self.eraser = !self.eraser,
            Button::Ink(i) => {
                self.ink = i;
                self.eraser = false;
            }
            Button::Clear => {
                if !self.marks.is_empty() {
                    self.marks.push(Mark::Clear);
                    self.undone.clear();
                    self.draw_mark(&Mark::Clear);
                    self.keep();
                }
            }
            Button::Save => self.save(),
        }
    }

    /// The marks written down, on a thread (~/.local/share/item/sheet).
    fn keep(&self) {
        let bytes = encode(&self.marks);
        std::thread::spawn(move || {
            let dir = sheet_dir();
            let _ = std::fs::create_dir_all(&dir);
            let tmp = dir.join("strokes.new");
            if std::fs::write(&tmp, bytes).is_ok() {
                let _ = std::fs::rename(&tmp, dir.join("strokes"));
            }
        });
    }

    /// The whole sheet drawn again from its marks (an undo, the start).
    fn redraw(&mut self) {
        self.fill_paper();
        // After the last clear only.
        let from = self.marks.iter().rposition(|m| matches!(m, Mark::Clear)).map_or(0, |i| i + 1);
        let marks: Vec<Mark> = self.marks[from..].to_vec();
        for m in &marks {
            self.draw_mark(m);
        }
    }

    fn draw_mark(&mut self, m: &Mark) {
        match m {
            Mark::Clear => self.fill_paper(),
            Mark::Stroke { ink, erase, pieces } => {
                for piece in pieces {
                    self.ink_now(piece, *erase, *ink);
                }
            }
        }
    }

    fn fill_paper(&mut self) {
        let (w, h) = (layout::LAYOUT.0 * SCALE, layout::LAYOUT.1 * SCALE);
        let Some(c) = &mut self.canvas else { return };
        let mut ctx = c.render();
        let _ = ctx.draw(|mem| {
            for px in mem.chunks_exact_mut(4) {
                px.copy_from_slice(&PAPER);
            }
            Ok::<_, ()>(vec![Rectangle::<i32, Buffer>::from_size((w, h).into())])
        });
    }

    /// The sheet as a PNG in ~/Pictures/Sketches, written on a thread: the
    /// right panel's part, or both panels' side by side when spread.
    fn save(&mut self) {
        let [left, right] = layout::panels();
        let s = SCALE;
        let (cw, h) = ((layout::LAYOUT.0 * s) as usize, (right.size.h * s) as usize);
        let parts: Vec<(usize, usize)> = if self.spread > 0.0 {
            vec![((left.loc.x * s) as usize, (left.size.w * s) as usize), ((right.loc.x * s) as usize, (right.size.w * s) as usize)]
        } else {
            vec![((right.loc.x * s) as usize, (right.size.w * s) as usize)]
        };
        let Some(c) = &mut self.canvas else { return };
        let mut copy = Vec::new();
        let mut ctx = c.render();
        let _ = ctx.draw(|mem| {
            copy = mem.to_vec();
            Ok::<_, ()>(vec![])
        });
        drop(ctx);
        std::thread::spawn(move || {
            let w: usize = parts.iter().map(|p| p.1).sum();
            let mut out = Vec::with_capacity(w * h * 4);
            for y in 0..h {
                for &(x0, pw) in &parts {
                    let i = (y * cw + x0) * 4;
                    out.extend_from_slice(&copy[i..i + pw * 4]);
                }
            }
            let dir = format!("{}/Pictures/Sketches", std::env::var("HOME").unwrap_or_default());
            let _ = std::fs::create_dir_all(&dir);
            let t = crate::shade::local_time();
            let path = format!("{dir}/sketch-{}-{:02}-{:02}-{:02}{:02}{:02}.png", t.tm_year + 1900, t.tm_mon + 1, t.tm_mday, t.tm_hour, t.tm_min, t.tm_sec);
            let saved = tiny_skia::IntSize::from_wh(w as u32, h as u32).and_then(|size| tiny_skia::Pixmap::from_vec(out, size)).map(|px| px.save_png(&path));
            match saved {
                Some(Ok(())) => tracing::info!("pen sheet: saved {path}"),
                other => tracing::warn!("pen sheet: not saved: {:?}", other.map(|r| r.err())),
            }
        });
    }

    /// Whether the pen's events are the sheet's: it is open and the pen is
    /// over it - the right panel, or both when spread.
    pub fn takes_pen(&self, pos: Point<f64, Logical>) -> bool {
        self.open() && (layout::panel_at(pos) == Some(1) || (self.spread >= 1.0 && layout::panel_at(pos) == Some(0)))
    }

    pub fn set_barrel(&mut self, held: bool) {
        self.barrel = held;
    }

    /// The ink a stroke begun now takes.
    fn ink_now_picked(&self) -> [u8; 4] {
        match self.ink {
            1 => crate::accent::get(),
            2 => RED,
            3 => BLUE,
            _ => BLACK,
        }
    }

    /// The pen's tip down: a button on the strip, or a stroke begun - a dot,
    /// a little thinner than the line will be (the tip tapers in).
    pub fn pen_down(&mut self, pos: Point<f64, Logical>, pressure: f64, eraser_end: bool) {
        if let Some(b) = self.button_at(pos) {
            self.button(b);
            return;
        }
        if Self::strip_rect(self.spread).contains(pos) {
            return;
        }
        let (x, y) = local(pos);
        let erase = eraser_end || self.barrel || self.eraser;
        let w = Self::width(pressure, erase);
        self.cost = (0, 0, 0, 0, hybris_hwc::now_ns());
        self.recent.clear();
        self.tip = None;
        self.reach.set((0.0, 0.0));
        self.stroke_ink = self.ink_now_picked();
        self.stroke_pieces.clear();
        // The line tapers in; the eraser is its whole width at once.
        let w0 = if erase { w } else { w * TAPER };
        self.eraser_at = erase.then_some((x, y));
        self.ink(&[(x, y, w0), (x, y, w0)], erase);
        self.stroke = Some((vec![(x, y, w0)], erase));
    }

    /// The tip ahead: from `from` (where the curve has got to) through `at`
    /// (the pen's last sample) and on as far as the pen goes in PREDICT_US at
    /// its speed - no further than PREDICT_MAX, none when it is slow - the
    /// width thinning toward its end.
    fn tip_ahead(&mut self, from: (f32, f32, f32), at: (f32, f32, f32)) -> Option<(f32, f32)> {
        let last = *self.recent.back()?;
        // The speed now (the last ~12 ms) and before (the ~12 ms before):
        // slowing, the tip shortens as much; turning, it is let go - else it
        // shot on past a turn or a stop.
        let velocity = |from_us: u64, to_us: u64| -> Option<(f32, f32)> {
            let span: Vec<_> = self.recent.iter().filter(|p| p.2 >= from_us && p.2 <= to_us).collect();
            let (a, b) = (span.first()?, span.last()?);
            let dt = b.2.saturating_sub(a.2) as f32;
            (dt > 0.0).then(|| ((b.0 - a.0) / dt, (b.1 - a.1) / dt))
        };
        let half = SPEED_SPAN_US / 2;
        let now_v = velocity(last.2.saturating_sub(half), last.2);
        let before_v = velocity(last.2.saturating_sub(SPEED_SPAN_US), last.2.saturating_sub(half));
        let mut pts = vec![from, at];
        if let Some((vx, vy)) = now_v {
            let speed = vx.hypot(vy);
            let keep = match before_v {
                Some((bx, by)) if speed > 0.0 && bx.hypot(by) > 0.0 => {
                    let before = bx.hypot(by);
                    let cos = (vx * bx + vy * by) / (speed * before);
                    (speed / before).min(1.0) * ((cos - TURN_COS) / (1.0 - TURN_COS)).clamp(0.0, 1.0)
                }
                _ => 0.0,
            };
            let (mut dx, mut dy) = (vx * PREDICT_US * keep, vy * PREDICT_US * keep);
            let len = dx.hypot(dy);
            if len > PREDICT_MAX {
                (dx, dy) = (dx * PREDICT_MAX / len, dy * PREDICT_MAX / len);
            }
            // Eased from the last reach: the samples' speed is noisy.
            let (rx, ry) = self.reach.get();
            let (dx, dy) = (rx + (dx - rx) * REACH_EASE, ry + (dy - ry) * REACH_EASE);
            self.reach.set((dx, dy));
            if dx.hypot(dy) > PREDICT_MIN {
                pts.push((at.0 + dx, at.1 + dy, at.2 * TAPER));
            }
        }
        let wmax = pts.iter().map(|p| p.2).fold(0.0, f32::max);
        let pad = wmax / 2.0 + 2.0;
        let (bx, by) = (pts.iter().map(|p| p.0).fold(f32::MAX, f32::min) - pad, pts.iter().map(|p| p.1).fold(f32::MAX, f32::min) - pad);
        let (ex, ey) = (pts.iter().map(|p| p.0).fold(f32::MIN, f32::max) + pad, pts.iter().map(|p| p.1).fold(f32::MIN, f32::max) + pad);
        if ex - bx > TIP_PX as f32 || ey - by > TIP_PX as f32 {
            return None;
        }
        // To the pen, whole; the guess ahead of it fades out and thins, so
        // its small jumps hardly show.
        let mut pieces = vec![(pts[0], pts[1], 1.0f32)];
        if let Some(&end) = pts.get(2) {
            let k = FADE_PIECES as f32;
            for i in 0..FADE_PIECES {
                let (t0, t1) = (i as f32 / k, (i + 1) as f32 / k);
                let lerp = |t: f32| (at.0 + (end.0 - at.0) * t, at.1 + (end.1 - at.1) * t, at.2 + (end.2 - at.2) * t);
                pieces.push((lerp(t0), lerp(t1), (1.0 - (t0 + t1) / 2.0) * FADE_FROM));
            }
        }
        // Into the one buffer kept for it: the last tip cleared, this one
        // drawn, only the two's rectangle uploaded - a new texture each move
        // cost an allocation and a whole upload a frame.
        let ink = self.stroke_ink;
        let old = self.tip_rect.take();
        let (w, h) = ((ex - bx).ceil() as i32, (ey - by).ceil() as i32);
        let buf = self.tip_buf.get_or_insert_with(|| MemoryRenderBuffer::new(Fourcc::Abgr8888, (TIP_PX, TIP_PX), SCALE, Transform::Normal, None));
        let mut ctx = buf.render();
        let _ = ctx.draw(|mem| {
            let Some(mut pm) = tiny_skia::PixmapMut::from_bytes(mem, TIP_PX as u32, TIP_PX as u32) else { return Ok::<_, ()>(vec![]) };
            if let Some((ox, oy, ow, oh)) = old {
                if let Some(r) = tiny_skia::Rect::from_xywh(ox as f32, oy as f32, ow as f32, oh as f32) {
                    let mut clear = tiny_skia::Paint::default();
                    clear.blend_mode = tiny_skia::BlendMode::Clear;
                    pm.fill_rect(r, &clear, tiny_skia::Transform::identity(), None);
                }
            }
            let mut paint = tiny_skia::Paint::default();
            paint.anti_alias = true;
            for ((x0, y0, w0), (x1, y1, w1), a) in pieces {
                paint.set_color_rgba8(ink[0], ink[1], ink[2], (ink[3] as f32 * a) as u8);
                let mut pb = tiny_skia::PathBuilder::new();
                pb.move_to(x0 - bx, y0 - by);
                pb.line_to(x1 - bx + if x0 == x1 && y0 == y1 { 0.01 } else { 0.0 }, y1 - by);
                let Some(path) = pb.finish() else { continue };
                // The fading pieces end flat: round ends would overlap,
                // darker at each joint.
                let cap = if a < 1.0 { tiny_skia::LineCap::Butt } else { tiny_skia::LineCap::Round };
                let stroke = tiny_skia::Stroke { width: (w0 + w1) / 2.0, line_cap: cap, line_join: tiny_skia::LineJoin::Round, ..Default::default() };
                pm.stroke_path(&path, &paint, &stroke, tiny_skia::Transform::identity(), None);
            }
            let (ow, oh) = old.map_or((0, 0), |o| (o.2, o.3));
            Ok(vec![Rectangle::<i32, Buffer>::new((0, 0).into(), (w.max(ow).min(TIP_PX), h.max(oh).min(TIP_PX)).into())])
        });
        drop(ctx);
        self.tip_rect = Some((0, 0, w, h));
        Some((bx, by))
    }

    /// The line's width at a pressure (physical px).
    fn width(pressure: f64, erase: bool) -> f32 {
        if erase { ERASER } else { THIN + (THICK - THIN) * pressure.clamp(0.0, 1.0) as f32 }
    }

    /// The pen moved: the stroke goes on as a curve through the pen's own
    /// samples (Catmull-Rom: each piece from one sample to the next, shaped
    /// by those either side), its width eased toward the pressure's, so
    /// neither the line nor its width steps at the samples. A piece is drawn
    /// once the sample after it is known; the tip covers the rest, to the
    /// pen, and since both pass through the same samples the tip never
    /// shows a corner the line then cuts. A sample too near the last is left
    /// out (the tip's tremble).
    pub fn pen_motion(&mut self, pos: Point<f64, Logical>, pressure: f64, eraser_end: bool, time_us: u64) {
        let Some((mut pts, erase)) = self.stroke.take() else { return };
        let (x, y) = local(pos);
        let &(lx, ly, lw) = pts.last().expect("a point");
        if (x - lx).hypot(y - ly) < MIN_STEP {
            self.stroke = Some((pts, erase));
            return;
        }
        // The eraser goes straight to the pen at once: no curve to wait a
        // sample for, nothing guessed ahead - what it takes is gone under
        // the pen, its ring round it.
        if erase {
            self.ink(&[(lx, ly, ERASER), (x, y, ERASER)], true);
            self.eraser_at = Some((x, y));
            self.stroke = Some((vec![(x, y, ERASER)], erase));
            return;
        }
        let w = lw + (Self::width(pressure, eraser_end) - lw) * WIDTH_EASE;
        pts.push((x, y, w));
        let n = pts.len();
        // The piece from the last-but-one sample to the one before it: the
        // first stroke's start stands in for the sample before it.
        if n >= 3 {
            let p0 = if n >= 4 { pts[n - 4] } else { pts[n - 3] };
            let line = spline(p0, pts[n - 3], pts[n - 2], pts[n - 1]);
            self.ink(&line, erase);
        }
        // Its speed from the last ~25 ms of samples: the tip ahead.
        self.recent.push_back((x, y, time_us));
        while self.recent.len() > 2 && self.recent.front().is_some_and(|f| time_us.saturating_sub(f.2) > SPEED_SPAN_US) {
            self.recent.pop_front();
        }
        self.tip = if erase { None } else { self.tip_ahead(pts[n - 2], (x, y, w)) };
        if pts.len() > 4 {
            pts.remove(0);
        }
        self.stroke = Some((pts, erase));
    }

    /// The tip lifted: the stroke's end, to the last sample, tapering out;
    /// the stroke kept (undo, the file).
    pub fn pen_up(&mut self) {
        self.tip = None;
        self.eraser_at = None;
        self.recent.clear();
        let (events, pieces, ns, worst, at) = self.cost;
        if at > 0 {
            let long = (hybris_hwc::now_ns() - at) as f64 / 1e9;
            let (frames, ens, emax) = self.elements_cost.take();
            tracing::info!("pen: a stroke: {events} events ({:.0}/s), {pieces} pieces, drawing {:.1} ms in all, {:.2} ms the longest; the sheet's elements {:.2} ms a frame, {:.2} the most ({frames} frames)", events as f64 / long.max(1e-3), ns as f64 / 1e6, worst as f64 / 1e6, ens as f64 / 1e6 / frames.max(1) as f64, emax as f64 / 1e6);
        }
        if let Some((pts, erase)) = self.stroke.take() {
            let n = pts.len();
            if n >= 2 {
                // The last piece, to the last sample, tapering out.
                let p0 = if n >= 3 { pts[n - 3] } else { pts[n - 2] };
                let (p1, p2) = (pts[n - 2], pts[n - 1]);
                self.ink(&spline(p0, p1, (p2.0, p2.1, p2.2 * TAPER), (p2.0, p2.1, p2.2 * TAPER)), erase);
            }
            let pieces = std::mem::take(&mut self.stroke_pieces);
            if !pieces.is_empty() {
                self.marks.push(Mark::Stroke { ink: self.stroke_ink, erase, pieces });
                self.undone.clear();
                self.keep();
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
            self.pen_motion(at(k), k, false, i as u64 * 4000);
        }
        self.pen_up();
        self.pen_down(at(0.7) + Point::from((0.0, -150.0)), 1.0, true);
        self.pen_motion(at(0.7) + Point::from((0.0, 150.0)), 1.0, true, 900_000);
        self.pen_up();
    }

    /// A run of a stroke into the sheet - kept with the stroke - and timed.
    fn ink(&mut self, pts: &[(f32, f32, f32)], erase: bool) {
        if pts.len() < 2 {
            return;
        }
        let t = hybris_hwc::now_ns();
        self.ink_now(pts, erase, self.stroke_ink);
        self.stroke_pieces.push(pts.to_vec());
        let d = hybris_hwc::now_ns() - t;
        let (e, p, ns, worst, at) = self.cost;
        self.cost = (e + 1, p + pts.len() as u32 - 1, ns + d, worst.max(d), at);
    }

    /// A run into the sheet's memory - short pieces, each with its own
    /// width between its ends', round-capped so they join; only their
    /// bounds are damage.
    fn ink_now(&mut self, pts: &[(f32, f32, f32)], erase: bool, ink: [u8; 4]) {
        let (w, h) = ((layout::LAYOUT.0 * SCALE) as u32, (layout::LAYOUT.1 * SCALE) as u32);
        let canvas = self.canvas();
        let mut ctx = canvas.render();
        let _ = ctx.draw(|mem| {
            let Some(mut pm) = tiny_skia::PixmapMut::from_bytes(mem, w, h) else { return Ok::<_, ()>(vec![]) };
            let mut paint = tiny_skia::Paint::default();
            paint.anti_alias = true;
            let c = if erase { PAPER } else { ink };
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
        let mut moving = self.grab.is_some() || self.strip_touch.as_ref().is_some_and(|t| t.pulling);
        if let Some(r) = &self.run {
            if frame_ns >= r.start_ns + r.duration_ns {
                self.run = None;
                // One frame more: the dock learns the panel is free (or
                // taken) only on a frame.
                moving = true;
            } else {
                moving = true;
            }
        }
        if let Some(r) = &self.spread_run {
            moving = true;
            if frame_ns >= r.start_ns + r.duration_ns {
                self.spread_run = None;
            }
        }
        self.tell();
        moving
    }

    pub fn warm_up(&self, renderer: &mut GlesRenderer) -> usize {
        let on = self.button_on.get();
        self.icons
            .iter()
            .flatten()
            .chain(self.eraser_icon.iter())
            .chain([&on, &self.ring])
            .chain(self.canvas.iter())
            .filter(|b| MemoryRenderBufferRenderElement::from_buffer(renderer, (0.0, 0.0), b, None, None, None, Kind::Unspecified).is_ok())
            .count()
    }

    pub fn elements(&self, renderer: &mut GlesRenderer, frame_ns: u64) -> Vec<ShellElement> {
        self.drawn_at(renderer, self.at(frame_ns), self.spread_at(frame_ns))
    }

    /// The sheet all the way out, wherever it is: for the ribbon to carry.
    pub fn picture(&self, renderer: &mut GlesRenderer) -> Vec<ShellElement> {
        self.drawn_at(renderer, 1.0, self.spread)
    }

    /// Its canvas made, for the ribbon to show it as it comes.
    pub fn ready(&mut self) {
        self.canvas();
    }

    /// The canvas, to be sent to the GPU ahead of its first frame.
    pub fn canvas_buffer(&self) -> Option<&MemoryRenderBuffer> {
        self.canvas.as_ref()
    }

    /// The inks' dots, in the accent of now.
    fn dots(&self) -> Vec<MemoryRenderBuffer> {
        let mut d = self.dots.borrow_mut();
        if d.0 != crate::accent::version() || d.1.is_empty() {
            let size = BUTTON * 0.5;
            *d = (crate::accent::version(), [BLACK, crate::accent::get(), RED, BLUE].iter().map(|c| crate::grid::rounded(size, size, size / 2.0, if *c == BLACK { [200, 200, 204, 255] } else { *c })).collect());
        }
        d.1.clone()
    }

    fn drawn_at(&self, renderer: &mut GlesRenderer, p: f64, spread: f64) -> Vec<ShellElement> {
        let t = hybris_hwc::now_ns();
        let out = self.drawn_at_inner(renderer, p, spread);
        if self.stroke.is_some() {
            let d = hybris_hwc::now_ns() - t;
            let (f, ns, max) = self.elements_cost.get();
            self.elements_cost.set((f + 1, ns + d, max.max(d)));
        }
        out
    }

    fn drawn_at_inner(&self, renderer: &mut GlesRenderer, p: f64, spread: f64) -> Vec<ShellElement> {
        let Some(canvas) = &self.canvas else { return Vec::new() };
        if p <= 0.0 {
            return Vec::new();
        }
        let [left, right] = layout::panels();
        // It slides in from the right edge with the finger.
        let dx = right.size.w as f64 * (1.0 - p);
        let s = SCALE as f64;
        let mut out = Vec::new();
        // Only the part of its buffer the tip took: the whole buffer drew,
        // and damaged, far more of the screen than the tip.
        let mut tip = None;
        if let (Some((tx, ty)), Some(buf), Some((_, _, w, h))) = (&self.tip, &self.tip_buf, self.tip_rect) {
            let src = Rectangle::<f64, Logical>::new((0.0, 0.0).into(), (w as f64 / s, h as f64 / s).into());
            let size = Size::<i32, Logical>::from(((w as f64 / s).ceil() as i32, (h as f64 / s).ceil() as i32));
            if let Ok(e) = MemoryRenderBufferRenderElement::from_buffer(renderer, ((*tx as f64 + dx * s).round(), (*ty as f64).round()), buf, None, Some(src), Some(size), Kind::Unspecified) {
                tip = Some(ShellElement::Text(e));
            }
        }
        // The canvas's part on a panel from x0 to x1 (logical).
        let part = |renderer: &mut GlesRenderer, x0: f64, x1: f64| -> Option<ShellElement> {
            if x1 - x0 < 1.0 {
                return None;
            }
            let src = Rectangle::<f64, Logical>::new((x0, 0.0).into(), (x1 - x0, right.size.h as f64).into());
            let size = Size::<i32, Logical>::from(((x1 - x0).round() as i32, right.size.h));
            MemoryRenderBufferRenderElement::from_buffer(renderer, (((x0 + dx) * s).round(), 0.0), canvas, None, Some(src), Some(size), Kind::Unspecified).ok().map(ShellElement::Text)
        };
        let strip = Self::strip_rect(spread);
        let put = |out: &mut Vec<ShellElement>, renderer: &mut GlesRenderer, b: &MemoryRenderBuffer, x: f64, y: f64| {
            if let Ok(e) = MemoryRenderBufferRenderElement::from_buffer(renderer, (((x + dx) * s).round(), (y * s).round()), b, None, None, None, Kind::Unspecified) {
                out.push(ShellElement::Text(e));
            }
        };
        // The strip: its buttons over its glass.
        let dots = self.dots();
        let on = self.button_on.get();
        for (i, b) in BUTTONS.iter().enumerate() {
            let r = Self::button_rect(spread, i);
            match b {
                Button::Ink(k) => {
                    let d = BUTTON * 0.5;
                    put(&mut out, renderer, &dots[*k], r.loc.x + (BUTTON - d) / 2.0, r.loc.y + (BUTTON - d) / 2.0);
                    if *k == self.ink && !self.eraser {
                        put(&mut out, renderer, &self.ring, r.loc.x, r.loc.y);
                    }
                }
                _ => {
                    let icon = if *b == Button::Tool && self.eraser { self.eraser_icon.as_ref() } else { self.icons[i].as_ref() };
                    if let Some(ic) = icon {
                        put(&mut out, renderer, ic, r.loc.x + (BUTTON - 24.0) / 2.0, r.loc.y + (BUTTON - 24.0) / 2.0);
                    }
                    let lit = *b == Button::Tool && self.eraser || self.strip_touch.as_ref().is_some_and(|t| t.button == Some(*b));
                    if lit {
                        put(&mut out, renderer, &on, r.loc.x, r.loc.y);
                    }
                }
            }
        }
        put(&mut out, renderer, &self.strip_bg, strip.loc.x, strip.loc.y);
        // The stroke's tip ahead of the pen, over the sheet.
        out.extend(tip);
        if let Some((ex, ey)) = self.eraser_at {
            let r = ERASER as f64 / 2.0 + 2.0;
            if let Ok(e) = MemoryRenderBufferRenderElement::from_buffer(renderer, ((ex as f64 - r + dx * s).round(), (ey as f64 - r).round()), &self.eraser_ring, None, None, None, Kind::Unspecified) {
                out.insert(0, ShellElement::Text(e));
            }
        }
        // The right panel's part, and the left's as far as the strip has
        // gone (from its right edge to the hinge).
        if let Some(e) = part(renderer, right.loc.x as f64, (right.loc.x + right.size.w) as f64) {
            out.push(e);
        }
        if spread > 0.0 {
            if let Some(e) = part(renderer, strip.loc.x + STRIP_W + STRIP_EDGE, (left.loc.x + left.size.w) as f64) {
                out.push(e);
            }
        }
        out
    }
}

fn run_at(r: &Run, frame_ns: u64) -> f64 {
    let k = (frame_ns.saturating_sub(r.start_ns) as f64 / r.duration_ns.max(1) as f64).clamp(0.0, 1.0);
    r.from + (r.to - r.from) * (1.0 - (1.0 - k).powi(3))
}

/// A point of the screen in the sheet's pixels (it is as wide as the screen).
fn local(pos: Point<f64, Logical>) -> (f32, f32) {
    ((pos.x * SCALE as f64) as f32, (pos.y * SCALE as f64) as f32)
}

/// The ring round the ink picked.
fn ring(size: f64) -> MemoryRenderBuffer {
    let px = (size * SCALE as f64) as u32;
    let mut pm = tiny_skia::Pixmap::new(px, px).expect("ring");
    let mut paint = tiny_skia::Paint::default();
    paint.anti_alias = true;
    paint.set_color_rgba8(255, 255, 255, 220);
    let c = px as f32 / 2.0;
    if let Some(path) = tiny_skia::PathBuilder::from_circle(c, c, c - 2.0 * SCALE as f32) {
        pm.stroke_path(&path, &paint, &tiny_skia::Stroke { width: 2.0 * SCALE as f32, ..Default::default() }, tiny_skia::Transform::identity(), None);
    }
    MemoryRenderBuffer::from_slice(pm.data(), Fourcc::Abgr8888, (px as i32, px as i32), SCALE, Transform::Normal, None)
}

/// The eraser's ring: its size, a dark line in a light one, seen on the
/// paper and on the ink alike.
fn eraser_ring() -> MemoryRenderBuffer {
    let px = (ERASER + 4.0).ceil() as u32;
    let mut pm = tiny_skia::Pixmap::new(px, px).expect("ring");
    let c = px as f32 / 2.0;
    for (radius, width, rgba) in [(ERASER / 2.0, 3.0, [255, 255, 255, 200]), (ERASER / 2.0, 1.2, [60, 60, 66, 220])] {
        let mut paint = tiny_skia::Paint::default();
        paint.anti_alias = true;
        paint.set_color_rgba8(rgba[0], rgba[1], rgba[2], rgba[3]);
        if let Some(path) = tiny_skia::PathBuilder::from_circle(c, c, radius) {
            pm.stroke_path(&path, &paint, &tiny_skia::Stroke { width, ..Default::default() }, tiny_skia::Transform::identity(), None);
        }
    }
    MemoryRenderBuffer::from_slice(pm.data(), Fourcc::Abgr8888, (px as i32, px as i32), SCALE, Transform::Normal, None)
}

fn sheet_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".local/share/item/sheet")
}

/// The marks as bytes: per mark its kind (0 a stroke, 1 a clear), and a
/// stroke's ink, whether it erased, its pieces, each its points (x, y,
/// width as f32s), all little-endian.
fn encode(marks: &[Mark]) -> Vec<u8> {
    let mut b = b"ITEMSHEET1".to_vec();
    for m in marks {
        match m {
            Mark::Clear => b.push(1),
            Mark::Stroke { ink, erase, pieces } => {
                b.push(0);
                b.extend_from_slice(ink);
                b.push(*erase as u8);
                b.extend_from_slice(&(pieces.len() as u32).to_le_bytes());
                for piece in pieces {
                    b.extend_from_slice(&(piece.len() as u32).to_le_bytes());
                    for &(x, y, w) in piece {
                        for v in [x, y, w] {
                            b.extend_from_slice(&v.to_le_bytes());
                        }
                    }
                }
            }
        }
    }
    b
}

/// The marks read back, as many as are whole.
fn load() -> Vec<Mark> {
    let Ok(b) = std::fs::read(sheet_dir().join("strokes")) else { return Vec::new() };
    let Some(mut r) = b.strip_prefix(b"ITEMSHEET1") else { return Vec::new() };
    let mut marks = Vec::new();
    fn take<'a>(r: &mut &'a [u8], n: usize) -> Option<&'a [u8]> {
        if r.len() < n {
            return None;
        }
        let (a, rest) = r.split_at(n);
        *r = rest;
        Some(a)
    }
    let u32_of = |s: &[u8]| u32::from_le_bytes([s[0], s[1], s[2], s[3]]) as usize;
    loop {
        let Some(kind) = take(&mut r, 1) else { break };
        if kind[0] == 1 {
            marks.push(Mark::Clear);
            continue;
        }
        let Some(head) = take(&mut r, 9) else { break };
        let (ink, erase, n) = ([head[0], head[1], head[2], head[3]], head[4] != 0, u32_of(&head[5..9]));
        let mut pieces = Vec::with_capacity(n);
        for _ in 0..n {
            let Some(c) = take(&mut r, 4) else { return marks };
            let k = u32_of(c);
            let Some(data) = take(&mut r, k * 12) else { return marks };
            let f = |i: usize| f32::from_le_bytes([data[i], data[i + 1], data[i + 2], data[i + 3]]);
            pieces.push((0..k).map(|j| (f(j * 12), f(j * 12 + 4), f(j * 12 + 8))).collect());
        }
        marks.push(Mark::Stroke { ink, erase, pieces });
    }
    marks
}

/// A Catmull-Rom piece from `p1` to `p2`, shaped by `p0` before and `p3`
/// after, as short pieces, the width going from `p1`'s to `p2`'s.
fn spline(p0: (f32, f32, f32), p1: (f32, f32, f32), p2: (f32, f32, f32), p3: (f32, f32, f32)) -> Vec<(f32, f32, f32)> {
    let len = (p2.0 - p1.0).hypot(p2.1 - p1.1);
    let n = ((len / PIECE).ceil() as usize).clamp(1, 64);
    let at = |a: f32, b: f32, c: f32, d: f32, t: f32| {
        let (t2, t3) = (t * t, t * t * t);
        0.5 * (2.0 * b + (c - a) * t + (2.0 * a - 5.0 * b + 4.0 * c - d) * t2 + (3.0 * b - a - 3.0 * c + d) * t3)
    };
    (0..=n)
        .map(|i| {
            let t = i as f32 / n as f32;
            (at(p0.0, p1.0, p2.0, p3.0, t), at(p0.1, p1.1, p2.1, p3.1, t), p1.2 + (p2.2 - p1.2) * t)
        })
        .collect()
}

/// The port's sfduo-pen-split (item's pen-split), which owns the digitizer,
/// told where the pen is the sheet's: 0 nowhere, 1 over the right panel, 2
/// over both panels (spread). Only there does it give the pen as a tablet
/// tool (pressure, the eraser end); elsewhere the pen stays one more finger,
/// so the shell's swipes take it. Never told, it kept the pen a finger and
/// the sheet did not draw.
fn tell_pen_split(level: u8) {
    let sock = std::os::unix::net::UnixDatagram::unbound();
    if let Ok(s) = sock {
        if s.send_to(level.to_string().as_bytes(), "/run/sfduo-pen.sock").is_err() {
            tracing::debug!("pen: sfduo-pen-split not listening");
        }
    }
}
