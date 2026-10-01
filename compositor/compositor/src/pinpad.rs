//! The PIN pad on the right panel, for the lock screen (lock.rs) and the
//! first setup (setup.rs): the digits typed as dots, a line under them, the
//! keys; it comes in softly and shakes at a wrong PIN.
//!
//! In the setup its keys are water (`water`): drops born of the setup's
//! drop over the pad - a stream comes down from it into the pad's middle,
//! parts into a drop for each row, and each row's into its three keys,
//! threads between them as they part; the digits come up on them once they
//! stand. Pressed, a key's drop gives under the finger and comes alive; let
//! go, it springs back. Leaving, they gather back the way they came.

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
/// The keys born of a drop: the stream down, the rows, the keys.
const STREAM_NS: f64 = 380e6;
const ROWS_NS: f64 = 340e6;
const KEYS_NS: f64 = 400e6;
pub const SPLIT_NS: u64 = (STREAM_NS + ROWS_NS + KEYS_NS) as u64;
/// A key's drop; the stream's at its fullest.
const KEY_DROP_R: f64 = 33.0;
const STREAM_R: f64 = 42.0;
/// Melting together while parting, apart at rest.
const PART_MELT: f64 = 26.0;
const REST_MELT: f64 = 0.8;
/// The digits coming up on the keys that stand.
const LABELS_NS: f64 = 220e6;
/// A key let go wobbles; one pressed is alive a while.
const WOBBLE_NS: f64 = 900e6;
const WOBBLE_PERIOD_NS: f64 = 360e6;
const ALIVE_NS: f64 = 700e6;

/// Drops drawn as one (Dock::group): each its middle, radius and squash;
/// how much they melt together; how alive.
pub struct Group {
    pub drops: Vec<((f64, f64), f64, (f64, f64))>,
    pub melt: f64,
    pub life: f64,
}

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
    /// The keys as water: born of a drop from when, gathering back from
    /// when; each key's last press and let go.
    water: bool,
    /// As water, born from the pad's middle each time it comes (the lock
    /// screen's), not of a drop above it (the setup's).
    bloom: bool,
    split_at: Option<u64>,
    gather_at: Option<u64>,
    poked: [u64; 12],
    popped: [u64; 12],
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
            water: false,
            bloom: false,
            split_at: None,
            gather_at: None,
            poked: [0; 12],
            popped: [0; 12],
        }
    }

    /// Its keys as water (the first setup's).
    pub fn water(&mut self) {
        self.water = true;
    }

    /// Its keys as water, born from the pad's middle each time it comes in
    /// (the lock screen's).
    pub fn water_bloom(&mut self) {
        self.water = true;
        self.bloom = true;
    }

    /// The keys born of the drop from `at`.
    pub fn split(&mut self, at: u64) {
        self.split_at = Some(at);
        self.gather_at = None;
    }

    /// The keys gathering back into the drop from `at`.
    pub fn gather(&mut self, at: u64) {
        if self.split_at.is_some() && self.gather_at.is_none() {
            self.gather_at = Some(at);
            self.pressed = None;
        }
    }

    /// How far along being born the keys are, ns (SPLIT_NS: standing);
    /// None before, or once gathered.
    fn born(&self, frame_ns: u64) -> Option<f64> {
        let s = self.split_at?;
        if frame_ns < s {
            return None;
        }
        let mut t = ((frame_ns - s) as f64).min(SPLIT_NS as f64);
        if let Some(g) = self.gather_at {
            t = t.min(SPLIT_NS as f64 - frame_ns.saturating_sub(g) as f64);
        }
        (t > 0.0).then_some(t)
    }

    /// Whether the keys stand (and take touches).
    fn standing(&self, frame_ns: u64) -> bool {
        !self.water || (self.gather_at.is_none() && self.born(frame_ns) == Some(SPLIT_NS as f64))
    }

    /// The keys as drops at `frame_ns`, the setup's drop (its middle and
    /// radius) where they come from; whether that drop is drawn with them
    /// (and not on its own).
    pub fn drops(&self, frame_ns: u64, mother: (f64, f64), mother_r: f64) -> Option<(Vec<Group>, bool)> {
        let t = self.born(frame_ns)?;
        let centre = |i: usize| {
            let r = Self::key_rect(i);
            (r.loc.x + KEY / 2.0, r.loc.y + KEY / 2.0)
        };
        let dx = self.shaken_at.map(|t| shake(frame_ns.saturating_sub(t))).unwrap_or(0.0);
        let mx = centre(1).0;
        let rows: Vec<f64> = (0..4).map(|i| centre(3 * i).1).collect();
        let middle = (mx, (rows[0] + rows[3]) / 2.0);
        let lerp = |a: f64, b: f64, k: f64| a + (b - a) * k;
        let since = |at: u64| if at == 0 { f64::MAX } else { frame_ns.saturating_sub(at) as f64 };
        if t < STREAM_NS {
            // The stream comes down from the drop, growing.
            let k = ease(t / STREAM_NS);
            let stream = ((lerp(mother.0, middle.0, k), lerp(mother.1, middle.1, k)), lerp(mother_r * 0.6, STREAM_R, k), (1.0, 1.0 + 0.25 * (1.0 - k) * k * 4.0));
            let melt = lerp(PART_MELT * 1.4, PART_MELT, k);
            return Some((vec![Group { drops: vec![(mother, mother_r, (1.0, 1.0)), stream], melt, life: 0.7 }], true));
        }
        if t < STREAM_NS + ROWS_NS {
            // It parts into a drop for each row.
            let k = ease((t - STREAM_NS) / ROWS_NS);
            let drops = rows.iter().map(|&y| ((middle.0 + dx, lerp(middle.1, y, k)), lerp(STREAM_R * 0.75, KEY_DROP_R, k), (1.0, 1.0))).collect();
            return Some((vec![Group { drops, melt: lerp(PART_MELT, PART_MELT * 0.7, k), life: 0.7 }], false));
        }
        // Each row's into its three keys; then they stand, each giving
        // under a finger and springing back.
        let k = ease((t - STREAM_NS - ROWS_NS) / KEYS_NS);
        let groups = (0..4)
            .map(|row| {
                let mut life: f64 = if k < 1.0 { 0.7 * (1.0 - k) } else { 0.0 };
                let drops = (0..3)
                    .map(|col| {
                        let i = 3 * row + col;
                        let (x, y) = centre(i);
                        let u = since(self.popped[i]);
                        let wobble = if u < WOBBLE_NS { 0.12 * (-u / (WOBBLE_NS / 4.0)).exp() * (u / WOBBLE_PERIOD_NS * std::f64::consts::TAU).sin() } else { 0.0 };
                        let mut sq = (1.0 - 0.6 * wobble, 1.0 + wobble);
                        if self.pressed == Some(i) {
                            sq = (1.07, 0.9);
                        }
                        life = life.max((1.0 - since(self.poked[i]) / ALIVE_NS).max(0.0));
                        ((lerp(mx, x, k) + dx, y), KEY_DROP_R, sq)
                    })
                    .collect();
                Group { drops, melt: lerp(PART_MELT * 0.7, REST_MELT, k), life }
            })
            .collect();
        Some((groups, false))
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
        if self.bloom {
            // From the rows' parting: there is no drop above to come from.
            self.split(t.saturating_sub(STREAM_NS as u64));
        }
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
        let now = hybris_hwc::now_ns();
        if !self.standing(now) {
            return;
        }
        self.pressed = (0..KEYS.len()).find(|&i| Self::key_rect(i).contains((x, y)));
        if let Some(i) = self.pressed {
            self.poked[i] = now;
        }
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
        self.popped[i] = hybris_hwc::now_ns();
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
        let since = |at: u64| if at == 0 { f64::MAX } else { frame_ns.saturating_sub(at) as f64 };
        frame_ns < self.shown_since + IN_NS
            || self.shaken_at.is_some_and(|t| frame_ns < t + SHAKE_NS)
            || (self.water && self.split_at.is_some_and(|s| frame_ns < s + SPLIT_NS + LABELS_NS as u64))
            || self.gather_at.is_some_and(|g| frame_ns < g + SPLIT_NS)
            || self.popped.iter().chain(self.poked.iter()).any(|&t| since(t) < WOBBLE_NS.max(ALIVE_NS))
            || self.pressed.is_some()
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
        // As water, the digits only once the keys stand, on their drops
        // (the output's).
        let labels = if !self.water {
            1.0
        } else if self.gather_at.is_some() {
            0.0
        } else {
            self.split_at.map(|s| ease((frame_ns as f64 - s as f64 - SPLIT_NS as f64) / LABELS_NS)).unwrap_or(0.0)
        };
        for (i, l) in self.keys.iter().enumerate() {
            let r = Self::key_rect(i);
            let a = alpha * labels as f32;
            if a <= 0.001 {
                continue;
            }
            let at = |out: &mut Vec<ShellElement>, renderer: &mut GlesRenderer, b: &MemoryRenderBuffer, x: f64, y: f64| {
                if let Ok(e) = MemoryRenderBufferRenderElement::from_buffer(renderer, (((x + dx) * SCALE as f64).round(), ((y + rise) * SCALE as f64).round()), b, Some(a), None, None, Kind::Unspecified) {
                    out.push(ShellElement::Text(e));
                }
            };
            match (&self.erase, KEYS[i]) {
                (Some(e), "←") => at(&mut out, renderer, e, r.loc.x + (KEY - 28.0) / 2.0, r.loc.y + (KEY - 28.0) / 2.0),
                _ => at(&mut out, renderer, &l.buffer, r.loc.x + (KEY - l.extent.w as f64) / 2.0, r.loc.y + (KEY - l.extent.h as f64) / 2.0),
            }
            if !self.water {
                at(&mut out, renderer, if self.pressed == Some(i) { &self.key_on } else { &self.key_bg }, r.loc.x, r.loc.y);
            }
        }
        out
    }
}
