//! The PIN pad on the right panel, for the lock screen (lock.rs) and the
//! first setup (setup.rs): the digits typed as dots, a line under them, the
//! keys; it comes in softly and shakes at a wrong PIN.

use smithay::backend::renderer::element::memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement};
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::utils::Rectangle;

use crate::layout::{self, SCALE};
use crate::shade::ShellElement;
use crate::text::{Font, Label};

/// The pad's keys, row by row.
const KEYS: [&str; 12] = ["1", "2", "3", "4", "5", "6", "7", "8", "9", "←", "0", "OK"];
const KEY: f64 = 76.0;
const KEY_GAP: f64 = 22.0;
const PAD_TOP: f64 = 330.0;
const DOTS_Y: f64 = 220.0;
const MAX_PIN: usize = 16;
/// Coming in, and a shake.
pub const IN_NS: u64 = 200_000_000;
pub const SHAKE_NS: u64 = 420_000_000;

/// Ease in and out (cubic).
pub fn ease(k: f64) -> f64 {
    let k = k.clamp(0.0, 1.0);
    if k < 0.5 { 4.0 * k * k * k } else { 1.0 - (-2.0 * k + 2.0).powi(3) / 2.0 }
}

/// A damped shake's offset, logical px, `t` ns into it.
pub fn shake(t: u64) -> f64 {
    if t >= SHAKE_NS {
        return 0.0;
    }
    let u = t as f64 / SHAKE_NS as f64;
    14.0 * (1.0 - u) * (u * std::f64::consts::TAU * 3.5).sin()
}

/// What a key let go did.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Press {
    /// A digit or the backspace: what is typed changed.
    Edit,
    /// OK, with something typed.
    Ok,
}

pub struct PinPad {
    pub pin: String,
    pressed: Option<usize>,
    font: Option<Font>,
    message: Label,
    keys: Vec<Label>,
    key_bg: MemoryRenderBuffer,
    key_on: MemoryRenderBuffer,
    dot: MemoryRenderBuffer,
    /// The dots while a wrong PIN shakes them.
    dot_red: MemoryRenderBuffer,
    /// The backspace key's icon (the fonts have no arrow).
    erase: Option<MemoryRenderBuffer>,
    shown_since: u64,
    shaken_at: Option<u64>,
}

impl PinPad {
    pub fn new() -> PinPad {
        let font = Font::load(&["/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf", "/usr/share/fonts/truetype/lato/Lato-Regular.ttf"]);
        let mut keys: Vec<Label> = KEYS.iter().map(|_| Label::new(28.0, [1.0, 1.0, 1.0, 0.95])).collect();
        let mut message = Label::new(16.0, [1.0, 1.0, 1.0, 0.7]);
        if let Some(f) = &font {
            for (l, k) in keys.iter_mut().zip(KEYS) {
                l.set(f, k);
            }
            message.set(f, "Enter PIN");
        }
        PinPad {
            pin: String::new(),
            pressed: None,
            font,
            message,
            keys,
            key_bg: crate::grid::rounded(KEY, KEY, KEY / 2.0, [30, 30, 30, 30]),
            key_on: crate::grid::rounded(KEY, KEY, KEY / 2.0, [77, 77, 77, 77]),
            dot: crate::grid::rounded(12.0, 12.0, 6.0, [240, 240, 240, 240]),
            dot_red: crate::grid::rounded(12.0, 12.0, 6.0, [235, 80, 70, 255]),
            erase: crate::quick::symbolic("/usr/share/icons/Adwaita/symbolic/ui/edit-clear-symbolic.svg", 28)
                .or_else(|| crate::quick::symbolic("/usr/share/icons/Adwaita/symbolic/actions/edit-clear-symbolic.svg", 28)),
            shown_since: 0,
            shaken_at: None,
        }
    }

    /// The line under the dots.
    pub fn say(&mut self, text: &str) {
        if let Some(f) = &self.font {
            self.message.set(f, text);
        }
    }

    /// It comes in now, empty.
    pub fn show(&mut self) {
        self.show_at(hybris_hwc::now_ns());
    }

    /// It comes in at `t` (ns), empty.
    pub fn show_at(&mut self, t: u64) {
        self.shown_since = t;
        self.clear();
    }

    pub fn clear(&mut self) {
        unsafe { self.pin.as_bytes_mut().fill(0) };
        self.pin.clear();
        self.pressed = None;
    }

    /// A wrong PIN.
    pub fn shake(&mut self) {
        self.shaken_at = Some(hybris_hwc::now_ns());
    }

    /// What is typed, taken (the pad is left empty).
    pub fn take(&mut self) -> String {
        std::mem::take(&mut self.pin)
    }

    /// Key `i`'s rect on the right panel, logical px.
    fn key_rect(i: usize) -> Rectangle<f64, smithay::utils::Logical> {
        let p = layout::panels()[1];
        let w = 3.0 * KEY + 2.0 * KEY_GAP;
        let x0 = p.loc.x as f64 + (p.size.w as f64 - w) / 2.0;
        let (col, row) = ((i % 3) as f64, (i / 3) as f64);
        Rectangle::new((x0 + col * (KEY + KEY_GAP), PAD_TOP + row * (KEY + KEY_GAP)).into(), (KEY, KEY).into())
    }

    /// The OK key's middle: an unlock's wave starts there.
    pub fn ok_centre() -> (f64, f64) {
        let r = Self::key_rect(KEYS.len() - 1);
        (r.loc.x + KEY / 2.0, r.loc.y + KEY / 2.0)
    }

    /// A finger down: on a key, it is pressed.
    pub fn down(&mut self, x: f64, y: f64) {
        self.pressed = (0..KEYS.len()).find(|&i| Self::key_rect(i).contains((x, y)));
    }

    /// The finger moved off: no key.
    pub fn slide(&mut self) {
        self.pressed = None;
    }

    pub fn pressing(&self) -> bool {
        self.pressed.is_some()
    }

    /// The finger let go: the key pressed, done.
    pub fn up(&mut self) -> Option<Press> {
        let i = self.pressed.take()?;
        match KEYS[i] {
            "←" => {
                self.pin.pop();
                Some(Press::Edit)
            }
            "OK" => (!self.pin.is_empty()).then_some(Press::Ok),
            d => {
                if self.pin.len() < MAX_PIN {
                    self.pin.push_str(d);
                }
                Some(Press::Edit)
            }
        }
    }

    /// Whether it still moves (coming in, shaking).
    pub fn moving(&self, frame_ns: u64) -> bool {
        frame_ns < self.shown_since + IN_NS || self.shaken_at.is_some_and(|t| frame_ns < t + SHAKE_NS)
    }

    pub fn buffers(&self) -> impl Iterator<Item = &MemoryRenderBuffer> {
        [&self.key_bg, &self.key_on, &self.dot, &self.dot_red].into_iter().chain(self.erase.iter()).chain(self.keys.iter().map(|l| &l.buffer)).chain(std::iter::once(&self.message.buffer))
    }

    /// The pad, moved `dx` logical px (with its half of the screen).
    pub fn elements(&self, renderer: &mut GlesRenderer, frame_ns: u64, dx: f64) -> Vec<ShellElement> {
        let right = layout::panels()[1];
        let k = (frame_ns.saturating_sub(self.shown_since) as f64 / IN_NS as f64).clamp(0.0, 1.0);
        let alpha = ease(k) as f32;
        let rise = 40.0 * (1.0 - ease(k));
        let dx = dx + self.shaken_at.map(|t| shake(frame_ns.saturating_sub(t))).unwrap_or(0.0);
        let mut out = Vec::new();
        let mut put = |out: &mut Vec<ShellElement>, b: &MemoryRenderBuffer, x: f64, y: f64| {
            if let Ok(e) = MemoryRenderBufferRenderElement::from_buffer(renderer, (((x + dx) * SCALE as f64).round(), ((y + rise) * SCALE as f64).round()), b, Some(alpha), None, None, Kind::Unspecified) {
                out.push(ShellElement::Text(e));
            }
        };
        // What is typed, as dots (red while a wrong PIN shakes them); what
        // is going on, under them.
        let shaking = self.shaken_at.is_some_and(|t| frame_ns < t + SHAKE_NS);
        let dot = if shaking { &self.dot_red } else { &self.dot };
        // A wrong PIN's dots stay to be seen red, cleared after.
        let n = self.pin.len() as f64;
        let dots_w = n * 12.0 + (n - 1.0).max(0.0) * 10.0;
        for i in 0..self.pin.len() {
            put(&mut out, dot, right.loc.x as f64 + (right.size.w as f64 - dots_w) / 2.0 + i as f64 * 22.0, DOTS_Y);
        }
        put(&mut out, &self.message.buffer, (right.loc.x + (right.size.w - self.message.extent.w) / 2) as f64, DOTS_Y + 36.0);
        for (i, l) in self.keys.iter().enumerate() {
            let r = Self::key_rect(i);
            match (&self.erase, KEYS[i]) {
                (Some(e), "←") => put(&mut out, e, r.loc.x + (KEY - 28.0) / 2.0, r.loc.y + (KEY - 28.0) / 2.0),
                _ => put(&mut out, &l.buffer, r.loc.x + (KEY - l.extent.w as f64) / 2.0, r.loc.y + (KEY - l.extent.h as f64) / 2.0),
            }
            put(&mut out, if self.pressed == Some(i) { &self.key_on } else { &self.key_bg }, r.loc.x, r.loc.y);
        }
        out
    }
}
