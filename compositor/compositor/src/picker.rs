//! The desktop in its editing mode: the wallpaper, the clock (walls.rs,
//! clock.rs).
//!
//! A finger held still on a desk for half a second (grid.rs, the watchdog
//! in main.rs) opens it: the dock dips away, so the wallpaper shows whole
//! under the clock, and two sheets of glass rise at the panels' feet, the
//! same height, none across the hinge (the owner, 2026-10-25: the panel
//! below the strip looked apart). The left one is the pictures: "Yours"
//! and, with Unsplash's key, "Unsplash" (its Shuffle at the top right, its
//! batch below; unsplash.rs), two rows that scroll sideways and run on when
//! let go; a tap puts one on (it fades in over the one before), the one on
//! has a light ring, those kept from Unsplash a "u" (a long press offers
//! Remove). The right one is their look, a row each with its word before
//! it: Layout (Both panels / Each panel), Zoom (still if the picture's
//! pixels allow none), with one picture on each panel the vignette's Dark /
//! Glow, Strength, Size and Softness, and the Accent's swatches.
//!
//! Everything else is edited where it is, as on a phone's lock screen:
//! - **The clock**: a finger moves it on its panel; a tap on it brings a card
//!   by it with its fonts (each its own "12:34") and its brightness.
//! - **The picture**: above the sheets, two fingers zoom it, one moves it -
//!   drawn at once on the GPU from the picture itself (output.rs), made at
//!   full quality when they let go, without a jump.
//! - A tap above the sheets on the other panel chooses for that one (with a
//!   picture on each); a tap elsewhere closes the clock's card if it is up,
//!   else the editing - the sheets going down and the dock coming back.

use smithay::backend::input::TouchSlot;
use smithay::backend::renderer::element::memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement};
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::utils::{Logical, Point, Rectangle, Size};

use crate::layout::{self, SCALE};
use crate::shade::ShellElement;
use crate::text::{Font, Label};
use crate::walls::Vignette;

/// A picture on the pictures' sheet, logical px, and between them.
const THUMB_W: f64 = 180.0;
const THUMB_H: f64 = 101.0;
/// With one picture on each panel the pictures are tall, 3:4 as a panel
/// is, in one row (walls.rs's THUMB_TALL_PX).
const TALL_W: f64 = 180.0;
const TALL_H: f64 = 240.0;
const GAP: f64 = 16.0;
/// The sheets' distance from the bottom, their side margin.
/// The accent's row under the pictures (accent.rs): a swatch each, the
/// first the wallpaper's own colour (drawn as a ring), then the fixed ones.
const SWATCH: f64 = 34.0;
const SWATCH_GAP: f64 = 16.0;
const STRIP_BOTTOM: f64 = 16.0;
const SIDE: f64 = 28.0;
/// Opening and closing.
const SLIDE_NS: f64 = 320e6;
/// The picture's credits, top left on the left panel: how long they take to
/// come in, and their margin.
const INFO_NS: f64 = 500e6;
const INFO_AT: (f64, f64) = (28.0, 36.0);
/// A touch that moves this far scrolls.
const TAP: f64 = 10.0;
/// The run after a let go: how fast it slows (per ms).
const FRICTION: f64 = 0.0035;
/// The mode switch.
const MODE_W: f64 = 128.0;
const MODE_H: f64 = 30.0;
/// The cards: their margin, a row's height, the words' room before a
/// slider; a slider's track and knob; the clock's fonts' chips; the
/// panel's card's width; the vignette's Dark and Glow.
const CARD_PAD: f64 = 18.0;
const ROW_H: f64 = 44.0;
const LABEL_W: f64 = 96.0;
const TRACK_H: f64 = 6.0;
const KNOB: f64 = 26.0;
const CHIP_W: f64 = 86.0;
const CHIP_H: f64 = 54.0;
const CHIP_GAP: f64 = 10.0;
const TINT_W: f64 = 84.0;
/// How long a change of the sheets' height takes.
const FOLD_NS: f64 = 280e6;
const TINT_H: f64 = 28.0;
/// The sheets: their padding, the pictures' header (the tabs, Shuffle),
/// a tab's and Shuffle's width.
const SHEET_PAD: f64 = 16.0;
const HEAD_H: f64 = 44.0;
const TAB_W: f64 = 104.0;
const PILL_W: f64 = 112.0;
/// The gap before Unsplash's batch on the pictures' sheet.
const SECTION_GAP: f64 = 28.0;

/// A row of the look's sheet.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Row {
    Layout,
    Slider(Knob),
    Tint,
    Accent,
}

/// Fingers above the strip: moving or zooming the picture.
struct Shape {
    /// (slot, where it came down, where it is).
    fingers: Vec<(TouchSlot, Point<f64, Logical>, Point<f64, Logical>)>,
    /// The two fingers' distance and middle when the second came down.
    pinch_from: Option<(f64, Point<f64, Logical>)>,
    /// The zoom and move from before that.
    zoom: f64,
    moved: (f64, f64),
    /// Whether it went beyond a tap.
    changed: bool,
}

impl Shape {
    fn middle(&self) -> Point<f64, Logical> {
        let n = self.fingers.len().max(1) as f64;
        let (x, y) = self.fingers.iter().fold((0.0, 0.0), |a, f| (a.0 + f.2.x, a.1 + f.2.y));
        (x / n, y / n).into()
    }

    fn spread(&self) -> f64 {
        match self.fingers.as_slice() {
            [a, b, ..] => ((a.2.x - b.2.x).powi(2) + (a.2.y - b.2.y).powi(2)).sqrt(),
            _ => 0.0,
        }
    }

    /// The zoom and the move so far, `room` how far the zoom may go out and in.
    fn now(&self, room: (f64, f64)) -> (f64, (f64, f64)) {
        match (self.pinch_from, self.fingers.len()) {
            (Some((d0, m0)), 2..) if d0 > 1.0 => {
                let m = self.middle();
                ((self.zoom * self.spread() / d0).clamp(room.0, room.1), (self.moved.0 + m.x - m0.x, self.moved.1 + m.y - m0.y))
            }
            (_, 1) => {
                let f = &self.fingers[0];
                (self.zoom, (self.moved.0 + f.2.x - f.1.x, self.moved.1 + f.2.y - f.1.y))
            }
            _ => (self.zoom, self.moved),
        }
    }
}

struct Hold {
    slot: TouchSlot,
    start: Point<f64, Logical>,
    from: f64,
    last: (u64, f64),
    velocity: f64,
    moved: bool,
}

/// A slider: what it sets.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Knob {
    Brightness,
    Zoom,
    Strength,
    Reach,
    Soft,
}

impl Knob {
    fn word(self) -> &'static str {
        match self {
            Knob::Brightness => "Brightness",
            Knob::Zoom => "Zoom",
            Knob::Strength => "Strength",
            Knob::Reach => "Size",
            Knob::Soft => "Softness",
        }
    }
}

/// What a touch in the editing chose.
pub enum Picked {
    Wallpaper(usize),
    /// None: the accent from the wallpaper; Some(i): the palette's.
    Accent(Option<usize>),
    /// One picture on each panel (true) or one on both.
    Each(bool),
    /// The panel chosen for, with one on each: 0 left, 1 right.
    Panel(usize),
    /// The picture zoomed by the first and moved by the second (logical px).
    View(f64, (f64, f64)),
    /// The clock: one of clock::FONTS; its brightness; moved (logical px).
    Font(usize),
    Brightness(f32),
    ClockMoved((f64, f64)),
    /// The vignette, with a picture on each panel.
    Vignette(Vignette),
    /// Another batch from Unsplash (unsplash.rs): the group's first tile.
    Shuffle,
    /// The strip's picture `i` (one kept from Unsplash) out of the set: its
    /// Remove, after a long press on it.
    Remove(usize),
    /// The credit tapped: its link (the photographer's page) opened.
    Credit,
}

/// A press this long on a picture kept from Unsplash offers its Remove.
const LONG_NS: u64 = 600_000_000;

pub struct Picker {
    open: bool,
    /// When it last opened or closed.
    since: u64,
    scroll: f64,
    hold: Option<Hold>,
    /// The run after a let go: velocity (px per ms), since when.
    run: Option<(f64, u64, f64)>,
    /// The finger that opened it, held till it lets go.
    opener: Option<TouchSlot>,
    ring: MemoryRenderBuffer,
    ring_tall: MemoryRenderBuffer,
    placeholder_tall: MemoryRenderBuffer,
    /// The fixed swatches; the wallpaper's, drawn again with it (and its
    /// hole); the light ring round the one chosen.
    swatches: Vec<MemoryRenderBuffer>,
    wall_swatch: std::cell::RefCell<(u64, MemoryRenderBuffer)>,
    hole: MemoryRenderBuffer,
    swatch_ring: MemoryRenderBuffer,
    /// "Both panels" and "Each panel"; the half on, lit.
    modes: [Label; 2],
    mode_glass: MemoryRenderBuffer,
    mode_lit: MemoryRenderBuffer,
    /// Unsplash's group in the strip (with a key in the keyring): where it
    /// starts (its first tile Shuffle, "Loading…" while it fetches), what
    /// it says in place of photos (no connection, the limit), the strip's
    /// pictures kept from Unsplash (a "u" on them), the one offered for
    /// removal.
    pub group: Option<usize>,
    pub group_busy: bool,
    pub group_note: Option<String>,
    note_label: std::cell::RefCell<Option<(String, Label)>>,
    pub kept: std::collections::HashSet<usize>,
    removing: Option<usize>,
    shuffle_words: [Label; 3],
    placeholder: MemoryRenderBuffer,
    badge: MemoryRenderBuffer,
    remove_words: Label,
    u_word: Label,
    hint_words: Label,
    remove_glass: MemoryRenderBuffer,
    /// When the finger on the strip came down (a long press).
    pressed_at: u64,
    /// "Dark" and "Glow", the vignette's.
    tints: [Label; 2],
    tint_lit: MemoryRenderBuffer,
    tint_glass: MemoryRenderBuffer,
    /// The sliders' words, and "Vignette".
    words: Vec<(Knob, Label)>,
    vignette_word: Label,
    /// The clock's fonts, each "12:34" in it; a chip, the ring of the one on.
    font_samples: Vec<Label>,
    chip: MemoryRenderBuffer,
    chip_ring: MemoryRenderBuffer,
    knob: MemoryRenderBuffer,
    /// Glass made to a size once: the cards', the sliders' tracks.
    sized: std::cell::RefCell<Vec<((u32, u32, u8), MemoryRenderBuffer)>>,
    shape: Option<Shape>,
    /// The zoom and move shown until the wallpaper made from them is on.
    preview: Option<(f64, (f64, f64))>,
    /// How far the picture may zoom out and in from where it is (walls.rs).
    room: (f64, f64),
    /// With one on each panel, the one chosen for: 0 left, 1 right.
    pub each_mark: Option<usize>,
    /// The panel the editing opened on: the panel's card is there with one
    /// picture on both.
    pub opened_on: usize,
    /// The clock's card is up.
    clock_card: bool,
    /// The batch's picture put on last (its index in the strip): ringed
    /// there, where it was tapped, not only among the set.
    pub ring_extra: Option<usize>,
    tab_words: [Label; 2],
    layout_word: Label,
    accent_word: Label,
    sheet_glass: std::cell::RefCell<Option<((u32, u32), MemoryRenderBuffer)>>,
    /// The frame being drawn: the card's opening is timed by it.
    frame: std::cell::Cell<u64>,
    /// The credits of the picture on (who took it, where, with what), as
    /// lines, and from when they come in (u64::MAX: not yet, the picture is
    /// still being made).
    info: Option<(String, Vec<Label>)>,
    info_since: u64,
    /// The sheets' height as shown, going from one to another (a vignette's
    /// rows coming with one picture on each panel): from, to, since.
    lift_shown: std::cell::Cell<(f64, f64, u64)>,
    /// Where the clock was last drawn (clock.rs), logical px.
    clock_rect: std::cell::Cell<Option<Rectangle<f64, Logical>>>,
    /// A slider held: which, its slot, its value now.
    sliding: Option<(Knob, TouchSlot, f32)>,
    /// The clock held: its slot, where it came down, where it is.
    clock_hold: Option<(TouchSlot, Point<f64, Logical>, Point<f64, Logical>)>,
    /// The clock moved, shown till it stands there itself.
    clock_kept: Option<(f64, f64)>,
    /// What the sliders start from: the clock's look, the walls'.
    pub brightness: f32,
    pub vignette: Vignette,
    pub font: usize,
    /// The clock's panel as it opened: the dock goes away meanwhile, and
    /// the clock stands where the dock was.
    pub clock_panel: Option<usize>,
}

/// A value going to `to` eased over FOLD_NS from where it was shown (`cell`:
/// from, to, since), at `now`; the first one taken as it is.
fn eased(cell: &std::cell::Cell<(f64, f64, u64)>, to: f64, now: u64) -> f64 {
    let (from, was, since) = cell.get();
    let at = |from: f64, was: f64, since: u64| {
        let t = (now.saturating_sub(since) as f64 / FOLD_NS).clamp(0.0, 1.0);
        from + (was - from) * t * t * (3.0 - 2.0 * t)
    };
    if was.is_nan() {
        cell.set((to, to, now));
        return to;
    }
    if (was - to).abs() > 0.01 {
        let here = at(from, was, since);
        cell.set((here, to, now));
        return here;
    }
    at(from, was, since)
}

fn label(size: f32, alpha: f32, text: &str) -> Label {
    let mut l = Label::new(size, [1.0, 1.0, 1.0, alpha]);
    if let Some(f) = Font::load(&["/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf", "/usr/share/fonts/truetype/lato/Lato-Regular.ttf"]) {
        l.set(&f, text);
    }
    l
}

impl Picker {
    pub fn new() -> Picker {
        Picker {
            open: false,
            since: 0,
            scroll: 0.0,
            hold: None,
            run: None,
            opener: None,
            ring: crate::grid::rounded(THUMB_W + 8.0, THUMB_H + 8.0, 17.0, [235, 235, 235, 235]),
            ring_tall: crate::grid::rounded(TALL_W + 8.0, TALL_H + 8.0, 17.0, [235, 235, 235, 235]),
            placeholder_tall: crate::grid::rounded(TALL_W, TALL_H, 26.0 / 2.0, [255, 255, 255, 12]),
            swatches: crate::accent::PALETTE.iter().map(|c| crate::grid::rounded(SWATCH, SWATCH, SWATCH / 2.0, *c)).collect(),
            wall_swatch: std::cell::RefCell::new((0, crate::grid::rounded(SWATCH, SWATCH, SWATCH / 2.0, crate::accent::wall()))),
            hole: crate::grid::rounded(SWATCH - 12.0, SWATCH - 12.0, (SWATCH - 12.0) / 2.0, [14, 16, 20, 255]),
            swatch_ring: crate::grid::rounded(SWATCH + 8.0, SWATCH + 8.0, (SWATCH + 8.0) / 2.0, [235, 235, 235, 235]),
            modes: [label(13.0, 0.85, "Both panels"), label(13.0, 0.85, "Each panel")],
            mode_glass: crate::grid::rounded(2.0 * MODE_W + 8.0, MODE_H + 8.0, (MODE_H + 8.0) / 2.0, [255, 255, 255, 26]),
            mode_lit: crate::grid::rounded(MODE_W, MODE_H, MODE_H / 2.0, [255, 255, 255, 64]),
            group: None,
            group_busy: false,
            group_note: None,
            note_label: Default::default(),
            kept: Default::default(),
            removing: None,
            shuffle_words: [label(15.0, 0.9, "Shuffle"), label(13.0, 0.6, "Loading…"), label(12.0, 0.55, "unsplash")],
            placeholder: crate::grid::rounded(THUMB_W, THUMB_H, 26.0 / 2.0, [255, 255, 255, 12]),
            badge: crate::grid::rounded(22.0, 22.0, 11.0, [0, 0, 0, 150]),
            remove_words: label(13.0, 0.95, "Remove"),
            u_word: label(13.0, 0.9, "u"),
            hint_words: label(12.0, 0.6, "Shuffle: dark photos"),
            remove_glass: crate::grid::rounded(92.0, 32.0, 16.0, [200, 60, 50, 220]),
            pressed_at: 0,
            tints: [label(13.0, 0.85, "Dark"), label(13.0, 0.85, "Glow")],
            tint_lit: crate::grid::rounded(TINT_W, TINT_H, TINT_H / 2.0, [255, 255, 255, 64]),
            tint_glass: crate::grid::rounded(2.0 * TINT_W + 8.0, TINT_H + 8.0, (TINT_H + 8.0) / 2.0, [255, 255, 255, 26]),
            words: [Knob::Brightness, Knob::Zoom, Knob::Strength, Knob::Reach, Knob::Soft].iter().map(|k| (*k, label(13.0, 0.75, k.word()))).collect(),
            vignette_word: label(14.0, 0.9, "Vignette"),
            font_samples: crate::clock::FONTS
                .iter()
                .map(|(_, time, _)| {
                    let mut l = Label::new(22.0, [1.0, 1.0, 1.0, 0.92]);
                    if let Some(f) = Font::load(&[time]) {
                        l.set(&f, "12:34");
                    }
                    l
                })
                .collect(),
            chip: crate::grid::rounded(CHIP_W, CHIP_H, 14.0, [255, 255, 255, 22]),
            chip_ring: crate::grid::rounded(CHIP_W + 6.0, CHIP_H + 6.0, 17.0, [235, 235, 235, 235]),
            knob: crate::grid::rounded(KNOB, KNOB, KNOB / 2.0, [240, 240, 240, 245]),
            sized: Default::default(),
            shape: None,
            preview: None,
            room: (1.0, 1.0),
            each_mark: None,
            opened_on: 1,
            clock_card: false,
            ring_extra: None,
            tab_words: [label(13.0, 0.85, "Yours"), label(13.0, 0.85, "Unsplash")],
            layout_word: label(13.0, 0.7, "Layout"),
            accent_word: label(13.0, 0.7, "Accent"),
            sheet_glass: Default::default(),
            frame: std::cell::Cell::new(0),
            info: None,
            info_since: u64::MAX,
            lift_shown: std::cell::Cell::new((f64::NAN, f64::NAN, 0)),
            clock_rect: Default::default(),
            sliding: None,
            clock_hold: None,
            clock_kept: None,
            brightness: 0.95,
            vignette: Vignette::default(),
            font: 0,
            clock_panel: None,
        }
    }

    // ---- where things are ----------------------------------------------
    //
    // Two sheets of the same height at the panels' feet, none across the
    // hinge: the left one the pictures (Yours / Unsplash tabs, two rows that
    // scroll sideways), the right one their look, a row each with its word
    // before it (Layout, Zoom; with one on each panel the vignette's Dark /
    // Glow and its sliders; the accent).

    /// The right sheet's rows, top to bottom.
    fn look_rows(&self) -> Vec<Row> {
        let mut rows = vec![Row::Layout, Row::Slider(Knob::Zoom)];
        if self.each_mark.is_some() {
            rows.extend([Row::Tint, Row::Slider(Knob::Strength), Row::Slider(Knob::Reach), Row::Slider(Knob::Soft)]);
        }
        rows.push(Row::Accent);
        rows
    }

    /// The sheets' height: the pictures' or the look's, whichever is more,
    /// going from one to the other eased.
    fn sheet_h(&self) -> f64 {
        let pictures = 2.0 * SHEET_PAD + HEAD_H + if self.tall() { TALL_H } else { 2.0 * THUMB_H + GAP };
        let look = 2.0 * SHEET_PAD + self.look_rows().len() as f64 * ROW_H;
        eased(&self.lift_shown, pictures.max(look), self.frame.get())
    }

    /// Sheet `p` (0 the pictures on the left panel, 1 the look on the
    /// right), logical px, open.
    fn sheet(&self, p: usize) -> Rectangle<f64, Logical> {
        let panel = layout::panels()[p.min(1)].to_f64();
        let h = self.sheet_h();
        Rectangle::new((panel.loc.x + SIDE, layout::size().1 as f64 - STRIP_BOTTOM - h).into(), (panel.size.w - 2.0 * SIDE, h).into())
    }

    /// Where the sheets begin: above it the fingers move the picture.
    fn sheets_top(&self) -> f64 {
        self.sheet(0).loc.y
    }

    /// A row of the look's sheet: its rect, logical px.
    fn row_rect(&self, row: Row) -> Option<Rectangle<f64, Logical>> {
        let rows = self.look_rows();
        let i = rows.iter().position(|r| *r == row)?;
        let s = self.sheet(1);
        // The rows in the sheet's middle (it is as tall as the pictures').
        let top = s.loc.y + (s.size.h - rows.len() as f64 * ROW_H) / 2.0;
        Some(Rectangle::new((s.loc.x + SHEET_PAD, top + i as f64 * ROW_H).into(), (s.size.w - 2.0 * SHEET_PAD, ROW_H).into()))
    }

    /// Swatch `i`'s rect (0 the wallpaper's), logical px, open.
    fn swatch(&self, i: usize) -> Rectangle<f64, Logical> {
        let r = self.row_rect(Row::Accent).unwrap_or_default();
        Rectangle::new((r.loc.x + LABEL_W + i as f64 * (SWATCH + SWATCH_GAP), r.loc.y + (ROW_H - SWATCH) / 2.0).into(), (SWATCH, SWATCH).into())
    }

    /// The layout's halves, logical px, open: 0 "Both panels", 1 "Each".
    fn mode(&self, i: usize) -> Rectangle<f64, Logical> {
        let r = self.row_rect(Row::Layout).unwrap_or_default();
        Rectangle::new((r.loc.x + LABEL_W + 4.0 + i as f64 * MODE_W, r.loc.y + (ROW_H - MODE_H) / 2.0).into(), (MODE_W, MODE_H).into())
    }

    /// The pictures' tabs, logical px, open: 0 Yours, 1 Unsplash (with a key).
    fn tab(&self, i: usize) -> Rectangle<f64, Logical> {
        let s = self.sheet(0);
        Rectangle::new((s.loc.x + SHEET_PAD + 4.0 + i as f64 * TAB_W, s.loc.y + SHEET_PAD + (HEAD_H - MODE_H) / 2.0 - 4.0).into(), (TAB_W, MODE_H).into())
    }

    /// Unsplash's Shuffle, at the pictures' sheet's top right, logical px.
    fn shuffle_pill(&self) -> Rectangle<f64, Logical> {
        let s = self.sheet(0);
        Rectangle::new((s.loc.x + s.size.w - SHEET_PAD - PILL_W, s.loc.y + SHEET_PAD + (HEAD_H - MODE_H) / 2.0 - 4.0).into(), (PILL_W, MODE_H).into())
    }

    /// The pictures on the sheet, as indices into the strip's list: the
    /// set's, then Unsplash's batch (the group's own index between them is
    /// no picture: Shuffle is at the sheet's top right).
    fn items(&self, count: usize) -> Vec<usize> {
        (0..count).filter(|&i| self.group != Some(i) || self.batch_empty(count)).collect()
    }

    /// No batch yet (Shuffle not pressed, or it found nothing): the group's
    /// own index stands for a tile that says what Shuffle does.
    fn batch_empty(&self, count: usize) -> bool {
        self.group.is_some_and(|g| count <= g + 1)
    }

    /// The pictures tall (one on each panel: 3:4, one row) or wide (16:9,
    /// two rows).
    fn tall(&self) -> bool {
        self.each_mark.is_some()
    }

    fn tile(&self) -> (f64, f64) {
        if self.tall() { (TALL_W, TALL_H) } else { (THUMB_W, THUMB_H) }
    }

    fn rows(&self) -> usize {
        if self.tall() { 1 } else { 2 }
    }

    /// Picture `i`'s column and row, and the gap before it (Unsplash's batch
    /// starts a column of its own, after SECTION_GAP).
    fn place(&self, i: usize) -> (usize, usize, f64) {
        let rows = self.rows();
        match self.group {
            Some(g) if i >= g => {
                let j = i.saturating_sub(g + 1);
                (g.div_ceil(rows) + j / rows, j % rows, SECTION_GAP)
            }
            _ => (i / rows, i % rows, 0.0),
        }
    }

    /// Picture `i`'s left edge on the sheet, unscrolled.
    fn place_x(&self, i: usize) -> f64 {
        let (col, _, gap) = self.place(i);
        SHEET_PAD + col as f64 * (self.tile().0 + GAP) + gap
    }

    /// Where Unsplash's batch starts on the sheet, unscrolled.
    fn batch_x(&self) -> Option<f64> {
        let g = self.group?;
        Some(self.place_x(g + 1))
    }

    /// Picture `i`'s rect at the scroll `scroll`, logical px, open (it may
    /// be outside the sheet: the drawing cuts it at the sheet's sides).
    fn thumb(&self, i: usize, scroll: f64) -> Rectangle<f64, Logical> {
        let s = self.sheet(0);
        let (_, row, _) = self.place(i);
        let (w, h) = self.tile();
        let x = s.loc.x + self.place_x(i) - scroll;
        let y = s.loc.y + SHEET_PAD + HEAD_H + row as f64 * (h + GAP);
        Rectangle::new((x, y).into(), (w, h).into())
    }

    fn max_scroll(&self, count: usize) -> f64 {
        let last = self.items(count).last().map_or(0.0, |&i| self.place_x(i) + self.tile().0);
        (last + SHEET_PAD - self.sheet(0).size.w).max(0.0)
    }

    /// Whether Unsplash's batch is what the sheet shows at `scroll` (its
    /// name in the header lit).
    fn batch_in_view(&self, scroll: f64) -> bool {
        self.batch_x().is_some_and(|x| x - scroll < self.sheet(0).size.w / 2.0)
    }

    /// The credit's rect, where a tap opens its link.
    fn info_rect(&self) -> Rectangle<f64, Logical> {
        Rectangle::new((INFO_AT.0 - 8.0, INFO_AT.1 - 8.0).into(), (700.0, 40.0).into())
    }

    /// Whether the picture chosen for may zoom at all.
    fn zooms(&self) -> bool {
        self.zoom_max() > 1.01
    }

    /// The sliders on the look's sheet: the zoom (still, if the picture may
    /// not zoom), the vignette's with a picture on each panel.
    fn panel_knobs(&self) -> Vec<Knob> {
        self.look_rows().into_iter().filter_map(|r| if let Row::Slider(k) = r { Some(k) } else { None }).filter(|k| *k != Knob::Zoom || self.zooms()).collect()
    }

    /// A slider's track, logical px.
    fn track_rect(&self, k: Knob) -> Option<Rectangle<f64, Logical>> {
        let (x0, w, y) = if k == Knob::Brightness {
            let card = self.clock_card_rect()?;
            (card.loc.x + CARD_PAD + LABEL_W, card.size.w - 2.0 * CARD_PAD - LABEL_W - KNOB / 2.0, card.loc.y + CARD_PAD + CHIP_H + 10.0 + ROW_H / 2.0)
        } else {
            let r = self.row_rect(Row::Slider(k))?;
            (r.loc.x + LABEL_W, r.size.w - LABEL_W - KNOB / 2.0, r.loc.y + ROW_H / 2.0)
        };
        Some(Rectangle::new((x0, y - TRACK_H / 2.0).into(), (w, TRACK_H).into()))
    }

    /// The vignette's Dark and Glow, logical px.
    fn tint_rect(&self, i: usize) -> Option<Rectangle<f64, Logical>> {
        let r = self.row_rect(Row::Tint)?;
        Some(Rectangle::new((r.loc.x + LABEL_W + 4.0 + i as f64 * TINT_W, r.loc.y + (ROW_H - TINT_H) / 2.0).into(), (TINT_W, TINT_H).into()))
    }

    /// The clock's card, under the clock (over it if there is no room),
    /// kept on its panel.
    fn clock_card_rect(&self) -> Option<Rectangle<f64, Logical>> {
        if !self.clock_card {
            return None;
        }
        let c = self.clock_rect.get()?;
        let n = self.font_samples.len() as f64;
        let w = CARD_PAD * 2.0 + n * CHIP_W + (n - 1.0) * CHIP_GAP;
        let h = CARD_PAD * 2.0 + CHIP_H + 10.0 + ROW_H;
        let whole = Rectangle::new((0.0, 0.0).into(), (layout::size().0 as f64, layout::size().1 as f64).into());
        let p = layout::panel_at(c.loc).or_else(|| layout::panel_at((c.loc.x + c.size.w - 1.0, c.loc.y).into())).map(|i| layout::panels()[i].to_f64()).unwrap_or(whole);
        let x = (c.loc.x + c.size.w - w).clamp(p.loc.x + 12.0, (p.loc.x + p.size.w - w - 12.0).max(p.loc.x + 12.0));
        let below = c.loc.y + c.size.h + 14.0;
        let y = if below + h < self.sheets_top() - 10.0 { below } else { (c.loc.y - h - 14.0).max(12.0) };
        Some(Rectangle::new((x, y).into(), (w, h).into()))
    }

    fn chip_rect(&self, i: usize) -> Option<Rectangle<f64, Logical>> {
        let card = self.clock_card_rect()?;
        Some(Rectangle::new((card.loc.x + CARD_PAD + i as f64 * (CHIP_W + CHIP_GAP), card.loc.y + CARD_PAD).into(), (CHIP_W, CHIP_H).into()))
    }

    // ---- the sliders' values ---------------------------------------------

    fn zoom_now(&self) -> f64 {
        1.0 / self.room.0.max(1e-3)
    }

    fn zoom_max(&self) -> f64 {
        self.room.1 * self.zoom_now()
    }

    fn base(&self, k: Knob) -> f32 {
        match k {
            Knob::Brightness => self.brightness,
            Knob::Zoom => self.zoom_now() as f32,
            Knob::Strength => self.vignette.strength,
            Knob::Reach => self.vignette.size,
            Knob::Soft => self.vignette.soft,
        }
    }

    fn value(&self, k: Knob) -> f32 {
        match self.sliding {
            Some((kk, _, v)) if kk == k => v,
            _ => self.base(k),
        }
    }

    /// The ends of a slider's range.
    fn range(&self, k: Knob) -> (f32, f32) {
        match k {
            Knob::Brightness => (0.25, 1.0),
            Knob::Zoom => (1.0, self.zoom_max().max(1.0) as f32),
            _ => (0.0, 1.0),
        }
    }

    fn value_at(&self, k: Knob, x: f64) -> f32 {
        let Some(r) = self.track_rect(k) else { return self.base(k) };
        let (lo, hi) = self.range(k);
        let t = ((x - r.loc.x) / r.size.w).clamp(0.0, 1.0) as f32;
        lo + (hi - lo) * t
    }

    fn value_x(&self, k: Knob, v: f32) -> Option<f64> {
        let r = self.track_rect(k)?;
        let (lo, hi) = self.range(k);
        let t = if hi > lo { (v - lo) / (hi - lo) } else { 0.0 };
        Some(r.loc.x + r.size.w * t.clamp(0.0, 1.0) as f64)
    }

    /// The brightness as the slider has it, while it is held.
    pub fn brightness_live(&self) -> Option<f32> {
        match self.sliding {
            Some((Knob::Brightness, _, v)) => Some(v),
            _ => None,
        }
    }

    /// The vignette with `k` at `v`.
    fn vignette_with(&self, k: Knob, v: f32) -> Option<Vignette> {
        let mut g = self.vignette;
        match k {
            Knob::Strength => g.strength = v,
            Knob::Reach => g.size = v,
            Knob::Soft => g.soft = v,
            _ => return None,
        }
        Some(g)
    }

    /// The vignette as its sliders have it, while one is held.
    pub fn vignette_live(&self) -> Option<Vignette> {
        let (k, _, v) = self.sliding?;
        self.vignette_with(k, v)
    }

    /// The clock moved by the finger on it (logical px), or as kept till it
    /// stands there itself.
    pub fn clock_moved(&self) -> (f64, f64) {
        match self.clock_hold {
            Some((_, a, b)) => (b.x - a.x, b.y - a.y),
            None => self.clock_kept.unwrap_or((0.0, 0.0)),
        }
    }

    /// The clock stands where it was moved to.
    pub fn clock_done(&mut self) {
        self.clock_kept = None;
    }

    /// Where the clock was drawn (clock.rs), for its card and its finger.
    pub fn set_clock_rect(&self, r: Option<Rectangle<f64, Logical>>) {
        self.clock_rect.set(r);
    }

    /// The zoom and move to show the wallpaper at (output.rs): `live`, the
    /// fingers' or the zoom slider's own, to be kept inside the picture
    /// (walls.rs); else as kept when they let go, shown as it is.
    pub fn preview(&self) -> Option<(f64, (f64, f64), bool)> {
        if let Some((Knob::Zoom, _, v)) = self.sliding {
            return Some((v as f64 / self.zoom_now(), (0.0, 0.0), true));
        }
        match &self.shape {
            Some(sh) if sh.changed => {
                let (z, d) = sh.now(self.room);
                Some((z, d, true))
            }
            _ => self.preview.map(|(z, d)| (z, d, false)),
        }
    }

    /// The zoom and move kept as the wallpaper will be made from them:
    /// shown until it is on.
    pub fn hold_preview(&mut self, z: f64, d: (f64, f64)) {
        self.preview = Some((z, d));
    }

    /// The wallpaper made from the last preview is on.
    pub fn preview_done(&mut self) {
        self.preview = None;
    }

    /// How far the picture now chosen for may zoom out and in (walls.rs).
    pub fn set_room(&mut self, room: (f64, f64)) {
        self.room = (room.0.min(1.0), room.1.max(1.0));
    }

    // ---- open and close ------------------------------------------------

    /// How far open, 0 to 1, at `frame_ns`.
    pub fn shown(&self, frame_ns: u64) -> f64 {
        let k = (frame_ns.saturating_sub(self.since) as f64 / SLIDE_NS).clamp(0.0, 1.0);
        let e = 1.0 - (1.0 - k).powi(3);
        if self.open { e } else { 1.0 - e }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Whether it is on screen at all (open, or closing).
    pub fn visible(&self, frame_ns: u64) -> bool {
        self.open || self.shown(frame_ns) > 0.0
    }

    /// Opened by the finger `slot` held on a desk, with `current` on.
    pub fn open(&mut self, slot: TouchSlot, current: usize, count: usize) {
        self.open = true;
        self.since = hybris_hwc::now_ns();
        self.opener = Some(slot);
        self.hold = None;
        self.run = None;
        self.clock_card = false;
        self.ring_extra = None;
        self.removing = None;
        self.lift_shown.set((f64::NAN, f64::NAN, 0));
        self.recenter(current, count);
        self.info_since = u64::MAX;
        tracing::info!("picker: open");
    }

    /// The credits of the picture `name`, coming in from `at_ns`; a new
    /// picture's made anew.
    pub fn set_info(&mut self, name: &str, lines: &[String], at_ns: u64) {
        let same = self.info.as_ref().is_some_and(|(n, _)| n == name);
        // The same picture moved or zoomed: they stay as they are.
        if same && self.info_since != u64::MAX {
            return;
        }
        if !same {
            let labels = lines.iter().enumerate().map(|(i, l)| if i == 0 { label(17.0, 0.95, l) } else { label(13.0, 0.75, l) }).collect();
            self.info = Some((name.to_owned(), labels));
        }
        self.info_since = at_ns;
    }

    /// A new picture chosen: its credits wait until it is on.
    pub fn info_wait(&mut self) {
        self.info_since = u64::MAX;
    }

    /// The one on in the middle of the strip, as far as it goes.
    pub fn recenter(&mut self, current: usize, count: usize) {
        let w = self.sheet(0).size.w;
        let at = self.place_x(current) + self.tile().0 / 2.0;
        self.scroll = (at - w / 2.0).clamp(0.0, self.max_scroll(count));
        self.run = None;
    }

    /// The sheet scrolled to Unsplash's batch (Shuffle pressed, or its name
    /// in the header tapped), or back to the set's start.
    pub fn show_unsplash(&mut self, count: usize) {
        self.scroll = self.batch_x().map_or(0.0, |x| (x - SHEET_PAD).clamp(0.0, self.max_scroll(count)));
        self.run = None;
    }

    pub fn close(&mut self) {
        if self.open {
            self.open = false;
            self.since = hybris_hwc::now_ns();
            self.hold = None;
            self.shape = None;
            self.clock_hold = None;
            self.sliding = None;
            self.clock_card = false;
            tracing::info!("picker: closed");
        }
    }

    fn scroll_at(&self, now: u64, count: usize) -> f64 {
        let max = self.max_scroll(count);
        if let Some(h) = &self.hold {
            return (h.from - (h.last.1 - h.start.x)).clamp(0.0, max);
        }
        match self.run {
            Some((v, at, from)) => {
                let t = now.saturating_sub(at) as f64 / 1e6;
                // Slowing evenly with friction: v e^(-f t), its integral.
                let gone = v / FRICTION * (1.0 - (-FRICTION * t).exp());
                (from - gone).clamp(0.0, max)
            }
            None => self.scroll,
        }
    }

    // ---- touches -------------------------------------------------------

    /// Every touch is its own while it is open. Returns whether it took it.
    pub fn down(&mut self, slot: TouchSlot, pos: Point<f64, Logical>, time_us: u64, count: usize, clock: Option<Rectangle<f64, Logical>>) -> bool {
        if !self.open {
            return false;
        }
        if clock.is_some() {
            self.clock_rect.set(clock);
        }
        let near = |r: Rectangle<f64, Logical>, m: f64| Rectangle::<f64, Logical>::new((r.loc.x - m, r.loc.y - m).into(), (r.size.w + 2.0 * m, r.size.h + 2.0 * m).into()).contains(pos);
        if self.shape.as_ref().is_none_or(|s| s.fingers.is_empty()) {
            // A slider on a card: anywhere near its track (the panel's card
            // open).
            let mut knobs = self.panel_knobs();
            if self.clock_card {
                knobs.push(Knob::Brightness);
            }
            for k in knobs {
                if let Some(r) = self.track_rect(k) {
                    if near(r, KNOB * 0.8) {
                        self.sliding = Some((k, slot, self.value_at(k, pos.x)));
                        return true;
                    }
                }
            }
            // The clock itself, a little more room round it than it shows.
            if let Some(c) = self.clock_rect.get() {
                if near(c, 16.0) {
                    self.clock_hold = Some((slot, pos, pos));
                    self.clock_kept = None;
                    return true;
                }
            }
            // A card: its taps are taken on the way up.
            if self.clock_card_rect().is_some_and(|r| r.contains(pos)) || self.sheet(1).contains(pos) {
                self.hold = Some(Hold { slot, start: pos, from: self.scroll, last: (time_us, pos.x), velocity: 0.0, moved: false });
                self.pressed_at = hybris_hwc::now_ns();
                return true;
            }
        }
        // Above the strip, or a second finger there: moving or zooming the
        // picture.
        if pos.y < self.sheets_top() || self.shape.as_ref().is_some_and(|s| !s.fingers.is_empty()) {
            let room = self.room;
            let shape = self.shape.get_or_insert(Shape { fingers: Vec::new(), pinch_from: None, zoom: 1.0, moved: (0.0, 0.0), changed: false });
            if shape.fingers.len() < 2 {
                // What the finger before did is kept; the pair starts afresh.
                let (z, m) = shape.now(room);
                shape.zoom = z;
                shape.moved = m;
                shape.fingers.iter_mut().for_each(|f| f.1 = f.2);
                shape.fingers.push((slot, pos, pos));
                if shape.fingers.len() == 2 {
                    shape.pinch_from = Some((shape.spread(), shape.middle()));
                    shape.changed = true;
                }
            }
            return true;
        }
        let now = hybris_hwc::now_ns();
        self.scroll = self.scroll_at(now, count);
        self.run = None;
        self.hold = Some(Hold { slot, start: pos, from: self.scroll, last: (time_us, pos.x), velocity: 0.0, moved: false });
        self.pressed_at = hybris_hwc::now_ns();
        true
    }

    pub fn holds(&self, slot: TouchSlot) -> bool {
        self.hold.as_ref().is_some_and(|h| h.slot == slot)
            || self.opener == Some(slot)
            || self.shape.as_ref().is_some_and(|s| s.fingers.iter().any(|f| f.0 == slot))
            || self.clock_hold.is_some_and(|c| c.0 == slot)
            || self.sliding.is_some_and(|s| s.1 == slot)
    }

    pub fn motion(&mut self, slot: TouchSlot, pos: Point<f64, Logical>, time_us: u64) {
        if let Some(c) = self.clock_hold.as_mut().filter(|c| c.0 == slot) {
            c.2 = pos;
            return;
        }
        if let Some((k, _, _)) = self.sliding.filter(|s| s.1 == slot) {
            let v = self.value_at(k, pos.x);
            if let Some(sl) = self.sliding.as_mut() {
                sl.2 = v;
            }
            return;
        }
        if let Some(shape) = self.shape.as_mut() {
            if let Some(f) = shape.fingers.iter_mut().find(|f| f.0 == slot) {
                f.2 = pos;
                if (pos.x - f.1.x).abs() > TAP || (pos.y - f.1.y).abs() > TAP {
                    shape.changed = true;
                }
                return;
            }
        }
        let Some(h) = self.hold.as_mut().filter(|h| h.slot == slot) else { return };
        let dt = time_us.saturating_sub(h.last.0) as f64 / 1000.0;
        if dt > 0.0 {
            h.velocity = h.velocity * 0.4 + (pos.x - h.last.1) / dt * 0.6;
        }
        h.last = (time_us, pos.x);
        h.moved |= (pos.x - h.start.x).abs() > TAP || (pos.y - h.start.y).abs() > TAP;
    }

    /// The finger lets go: what it chose, if anything.
    pub fn up(&mut self, slot: TouchSlot, count: usize) -> Option<Picked> {
        if self.opener == Some(slot) {
            self.opener = None;
            return None;
        }
        if let Some((_, a, b)) = self.clock_hold.take_if(|c| c.0 == slot) {
            let d = (b.x - a.x, b.y - a.y);
            if d.0.abs() < TAP && d.1.abs() < TAP {
                // A tap on the clock: its card, or away again.
                self.clock_card = !self.clock_card;
                return None;
            }
            self.clock_kept = Some(d);
            return Some(Picked::ClockMoved(d));
        }
        if let Some((k, _, v)) = self.sliding.take_if(|s| s.1 == slot) {
            return Some(match k {
                Knob::Brightness => {
                    self.brightness = v;
                    Picked::Brightness(v)
                }
                Knob::Zoom => {
                    let z = v as f64 / self.zoom_now();
                    Picked::View(z, (0.0, 0.0))
                }
                Knob::Strength | Knob::Reach | Knob::Soft => {
                    let g = self.vignette_with(k, v).unwrap_or(self.vignette);
                    self.vignette = g;
                    Picked::Vignette(g)
                }
            });
        }
        if let Some(shape) = self.shape.as_mut() {
            if let Some(i) = shape.fingers.iter().position(|f| f.0 == slot) {
                let (z, m) = shape.now(self.room);
                let f = shape.fingers.remove(i);
                if !shape.fingers.is_empty() {
                    // One left: it goes on moving from here.
                    shape.zoom = z;
                    shape.moved = m;
                    shape.pinch_from = None;
                    shape.fingers.iter_mut().for_each(|f| f.1 = f.2);
                    return None;
                }
                let changed = shape.changed;
                self.shape = None;
                if changed {
                    return Some(Picked::View(z, m));
                }
                // A tap above the strip: the clock's card up goes; the other
                // panel is chosen for (with one on each); else the editing
                // closes.
                if self.clock_card {
                    self.clock_card = false;
                    return None;
                }
                if let Some(p) = layout::panel_at(f.1) {
                    if self.each_mark.is_some_and(|t| t != p) {
                        return Some(Picked::Panel(p));
                    }
                }
                self.close();
                return None;
            }
        }
        let h = self.hold.take_if(|h| h.slot == slot)?;
        let now = hybris_hwc::now_ns();
        // On a card: its chips and switches; nothing else there.
        if self.clock_card_rect().is_some_and(|r| r.contains(h.start)) {
            if let Some(i) = (0..self.font_samples.len()).find(|&i| self.chip_rect(i).is_some_and(|r| r.contains(h.start))) {
                self.font = i;
                return Some(Picked::Font(i));
            }
            return None;
        }
        // The look's sheet: the layout's halves, the vignette's Dark and
        // Glow, the accent's swatches (a little more room round them).
        if self.sheet(1).contains(h.start) {
            if let Some(i) = (0..2).find(|&i| self.mode(i).contains(h.start)) {
                return Some(Picked::Each(i == 1));
            }
            if let Some(i) = (0..2).find(|&i| self.tint_rect(i).is_some_and(|r| r.contains(h.start))) {
                self.vignette.glow = i == 1;
                return Some(Picked::Vignette(self.vignette));
            }
            return (0..=crate::accent::PALETTE.len())
                .find(|&i| {
                    let r = self.swatch(i);
                    Rectangle::<f64, Logical>::new((r.loc.x - 6.0, r.loc.y - 6.0).into(), (r.size.w + 12.0, r.size.h + 12.0).into()).contains(h.start)
                })
                .map(|i| Picked::Accent(i.checked_sub(1)));
        }
        let scroll = (h.from - (h.last.1 - h.start.x)).clamp(0.0, self.max_scroll(count));
        self.scroll = scroll;
        if h.moved {
            if h.velocity.abs() > 0.05 {
                self.run = Some((h.velocity, now, scroll));
            }
            return None;
        }
        // The pictures' header: the tabs, Shuffle.
        // Yours / Unsplash: to where each starts on the sheet.
        if let Some(i) = (0..2).find(|&i| (i == 0 || self.group.is_some()) && self.tab(i).contains(h.start)) {
            self.removing = None;
            if i == 1 {
                self.show_unsplash(count);
            } else {
                self.scroll = 0.0;
                self.run = None;
            }
            return None;
        }
        if self.group.is_some() && self.shuffle_pill().contains(h.start) {
            return (!self.group_busy).then_some(Picked::Shuffle);
        }
        // Remove offered: its pill tapped removes; anything else lets it be.
        if let Some(i) = self.removing.take() {
            if self.remove_rect(i, scroll).contains(h.start) {
                return Some(Picked::Remove(i));
            }
            return None;
        }
        let sheet = self.sheet(0);
        let i = self.items(count).into_iter().find(|&i| self.thumb(i, scroll).contains(h.start) && sheet.contains(h.start))?;
        if self.group == Some(i) {
            return (!self.group_busy).then_some(Picked::Shuffle);
        }
        // Held long on one kept from Unsplash: its Remove offered.
        if self.kept.contains(&i) && hybris_hwc::now_ns().saturating_sub(self.pressed_at) > LONG_NS {
            self.removing = Some(i);
            return None;
        }
        Some(Picked::Wallpaper(i))
    }

    /// The Remove pill over picture `i`.
    fn remove_rect(&self, i: usize, scroll: f64) -> Rectangle<f64, Logical> {
        let r = self.thumb(i, scroll);
        Rectangle::new((r.loc.x + (r.size.w - 92.0) / 2.0, r.loc.y + (r.size.h - 32.0) / 2.0).into(), (92.0, 32.0).into())
    }

    /// The credit tapped (picker open, nothing else under the finger).
    pub fn credit_at(&self, pos: Point<f64, Logical>) -> bool {
        self.info.is_some() && self.info_rect().contains(pos)
    }

    /// After a frame: whether it still moves.
    pub fn settle(&mut self, frame_ns: u64, count: usize) -> bool {
        let sliding = frame_ns < self.since + SLIDE_NS as u64 || (self.info_since != u64::MAX && frame_ns < self.info_since.saturating_add(INFO_NS as u64));
        if let Some((v, at, _)) = self.run {
            let t = frame_ns.saturating_sub(at) as f64 / 1e6;
            if (v * (-FRICTION * t).exp()).abs() < 0.01 {
                self.scroll = self.scroll_at(frame_ns, count);
                self.run = None;
            }
        }
        self.frame.set(frame_ns);
        let folding = frame_ns < self.lift_shown.get().2 + FOLD_NS as u64;
        sliding || folding || self.run.is_some() || self.hold.is_some() || self.shape.is_some() || self.clock_hold.is_some() || self.sliding.is_some()
    }

    // ---- drawing -------------------------------------------------------

    /// Glass `w` x `h` made once per size: a card's (`kind` 0) or a
    /// slider's track (1).
    fn sized(&self, w: f64, h: f64, kind: u8) -> MemoryRenderBuffer {
        let key = (w.round() as u32, h.round() as u32, kind);
        let mut cache = self.sized.borrow_mut();
        if let Some((_, b)) = cache.iter().find(|(k, _)| *k == key) {
            return b.clone();
        }
        let b = match kind {
            0 => crate::grid::rounded(w, h, 22.0, [8, 10, 14, 168]),
            2 => crate::grid::rounded(w, h, 16.0, [8, 10, 14, 110]),
            _ => crate::grid::rounded(w, h, h / 2.0, [255, 255, 255, 60]),
        };
        cache.push((key, b.clone()));
        b
    }

    /// The editing at `frame_ns`: the strip (the pictures by index, the one
    /// on ringed) rising from the bottom, the cards with it.
    pub fn elements(&self, renderer: &mut GlesRenderer, frame_ns: u64, thumbs: &[Option<MemoryRenderBuffer>], current: usize) -> Vec<ShellElement> {
        let mut out = Vec::new();
        let k = self.shown(frame_ns);
        if k <= 0.0 {
            return out;
        }
        self.frame.set(frame_ns);
        let lift = (1.0 - k) * (self.sheet_h() + STRIP_BOTTOM + 10.0);
        let alpha = k as f32;
        let s = SCALE as f64;
        // A row fading in as the panel's card opens over it, and a piece of
        // a buffer (the card's background, drawn once open, shown as tall
        // as the card is).
        let fade = std::cell::Cell::new(1.0f32);
        let crop = std::cell::Cell::new(None::<Rectangle<f64, Logical>>);
        let mut put = |out: &mut Vec<ShellElement>, b: &MemoryRenderBuffer, x: f64, y: f64, dy: f64| {
            let src = crop.get();
            let size = src.map(|r| Size::from((r.size.w.round() as i32, r.size.h.round() as i32)));
            if let Ok(e) = MemoryRenderBufferRenderElement::from_buffer(renderer, ((x * s).round(), ((y + dy) * s).round()), b, Some(alpha * fade.get()), src, size, Kind::Unspecified) {
                out.push(ShellElement::Text(e));
            }
        };
        let centred = |l: &Label, r: Rectangle<f64, Logical>| (r.loc.x + (r.size.w - l.extent.w as f64) / 2.0, r.loc.y + (r.size.h - l.extent.h as f64) / 2.0);
        // The look's sheet: each row's word, then its control.
        let look = self.sheet(1);
        let word_at = |l: &Label, r: Rectangle<f64, Logical>| (r.loc.x, r.loc.y + (r.size.h - l.extent.h as f64) / 2.0);
        // Its sliders (the zoom dim, without a knob, if the picture may
        // not zoom); the clock's card's brightness.
        let mut sliders: Vec<Knob> = self.look_rows().into_iter().filter_map(|r| if let Row::Slider(k) = r { Some(k) } else { None }).collect();
        if self.clock_card {
            sliders.push(Knob::Brightness);
        }
        for kk in sliders {
            let Some(t) = self.track_rect(kk) else { continue };
            let live = kk != Knob::Zoom || self.zooms();
            fade.set(if live { 1.0 } else { 0.35 });
            if live {
                if let Some(x) = self.value_x(kk, self.value(kk)) {
                    put(&mut out, &self.knob, x - KNOB / 2.0, t.loc.y + TRACK_H / 2.0 - KNOB / 2.0, lift);
                }
            }
            put(&mut out, &self.sized(t.size.w, TRACK_H, 1), t.loc.x, t.loc.y, lift);
            if let Some((_, l)) = self.words.iter().find(|(w, _)| *w == kk) {
                put(&mut out, &l.buffer, t.loc.x - LABEL_W, t.loc.y + TRACK_H / 2.0 - l.extent.h as f64 / 2.0, lift);
            }
        }
        fade.set(1.0);
        // The clock's card: its fonts.
        if let Some(card) = self.clock_card_rect() {
            for (i, l) in self.font_samples.iter().enumerate() {
                let Some(r) = self.chip_rect(i) else { continue };
                let (x, y) = centred(l, r);
                put(&mut out, &l.buffer, x, y, lift);
                if i == self.font {
                    put(&mut out, &self.chip_ring, r.loc.x - 3.0, r.loc.y - 3.0, lift);
                }
                put(&mut out, &self.chip, r.loc.x, r.loc.y, lift);
            }
            put(&mut out, &self.sized(card.size.w, card.size.h, 0), card.loc.x, card.loc.y, lift);
        }
        // Layout: Both panels / Each panel, the one on lit.
        if let Some(r) = self.row_rect(Row::Layout) {
            let (x, y) = word_at(&self.layout_word, r);
            put(&mut out, &self.layout_word.buffer, x, y, lift);
            for (i, l) in self.modes.iter().enumerate() {
                let (x, y) = centred(l, self.mode(i));
                put(&mut out, &l.buffer, x, y, lift);
            }
            let lr = self.mode(usize::from(self.each_mark.is_some()));
            put(&mut out, &self.mode_lit, lr.loc.x, lr.loc.y, lift);
            let r0 = self.mode(0);
            put(&mut out, &self.mode_glass, r0.loc.x - 4.0, r0.loc.y - 4.0, lift);
        }
        // Vignette: Dark / Glow.
        if let (Some(r), Some(r0), Some(r1)) = (self.row_rect(Row::Tint), self.tint_rect(0), self.tint_rect(1)) {
            let (x, y) = word_at(&self.vignette_word, r);
            put(&mut out, &self.vignette_word.buffer, x, y, lift);
            for (i, l) in self.tints.iter().enumerate() {
                let (x, y) = centred(l, if i == 0 { r0 } else { r1 });
                put(&mut out, &l.buffer, x, y, lift);
            }
            let lit = if self.vignette.glow { r1 } else { r0 };
            put(&mut out, &self.tint_lit, lit.loc.x, lit.loc.y, lift);
            put(&mut out, &self.tint_glass, r0.loc.x - 4.0, r0.loc.y - 4.0, lift);
        }
        // Accent: the wallpaper's colour as a ring, the fixed ones whole,
        // the one chosen ringed.
        if let Some(r) = self.row_rect(Row::Accent) {
            let (x, y) = word_at(&self.accent_word, r);
            put(&mut out, &self.accent_word.buffer, x, y, lift);
            let chosen = crate::accent::chosen().map(|i| i + 1).unwrap_or(0);
            let wall = {
                let mut w = self.wall_swatch.borrow_mut();
                if w.0 != crate::accent::version() {
                    *w = (crate::accent::version(), crate::grid::rounded(SWATCH, SWATCH, SWATCH / 2.0, crate::accent::wall()));
                }
                w.1.clone()
            };
            for i in 0..=self.swatches.len() {
                let r = self.swatch(i);
                if i == 0 {
                    put(&mut out, &self.hole, r.loc.x + 6.0, r.loc.y + 6.0, lift);
                    put(&mut out, &wall, r.loc.x, r.loc.y, lift);
                } else {
                    put(&mut out, &self.swatches[i - 1], r.loc.x, r.loc.y, lift);
                }
                if i == chosen {
                    put(&mut out, &self.swatch_ring, r.loc.x - 4.0, r.loc.y - 4.0, lift);
                }
            }
        }
        // The picture's credits, top left on the left panel, over a soft
        // glass, coming in once the picture is on.
        if let Some((_, lines)) = self.info.as_ref().filter(|(_, l)| !l.is_empty() && self.info_since != u64::MAX) {
            let t = (frame_ns.saturating_sub(self.info_since) as f64 / INFO_NS).clamp(0.0, 1.0);
            let e = (t * t * (3.0 - 2.0 * t)) as f32;
            if e > 0.0 {
                fade.set(e);
                let (x, mut y) = (layout::panels()[0].loc.x as f64 + INFO_AT.0, INFO_AT.1 - (1.0 - e as f64) * 8.0);
                let (pad, gap) = (14.0, 4.0);
                let w = lines.iter().map(|l| l.extent.w as f64).fold(0.0, f64::max) + 2.0 * pad;
                let h = lines.iter().map(|l| l.extent.h as f64).sum::<f64>() + gap * (lines.len() - 1) as f64 + 2.0 * pad - 8.0;
                let top = y;
                y += pad - 4.0;
                for l in lines {
                    put(&mut out, &l.buffer, x + pad, y, 0.0);
                    y += l.extent.h as f64 + gap;
                }
                put(&mut out, &self.sized(w.round(), h.round(), 2), x, top, 0.0);
                fade.set(1.0);
            }
        }
        // The pictures' sheet: its header (Yours / Unsplash, Shuffle and
        // what Unsplash said), then the open tab's pictures in two rows,
        // cut at the sheet's sides.
        let pics = self.sheet(0);
        let tabs = if self.group.is_some() { 2 } else { 1 };
        let scroll = self.scroll_at(frame_ns, thumbs.len());
        // Yours · Unsplash: the names of the strip's two parts, the one in
        // view bright, each a jump to where it starts.
        let in_view = usize::from(self.batch_in_view(scroll));
        for i in 0..tabs {
            let l = &self.tab_words[i];
            let r = self.tab(i);
            fade.set(if i == in_view { 1.0 } else { 0.5 });
            put(&mut out, &l.buffer, r.loc.x + 6.0, r.loc.y + (r.size.h - l.extent.h as f64) / 2.0, lift);
        }
        fade.set(1.0);
        // The line between the parts, in the strip.
        if let Some(bx) = self.batch_x() {
            let x = pics.loc.x + bx - scroll - SECTION_GAP / 2.0 - GAP / 2.0;
            if x > pics.loc.x + SHEET_PAD && x < pics.loc.x + pics.size.w - SHEET_PAD {
                let (_, h) = self.tile();
                let rows = self.rows() as f64;
                let tall = rows * h + (rows - 1.0) * GAP;
                put(&mut out, &self.sized(2.0, tall, 1), x, pics.loc.y + SHEET_PAD + HEAD_H, lift);
            }
        }
        if self.group.is_some() {
            let p = self.shuffle_pill();
            let l = &self.shuffle_words[usize::from(self.group_busy)];
            let (x, y) = centred(l, p);
            put(&mut out, &l.buffer, x, y, lift);
            put(&mut out, &self.sized(PILL_W, MODE_H, 1), p.loc.x, p.loc.y, lift);
            if let Some(note) = &self.group_note {
                let mut cached = self.note_label.borrow_mut();
                if cached.as_ref().is_none_or(|(n, _)| n != note) {
                    *cached = Some((note.clone(), label(13.0, 0.7, note)));
                }
                // In the batch's place, where its pictures would be.
                if let (Some((_, l)), Some(bx)) = (cached.as_ref(), self.batch_x()) {
                    let x = pics.loc.x + bx - scroll;
                    if x < pics.loc.x + pics.size.w - 40.0 {
                        let y = pics.loc.y + SHEET_PAD + HEAD_H + self.tile().1 / 2.0 - l.extent.h as f64 / 2.0;
                        put(&mut out, &l.buffer, x.max(pics.loc.x + SHEET_PAD), y, lift);
                    }
                }
            }
        }
        let (left, right) = (pics.loc.x + SHEET_PAD, pics.loc.x + pics.size.w - SHEET_PAD);
        let empty = self.batch_empty(thumbs.len());
        for i in self.items(thumbs.len()) {
            let r = self.thumb(i, scroll);
            // Less than a sliver showing: not drawn.
            if r.loc.x + r.size.w <= left + 12.0 || r.loc.x >= right - 12.0 {
                continue;
            }
            // No batch yet: the group's tile says what Shuffle brings.
            if empty && self.group == Some(i) {
                if r.loc.x >= left && r.loc.x + r.size.w <= right {
                    let hint = &self.hint_words;
                    let (x, y) = centred(hint, r);
                    put(&mut out, &hint.buffer, x, y, lift);
                    put(&mut out, if self.tall() { &self.placeholder_tall } else { &self.placeholder }, r.loc.x, r.loc.y, lift);
                }
                continue;
            }
            // Cut at the sheet's sides: the part of it inside.
            let (x0, x1) = (r.loc.x.max(left), (r.loc.x + r.size.w).min(right));
            let whole = x0 == r.loc.x && x1 == r.loc.x + r.size.w;
            let part = Rectangle::<f64, Logical>::new((x0 - r.loc.x, 0.0).into(), (x1 - x0, r.size.h).into());
            if whole && self.removing == Some(i) {
                let p = self.remove_rect(i, scroll);
                let (x, y) = centred(&self.remove_words, p);
                put(&mut out, &self.remove_words.buffer, x, y, lift);
                put(&mut out, &self.remove_glass, p.loc.x, p.loc.y, lift);
            }
            // One kept from Unsplash: a "u" in its corner.
            if whole && self.kept.contains(&i) {
                let b = Rectangle::<f64, Logical>::new((r.loc.x + r.size.w - 30.0, r.loc.y + 8.0).into(), (22.0, 22.0).into());
                let (x, y) = centred(&self.u_word, b);
                put(&mut out, &self.u_word.buffer, x, y - 1.0, lift);
                put(&mut out, &self.badge, b.loc.x, b.loc.y, lift);
            }
            crop.set((!whole).then_some(part));
            match thumbs.get(i).cloned().flatten().as_ref() {
                Some(t) => put(&mut out, t, x0, r.loc.y, lift),
                // Still coming (Unsplash's batch): its place shown.
                None => put(&mut out, if self.tall() { &self.placeholder_tall } else { &self.placeholder }, x0, r.loc.y, lift),
            }
            crop.set(None);
            // The ring behind the picture (the list is front to back): only
            // its 4 px round it show.
            if whole && (i == current || self.ring_extra == Some(i)) {
                put(&mut out, if self.tall() { &self.ring_tall } else { &self.ring }, r.loc.x - 4.0, r.loc.y - 4.0, lift);
            }
        }
        // The two sheets' glass.
        for sh in [pics, look] {
            let key = (sh.size.w.round() as u32, sh.size.h.round() as u32);
            let mut g = self.sheet_glass.borrow_mut();
            if g.as_ref().is_none_or(|(k, _)| *k != key) {
                *g = Some((key, crate::grid::rounded(sh.size.w, sh.size.h, 26.0, [8, 10, 14, 150])));
            }
            if let Some((_, b)) = g.as_ref() {
                put(&mut out, b, sh.loc.x, sh.loc.y, lift);
            }
        }
        out
    }
}
