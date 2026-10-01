//! The system's own dialog: when something outside an app asks the user for
//! a secret (polkit for the PIN, polkit.rs; later NetworkManager for a
//! Wi-Fi password), the compositor asks, not a window of some program -
//! only the shell asks for secrets (docs/LOCK-AND-SETUP.md).
//!
//! It comes over everything but the lock screen, the screen dimmed under
//! it: on the left what asks and why, and Cancel; on the right where to
//! answer - the PIN pad (pinpad.rs), or for words the compositor's own
//! keyboard (keys.rs), the words shown as dots in a field on the left (a
//! tap on it shows them). It fades in and out; a wrong answer shakes the
//! pad. Every touch is its own while it is up.

use smithay::backend::input::TouchSlot;
use smithay::backend::renderer::element::memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement};
use smithay::backend::renderer::element::solid::SolidColorRenderElement;
use smithay::backend::renderer::element::{Id, Kind};
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::renderer::utils::CommitCounter;
use smithay::utils::{Logical, Physical, Point, Rectangle};

use crate::layout::{self, SCALE};
use crate::pinpad::{ease, PinPad, Press};
use crate::shade::ShellElement;
use crate::text::{Font, Label};

const FADE_NS: f64 = 220e6;
const BUTTON_W: f64 = 200.0;
const BUTTON_H: f64 = 52.0;
/// The words' room on the left panel.
const SIDE: f64 = 56.0;
const TITLE_Y: f64 = 150.0;

/// What the user answered.
pub enum Answer {
    Pin(String),
    Text(String),
    Cancel,
}

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Pin,
    Text,
}

const FIELD_H: f64 = 56.0;

pub struct Dialog {
    open: bool,
    since: u64,
    title: Label,
    lines: Vec<Label>,
    pad: PinPad,
    keys: crate::keys::Keys,
    mode: Mode,
    /// The words typed, as shown in the field; whether shown as they are.
    field: Label,
    reveal: bool,
    hint: Label,
    field_bg: MemoryRenderBuffer,
    on_field: Option<TouchSlot>,
    fonts: Option<(Font, Font)>,
    cancel: Label,
    button: MemoryRenderBuffer,
    /// A finger on Cancel.
    on_cancel: Option<TouchSlot>,
    dim: Id,
    left_id: Id,
}

impl Dialog {
    pub fn new() -> Dialog {
        let thin = Font::load(&["/usr/share/fonts/truetype/lato/Lato-Light.ttf", "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf"]);
        let regular = Font::load(&["/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf", "/usr/share/fonts/truetype/lato/Lato-Regular.ttf"]);
        let mut cancel = Label::new(18.0, [1.0, 1.0, 1.0, 0.95]);
        if let Some(f) = &regular {
            cancel.set(f, "Cancel");
        }
        Dialog {
            open: false,
            since: 0,
            title: Label::new(36.0, [1.0, 1.0, 1.0, 0.95]),
            lines: Vec::new(),
            pad: PinPad::new(),
            keys: crate::keys::Keys::new(),
            mode: Mode::Pin,
            field: Label::new(22.0, [1.0, 1.0, 1.0, 0.95]),
            reveal: false,
            hint: Label::new(14.0, [1.0, 1.0, 1.0, 0.6]),
            field_bg: crate::grid::rounded(layout::panels()[0].size.w as f64 - 2.0 * SIDE, FIELD_H, 16.0, [255, 255, 255, 30]),
            on_field: None,
            fonts: thin.zip(regular),
            cancel,
            button: crate::grid::rounded(BUTTON_W, BUTTON_H, BUTTON_H / 2.0, [60, 60, 60, 60]),
            on_cancel: None,
            dim: Id::new(),
            left_id: Id::new(),
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Whether it is on screen at all (open, or fading out).
    pub fn visible(&self, frame_ns: u64) -> bool {
        self.open || (frame_ns as f64) < self.since as f64 + FADE_NS
    }

    fn shown(&self, frame_ns: u64) -> f64 {
        let k = ease(((frame_ns as f64 - self.since as f64) / FADE_NS).clamp(0.0, 1.0));
        if self.open { k } else { 1.0 - k }
    }

    /// Asks for the PIN: `title`, and `message` wrapped on the left.
    pub fn ask_pin(&mut self, title: &str, message: &str) {
        self.mode = Mode::Pin;
        self.words(title, message);
        self.pad.show();
        self.pad.say("Enter your PIN");
        self.open = true;
        self.since = hybris_hwc::now_ns();
        tracing::info!("dialog: up");
    }

    /// Asks for words (a password): `title`, `message`, the answer key's
    /// word, and a hint under the field.
    pub fn ask_text(&mut self, title: &str, message: &str, enter: &str, hint: &str) {
        self.mode = Mode::Text;
        self.words(title, message);
        self.keys.clear();
        self.keys.enter_word = enter.to_owned();
        self.reveal = false;
        self.show_field();
        if let Some((_, regular)) = &self.fonts {
            self.hint.set(regular, hint);
        }
        self.open = true;
        self.since = hybris_hwc::now_ns();
        tracing::info!("dialog: up (words)");
    }

    /// The field again: dots, or the words when shown.
    fn show_field(&mut self) {
        let Some((_, regular)) = &self.fonts else { return };
        let shown = if self.reveal { self.keys.text.clone() } else { "•".repeat(self.keys.text.chars().count()) };
        self.field.set(regular, &shown);
    }

    /// Words the answer was wrong for: the hint says so, the field empties.
    pub fn wrong_text(&mut self, hint: &str) {
        self.keys.clear();
        self.show_field();
        if let Some((_, regular)) = &self.fonts {
            self.hint.set(regular, hint);
        }
        crate::fingerprint::buzz("bell-terminal");
    }

    fn field_rect() -> Rectangle<f64, Logical> {
        let left = layout::panels()[0];
        Rectangle::new((left.loc.x as f64 + SIDE, 430.0).into(), (left.size.w as f64 - 2.0 * SIDE, FIELD_H).into())
    }

    fn words(&mut self, title: &str, message: &str) {
        let Some((thin, regular)) = &self.fonts else { return };
        self.title.set(thin, title);
        let width = (layout::panels()[0].size.w as f64 - 2.0 * SIDE) * SCALE as f64;
        self.lines = wrap(regular, message, 17.0, width)
            .into_iter()
            .map(|line| {
                let mut l = Label::new(17.0, [1.0, 1.0, 1.0, 0.72]);
                l.set(regular, &line);
                l
            })
            .collect();
    }

    pub fn close(&mut self) {
        if self.open {
            self.open = false;
            self.since = hybris_hwc::now_ns();
            self.pad.clear();
            self.keys.clear();
            self.on_cancel = None;
            self.on_field = None;
            tracing::info!("dialog: down");
        }
    }

    /// The answer is being checked.
    pub fn checking(&mut self) {
        self.pad.say("Checking…");
    }

    /// The answer was wrong: try again.
    pub fn wrong(&mut self) {
        self.pad.clear();
        self.pad.say("Wrong PIN, try again");
        self.pad.shake();
        crate::fingerprint::buzz("bell-terminal");
    }

    fn button_rect() -> Rectangle<f64, Logical> {
        let left = layout::panels()[0];
        Rectangle::new(((left.loc.x as f64 + (left.size.w as f64 - BUTTON_W) / 2.0), left.size.h as f64 - 150.0).into(), (BUTTON_W, BUTTON_H).into())
    }

    pub fn down(&mut self, slot: TouchSlot, pos: Point<f64, Logical>) {
        if Self::button_rect().contains(pos) {
            self.on_cancel = Some(slot);
        } else if self.mode == Mode::Text && Self::field_rect().contains(pos) {
            self.on_field = Some(slot);
        } else if self.mode == Mode::Text {
            self.keys.down(slot, pos);
        } else {
            self.pad.down(pos.x, pos.y);
        }
    }

    pub fn motion(&mut self, slot: TouchSlot, pos: Point<f64, Logical>) {
        if self.on_cancel == Some(slot) && !Self::button_rect().contains(pos) {
            self.on_cancel = None;
        }
        if self.on_cancel.is_none() && self.pad.pressing() {
            self.pad.slide();
        }
    }

    pub fn up(&mut self, slot: TouchSlot) -> Option<Answer> {
        if self.on_cancel.take_if(|s| *s == slot).is_some() {
            return Some(Answer::Cancel);
        }
        if self.on_field.take_if(|s| *s == slot).is_some() {
            self.reveal = !self.reveal;
            self.show_field();
            return None;
        }
        if self.mode == Mode::Text {
            return match self.keys.up(slot) {
                Some(crate::keys::Typed::Edit) => {
                    self.show_field();
                    None
                }
                Some(crate::keys::Typed::Enter) if !self.keys.text.is_empty() => Some(Answer::Text(std::mem::take(&mut self.keys.text))),
                _ => None,
            };
        }
        match self.pad.up() {
            Some(Press::Ok) => Some(Answer::Pin(self.pad.take())),
            _ => None,
        }
    }

    /// After a frame: whether it still moves.
    pub fn settle(&self, frame_ns: u64) -> bool {
        ((frame_ns as f64) < self.since as f64 + FADE_NS + 20e6) || (self.open && self.pad.moving(frame_ns))
    }

    pub fn elements(&self, renderer: &mut GlesRenderer, frame_ns: u64) -> Vec<ShellElement> {
        let k = self.shown(frame_ns);
        if k <= 0.0 {
            return Vec::new();
        }
        let alpha = k as f32;
        let s = SCALE as f64;
        let rise = 14.0 * (1.0 - k);
        let mut out = Vec::new();
        match self.mode {
            Mode::Pin => out.extend(self.pad.elements(renderer, frame_ns, 0.0)),
            Mode::Text => out.extend(self.keys.elements(renderer, alpha, rise)),
        }
        let left = layout::panels()[0];
        let mut put = |out: &mut Vec<ShellElement>, b: &MemoryRenderBuffer, x: f64, y: f64, a: f32| {
            if let Ok(e) = MemoryRenderBufferRenderElement::from_buffer(renderer, ((x * s).round(), ((y + rise) * s).round()), b, Some(a), None, None, Kind::Unspecified) {
                out.push(ShellElement::Text(e));
            }
        };
        let cx = |w: i32| left.loc.x as f64 + (left.size.w - w) as f64 / 2.0;
        put(&mut out, &self.title.buffer, cx(self.title.extent.w), TITLE_Y, alpha);
        let mut y = TITLE_Y + self.title.extent.h as f64 + 24.0;
        for l in &self.lines {
            put(&mut out, &l.buffer, cx(l.extent.w), y, alpha);
            y += l.extent.h as f64 + 6.0;
        }
        if self.mode == Mode::Text {
            let f = Self::field_rect();
            put(&mut out, &self.field.buffer, f.loc.x + 20.0, f.loc.y + (FIELD_H - self.field.extent.h as f64) / 2.0, alpha);
            put(&mut out, &self.field_bg, f.loc.x, f.loc.y, alpha);
            put(&mut out, &self.hint.buffer, cx(self.hint.extent.w), f.loc.y + FIELD_H + 12.0, alpha);
        }
        let b = Self::button_rect();
        let pressed = if self.on_cancel.is_some() { 0.6 } else { 1.0 };
        put(&mut out, &self.cancel.buffer, b.loc.x + (BUTTON_W - self.cancel.extent.w as f64) / 2.0, b.loc.y + (BUTTON_H - self.cancel.extent.h as f64) / 2.0, alpha * pressed);
        put(&mut out, &self.button, b.loc.x, b.loc.y, alpha);
        // The screen dimmed under it, darker on the left where it speaks.
        let (w, h) = layout::LAYOUT;
        let commit = CommitCounter::from((k * 1000.0) as usize);
        let lrect = Rectangle::<i32, Physical>::new(left.loc.to_physical(SCALE), left.size.to_physical(SCALE));
        out.push(ShellElement::Solid(SolidColorRenderElement::new(self.left_id.clone(), lrect, commit, [0.0, 0.0, 0.0, 0.35 * alpha], Kind::Unspecified)));
        let rect = Rectangle::<i32, Physical>::from_size((w * SCALE, h * SCALE).into());
        out.push(ShellElement::Solid(SolidColorRenderElement::new(self.dim.clone(), rect, commit, [0.0, 0.0, 0.0, 0.6 * alpha], Kind::Unspecified)));
        out
    }
}

/// `text` in lines no wider than `width` physical px at `size`.
fn wrap(font: &Font, text: &str, size: f32, width: f64) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        let candidate = if line.is_empty() { word.to_owned() } else { format!("{line} {word}") };
        if !line.is_empty() && font.rasterize(&candidate, size, [1.0; 4]).1 as f64 > width {
            lines.push(std::mem::replace(&mut line, word.to_owned()));
        } else {
            line = candidate;
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}
