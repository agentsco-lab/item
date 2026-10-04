//! The desktop in its editing mode: choosing the wallpaper (walls.rs).
//!
//! A finger held still on a desk for half a second (grid.rs, the watchdog
//! in main.rs) opens it: the dock dips away and the clock fades, so the
//! wallpaper shows whole, and a strip of the wallpapers rises from the
//! bottom across both panels, on a dark glass. The strip scrolls under the
//! finger and runs on when let go; a tap on a picture puts it on (it fades
//! in over the one before); the one on has a light ring. A tap above the
//! strip closes it, the strip going down and the dock coming back.
//!
//! Beside the tabs, "Both panels" and "Each panel": with a picture on each,
//! the one chosen for is the panel the picker was opened on, marked "This
//! panel"; a tap above the strip on the other panel chooses for that one
//! instead. Above the strip two fingers zoom the picture in (as far as its
//! pixels allow, walls.rs) and one moves it: the wallpaper follows at once
//! on the GPU (`preview`), and when the fingers let go it is made again at
//! full quality and kept.
//!
//! "Wallpaper" and "Clock" are tabs: on the clock's, its fonts (each its own
//! "12:34") and its brightness. The clock stays on the desktop while the
//! picker is open, and a finger takes it and moves it on its panel. With a
//! picture on each panel the wallpaper's tab has the vignette's slider: each
//! panel darkening toward its edges (output.rs, vignette.frag). A slider is
//! seen as it moves and kept when it is let go.

use smithay::backend::input::TouchSlot;
use smithay::backend::renderer::element::memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement};
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::utils::{Logical, Point, Rectangle};

use crate::layout::{self, SCALE};
use crate::shade::ShellElement;
use crate::text::{Font, Label};

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

/// Fingers above the strip: moving or zooming the picture.
struct Shape {
    /// (slot, where it came down, where it is).
    fingers: Vec<(TouchSlot, Point<f64, Logical>, Point<f64, Logical>)>,
    /// The two fingers' distance and middle when the second came down; the
    /// zoom and move from before that.
    pinch_from: Option<(f64, Point<f64, Logical>)>,
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
    /// "Both panels" and "Each panel", the one on lit; "This panel".
    modes: [Label; 2],
    mode_glass: MemoryRenderBuffer,
    mode_lit: MemoryRenderBuffer,
    this_panel: Label,
    this_glass: MemoryRenderBuffer,
    shape: Option<Shape>,
    /// The zoom and move shown until the wallpaper made from them is on:
    /// (zoom, move in logical px).
    preview: Option<(f64, (f64, f64))>,
    /// How far the picture may zoom out and in from where it is (walls.rs).
    room: (f64, f64),
    /// With one on each panel, the one chosen for: 0 left, 1 right.
    pub each_mark: Option<usize>,
    /// 0 the wallpaper's tab, 1 the clock's.
    tab: usize,
    tabs: [Label; 2],
    tab_lit: MemoryRenderBuffer,
    /// The clock's fonts, each "12:34" in it; the chosen one's ring.
    font_samples: Vec<Label>,
    font_card: MemoryRenderBuffer,
    font_ring: MemoryRenderBuffer,
    /// The sliders' words, track, fill, knob.
    knob_words: [Label; 2],
    track: MemoryRenderBuffer,
    knob: MemoryRenderBuffer,
    /// A slider held: which, its slot, its value now.
    sliding: Option<(Knob, TouchSlot, f32)>,
    /// The clock held: its slot, where it came down, where it is.
    clock_hold: Option<(TouchSlot, Point<f64, Logical>, Point<f64, Logical>)>,
    /// The clock moved, shown till the clock draws itself there.
    clock_kept: Option<(f64, f64)>,
    /// The values the sliders start from (the clock's look, the walls').
    pub brightness: f32,
    pub vignette: f32,
    /// The clock's font chosen.
    pub font: usize,
    /// The clock's panel as it opened: the dock goes away while it is open,
    /// and the clock stands where the dock was.
    pub clock_panel: Option<usize>,
}

/// What a touch in the picker chose.
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
    /// The vignette's strength, with a picture on each panel.
    Vignette(f32),
}

/// A slider: what it sets.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Knob {
    Brightness,
    Vignette,
}

/// The sliders' and the clock's tab's sizes.
const SLIDER_W: f64 = 260.0;
const SLIDER_H: f64 = 6.0;
const KNOB: f64 = 26.0;
const FONT_W: f64 = 150.0;
const FONT_H: f64 = 84.0;
const TAB_W: f64 = 112.0;

/// The mode switch: its size and place on the strip.
const MODE_W: f64 = 128.0;
const MODE_H: f64 = 30.0;
const MODE_TOP: f64 = 12.0;

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
            modes: {
                let font = Font::load(&["/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf", "/usr/share/fonts/truetype/lato/Lato-Regular.ttf"]);
                ["Both panels", "Each panel"].map(|t| {
                    let mut l = Label::new(13.0, [1.0, 1.0, 1.0, 0.85]);
                    if let Some(f) = &font {
                        l.set(f, t);
                    }
                    l
                })
            },
            mode_glass: crate::grid::rounded(2.0 * MODE_W + 8.0, MODE_H + 8.0, (MODE_H + 8.0) / 2.0, [255, 255, 255, 26]),
            mode_lit: crate::grid::rounded(MODE_W, MODE_H, MODE_H / 2.0, [255, 255, 255, 64]),
            this_panel: {
                let mut l = Label::new(14.0, [1.0, 1.0, 1.0, 0.9]);
                if let Some(f) = Font::load(&["/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf", "/usr/share/fonts/truetype/lato/Lato-Regular.ttf"]) {
                    l.set(&f, "This panel");
                }
                l
            },
            this_glass: crate::grid::rounded(120.0, 34.0, 17.0, [8, 10, 14, 150]),
            shape: None,
            preview: None,
            room: (1.0, 1.0),
            each_mark: None,
            tab: 0,
            tabs: {
                let font = Font::load(&["/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf", "/usr/share/fonts/truetype/lato/Lato-Regular.ttf"]);
                ["Wallpaper", "Clock"].map(|t| {
                    let mut l = Label::new(15.0, [1.0, 1.0, 1.0, 0.85]);
                    if let Some(f) = &font {
                        l.set(f, t);
                    }
                    l
                })
            },
            tab_lit: crate::grid::rounded(TAB_W, MODE_H, MODE_H / 2.0, [255, 255, 255, 64]),
            font_samples: crate::clock::FONTS
                .iter()
                .map(|(_, time, _)| {
                    let mut l = Label::new(30.0, [1.0, 1.0, 1.0, 0.92]);
                    if let Some(f) = Font::load(&[time]) {
                        l.set(&f, "12:34");
                    }
                    l
                })
                .collect(),
            font_card: crate::grid::rounded(FONT_W, FONT_H, 18.0, [255, 255, 255, 22]),
            font_ring: crate::grid::rounded(FONT_W + 8.0, FONT_H + 8.0, 22.0, [235, 235, 235, 235]),
            knob_words: {
                let font = Font::load(&["/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf", "/usr/share/fonts/truetype/lato/Lato-Regular.ttf"]);
                ["Brightness", "Vignette"].map(|t| {
                    let mut l = Label::new(13.0, [1.0, 1.0, 1.0, 0.75]);
                    if let Some(f) = &font {
                        l.set(f, t);
                    }
                    l
                })
            },
            track: crate::grid::rounded(SLIDER_W, SLIDER_H, SLIDER_H / 2.0, [255, 255, 255, 60]),
            knob: crate::grid::rounded(KNOB, KNOB, KNOB / 2.0, [240, 240, 240, 245]),
            sliding: None,
            clock_hold: None,
            clock_kept: None,
            brightness: 0.95,
            vignette: 0.4,
            font: 0,
            clock_panel: None,
        }
    }

    /// The mode switch's halves, logical px, open: 0 "Both panels", 1 "Each".
    fn mode(i: usize) -> Rectangle<f64, Logical> {
        let s = Self::strip();
        let x0 = s.loc.x + s.size.w - 22.0 - 2.0 * MODE_W - 4.0;
        Rectangle::new((x0 + 4.0 + i as f64 * MODE_W, s.loc.y + MODE_TOP + 4.0).into(), (MODE_W, MODE_H).into())
    }

    /// The tabs' rects, logical px, open.
    fn tab_rect(i: usize) -> Rectangle<f64, Logical> {
        let s = Self::strip();
        Rectangle::new((s.loc.x + 18.0 + i as f64 * (TAB_W + 4.0), s.loc.y + MODE_TOP + 4.0).into(), (TAB_W, MODE_H).into())
    }

    /// Font `i`'s card, logical px, open.
    fn font_rect(i: usize) -> Rectangle<f64, Logical> {
        let s = Self::strip();
        Rectangle::new((s.loc.x + 22.0 + i as f64 * (FONT_W + GAP), s.loc.y + 56.0).into(), (FONT_W, FONT_H).into())
    }

    /// A slider's track, logical px, open: the brightness's on the clock's
    /// tab, the vignette's on the wallpaper's, at the right of the swatches'
    /// row.
    fn slider_rect(_k: Knob) -> Rectangle<f64, Logical> {
        let s = Self::strip();
        Rectangle::new((s.loc.x + s.size.w - 30.0 - SLIDER_W, s.loc.y + SWATCHES_Y + SWATCH / 2.0 - SLIDER_H / 2.0).into(), (SLIDER_W, SLIDER_H).into())
    }

    /// The slider shown now, if any.
    fn shown_slider(&self) -> Option<Knob> {
        match self.tab {
            1 => Some(Knob::Brightness),
            _ => self.each_mark.is_some().then_some(Knob::Vignette),
        }
    }

    fn slider_value(&self, k: Knob) -> f32 {
        match self.sliding {
            Some((kk, _, v)) if kk == k => v,
            _ => match k {
                Knob::Brightness => self.brightness,
                Knob::Vignette => self.vignette,
            },
        }
    }

    /// A slider's value under `x`: brightness 25-100 %, the vignette 0-1.
    fn value_at(k: Knob, x: f64) -> f32 {
        let r = Self::slider_rect(k);
        let t = ((x - r.loc.x) / r.size.w).clamp(0.0, 1.0) as f32;
        match k {
            Knob::Brightness => 0.25 + 0.75 * t,
            Knob::Vignette => t,
        }
    }

    fn value_pos(k: Knob, v: f32) -> f64 {
        let r = Self::slider_rect(k);
        let t = match k {
            Knob::Brightness => (v - 0.25) / 0.75,
            Knob::Vignette => v,
        };
        r.loc.x + r.size.w * t.clamp(0.0, 1.0) as f64
    }

    /// The brightness as the slider has it, while it is held.
    pub fn brightness_live(&self) -> Option<f32> {
        match self.sliding {
            Some((Knob::Brightness, _, v)) => Some(v),
            _ => None,
        }
    }

    /// The vignette as the slider has it, while it is held.
    pub fn vignette_live(&self) -> Option<f32> {
        match self.sliding {
            Some((Knob::Vignette, _, v)) => Some(v),
            _ => None,
        }
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

    /// The zoom and move to show the wallpaper at (output.rs), while the
    /// fingers are on it or till the wallpaper made from them is on.
    /// `live`: the fingers' own, to be kept inside the picture (walls.rs);
    /// else as kept when they let go, shown as it is.
    pub fn preview(&self) -> Option<(f64, (f64, f64), bool)> {
        match &self.shape {
            Some(sh) if sh.changed => {
                let (z, d) = sh.now(self.room);
                Some((z, d, true))
            }
            _ => self.preview.map(|(z, d)| (z, d, false)),
        }
    }

    /// The zoom and move kept as the wallpaper will be made from them: shown
    /// until it is on.
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

    /// Swatch `i`'s rect (0 the wallpaper's), logical px, open.
    fn swatch(i: usize) -> Rectangle<f64, Logical> {
        let s = Self::strip();
        Rectangle::new((s.loc.x + 22.0 + i as f64 * (SWATCH + SWATCH_GAP), s.loc.y + SWATCHES_Y).into(), (SWATCH, SWATCH).into())
    }

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
            self.tab = 0;
            tracing::info!("picker: closed");
        }
    }

    fn max_scroll(count: usize) -> f64 {
        let content = 2.0 * (SIDE + 20.0) + count as f64 * (THUMB_W + GAP) - GAP;
        (content - layout::LAYOUT.0 as f64).max(0.0)
    }

    /// The strip's rect, logical px, open.
    fn strip() -> Rectangle<f64, Logical> {
        let (w, h) = (layout::LAYOUT.0 as f64, layout::LAYOUT.1 as f64);
        Rectangle::new((SIDE, h - STRIP_BOTTOM - STRIP_H).into(), (w - 2.0 * SIDE, STRIP_H).into())
    }

    /// Picture `i`'s rect at the scroll `scroll`, logical px, open.
    fn thumb(i: usize, scroll: f64) -> Rectangle<f64, Logical> {
        let s = Self::strip();
        let x = SIDE + 20.0 + i as f64 * (THUMB_W + GAP) - scroll;
        Rectangle::new((x, s.loc.y + 50.0).into(), (THUMB_W, THUMB_H).into())
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

    /// Every touch is its own while it is open. Returns whether it took it.
    pub fn down(&mut self, slot: TouchSlot, pos: Point<f64, Logical>, time_us: u64, count: usize, clock: Option<Rectangle<f64, Logical>>) -> bool {
        if !self.open {
            return false;
        }
        // The clock, a little more room round it than it shows.
        if self.shape.is_none() {
            if let Some(c) = clock {
                if Rectangle::<f64, Logical>::new((c.loc.x - 16.0, c.loc.y - 16.0).into(), (c.size.w + 32.0, c.size.h + 32.0).into()).contains(pos) {
                    self.clock_hold = Some((slot, pos, pos));
                    self.clock_kept = None;
                    return true;
                }
            }
        }
        // A slider: anywhere on its row near the track.
        if let Some(k) = self.shown_slider() {
            let r = Self::slider_rect(k);
            if Rectangle::<f64, Logical>::new((r.loc.x - KNOB, r.loc.y - KNOB).into(), (r.size.w + 2.0 * KNOB, r.size.h + 2.0 * KNOB).into()).contains(pos) {
                self.sliding = Some((k, slot, Self::value_at(k, pos.x)));
                return true;
            }
        }
        // Above the strip, or a second finger there: moving or zooming the
        // picture.
        if pos.y < Self::strip().loc.y || self.shape.as_ref().is_some_and(|s| !s.fingers.is_empty()) {
            let shape = self.shape.get_or_insert(Shape { fingers: Vec::new(), pinch_from: None, zoom: 1.0, moved: (0.0, 0.0), changed: false });
            if shape.fingers.len() < 2 {
                // What the finger before did is kept; the pair starts afresh.
                let (z, m) = shape.now(self.room);
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
        if let Some(sl) = self.sliding.as_mut().filter(|s| s.1 == slot) {
            sl.2 = Self::value_at(sl.0, pos.x);
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

    /// The finger lets go: a tap on a picture is its index; a tap above
    /// the strip closes it; a drag runs on.
    pub fn up(&mut self, slot: TouchSlot, count: usize) -> Option<Picked> {
        if self.opener == Some(slot) {
            self.opener = None;
            return None;
        }
        if let Some((_, a, b)) = self.clock_hold.take_if(|c| c.0 == slot) {
            let d = (b.x - a.x, b.y - a.y);
            if d.0.abs() < TAP && d.1.abs() < TAP {
                // A tap on the clock: its tab.
                self.tab = 1;
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
                Knob::Vignette => {
                    self.vignette = v;
                    Picked::Vignette(v)
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
                    self.preview = Some((z, m));
                    return Some(Picked::View(z, m));
                }
                // A tap above the strip: the other panel chosen for, with one
                // on each and the tap there; else closed.
                if let Some(p) = crate::layout::panel_at(f.1) {
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
        let scroll = (h.from - (h.last.1 - h.start.x)).clamp(0.0, Self::max_scroll(count));
        self.scroll = scroll;
        if h.moved {
            if h.velocity.abs() > 0.05 {
                self.run = Some((h.velocity, now, scroll));
            }
            return None;
        }
        if let Some(i) = (0..2).find(|&i| Self::tab_rect(i).contains(h.start)) {
            self.tab = i;
            return None;
        }
        if self.tab == 1 {
            if let Some(i) = (0..self.font_samples.len()).find(|&i| Self::font_rect(i).contains(h.start)) {
                self.font = i;
                return Some(Picked::Font(i));
            }
            return None;
        }
        if let Some(i) = (0..2).find(|&i| Self::mode(i).contains(h.start)) {
            return Some(Picked::Each(i == 1));
        }
        // A swatch: a little more room round it than it shows.
        if let Some(i) = (0..=crate::accent::PALETTE.len()).find(|&i| {
            let r = Self::swatch(i);
            Rectangle::<f64, Logical>::new((r.loc.x - 6.0, r.loc.y - 6.0).into(), (r.size.w + 12.0, r.size.h + 12.0).into()).contains(h.start)
        }) {
            return Some(Picked::Accent(i.checked_sub(1)));
        }
        (0..count).find(|&i| Self::thumb(i, scroll).contains(h.start)).map(Picked::Wallpaper)
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

    /// The strip at `frame_ns`: the pictures (`thumbs`, by index), the one
    /// on ringed, on its glass, rising from the bottom as it opens.
    pub fn elements(&self, renderer: &mut GlesRenderer, frame_ns: u64, thumbs: &[Option<MemoryRenderBuffer>], current: usize) -> Vec<ShellElement> {
        let mut out = Vec::new();
        let k = self.shown(frame_ns);
        if k <= 0.0 {
            return out;
        }
        let drop = (1.0 - k) * (STRIP_H + STRIP_BOTTOM + 10.0);
        let alpha = k as f32;
        let s = SCALE as f64;
        let mut put = |out: &mut Vec<ShellElement>, b: &MemoryRenderBuffer, x: f64, y: f64| {
            if let Ok(e) = MemoryRenderBufferRenderElement::from_buffer(renderer, ((x * s).round(), ((y + drop) * s).round()), b, Some(alpha), None, None, Kind::Unspecified) {
                out.push(ShellElement::Text(e));
            }
        };
        let strip = Self::strip();
        let scroll = self.scroll_at(frame_ns, thumbs.len());
        let width = layout::LAYOUT.0 as f64;
        // The slider shown, its knob, its words.
        if let Some(k) = self.shown_slider() {
            let r = Self::slider_rect(k);
            let x = Self::value_pos(k, self.slider_value(k));
            put(&mut out, &self.knob, x - KNOB / 2.0, r.loc.y + SLIDER_H / 2.0 - KNOB / 2.0);
            put(&mut out, &self.track, r.loc.x, r.loc.y);
            let words = &self.knob_words[usize::from(k == Knob::Vignette)];
            put(&mut out, &words.buffer, r.loc.x - 14.0 - words.extent.w as f64, r.loc.y + SLIDER_H / 2.0 - words.extent.h as f64 / 2.0);
        }
        // The tabs, the one on lit.
        for (i, label) in self.tabs.iter().enumerate() {
            let r = Self::tab_rect(i);
            put(&mut out, &label.buffer, r.loc.x + (r.size.w - label.extent.w as f64) / 2.0, r.loc.y + (r.size.h - label.extent.h as f64) / 2.0);
        }
        let tr = Self::tab_rect(self.tab);
        put(&mut out, &self.tab_lit, tr.loc.x, tr.loc.y);
        if self.tab == 1 {
            // The clock's fonts.
            for (i, label) in self.font_samples.iter().enumerate() {
                let r = Self::font_rect(i);
                put(&mut out, &label.buffer, r.loc.x + (r.size.w - label.extent.w as f64) / 2.0, r.loc.y + (r.size.h - label.extent.h as f64) / 2.0);
                if i == self.font {
                    put(&mut out, &self.font_ring, r.loc.x - 4.0, r.loc.y - 4.0);
                }
                put(&mut out, &self.font_card, r.loc.x, r.loc.y);
            }
            put(&mut out, &self.glass, strip.loc.x, strip.loc.y);
            return out;
        }
        for (i, t) in thumbs.iter().enumerate() {
            let r = Self::thumb(i, scroll);
            if r.loc.x + r.size.w < 0.0 || r.loc.x > width {
                continue;
            }
            if let Some(t) = t {
                put(&mut out, t, r.loc.x, r.loc.y);
            }
            if i == current {
                put(&mut out, &self.ring, r.loc.x - 4.0, r.loc.y - 4.0);
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
            let r = Self::swatch(i);
            if i == 0 {
                put(&mut out, &self.hole, r.loc.x + 6.0, r.loc.y + 6.0);
                put(&mut out, &wall, r.loc.x, r.loc.y);
            } else {
                put(&mut out, &self.swatches[i - 1], r.loc.x, r.loc.y);
            }
            if i == chosen {
                put(&mut out, &self.swatch_ring, r.loc.x - 4.0, r.loc.y - 4.0);
            }
        }
        // The mode switch: the half on lit, its words centred in each.
        let lit = usize::from(self.each_mark.is_some());
        for (i, label) in self.modes.iter().enumerate() {
            let r = Self::mode(i);
            let (lw, lh) = (label.extent.w as f64, label.extent.h as f64);
            put(&mut out, &label.buffer, r.loc.x + (r.size.w - lw) / 2.0, r.loc.y + (r.size.h - lh) / 2.0);
        }
        let lr = Self::mode(lit);
        put(&mut out, &self.mode_lit, lr.loc.x, lr.loc.y);
        let r0 = Self::mode(0);
        put(&mut out, &self.mode_glass, r0.loc.x - 4.0, r0.loc.y - 4.0);
        put(&mut out, &self.glass, strip.loc.x, strip.loc.y);
        // "This panel" at the top of the one chosen for.
        if let Some(p) = self.each_mark {
            let panel = crate::layout::panels()[p].to_f64();
            let (lw, lh) = (self.this_panel.extent.w as f64, self.this_panel.extent.h as f64);
            let (gx, gy) = (panel.loc.x + (panel.size.w - 120.0) / 2.0, 24.0);
            put(&mut out, &self.this_panel.buffer, gx + (120.0 - lw) / 2.0, gy + (34.0 - lh) / 2.0 - drop);
            put(&mut out, &self.this_glass, gx, gy - drop);
        }
        out
    }
}
