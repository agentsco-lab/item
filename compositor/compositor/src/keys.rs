//! A keyboard of the compositor's own, for the system's dialog (dialog.rs):
//! a password is typed here, not through the on-screen keyboard's input
//! method, so it goes nowhere but the dialog. On the right panel: letters
//! with shift, a layer of digits and signs, space, delete and the answer.

use smithay::backend::input::TouchSlot;
use smithay::backend::renderer::element::memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement};
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::utils::{Logical, Point, Rectangle};

use crate::layout::{self, SCALE};
use crate::shade::ShellElement;
use crate::text::{Font, Label};

const KEY_H: f64 = 62.0;
const GAP: f64 = 8.0;
const SIDE: f64 = 12.0;
/// The keyboard's top on the right panel.
const TOP: f64 = 520.0;

const LETTERS: [&str; 3] = ["qwertyuiop", "asdfghjkl", "zxcvbnm"];
const SIGNS: [&str; 3] = ["1234567890", "-/:;()$&@\"", ".,?!'#%"];

/// A key: what it does.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Key {
    Char(char),
    Shift,
    Delete,
    Layer,
    Space,
    Enter,
}

/// What a key let go did.
pub enum Typed {
    Edit,
    Enter,
}

pub struct Keys {
    pub text: String,
    shift: bool,
    signs: bool,
    pressed: Option<(TouchSlot, usize)>,
    font: Option<Font>,
    labels: std::cell::RefCell<std::collections::HashMap<String, Label>>,
    bg: MemoryRenderBuffer,
    bg_on: MemoryRenderBuffer,
    wide: MemoryRenderBuffer,
    space: MemoryRenderBuffer,
    enter: MemoryRenderBuffer,
    /// The answer key's word.
    pub enter_word: String,
}

impl Keys {
    pub fn new() -> Keys {
        let (w, _) = Self::unit();
        Keys {
            text: String::new(),
            shift: false,
            signs: false,
            pressed: None,
            font: Font::load(&["/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf", "/usr/share/fonts/truetype/lato/Lato-Regular.ttf"]),
            labels: Default::default(),
            bg: crate::grid::rounded(w, KEY_H, 12.0, [40, 40, 40, 40]),
            bg_on: crate::grid::rounded(w, KEY_H, 12.0, [90, 90, 90, 90]),
            wide: crate::grid::rounded(w * 1.5 + GAP / 2.0, KEY_H, 12.0, [28, 28, 28, 28]),
            space: crate::grid::rounded(w * 5.0 + GAP * 4.0, KEY_H, 12.0, [40, 40, 40, 40]),
            enter: crate::grid::rounded(w * 2.5 + GAP * 1.5, KEY_H, 12.0, [0x35, 0x84, 0xe4, 255]),
            enter_word: "Done".into(),
        }
    }

    /// A key's width and the row's step.
    fn unit() -> (f64, f64) {
        let width = layout::panels()[1].size.w as f64 - 2.0 * SIDE;
        let w = (width - 9.0 * GAP) / 10.0;
        (w, w + GAP)
    }

    pub fn clear(&mut self) {
        unsafe { self.text.as_bytes_mut().fill(0) };
        self.text.clear();
        self.shift = false;
        self.signs = false;
        self.pressed = None;
    }

    /// Every key and its rect, logical px.
    fn keys(&self) -> Vec<(Key, Rectangle<f64, Logical>)> {
        let p = layout::panels()[1];
        let (w, step) = Self::unit();
        let x0 = p.loc.x as f64 + SIDE;
        let rows = if self.signs { SIGNS } else { LETTERS };
        let mut out = Vec::new();
        for (r, row) in rows.iter().enumerate() {
            let y = TOP + r as f64 * (KEY_H + GAP);
            let n = row.chars().count() as f64;
            // Short rows centred; the third has shift and delete beside it.
            let mut x = x0 + (10.0 - n) * step / 2.0;
            // The third row: shift on the left (letters only), delete on
            // the right.
            if r == 2 {
                if !self.signs {
                    out.push((Key::Shift, Rectangle::new((x0, y).into(), (w * 1.5 + GAP / 2.0, KEY_H).into())));
                }
                out.push((Key::Delete, Rectangle::new((x0 + 10.0 * step - GAP - (w * 1.5 + GAP / 2.0), y).into(), (w * 1.5 + GAP / 2.0, KEY_H).into())));
            }
            for c in row.chars() {
                let c = if self.shift && !self.signs { c.to_ascii_uppercase() } else { c };
                out.push((Key::Char(c), Rectangle::new((x, y).into(), (w, KEY_H).into())));
                x += step;
            }
        }
        let y = TOP + 3.0 * (KEY_H + GAP);
        out.push((Key::Layer, Rectangle::new((x0, y).into(), (w * 1.5 + GAP / 2.0, KEY_H).into())));
        let sx = x0 + w * 1.5 + GAP * 1.5;
        out.push((Key::Space, Rectangle::new((sx, y).into(), (w * 5.0 + GAP * 4.0, KEY_H).into())));
        let ex = sx + w * 5.0 + GAP * 5.0;
        out.push((Key::Enter, Rectangle::new((ex, y).into(), (w * 2.5 + GAP * 1.5, KEY_H).into())));
        out
    }

    pub fn down(&mut self, slot: TouchSlot, pos: Point<f64, Logical>) -> bool {
        let keys = self.keys();
        match keys.iter().position(|(_, r)| r.contains(pos)) {
            Some(i) => {
                self.pressed = Some((slot, i));
                true
            }
            None => false,
        }
    }

    pub fn up(&mut self, slot: TouchSlot) -> Option<Typed> {
        let (_, i) = self.pressed.take_if(|(s, _)| *s == slot)?;
        let (key, _) = *self.keys().get(i)?;
        match key {
            Key::Char(c) => {
                if self.text.len() < 128 {
                    self.text.push(c);
                }
                // One letter shifted, as phones do.
                self.shift = false;
                Some(Typed::Edit)
            }
            Key::Shift => {
                self.shift = !self.shift;
                None
            }
            Key::Delete => {
                self.text.pop();
                Some(Typed::Edit)
            }
            Key::Layer => {
                self.signs = !self.signs;
                None
            }
            Key::Space => {
                self.text.push(' ');
                Some(Typed::Edit)
            }
            Key::Enter => Some(Typed::Enter),
        }
    }

    fn label(&self, text: &str, size: f32) -> Option<(MemoryRenderBuffer, i32, i32)> {
        let font = self.font.as_ref()?;
        let mut labels = self.labels.borrow_mut();
        let l = labels.entry(format!("{size}:{text}")).or_insert_with(|| {
            let mut l = Label::new(size, [1.0, 1.0, 1.0, 0.95]);
            l.set(font, text);
            l
        });
        Some((l.buffer.clone(), l.extent.w, l.extent.h))
    }

    pub fn elements(&self, renderer: &mut GlesRenderer, alpha: f32, rise: f64) -> Vec<ShellElement> {
        let mut out = Vec::new();
        let s = SCALE as f64;
        let mut put = |out: &mut Vec<ShellElement>, b: &MemoryRenderBuffer, x: f64, y: f64| {
            if let Ok(e) = MemoryRenderBufferRenderElement::from_buffer(renderer, ((x * s).round(), ((y + rise) * s).round()), b, Some(alpha), None, None, Kind::Unspecified) {
                out.push(ShellElement::Text(e));
            }
        };
        for (i, (key, r)) in self.keys().iter().enumerate() {
            let word = match key {
                Key::Char(c) => c.to_string(),
                Key::Shift => if self.shift { "⇧".into() } else { "shift".into() },
                Key::Delete => "⌫".into(),
                Key::Layer => if self.signs { "abc".into() } else { "?123".into() },
                Key::Space => "space".into(),
                Key::Enter => self.enter_word.clone(),
            };
            let size = if matches!(key, Key::Char(_)) { 24.0 } else { 16.0 };
            if let Some((b, w, h)) = self.label(&word, size) {
                put(&mut out, &b, r.loc.x + (r.size.w - w as f64) / 2.0, r.loc.y + (r.size.h - h as f64) / 2.0);
            }
            let on = self.pressed.is_some_and(|(_, p)| p == i) || (*key == Key::Shift && self.shift);
            let bg = match key {
                Key::Enter => &self.enter,
                Key::Space => &self.space,
                Key::Shift | Key::Delete | Key::Layer => &self.wide,
                Key::Char(_) => if on { &self.bg_on } else { &self.bg },
            };
            put(&mut out, bg, r.loc.x, r.loc.y);
        }
        out
    }
}
