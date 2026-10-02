//! An urgent notification with something to answer - Clocks' alarm ringing,
//! its Stop and Snooze - over everything, the lock screen too, as the call
//! screen is: on the left what it is, on the right its actions as buttons.
//! It lights the screen and keeps the phone awake until it is answered or
//! the app takes it back (the alarm stopped elsewhere). The lock screen
//! shows a notification's title only, without its buttons: an alarm rang
//! that nobody could stop.

use smithay::backend::input::TouchSlot;
use smithay::backend::renderer::element::memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement};
use smithay::backend::renderer::element::solid::SolidColorRenderElement;
use smithay::backend::renderer::element::{Id, Kind};
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::renderer::utils::CommitCounter;
use smithay::utils::{Logical, Physical, Point, Rectangle};

use crate::layout::{self, SCALE};
use crate::notify::Note;
use crate::shade::ShellElement;
use crate::text::{Font, Label};

const BUTTON_W: f64 = 220.0;
const BUTTON_H: f64 = 60.0;
const BUTTONS_GAP: f64 = 18.0;
const FADE_NS: f64 = 220e6;

pub struct Alert {
    /// The note shown (its id), and since when.
    shown: Option<u32>,
    since: u64,
    actions: Vec<(String, Label)>,
    fonts: Option<(Font, Font)>,
    app: Label,
    title: Label,
    body: Label,
    first_bg: MemoryRenderBuffer,
    other_bg: MemoryRenderBuffer,
    pressed: Option<(TouchSlot, usize)>,
    dim: Id,
}

impl Alert {
    pub fn new() -> Alert {
        let thin = Font::load(&["/usr/share/fonts/truetype/lato/Lato-Light.ttf", "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf"]);
        let regular = Font::load(&["/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf", "/usr/share/fonts/truetype/lato/Lato-Regular.ttf"]);
        Alert {
            shown: None,
            since: 0,
            actions: Vec::new(),
            fonts: thin.zip(regular),
            app: Label::new(18.0, [1.0, 1.0, 1.0, 0.7]),
            title: Label::new(44.0, [1.0, 1.0, 1.0, 0.97]),
            body: Label::new(20.0, [1.0, 1.0, 1.0, 0.75]),
            first_bg: crate::grid::rounded(BUTTON_W, BUTTON_H, BUTTON_H / 2.0, crate::layout::ACCENT),
            other_bg: crate::grid::rounded(BUTTON_W, BUTTON_H, BUTTON_H / 2.0, [60, 60, 60, 60]),
            pressed: None,
            dim: Id::new(),
        }
    }

    /// The urgent note now (or none): shown, or gone. Returns whether a new
    /// one came (the screen is to be lit).
    pub fn set(&mut self, note: Option<&Note>, now_ns: u64) -> bool {
        let id = note.map(|n| n.id);
        if id == self.shown {
            return false;
        }
        self.shown = id;
        self.since = now_ns;
        self.pressed = None;
        let Some(n) = note else { return false };
        let Some((thin, regular)) = &self.fonts else { return true };
        self.app.set(regular, &n.app);
        self.title.set(thin, &n.summary);
        self.body.set(regular, &n.body);
        self.actions = n
            .actions
            .iter()
            .take(3)
            .map(|(key, label)| {
                let mut l = Label::new(20.0, [1.0, 1.0, 1.0, 0.95]);
                l.set(regular, label);
                (key.clone(), l)
            })
            .collect();
        tracing::info!("alert: {} ({})", n.summary, n.actions.iter().map(|a| a.1.as_str()).collect::<Vec<_>>().join(", "));
        true
    }

    pub fn holds_screen(&self) -> bool {
        self.shown.is_some()
    }

    fn button_rect(&self, i: usize) -> Rectangle<f64, Logical> {
        let right = layout::panels()[1];
        let n = self.actions.len() as f64;
        let total = n * BUTTON_H + (n - 1.0) * BUTTONS_GAP;
        let y0 = (right.size.h as f64 - total) / 2.0 + 60.0;
        Rectangle::new((right.loc.x as f64 + (right.size.w as f64 - BUTTON_W) / 2.0, y0 + i as f64 * (BUTTON_H + BUTTONS_GAP)).into(), (BUTTON_W, BUTTON_H).into())
    }

    pub fn down(&mut self, slot: TouchSlot, pos: Point<f64, Logical>) {
        if let Some(i) = (0..self.actions.len()).find(|&i| self.button_rect(i).contains(pos)) {
            self.pressed = Some((slot, i));
        }
    }

    /// A finger let go: the action chosen, if it was on one (the note's id
    /// and the action's key).
    pub fn up(&mut self, slot: TouchSlot) -> Option<(u32, String)> {
        let (_, i) = self.pressed.take_if(|(s, _)| *s == slot)?;
        crate::fingerprint::buzz("button-pressed");
        Some((self.shown?, self.actions.get(i)?.0.clone()))
    }

    pub fn settle(&self, frame_ns: u64) -> bool {
        (frame_ns as f64) < self.since as f64 + FADE_NS + 20e6
    }

    pub fn elements(&self, renderer: &mut GlesRenderer, frame_ns: u64) -> Vec<ShellElement> {
        if self.shown.is_none() {
            return Vec::new();
        }
        let k = crate::pinpad::ease(frame_ns.saturating_sub(self.since) as f64 / FADE_NS);
        let alpha = k as f32;
        let s = SCALE as f64;
        let rise = 18.0 * (1.0 - k);
        let mut out = Vec::new();
        let mut put = |out: &mut Vec<ShellElement>, b: &MemoryRenderBuffer, x: f64, y: f64, a: f32| {
            if let Ok(e) = MemoryRenderBufferRenderElement::from_buffer(renderer, ((x * s).round(), ((y + rise) * s).round()), b, Some(a), None, None, Kind::Unspecified) {
                out.push(ShellElement::Text(e));
            }
        };
        let left = layout::panels()[0];
        let cx = |w: i32| left.loc.x as f64 + (left.size.w - w) as f64 / 2.0;
        let mut y = 230.0;
        put(&mut out, &self.app.buffer, cx(self.app.extent.w), y, alpha);
        y += self.app.extent.h as f64 + 24.0;
        put(&mut out, &self.title.buffer, cx(self.title.extent.w), y, alpha);
        y += self.title.extent.h as f64 + 14.0;
        put(&mut out, &self.body.buffer, cx(self.body.extent.w), y, alpha);
        for (i, (_, label)) in self.actions.iter().enumerate() {
            let r = self.button_rect(i);
            let a = alpha * if self.pressed.is_some_and(|(_, p)| p == i) { 0.65 } else { 1.0 };
            put(&mut out, &label.buffer, r.loc.x + (BUTTON_W - label.extent.w as f64) / 2.0, r.loc.y + (BUTTON_H - label.extent.h as f64) / 2.0, a);
            put(&mut out, if i == 0 { &self.first_bg } else { &self.other_bg }, r.loc.x, r.loc.y, a);
        }
        let (w, h) = layout::LAYOUT;
        let rect = Rectangle::<i32, Physical>::from_size((w * SCALE, h * SCALE).into());
        out.push(ShellElement::Solid(SolidColorRenderElement::new(self.dim.clone(), rect, CommitCounter::from((k * 1000.0) as usize), [0.02, 0.02, 0.03, alpha], Kind::Unspecified)));
        out
    }
}
