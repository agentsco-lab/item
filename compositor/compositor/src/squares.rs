//! The first setup's words on its table (setup.rs, table.frag), as
//! item/grid and its recovery write theirs: a letter a square, lowercase -
//! a title a letter on 2x2 of them - typed in a square at a time and erased
//! the same way; nothing cut. The table is one over both panels (the hinge
//! taken out), 18 squares across a panel, and may be zoomed about the
//! title's first square for a moment worth it: then the squares are bigger
//! and the words with them, laid out again for the squares there are.
//!
//! A line knows where it stands on the screen at the zoom it was laid out
//! for; the setup only shows lines of the zoom it is at (they are erased
//! before a zoom and typed again after it).

use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
use smithay::utils::{Logical, Rectangle, Transform};

use crate::layout::{self, SCALE};
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

/// How much bigger the squares are for a title of these words: as far as
/// its longest word still fits a letter on 2x2 from the second square, and
/// only if that is a real change (12 squares across at most); else 1.
pub fn zoom_for(title: &[&str]) -> f64 {
    let n = title.iter().map(|w| w.chars().count()).max().unwrap_or(1);
    let across = (2 * n + 2).max(8);
    if across <= 12 {
        ACROSS / across as f64
    } else {
        1.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_wrap_whole() {
        assert_eq!(wrap("this duo is yours in a minute", 11), vec!["this duo is", "yours in a", "minute"]);
        assert_eq!(wrap("abcdefghij", 4), vec!["abcd", "efgh", "ij"]);
    }

    #[test]
    fn a_short_title_zooms() {
        assert_eq!(zoom_for(&["hello"]), 1.5);
        assert_eq!(zoom_for(&["all", "set"]), 2.25);
        assert_eq!(zoom_for(&["fingerprint"]), 1.0);
    }
}
