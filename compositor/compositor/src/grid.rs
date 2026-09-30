//! The app grid, item's "all apps": every app in five columns on black, on
//! the panel it was swiped up on (sfduo-dock's GridPanel).
//!
//! - A swipe up anywhere on an empty panel raises it, following the finger:
//!   55 % of the panel's height of travel opens it all the way. A swipe down
//!   there pulls the shade instead (item's desktop catcher).
//! - Let go: faster than 0.3 px/ms up opens, down closes; otherwise open
//!   past 0.3. The rest runs 220 ms × what is left, easing out.
//! - A swipe down on it closes it the same way; a tap on empty space closes
//!   it; a tap on an app closes it and launches the app onto its panel (a
//!   window of the app put away comes back instead).
//! - While it is up its panel counts as taken: the dock leaves it.
//!
//! The grid is drawn once at start into one texture, icons and names, and
//! only moved after. item's sizes: 5 columns, icons 48, names 12 px
//! #e8e4d9 cut at 14 characters, cells 10 px above and below, 6 between,
//! the pressed cell lit rgba(255,255,255,0.12) with 16 px corners.

use resvg::tiny_skia;
use smithay::backend::allocator::Fourcc;
use smithay::backend::input::TouchSlot;
use smithay::backend::renderer::element::memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement};
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::utils::{Logical, Point, Rectangle, Transform};

use crate::apps::{self, Entry};
use crate::layout::{self, SCALE};
use crate::shade::ShellElement;
use crate::text::Font;

const COLUMNS: usize = 5;
const ICON: f64 = 48.0;
const PAD_X: f64 = 14.0;
/// Where the first row starts, below the shade's edge.
const TOP: f64 = 48.0;
const CELL_PAD: f64 = 10.0;
const LABEL_GAP: f64 = 6.0;
const LABEL_SIZE: f32 = 12.0;
const LABEL_H: f64 = 16.0;
const SPACING: f64 = 6.0;
const LABEL_CHARS: usize = 14;
const LABEL: [f32; 4] = [0xe8 as f32 / 255.0, 0xe4 as f32 / 255.0, 0xd9 as f32 / 255.0, 1.0];
/// 55 % of the panel's height of travel opens it (item's GRID_TRAVEL).
const TRAVEL: f64 = 0.55;
const FLICK: f64 = 0.3;
const COMMIT: f64 = 0.3;
const SLIDE_NS: f64 = 220e6;
/// Travel that makes a touch a swipe (item's SWIPE_PX).
const SWIPE: f64 = 12.0;

/// What a touch on the grid, or on an empty panel, asks for.
pub enum Ask {
    Nothing,
    /// Pull the shade: the touch began here.
    Shade(Point<f64, Logical>),
    /// Bring the system screen: the touch began here.
    System(Point<f64, Logical>),
    /// Bring the pen's sheet: the touch began here.
    Pen(Point<f64, Logical>),
    /// Launch (or bring back) an app onto a panel.
    Launch { exec: String, ids: Vec<String>, icon: Option<String>, panel: usize },
}

/// A touch not yet a swipe or a tap.
struct Pending {
    slot: TouchSlot,
    start: Point<f64, Logical>,
    panel: usize,
    on_grid: bool,
    cell: Option<usize>,
}

struct Grab {
    slot: TouchSlot,
    start_y: f64,
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

pub struct Grid {
    entries: Vec<Entry>,
    /// Each app's cell, from the grid's top left, logical px.
    cells: Vec<Rectangle<f64, Logical>>,
    texture: Option<MemoryRenderBuffer>,
    highlight: MemoryRenderBuffer,
    dot: MemoryRenderBuffer,
    panel: usize,
    /// How far up it stands at rest (0 down, 1 up).
    p: f64,
    pending: Option<Pending>,
    grab: Option<Grab>,
    run: Option<Run>,
    pressed: Option<usize>,
}

impl Grid {
    pub fn new() -> Grid {
        let entries = apps::all();
        let (w, h) = (layout::panels()[0].size.w as f64, layout::LAYOUT.1 as f64);
        let cell_w = (w - 2.0 * PAD_X) / COLUMNS as f64;
        let cell_h = CELL_PAD + ICON + LABEL_GAP + LABEL_H + CELL_PAD;
        let cells: Vec<Rectangle<f64, Logical>> = (0..entries.len())
            .map(|i| {
                let (col, row) = ((i % COLUMNS) as f64, (i / COLUMNS) as f64);
                Rectangle::new((PAD_X + col * cell_w, TOP + row * (cell_h + SPACING)).into(), (cell_w, cell_h).into())
            })
            .collect();
        let texture = draw(&entries, &cells, w, h);
        tracing::info!("grid: {} apps", entries.len());
        let highlight = rounded(cell_w, cell_h, 16.0, [255, 255, 255, 31]);
        Grid { entries, cells, texture, highlight, dot: running_dot(), panel: 0, p: 0.0, pending: None, grab: None, run: None, pressed: None }
    }

    fn at(&self, frame_ns: u64) -> f64 {
        if let Some(g) = &self.grab {
            return (g.from - (g.last.1 - g.start_y) / (layout::LAYOUT.1 as f64 * TRAVEL)).clamp(0.0, 1.0);
        }
        match &self.run {
            Some(r) => {
                let k = (frame_ns.saturating_sub(r.start_ns) as f64 / r.duration_ns.max(1) as f64).clamp(0.0, 1.0);
                r.from + (r.to - r.from) * (1.0 - (1.0 - k).powi(3))
            }
            None => self.p,
        }
    }

    /// The panel it holds while up: the dock leaves it.
    pub fn panel(&self) -> Option<usize> {
        (self.p > 0.0 || self.grab.is_some() || self.run.is_some()).then_some(self.panel)
    }

    fn run_to(&mut self, to: f64, now_ns: u64) {
        let from = self.at(now_ns);
        self.grab = None;
        self.run = Some(Run { from, to, start_ns: now_ns, duration_ns: (SLIDE_NS * (to - from).abs()).max(40e6) as u64 });
        self.p = to;
    }

    /// Opens or closes it on a panel (not from a finger).
    pub fn close(&mut self, now_ns: u64) {
        if self.panel().is_some() {
            self.run_to(0.0, now_ns);
        }
    }

    /// The cell under a point, with the grid up on its panel.
    fn cell_at(&self, pos: Point<f64, Logical>) -> Option<usize> {
        let origin = layout::panels()[self.panel].loc.to_f64();
        let local = pos - origin;
        self.cells.iter().position(|c| c.contains(local))
    }

    /// A touch down. `empty`: whether the panel under it has no window.
    /// Returns whether the grid takes it.
    pub fn down(&mut self, slot: TouchSlot, pos: Point<f64, Logical>, empty: bool) -> bool {
        let Some(panel) = layout::panel_at(pos) else { return false };
        let up = self.panel() == Some(panel);
        if !up && !empty {
            return false;
        }
        let cell = if up && self.run.is_none() { self.cell_at(pos) } else { None };
        self.pressed = cell;
        self.pending = Some(Pending { slot, start: pos, panel, on_grid: up, cell });
        true
    }

    pub fn holds(&self, slot: TouchSlot) -> bool {
        self.pending.as_ref().is_some_and(|p| p.slot == slot) || self.grab.as_ref().is_some_and(|g| g.slot == slot)
    }

    pub fn motion(&mut self, slot: TouchSlot, pos: Point<f64, Logical>, time_us: u64) -> Ask {
        if let Some(g) = self.grab.as_mut().filter(|g| g.slot == slot) {
            let dt = time_us.saturating_sub(g.last.0) as f64 / 1000.0;
            if dt > 0.0 {
                g.velocity = g.velocity * 0.4 + (pos.y - g.last.1) / dt * 0.6;
            }
            g.last = (time_us, pos.y);
            return Ask::Nothing;
        }
        let Some(p) = self.pending.as_ref().filter(|p| p.slot == slot) else { return Ask::Nothing };
        let (dx, dy) = (pos.x - p.start.x, pos.y - p.start.y);
        if dx.abs() < SWIPE && dy.abs() < SWIPE {
            return Ask::Nothing;
        }
        let p = self.pending.take().unwrap();
        self.pressed = None;
        if dy.abs() <= dx.abs() {
            // Right on the left panel's desktop: the system screen; left on
            // the right one's: the pen's sheet.
            return match (p.on_grid, p.panel, dx > 0.0) {
                (false, 0, true) => Ask::System(p.start),
                (false, 1, false) => Ask::Pen(p.start),
                _ => Ask::Nothing,
            };
        }
        let now = hybris_hwc::now_ns();
        match (p.on_grid, dy < 0.0) {
            // Up on an empty panel: the grid comes up with the finger.
            (false, true) => {
                self.panel = p.panel;
                self.run = None;
                self.grab = Some(Grab { slot, start_y: p.start.y, from: 0.0, last: (time_us, pos.y), velocity: 0.0 });
                Ask::Nothing
            }
            // Down on an empty panel: the shade.
            (false, false) => Ask::Shade(p.start),
            // Down on the grid: it goes down with the finger.
            (true, false) => {
                let from = self.at(now);
                self.run = None;
                self.grab = Some(Grab { slot, start_y: p.start.y, from, last: (time_us, pos.y), velocity: 0.0 });
                Ask::Nothing
            }
            (true, true) => Ask::Nothing,
        }
    }

    pub fn up(&mut self, slot: TouchSlot) -> Ask {
        let now = hybris_hwc::now_ns();
        if self.grab.as_ref().is_some_and(|g| g.slot == slot) {
            let v = self.grab.as_ref().unwrap().velocity;
            let p = self.at(now);
            let open = if v < -FLICK { true } else if v > FLICK { false } else { p >= COMMIT };
            self.run_to(if open { 1.0 } else { 0.0 }, now);
            return Ask::Nothing;
        }
        let Some(p) = self.pending.take_if(|p| p.slot == slot) else { return Ask::Nothing };
        self.pressed = None;
        if !p.on_grid {
            return Ask::Nothing;
        }
        // A tap on the grid: an app launches, empty space closes it.
        self.run_to(0.0, now);
        match p.cell {
            Some(i) => {
                let e = &self.entries[i];
                Ask::Launch { exec: e.exec.clone(), ids: e.ids.clone(), icon: e.icon.clone(), panel: self.panel }
            }
            None => Ask::Nothing,
        }
    }

    pub fn cancel(&mut self) {
        self.pending = None;
        self.pressed = None;
        if self.grab.is_some() {
            let open = self.at(hybris_hwc::now_ns()) >= COMMIT;
            self.run_to(if open { 1.0 } else { 0.0 }, hybris_hwc::now_ns());
        }
    }

    /// After a frame for `frame_ns`: whether it is still moving.
    pub fn settle(&mut self, frame_ns: u64) -> bool {
        if self.grab.is_some() {
            return true;
        }
        match &self.run {
            Some(r) if frame_ns >= r.start_ns + r.duration_ns => {
                self.run = None;
                true
            }
            Some(_) => true,
            None => false,
        }
    }

    pub fn warm_up(&self, renderer: &mut GlesRenderer) -> usize {
        [self.texture.as_ref(), Some(&self.highlight), Some(&self.dot)]
            .into_iter()
            .flatten()
            .filter(|b| MemoryRenderBufferRenderElement::from_buffer(renderer, (0.0, 0.0), b, None, None, None, Kind::Unspecified).is_ok())
            .count()
    }

    pub fn elements(&self, renderer: &mut GlesRenderer, frame_ns: u64, running: &[String]) -> Vec<ShellElement> {
        let p = self.at(frame_ns);
        let Some(texture) = &self.texture else { return Vec::new() };
        if p <= 0.0 {
            return Vec::new();
        }
        let panel = layout::panels()[self.panel];
        // It slides up whole, not scaled (item's bottom margin).
        let y = ((layout::LAYOUT.1 as f64 * (1.0 - p)) * SCALE as f64).round();
        let x = (panel.loc.x * SCALE) as f64;
        let mut out = Vec::new();
        // A dot under each running app's name.
        for (e, c) in self.entries.iter().zip(&self.cells) {
            if e.ids.iter().any(|id| running.contains(id)) {
                let loc = (
                    x + ((c.loc.x + (c.size.w - DOT) / 2.0) * SCALE as f64).round(),
                    y + ((c.loc.y + c.size.h - CELL_PAD / 2.0 - DOT / 2.0) * SCALE as f64).round(),
                );
                if let Ok(el) = MemoryRenderBufferRenderElement::from_buffer(renderer, loc, &self.dot, None, None, None, Kind::Unspecified) {
                    out.push(ShellElement::Text(el));
                }
            }
        }
        if let Some(i) = self.pressed {
            let c = self.cells[i];
            let loc = (x + (c.loc.x * SCALE as f64).round(), y + (c.loc.y * SCALE as f64).round());
            if let Ok(e) = MemoryRenderBufferRenderElement::from_buffer(renderer, loc, &self.highlight, None, None, None, Kind::Unspecified) {
                out.push(ShellElement::Text(e));
            }
        }
        if let Ok(e) = MemoryRenderBufferRenderElement::from_buffer(renderer, (x, y), texture, None, None, None, Kind::Unspecified) {
            out.push(ShellElement::Text(e));
        }
        out
    }
}

/// The running dot's size, logical px (item's "•" at 9 px).
pub const DOT: f64 = 4.0;

/// A running app's dot, item's #e8e4d9.
pub fn running_dot() -> MemoryRenderBuffer {
    rounded(DOT, DOT, DOT / 2.0, [0xe8, 0xe4, 0xd9, 255])
}

/// The grid on black, one panel's size: every app's icon and name in its
/// cell.
fn draw(entries: &[Entry], cells: &[Rectangle<f64, Logical>], w: f64, h: f64) -> Option<MemoryRenderBuffer> {
    let s = SCALE as f64;
    let (pw, ph) = ((w * s) as u32, (h * s) as u32);
    let mut canvas = tiny_skia::Pixmap::new(pw, ph)?;
    canvas.fill(tiny_skia::Color::BLACK);
    let font = Font::load(&["/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf", "/usr/share/fonts/truetype/lato/Lato-Regular.ttf"]);
    let paint = tiny_skia::PixmapPaint::default();
    for (e, c) in entries.iter().zip(cells) {
        if let Some(icon) = e.icon.as_deref().and_then(|n| apps::icon_pixmap(n, ICON as i32)) {
            let x = ((c.loc.x + (c.size.w - ICON) / 2.0) * s).round() as i32;
            let y = ((c.loc.y + CELL_PAD) * s).round() as i32;
            canvas.draw_pixmap(x, y, icon.as_ref(), &paint, tiny_skia::Transform::identity(), None);
        }
        if let Some(font) = &font {
            let mut name: String = e.name.chars().take(LABEL_CHARS).collect();
            if e.name.chars().count() > LABEL_CHARS {
                name.pop();
                name.push('…');
            }
            let (rgba, lw, lh) = font.rasterize(&name, LABEL_SIZE, LABEL);
            if let Some(label) = tiny_skia::IntSize::from_wh(lw as u32, lh as u32).and_then(|size| tiny_skia::Pixmap::from_vec(rgba, size)) {
                let x = ((c.loc.x + c.size.w / 2.0) * s - lw as f64 / 2.0).round() as i32;
                let y = ((c.loc.y + CELL_PAD + ICON + LABEL_GAP) * s).round() as i32;
                canvas.draw_pixmap(x, y, label.as_ref(), &paint, tiny_skia::Transform::identity(), None);
            }
        }
    }
    Some(MemoryRenderBuffer::from_slice(canvas.data(), Fourcc::Abgr8888, (pw as i32, ph as i32), SCALE, Transform::Normal, None))
}

/// A rounded rectangle `w` by `h` logical px, premultiplied RGBA colour.
pub fn rounded(w: f64, h: f64, r: f64, rgba: [u8; 4]) -> MemoryRenderBuffer {
    let s = SCALE as f64;
    let (pw, ph) = ((w * s).round() as u32, (h * s).round() as u32);
    let mut pixmap = tiny_skia::Pixmap::new(pw.max(1), ph.max(1)).expect("pixmap");
    let (fw, fh, r) = (pw as f32, ph as f32, (r * s) as f32);
    let k = r * 0.5523;
    let mut pb = tiny_skia::PathBuilder::new();
    pb.move_to(r, 0.0);
    pb.line_to(fw - r, 0.0);
    pb.cubic_to(fw - r + k, 0.0, fw, r - k, fw, r);
    pb.line_to(fw, fh - r);
    pb.cubic_to(fw, fh - r + k, fw - r + k, fh, fw - r, fh);
    pb.line_to(r, fh);
    pb.cubic_to(r - k, fh, 0.0, fh - r + k, 0.0, fh - r);
    pb.line_to(0.0, r);
    pb.cubic_to(0.0, r - k, r - k, 0.0, r, 0.0);
    pb.close();
    if let Some(path) = pb.finish() {
        let mut paint = tiny_skia::Paint::default();
        paint.set_color_rgba8(rgba[0], rgba[1], rgba[2], rgba[3]);
        paint.anti_alias = true;
        pixmap.fill_path(&path, &paint, tiny_skia::FillRule::Winding, tiny_skia::Transform::identity(), None);
    }
    MemoryRenderBuffer::from_slice(pixmap.data(), Fourcc::Abgr8888, (pw as i32, ph as i32), SCALE, Transform::Normal, None)
}
