//! The desktop in its editing mode: choosing the wallpaper (walls.rs).
//!
//! A finger held still on a desk for half a second (grid.rs, the watchdog
//! in main.rs) opens it: the dock dips away and the clock fades, so the
//! wallpaper shows whole, and a strip of the wallpapers rises from the
//! bottom across both panels, on a dark glass. The strip scrolls under the
//! finger and runs on when let go; a tap on a picture puts it on (it fades
//! in over the one before); the one on has a light ring. A tap above the
//! strip closes it, the strip going down and the dock coming back.

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
const STRIP_H: f64 = 176.0;
const STRIP_BOTTOM: f64 = 16.0;
const SIDE: f64 = 28.0;
/// Opening and closing.
const SLIDE_NS: f64 = 320e6;
/// A touch that moves this far scrolls.
const TAP: f64 = 10.0;
/// The run after a let go: how fast it slows (per ms).
const FRICTION: f64 = 0.0035;

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
    title: Label,
}

impl Picker {
    pub fn new() -> Picker {
        let width = layout::LAYOUT.0 as f64 - 2.0 * SIDE;
        let mut title = Label::new(15.0, [1.0, 1.0, 1.0, 0.75]);
        if let Some(f) = Font::load(&["/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf", "/usr/share/fonts/truetype/lato/Lato-Regular.ttf"]) {
            title.set(&f, "Wallpaper");
        }
        Picker {
            open: false,
            since: 0,
            scroll: 0.0,
            hold: None,
            run: None,
            opener: None,
            glass: crate::grid::rounded(width, STRIP_H, 26.0, [8, 10, 14, 150]),
            ring: crate::grid::rounded(THUMB_W + 8.0, THUMB_H + 8.0, 17.0, [235, 235, 235, 235]),
            title,
        }
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
    pub fn down(&mut self, slot: TouchSlot, pos: Point<f64, Logical>, time_us: u64, count: usize) -> bool {
        if !self.open {
            return false;
        }
        let now = hybris_hwc::now_ns();
        self.scroll = self.scroll_at(now, count);
        self.run = None;
        self.hold = Some(Hold { slot, start: pos, from: self.scroll, last: (time_us, pos.x), velocity: 0.0, moved: false });
        true
    }

    pub fn holds(&self, slot: TouchSlot) -> bool {
        self.hold.as_ref().is_some_and(|h| h.slot == slot) || self.opener == Some(slot)
    }

    pub fn motion(&mut self, slot: TouchSlot, pos: Point<f64, Logical>, time_us: u64) {
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
    pub fn up(&mut self, slot: TouchSlot, count: usize) -> Option<usize> {
        if self.opener == Some(slot) {
            self.opener = None;
            return None;
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
        if h.start.y < Self::strip().loc.y {
            self.close();
            return None;
        }
        (0..count).find(|&i| Self::thumb(i, scroll).contains(h.start))
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
        sliding || self.run.is_some() || self.hold.is_some()
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
        put(&mut out, &self.title.buffer, strip.loc.x + 22.0, strip.loc.y + 16.0);
        put(&mut out, &self.glass, strip.loc.x, strip.loc.y);
        out
    }
}
