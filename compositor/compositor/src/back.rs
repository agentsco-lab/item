//! Back, from the edge, item's (#80): a 14 px strip along the outer edge of
//! a panel with a window - the left panel's left edge, the right panel's
//! right edge, where the thumbs are when the Duo is held like a book - from
//! 48 px down to above the bottom band. A round "<" comes out of the edge
//! with the finger (0.7 of its travel, at most 0.9 of its size) and turns
//! blue past 64 px; let go there and the window on that panel gets the
//! focus and Alt+Left: back in libadwaita's navigation and in browsers.
//! Terminals are left out by app id: Alt+Left is a word back there.

use resvg::tiny_skia;
use smithay::backend::allocator::Fourcc;
use smithay::backend::input::TouchSlot;
use smithay::backend::renderer::element::memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement};
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::utils::{Logical, Point, Transform};

use crate::layout::{self, SCALE};
use crate::shade::ShellElement;

pub const STRIP: f64 = 14.0;
const TOP: f64 = 48.0;
const SIZE: f64 = 56.0;
const ARM: f64 = 64.0;
const FOLLOW: f64 = 0.7;
/// App ids that are terminals: no Alt+Left for them.
const TERMINALS: [&str; 4] = ["org.gnome.Console", "kgx", "foot", "org.gnome.Terminal"];

struct Grab {
    slot: TouchSlot,
    panel: usize,
    start_x: f64,
    x: f64,
    y: f64,
}

pub struct Back {
    grab: Option<Grab>,
    disc: MemoryRenderBuffer,
    armed: MemoryRenderBuffer,
}

impl Back {
    pub fn new() -> Back {
        Back { grab: None, disc: disc([40, 40, 44, 242]), armed: disc(crate::layout::ACCENT) }
    }

    /// Whether a point is on a panel's back strip.
    pub fn strip_at(pos: Point<f64, Logical>) -> Option<usize> {
        let panels = layout::panels();
        let bottom = layout::LAYOUT.1 as f64 - crate::gesture::EDGE;
        if pos.y < TOP || pos.y >= bottom {
            return None;
        }
        let left = panels[0].loc.x as f64;
        let right = (panels[1].loc.x + panels[1].size.w) as f64;
        if pos.x < left + STRIP {
            Some(0)
        } else if pos.x >= right - STRIP {
            Some(1)
        } else {
            None
        }
    }

    pub fn down(&mut self, slot: TouchSlot, pos: Point<f64, Logical>, panel: usize) {
        self.grab = Some(Grab { slot, panel, start_x: pos.x, x: pos.x, y: pos.y });
    }

    pub fn holds(&self, slot: TouchSlot) -> bool {
        self.grab.as_ref().is_some_and(|g| g.slot == slot)
    }

    pub fn motion(&mut self, slot: TouchSlot, pos: Point<f64, Logical>) {
        if let Some(g) = self.grab.as_mut().filter(|g| g.slot == slot) {
            g.x = pos.x;
            g.y = pos.y;
        }
    }

    /// How far in the finger has gone, from its edge.
    fn travel(g: &Grab) -> f64 {
        if g.panel == 0 { g.x - g.start_x } else { g.start_x - g.x }.max(0.0)
    }

    /// The finger lets go: the panel to go back on, if it went far enough.
    pub fn up(&mut self, slot: TouchSlot) -> Option<usize> {
        let g = self.grab.take_if(|g| g.slot == slot)?;
        (Self::travel(&g) >= ARM).then_some(g.panel)
    }

    pub fn cancel(&mut self) {
        self.grab = None;
    }

    pub fn warm_up(&self, renderer: &mut GlesRenderer) -> usize {
        [&self.disc, &self.armed]
            .iter()
            .filter(|b| MemoryRenderBufferRenderElement::from_buffer(renderer, (0.0, 0.0), b, None, None, None, Kind::Unspecified).is_ok())
            .count()
    }

    pub fn elements(&self, renderer: &mut GlesRenderer) -> Vec<ShellElement> {
        let Some(g) = &self.grab else { return Vec::new() };
        let travel = Self::travel(g);
        let out_by = (travel * FOLLOW).min(SIZE * 0.9);
        let panels = layout::panels();
        let x = if g.panel == 0 {
            panels[0].loc.x as f64 - SIZE + out_by
        } else {
            (panels[1].loc.x + panels[1].size.w) as f64 - out_by
        };
        let y = g.y - SIZE / 2.0;
        let texture = if travel >= ARM { &self.armed } else { &self.disc };
        let loc = ((x * SCALE as f64).round(), (y * SCALE as f64).round());
        match MemoryRenderBufferRenderElement::from_buffer(renderer, loc, texture, None, None, None, Kind::Unspecified) {
            Ok(e) => vec![ShellElement::Text(e)],
            Err(_) => Vec::new(),
        }
    }
}

/// Whether Alt+Left may go to an app: not a terminal.
pub fn wants_back(app_id: &str) -> bool {
    !TERMINALS.iter().any(|t| app_id.eq_ignore_ascii_case(t))
}

/// A disc of `SIZE` with a white "<", premultiplied RGBA fill.
fn disc(fill: [u8; 4]) -> MemoryRenderBuffer {
    let px = (SIZE * SCALE as f64) as u32;
    let mut pixmap = tiny_skia::Pixmap::new(px, px).expect("pixmap");
    let r = px as f32 / 2.0;
    let mut paint = tiny_skia::Paint::default();
    paint.anti_alias = true;
    paint.set_color_rgba8(fill[0], fill[1], fill[2], fill[3]);
    if let Some(circle) = tiny_skia::PathBuilder::from_circle(r, r, r) {
        pixmap.fill_path(&circle, &paint, tiny_skia::FillRule::Winding, tiny_skia::Transform::identity(), None);
    }
    let mut pb = tiny_skia::PathBuilder::new();
    let (c, a) = (r, r * 0.28);
    pb.move_to(c + a * 0.45, c - a);
    pb.line_to(c - a * 0.55, c);
    pb.line_to(c + a * 0.45, c + a);
    if let Some(chevron) = pb.finish() {
        paint.set_color_rgba8(255, 255, 255, 255);
        let stroke = tiny_skia::Stroke { width: 3.0 * SCALE as f32, line_cap: tiny_skia::LineCap::Round, line_join: tiny_skia::LineJoin::Round, ..Default::default() };
        pixmap.stroke_path(&chevron, &paint, &stroke, tiny_skia::Transform::identity(), None);
    }
    MemoryRenderBuffer::from_slice(pixmap.data(), Fourcc::Abgr8888, (px as i32, px as i32), SCALE, Transform::Normal, None)
}
