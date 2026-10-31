//! The words on the table - the first setup's (setup.rs) and the lock
//! screen's (lock.rs), over table.frag - as item/grid and its recovery
//! write theirs: a letter a square, lowercase - a title a letter on 2x2 of
//! them - typed in a square at a time and erased the same way; nothing
//! cut. The table is one over both panels (the hinge
//! taken out), 18 squares across a panel. It can be zoomed about the
//! title's first square (the squares bigger, the words laid out again for
//! them); the setup keeps one zoom now, its words alike on every step.
//!
//! A line knows where it stands on the screen at the zoom it was laid out
//! for; the setup only shows lines of the zoom it is at (they are erased
//! before a zoom and typed again after it).

use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement};
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::utils::{Logical, Rectangle, Transform};

use crate::layout::{self, SCALE};
use crate::shade::ShellElement;
use crate::text::Font;

/// A panel's squares across.
pub const ACROSS: f64 = 18.0;
/// The row the title begins on, which a zoom keeps in its place.
pub const TITLE_ROW: f64 = 6.0;
/// A letter typed, erased; a zoom.
pub const TYPE_NS: u64 = 55_000_000;
pub const ERASE_NS: u64 = 28_000_000;
pub const ZOOM_NS: u64 = 900_000_000;
/// The letters, lit and dim (the recovery's).
pub const LIT: [f32; 4] = [0.894, 0.902, 0.910, 1.0];
pub const DIM: [f32; 4] = [0.467, 0.475, 0.486, 1.0];

/// A square's side at zoom 1, logical px.
pub fn square() -> f64 {
    layout::panels()[0].size.w as f64 / ACROSS
}

/// The table's point a zoom keeps where it is: the title's first square.
pub fn anchor() -> (f64, f64) {
    let sq = square();
    (sq, TITLE_ROW * sq)
}

/// The screen's point of a table's point past the first panel: the hinge
/// put back.
fn gap() -> (f64, f64) {
    let p = layout::panels();
    if layout::turned() {
        (0.0, (p[1].loc.y - p[0].size.h) as f64)
    } else {
        ((p[1].loc.x - p[0].size.w) as f64, 0.0)
    }
}

/// A screen point in the table: the hinge taken out.
pub fn to_table(x: f64, y: f64) -> (f64, f64) {
    let p = layout::panels();
    let g = gap();
    if layout::turned() {
        if y >= p[1].loc.y as f64 { (x - g.0, y - g.1) } else { (x, y) }
    } else if x >= p[1].loc.x as f64 {
        (x - g.0, y - g.1)
    } else {
        (x, y)
    }
}

/// The squares at a zoom: their side and where the table's square (0, 0)
/// begins, in the table's px.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Grid {
    pub s: f64,
    pub o: (f64, f64),
}

impl Grid {
    pub fn at(zoom: f64) -> Grid {
        let a = anchor();
        Grid { s: square() * zoom, o: (a.0 * (1.0 - zoom), a.1 * (1.0 - zoom)) }
    }

    /// The first whole square's column in panel `panel` (0 left, 1 right).
    pub fn first_col(&self, panel: usize) -> f64 {
        let edge = panel as f64 * layout::panels()[0].size.w as f64;
        ((edge - self.o.0) / self.s).ceil()
    }

    /// The whole squares across panel `panel` from its first.
    pub fn cols(&self, panel: usize) -> usize {
        let w = layout::panels()[0].size.w as f64;
        let end = (panel as f64 + 1.0) * w;
        (((end - self.o.0) / self.s).floor() - self.first_col(panel)).max(1.0) as usize
    }

    /// The row nearest a screen height.
    pub fn row_at(&self, y: f64) -> f64 {
        ((y - self.o.1) / self.s).round()
    }

    /// Square (`col`, `row`)'s top left on the screen.
    pub fn screen(&self, col: f64, row: f64) -> (f64, f64) {
        let (x, y) = (self.o.0 + col * self.s, self.o.1 + row * self.s);
        let w = layout::panels()[0].size.w as f64;
        let g = gap();
        if x >= w - 0.5 {
            (x + g.0, y + g.1)
        } else {
            (x, y)
        }
    }
}

/// The table as a frame has it, the setup's and the lock's (output.rs,
/// table.frag, light.frag): its squares' side and where its square (0, 0)
/// is; the lamp under it, how far its light reaches, how bright; the
/// wallpaper's colour coming into the squares in a wave out from `centre`
/// (the drop), as far as `wave`, `soft` wide at its front; how much of it
/// is shown. Logical px, the table's (the hinge taken out).
#[derive(Clone, Copy, Debug)]
pub struct Table {
    pub s: f64,
    pub o: (f64, f64),
    pub lamp: (f64, f64),
    pub reach: f64,
    pub glow: f64,
    pub centre: (f64, f64),
    pub wave: f64,
    pub soft: f64,
    pub alpha: f64,
    /// The lamp's colour.
    pub light: [f32; 3],
}

/// A line of letters on the table.
#[derive(Clone)]
pub struct Line {
    pub text: String,
    /// Its first square, its letters' squares (1, or 2 for a title), the
    /// zoom it is laid out for.
    pub col: f64,
    pub row: f64,
    pub big: f64,
    pub zoom: f64,
    pub color: [f32; 4],
    /// What a tap on it does (a button, a network), if anything.
    pub tag: Option<String>,
    /// When its typing begins.
    pub at: u64,
    pub buffer: MemoryRenderBuffer,
}

impl Line {
    #[allow(clippy::too_many_arguments)]
    pub fn new(font: &Font, text: &str, col: f64, row: f64, big: f64, zoom: f64, color: [f32; 4], tag: Option<String>) -> Line {
        let side = Grid::at(zoom).s * big;
        let (rgba, w, h) = font.squared(text, side as f32, (side * 0.52) as f32, color);
        let buffer = MemoryRenderBuffer::from_slice(&rgba, Fourcc::Abgr8888, (w, h), SCALE, Transform::Normal, None);
        Line { text: text.to_owned(), col, row, big, zoom, color, tag, at: 0, buffer }
    }

    pub fn letters(&self) -> usize {
        self.text.chars().count()
    }

    /// The same words in the same place.
    pub fn same(&self, o: &Line) -> bool {
        self.text == o.text && self.col == o.col && self.row == o.row && self.big == o.big && self.zoom == o.zoom && self.color == o.color
    }

    /// Where it stands on the screen, as wide as its letters.
    pub fn rect(&self) -> Rectangle<f64, Logical> {
        let g = Grid::at(self.zoom);
        let (x, y) = g.screen(self.col, self.row);
        let side = g.s * self.big;
        Rectangle::new((x, y).into(), (side * self.letters() as f64, side).into())
    }

    /// Letters shown at `frame_ns`: typed since `at`.
    pub fn typed(&self, frame_ns: u64) -> usize {
        if frame_ns < self.at {
            return 0;
        }
        (((frame_ns - self.at) / TYPE_NS) as usize + 1).min(self.letters())
    }

    /// When it is all typed.
    pub fn done_at(&self) -> u64 {
        self.at + TYPE_NS * self.letters() as u64
    }
}

/// The words on a table, each typed from its time, and those going,
/// erased since when; the squares' zoom (from, to, since when: it moves
/// once the words before it are erased). The setup's and the lock's.
pub struct Words {
    pub lines: Vec<Line>,
    going: Vec<(Line, u64)>,
    zoom: (f64, f64, u64),
    /// Words were laid once (the first come at their zoom, not zoomed to).
    shown_once: bool,
}

impl Default for Words {
    fn default() -> Words {
        Words { lines: Vec::new(), going: Vec::new(), zoom: (1.0, 1.0, 0), shown_once: false }
    }
}

impl Words {
    /// The new words in: the same words in the same place stay as they
    /// are; the others are erased, letter by letter from their end, and the
    /// new typed after them, one line after another. A new zoom erases
    /// all, zooms, then types.
    pub fn reconcile(&mut self, mut new: Vec<Line>, zoom: f64) {
        let now = hybris_hwc::now_ns();
        let erase = |l: &Line| l.letters() as u64 * ERASE_NS;
        let old = std::mem::take(&mut self.lines);
        let mut start = now;
        if (zoom - self.zoom.1).abs() > 1e-6 {
            let erased = old.iter().map(erase).max().unwrap_or(0);
            self.going.extend(old.into_iter().map(|l| (l, now)));
            let from = self.zoom_now(now);
            self.zoom = (from, zoom, now + erased);
            start = now + erased + if self.shown_once { ZOOM_NS } else { 0 };
            if !self.shown_once {
                // The first words: no zoom to see, at the zoom at once.
                self.zoom = (zoom, zoom, now);
            }
        } else {
            let mut kept = vec![false; new.len()];
            for l in old {
                match new.iter().position(|n| n.same(&l)) {
                    Some(i) if !kept[i] => {
                        kept[i] = true;
                        new[i].at = l.at;
                    }
                    _ => {
                        start = start.max(now + erase(&l));
                        self.going.push((l, now));
                    }
                }
            }
            for (i, l) in new.iter_mut().enumerate() {
                if !kept[i] {
                    l.at = u64::MAX;
                }
            }
        }
        for l in new.iter_mut().filter(|l| l.at == 0 || l.at == u64::MAX) {
            l.at = start;
            start = l.done_at() + TYPE_NS;
        }
        self.lines = new;
        self.shown_once = true;
    }

    /// The words there at once, nothing typed or erased (the lock as it
    /// lights: it is seen too often to type).
    pub fn set(&mut self, new: Vec<Line>, zoom: f64) {
        self.lines = new;
        self.going.clear();
        self.zoom = (zoom, zoom, 0);
        self.shown_once = true;
    }

    /// All the words erased.
    pub fn erase_all(&mut self) {
        let now = hybris_hwc::now_ns();
        self.going.extend(std::mem::take(&mut self.lines).into_iter().map(|l| (l, now)));
    }

    /// The squares' zoom at a frame.
    pub fn zoom_now(&self, frame_ns: u64) -> f64 {
        let (from, to, at) = self.zoom;
        if frame_ns <= at {
            return from;
        }
        if frame_ns >= at + ZOOM_NS {
            return to;
        }
        let k = crate::pinpad::ease((frame_ns - at) as f64 / ZOOM_NS as f64);
        from + (to - from) * k
    }

    /// The tag of the typed words a tap at (`x`, `y`) takes - a square
    /// round them taking it too.
    pub fn tapped(&self, x: f64, y: f64) -> Option<String> {
        let now = hybris_hwc::now_ns();
        self.lines.iter().filter(|l| l.tag.is_some() && l.typed(now) > 0).find_map(|l| {
            let r = l.rect();
            let pad = Grid::at(l.zoom).s / 2.0;
            let hit = x >= r.loc.x - pad && x < r.loc.x + r.size.w + pad && y >= r.loc.y - pad && y < r.loc.y + r.size.h + pad;
            hit.then(|| l.tag.clone().unwrap_or_default())
        })
    }

    /// After a frame: the erased words gone; whether any are still typed
    /// or erased.
    pub fn settle(&mut self, frame_ns: u64) -> bool {
        self.going.retain(|(l, t)| frame_ns < t + l.letters() as u64 * ERASE_NS);
        !self.going.is_empty() || self.lines.iter().any(|l| frame_ns < l.done_at()) || frame_ns < self.zoom.2 + ZOOM_NS
    }

    /// The words as typed at `frame_ns`, moved by `dx`, at the zoom they are
    /// laid for (none while the squares move).
    pub fn elements(&self, renderer: &mut GlesRenderer, frame_ns: u64, dx: f64) -> Vec<ShellElement> {
        let zoom = self.zoom_now(frame_ns);
        let mut out = Vec::new();
        let mut line = |l: &Line, n: usize| {
            if n == 0 || (l.zoom - zoom).abs() > 1e-6 {
                return;
            }
            let r = l.rect();
            let side = r.size.h;
            let src = Rectangle::<f64, Logical>::from_size((side * n as f64, side).into());
            let at = (((r.loc.x + dx) * SCALE as f64).round(), (r.loc.y * SCALE as f64).round());
            if let Ok(e) = MemoryRenderBufferRenderElement::from_buffer(renderer, at, &l.buffer, Some(1.0), Some(src), Some(src.size.to_i32_round()), Kind::Unspecified) {
                out.push(ShellElement::Text(e));
            }
        };
        for (l, t) in &self.going {
            let gone = (frame_ns.saturating_sub(*t) / ERASE_NS) as usize;
            line(l, l.letters().saturating_sub(gone));
        }
        for l in &self.lines {
            line(l, l.typed(frame_ns));
        }
        out
    }
}

/// `text` broken between words into lines of at most `wide` letters (a
/// word longer than that broken in it).
pub fn wrap(text: &str, wide: usize) -> Vec<String> {
    let wide = wide.max(1);
    let mut out = Vec::new();
    let mut cur = String::new();
    for w in text.split_whitespace() {
        if !cur.is_empty() && cur.chars().count() + 1 + w.chars().count() > wide {
            out.push(std::mem::take(&mut cur));
        }
        if !cur.is_empty() {
            cur.push(' ');
        }
        cur.push_str(w);
        while cur.chars().count() > wide {
            let head: String = cur.chars().take(wide).collect();
            let rest: String = cur.chars().skip(wide).collect();
            out.push(head);
            cur = rest;
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_wrap_whole() {
        assert_eq!(wrap("this duo is yours in a minute", 11), vec!["this duo is", "yours in a", "minute"]);
        assert_eq!(wrap("abcdefghij", 4), vec!["abcd", "efgh", "ij"]);
    }
}
