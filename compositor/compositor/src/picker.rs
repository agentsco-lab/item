//! The desktop in its editing mode: the wallpaper, the clock (walls.rs,
//! clock.rs).
//!
//! A finger held still on a desk for half a second (grid.rs, the watchdog
//! in main.rs) opens it: the dock dips away, so the wallpaper shows whole
//! under the clock, and a strip of the wallpapers rises from the bottom
//! across both panels, on a dark glass. The strip scrolls under the finger
//! and runs on when let go; a tap on a picture puts it on (it fades in over
//! the one before); the one on has a light ring. Under the pictures, the
//! accent's swatches; at the top right "Both panels" and "Each panel".
//!
//! Everything else is edited where it is, as on a phone's lock screen:
//! - **The clock**: a finger moves it on its panel; a tap on it brings a card
//!   by it with its fonts (each its own "12:34") and its brightness.
//! - **The panel's card**, under the strip on the panel being chosen for (with a
//!   picture on each panel; else the one the editing opened on): the zoom,
//!   if the picture's pixels allow any (walls.rs), and with a picture on
//!   each panel the vignette - dark or a glow in the accent, its strength,
//!   how far it reaches, how soft it is.
//! - **The picture**: above the strip, two fingers zoom it, one moves it -
//!   drawn at once on the GPU from the picture itself (output.rs), made at
//!   full quality when they let go, without a jump.
//! - A tap above the strip on the other panel chooses for that one (with a
//!   picture on each); a tap elsewhere closes the clock's card if it is up,
//!   else the editing - the strip going down and the dock coming back.
//!
//! A slider is seen as it moves and kept when it is let go.

use smithay::backend::input::TouchSlot;
use smithay::backend::renderer::element::memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement};
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::utils::{Logical, Point, Rectangle};

use crate::layout::{self, SCALE};
use crate::shade::ShellElement;
use crate::text::{Font, Label};
use crate::walls::Vignette;

/// A picture in the strip, logical px, and between them.
const THUMB_W: f64 = 180.0;
const THUMB_H: f64 = 101.0;
const GAP: f64 = 16.0;
/// The strip: its height, its distance from the bottom, its side margin.
const STRIP_H: f64 = 236.0;
/// The accent's row under the pictures (accent.rs): a swatch each, the
/// first the wallpaper's own colour (drawn as a ring), then the fixed ones.
const SWATCH: f64 = 34.0;
const SWATCH_GAP: f64 = 16.0;
const SWATCHES_Y: f64 = 172.0;
const STRIP_BOTTOM: f64 = 16.0;
const SIDE: f64 = 28.0;
/// Opening and closing.
const SLIDE_NS: f64 = 320e6;
/// A touch that moves this far scrolls.
const TAP: f64 = 10.0;
/// The run after a let go: how fast it slows (per ms).
const FRICTION: f64 = 0.0035;
/// The mode switch.
const MODE_W: f64 = 128.0;
const MODE_H: f64 = 30.0;
const MODE_TOP: f64 = 12.0;
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
const PANEL_CARD_W: f64 = 520.0;
const TINT_W: f64 = 84.0;
const TINT_H: f64 = 28.0;

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
}

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
    glass: MemoryRenderBuffer,
    ring: MemoryRenderBuffer,
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

fn label(size: f32, alpha: f32, text: &str) -> Label {
    let mut l = Label::new(size, [1.0, 1.0, 1.0, alpha]);
    if let Some(f) = Font::load(&["/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf", "/usr/share/fonts/truetype/lato/Lato-Regular.ttf"]) {
        l.set(&f, text);
    }
    l
}

impl Picker {
    pub fn new() -> Picker {
        let width = layout::LAYOUT.0 as f64 - 2.0 * SIDE;
        Picker {
            open: false,
            since: 0,
            scroll: 0.0,
            hold: None,
            run: None,
            opener: None,
            glass: crate::grid::rounded(width, STRIP_H, 26.0, [8, 10, 14, 150]),
            ring: crate::grid::rounded(THUMB_W + 8.0, THUMB_H + 8.0, 17.0, [235, 235, 235, 235]),
            swatches: crate::accent::PALETTE.iter().map(|c| crate::grid::rounded(SWATCH, SWATCH, SWATCH / 2.0, *c)).collect(),
            wall_swatch: std::cell::RefCell::new((0, crate::grid::rounded(SWATCH, SWATCH, SWATCH / 2.0, crate::accent::wall()))),
            hole: crate::grid::rounded(SWATCH - 12.0, SWATCH - 12.0, (SWATCH - 12.0) / 2.0, [14, 16, 20, 255]),
            swatch_ring: crate::grid::rounded(SWATCH + 8.0, SWATCH + 8.0, (SWATCH + 8.0) / 2.0, [235, 235, 235, 235]),
            modes: [label(13.0, 0.85, "Both panels"), label(13.0, 0.85, "Each panel")],
            mode_glass: crate::grid::rounded(2.0 * MODE_W + 8.0, MODE_H + 8.0, (MODE_H + 8.0) / 2.0, [255, 255, 255, 26]),
            mode_lit: crate::grid::rounded(MODE_W, MODE_H, MODE_H / 2.0, [255, 255, 255, 64]),
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

    /// Swatch `i`'s rect (0 the wallpaper's), logical px, open.
    fn swatch(&self, i: usize) -> Rectangle<f64, Logical> {
        let s = self.strip();
        Rectangle::new((s.loc.x + 22.0 + i as f64 * (SWATCH + SWATCH_GAP), s.loc.y + SWATCHES_Y).into(), (SWATCH, SWATCH).into())
    }

    /// The mode switch's halves, logical px, open: 0 "Both panels", 1 "Each".
    fn mode(&self, i: usize) -> Rectangle<f64, Logical> {
        let s = self.strip();
        let x0 = s.loc.x + s.size.w - 22.0 - 2.0 * MODE_W - 4.0;
        Rectangle::new((x0 + 4.0 + i as f64 * MODE_W, s.loc.y + MODE_TOP + 4.0).into(), (MODE_W, MODE_H).into())
    }

    /// The strip's rect, logical px, open.
    /// The strip's rect, logical px, open: over the panel's card, if any.
    fn strip(&self) -> Rectangle<f64, Logical> {
        let (w, h) = (layout::LAYOUT.0 as f64, layout::LAYOUT.1 as f64);
        let card = self.panel_card_h().map(|c| c + 14.0).unwrap_or(0.0);
        Rectangle::new((SIDE, h - STRIP_BOTTOM - STRIP_H - card).into(), (w - 2.0 * SIDE, STRIP_H).into())
    }

    /// The panel's card's height, if it has anything.
    fn panel_card_h(&self) -> Option<f64> {
        let knobs = self.panel_knobs();
        if knobs.is_empty() {
            return None;
        }
        let rows = knobs.len() as f64 + f64::from(u8::from(self.each_mark.is_some()));
        Some(CARD_PAD * 2.0 + rows * ROW_H)
    }

    /// Picture `i`'s rect at the scroll `scroll`, logical px, open.
    fn thumb(&self, i: usize, scroll: f64) -> Rectangle<f64, Logical> {
        let s = self.strip();
        let x = SIDE + 20.0 + i as f64 * (THUMB_W + GAP) - scroll;
        Rectangle::new((x, s.loc.y + 50.0).into(), (THUMB_W, THUMB_H).into())
    }

    fn max_scroll(count: usize) -> f64 {
        let content = 2.0 * (SIDE + 20.0) + count as f64 * (THUMB_W + GAP) - GAP;
        (content - layout::LAYOUT.0 as f64).max(0.0)
    }

    /// The panel the panel's card is on.
    fn card_panel(&self) -> usize {
        self.each_mark.unwrap_or(self.opened_on)
    }

    /// Whether the picture chosen for may zoom at all.
    fn zooms(&self) -> bool {
        self.zoom_max() > 1.01
    }

    /// The sliders on the panel's card, top to bottom: the zoom if the
    /// picture may zoom; the vignette's with a picture on each panel.
    fn panel_knobs(&self) -> Vec<Knob> {
        let mut k = Vec::new();
        if self.zooms() {
            k.push(Knob::Zoom);
        }
        if self.each_mark.is_some() {
            k.extend([Knob::Strength, Knob::Reach, Knob::Soft]);
        }
        k
    }

    /// A panel card's row for a slider: the zoom first, then the vignette's
    /// title row (Dark, Glow), then its sliders.
    fn row_of(&self, k: Knob) -> Option<f64> {
        let zoom = f64::from(u8::from(self.zooms()));
        match k {
            Knob::Zoom => self.zooms().then_some(0.0),
            Knob::Strength => self.each_mark.map(|_| zoom + 1.0),
            Knob::Reach => self.each_mark.map(|_| zoom + 2.0),
            Knob::Soft => self.each_mark.map(|_| zoom + 3.0),
            Knob::Brightness => None,
        }
    }

    /// The panel's card, logical px, if it has anything.
    fn panel_card(&self) -> Option<Rectangle<f64, Logical>> {
        let h = self.panel_card_h()?;
        // Under the strip, at the panel's foot: the top is the clock's.
        let p = layout::panels()[self.card_panel()].to_f64();
        Some(Rectangle::new((p.loc.x + (p.size.w - PANEL_CARD_W) / 2.0, layout::LAYOUT.1 as f64 - STRIP_BOTTOM - h).into(), (PANEL_CARD_W, h).into()))
    }

    /// A slider's track on its card, logical px.
    fn track_rect(&self, k: Knob) -> Option<Rectangle<f64, Logical>> {
        let (card, y) = if k == Knob::Brightness {
            let card = self.clock_card_rect()?;
            (card, card.loc.y + CARD_PAD + CHIP_H + 10.0 + ROW_H / 2.0)
        } else {
            let card = self.panel_card()?;
            (card, card.loc.y + CARD_PAD + self.row_of(k)? * ROW_H + ROW_H / 2.0)
        };
        let x = card.loc.x + CARD_PAD + LABEL_W;
        let w = card.size.w - 2.0 * CARD_PAD - LABEL_W - KNOB / 2.0;
        Some(Rectangle::new((x, y - TRACK_H / 2.0).into(), (w, TRACK_H).into()))
    }

    /// The vignette's Dark and Glow, logical px.
    fn tint_rect(&self, i: usize) -> Option<Rectangle<f64, Logical>> {
        self.each_mark?;
        let card = self.panel_card()?;
        let row = f64::from(u8::from(self.zooms()));
        let y = card.loc.y + CARD_PAD + row * ROW_H + (ROW_H - TINT_H) / 2.0;
        let x0 = card.loc.x + card.size.w - CARD_PAD - 2.0 * TINT_W - 4.0;
        Some(Rectangle::new((x0 + 4.0 + i as f64 * TINT_W, y).into(), (TINT_W, TINT_H).into()))
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
        let whole = Rectangle::new((0.0, 0.0).into(), (layout::LAYOUT.0 as f64, layout::LAYOUT.1 as f64).into());
        let p = layout::panel_at(c.loc).or_else(|| layout::panel_at((c.loc.x + c.size.w - 1.0, c.loc.y).into())).map(|i| layout::panels()[i].to_f64()).unwrap_or(whole);
        let x = (c.loc.x + c.size.w - w).clamp(p.loc.x + 12.0, (p.loc.x + p.size.w - w - 12.0).max(p.loc.x + 12.0));
        let below = c.loc.y + c.size.h + 14.0;
        let y = if below + h < self.strip().loc.y - 10.0 { below } else { (c.loc.y - h - 14.0).max(12.0) };
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
        // The one on in the middle of the strip, as far as it goes.
        let width = layout::LAYOUT.0 as f64;
        let at = SIDE + 20.0 + current as f64 * (THUMB_W + GAP) + THUMB_W / 2.0;
        self.scroll = (at - width / 2.0).clamp(0.0, Self::max_scroll(count));
        tracing::info!("picker: open");
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
        let max = Self::max_scroll(count);
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
            // A slider on a card: anywhere near its track.
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
            if self.clock_card_rect().is_some_and(|r| r.contains(pos)) || self.panel_card().is_some_and(|r| r.contains(pos)) {
                self.hold = Some(Hold { slot, start: pos, from: self.scroll, last: (time_us, pos.x), velocity: 0.0, moved: false });
                return true;
            }
        }
        // Above the strip, or a second finger there: moving or zooming the
        // picture.
        if pos.y < self.strip().loc.y || self.shape.as_ref().is_some_and(|s| !s.fingers.is_empty()) {
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
        if self.panel_card().is_some_and(|r| r.contains(h.start)) {
            if let Some(i) = (0..2).find(|&i| self.tint_rect(i).is_some_and(|r| r.contains(h.start))) {
                self.vignette.glow = i == 1;
                return Some(Picked::Vignette(self.vignette));
            }
            return None;
        }
        let scroll = (h.from - (h.last.1 - h.start.x)).clamp(0.0, Self::max_scroll(count));
        self.scroll = scroll;
        if h.moved {
            if h.velocity.abs() > 0.05 {
                self.run = Some((h.velocity, now, scroll));
            }
            return None;
        }
        if let Some(i) = (0..2).find(|&i| self.mode(i).contains(h.start)) {
            return Some(Picked::Each(i == 1));
        }
        // A swatch: a little more room round it than it shows.
        if let Some(i) = (0..=crate::accent::PALETTE.len()).find(|&i| {
            let r = self.swatch(i);
            Rectangle::<f64, Logical>::new((r.loc.x - 6.0, r.loc.y - 6.0).into(), (r.size.w + 12.0, r.size.h + 12.0).into()).contains(h.start)
        }) {
            return Some(Picked::Accent(i.checked_sub(1)));
        }
        (0..count).find(|&i| self.thumb(i, scroll).contains(h.start)).map(Picked::Wallpaper)
    }

    /// After a frame: whether it still moves.
    pub fn settle(&mut self, frame_ns: u64, count: usize) -> bool {
        let sliding = frame_ns < self.since + SLIDE_NS as u64;
        if let Some((v, at, _)) = self.run {
            let t = frame_ns.saturating_sub(at) as f64 / 1e6;
            if (v * (-FRICTION * t).exp()).abs() < 0.01 {
                self.scroll = self.scroll_at(frame_ns, count);
                self.run = None;
            }
        }
        sliding || self.run.is_some() || self.hold.is_some() || self.shape.is_some() || self.clock_hold.is_some() || self.sliding.is_some()
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
        let drop = (1.0 - k) * (STRIP_H + STRIP_BOTTOM + 10.0);
        let lift = drop;
        let alpha = k as f32;
        let s = SCALE as f64;
        let mut put = |out: &mut Vec<ShellElement>, b: &MemoryRenderBuffer, x: f64, y: f64, dy: f64| {
            if let Ok(e) = MemoryRenderBufferRenderElement::from_buffer(renderer, ((x * s).round(), ((y + dy) * s).round()), b, Some(alpha), None, None, Kind::Unspecified) {
                out.push(ShellElement::Text(e));
            }
        };
        let centred = |l: &Label, r: Rectangle<f64, Logical>| (r.loc.x + (r.size.w - l.extent.w as f64) / 2.0, r.loc.y + (r.size.h - l.extent.h as f64) / 2.0);
        // The sliders of a card: knob, track, words.
        let mut sliders = Vec::new();
        if self.clock_card {
            sliders.push(Knob::Brightness);
        }
        sliders.extend(self.panel_knobs());
        for kk in sliders {
            let Some(t) = self.track_rect(kk) else { continue };
            if let Some(x) = self.value_x(kk, self.value(kk)) {
                put(&mut out, &self.knob, x - KNOB / 2.0, t.loc.y + TRACK_H / 2.0 - KNOB / 2.0, lift);
            }
            put(&mut out, &self.sized(t.size.w, TRACK_H, 1), t.loc.x, t.loc.y, lift);
            if let Some((_, l)) = self.words.iter().find(|(w, _)| *w == kk) {
                put(&mut out, &l.buffer, t.loc.x - LABEL_W, t.loc.y + TRACK_H / 2.0 - l.extent.h as f64 / 2.0, lift);
            }
        }
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
        // The panel's card: the vignette's title and Dark, Glow.
        if let Some(card) = self.panel_card() {
            if let (Some(r0), Some(r1)) = (self.tint_rect(0), self.tint_rect(1)) {
                for (i, l) in self.tints.iter().enumerate() {
                    let (x, y) = centred(l, if i == 0 { r0 } else { r1 });
                    put(&mut out, &l.buffer, x, y, lift);
                }
                let lit = if self.vignette.glow { r1 } else { r0 };
                put(&mut out, &self.tint_lit, lit.loc.x, lit.loc.y, lift);
                put(&mut out, &self.tint_glass, r0.loc.x - 4.0, r0.loc.y - 4.0, lift);
                put(&mut out, &self.vignette_word.buffer, card.loc.x + CARD_PAD, r0.loc.y + (r0.size.h - self.vignette_word.extent.h as f64) / 2.0, lift);
            }
            put(&mut out, &self.sized(card.size.w, card.size.h, 0), card.loc.x, card.loc.y, lift);
        }
        let strip = self.strip();
        let scroll = self.scroll_at(frame_ns, thumbs.len());
        let width = layout::LAYOUT.0 as f64;
        for (i, t) in thumbs.iter().enumerate() {
            let r = self.thumb(i, scroll);
            if r.loc.x + r.size.w < 0.0 || r.loc.x > width {
                continue;
            }
            if let Some(t) = t {
                put(&mut out, t, r.loc.x, r.loc.y, drop);
            }
            if i == current {
                put(&mut out, &self.ring, r.loc.x - 4.0, r.loc.y - 4.0, drop);
            }
        }
        // The accent's row: the wallpaper's colour as a ring, the fixed
        // ones whole, the one chosen ringed.
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
                put(&mut out, &self.hole, r.loc.x + 6.0, r.loc.y + 6.0, drop);
                put(&mut out, &wall, r.loc.x, r.loc.y, drop);
            } else {
                put(&mut out, &self.swatches[i - 1], r.loc.x, r.loc.y, drop);
            }
            if i == chosen {
                put(&mut out, &self.swatch_ring, r.loc.x - 4.0, r.loc.y - 4.0, drop);
            }
        }
        // The mode switch: the half on lit, its words centred in each.
        for (i, l) in self.modes.iter().enumerate() {
            let (x, y) = centred(l, self.mode(i));
            put(&mut out, &l.buffer, x, y, drop);
        }
        let lr = self.mode(usize::from(self.each_mark.is_some()));
        put(&mut out, &self.mode_lit, lr.loc.x, lr.loc.y, drop);
        let r0 = self.mode(0);
        put(&mut out, &self.mode_glass, r0.loc.x - 4.0, r0.loc.y - 4.0, drop);
        put(&mut out, &self.glass, strip.loc.x, strip.loc.y, drop);
        out
    }
}
