//! Locking, blanking and the power key.
//!
//! - The power key (qpnp_pon, KEY_POWER) with the screen on blanks it - the
//!   display's hwc2 power mode OFF, panel and backlight dark, no frames - and
//!   locks. With the screen off it lights it again, locked.
//! - The session's idle delay (org.gnome.desktop.session idle-delay, read at
//!   start; 0: never) does the same after that long without a touch.
//! - The lock screen is drawn by the compositor over everything: the time
//!   and the date on the left panel, a line on how to unlock on both. A
//!   swipe up brings the PIN pad on the right panel; the PIN is checked as
//!   phosh checks it (pam.rs), on a thread, and when it is right the lock
//!   screen fades out over 300 ms, as item's (patches/phosh/0005). A swipe
//!   down on the pad puts it away.
//! - The volume keys (gpio-keys) set the volume, locked or not.

use smithay::backend::renderer::element::memory::MemoryRenderBufferRenderElement;
use smithay::backend::renderer::element::solid::SolidColorRenderElement;
use smithay::backend::renderer::element::{Id, Kind};
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::renderer::utils::CommitCounter;
use smithay::backend::input::TouchSlot;
use smithay::utils::{Physical, Rectangle};

use crate::layout::{self, SCALE};
use crate::shade::{date_line, local_time, ShellElement};
use crate::text::{Font, Label};

const FADE_NS: u64 = 300_000_000;
/// A swipe up this far unlocks, or a fling up.
const UNLOCK: f64 = 120.0;
const FLING: f64 = 0.5;

/// The PIN pad's keys, row by row.
const KEYS: [&str; 12] = ["1", "2", "3", "4", "5", "6", "7", "8", "9", "←", "0", "OK"];
const KEY: f64 = 76.0;
const KEY_GAP: f64 = 22.0;
const PAD_TOP: f64 = 330.0;
const DOTS_Y: f64 = 220.0;
const MAX_PIN: usize = 16;

pub struct Lock {
    /// The PIN pad is up, and what is typed.
    entering: bool,
    pin: String,
    pressed: Option<usize>,
    /// A check running on its thread, and its answer when it comes.
    checking: std::sync::Arc<std::sync::Mutex<Option<Option<bool>>>>,
    wake: smithay::reexports::calloop::ping::Ping,
    message: Label,
    keys: Vec<Label>,
    key_bg: smithay::backend::renderer::element::memory::MemoryRenderBuffer,
    key_on: smithay::backend::renderer::element::memory::MemoryRenderBuffer,
    dot: smithay::backend::renderer::element::memory::MemoryRenderBuffer,
    /// The backspace key's icon (the fonts have no arrow).
    erase: Option<smithay::backend::renderer::element::memory::MemoryRenderBuffer>,
    pub locked: bool,
    pub blank: bool,
    /// When the unlock began: the lock screen fading.
    fading: Option<u64>,
    grab: Option<(TouchSlot, f64, (u64, f64), f64)>,
    /// How far the finger has lifted the lock screen, logical px.
    lift: f64,
    fonts: Option<(Font, Font)>,
    time: Label,
    date: Label,
    hint: Label,
    /// The hint at the foot of the panels, and a passing notice in its place
    /// (the fingerprint reader's) until when.
    hint_base: String,
    notice_until: u64,
    minute: i32,
    id: Id,
    /// When the last touch came, for the idle delay.
    pub last_touch_ns: u64,
    pub idle_ns: u64,
    /// The display was lit again since last asked.
    relit: bool,
    /// When the last unlock began, not yet asked for.
    unlocked: Option<u64>,
}

impl Lock {
    pub fn new(wake: smithay::reexports::calloop::ping::Ping) -> Lock {
        let thin = Font::load(&["/usr/share/fonts/truetype/lato/Lato-Light.ttf", "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf"]);
        let regular = Font::load(&["/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf", "/usr/share/fonts/truetype/lato/Lato-Regular.ttf"]);
        let idle_s = std::process::Command::new("gsettings")
            .args(["get", "org.gnome.desktop.session", "idle-delay"])
            .output()
            .ok()
            .and_then(|o| String::from_utf8_lossy(&o.stdout).split_whitespace().last().and_then(|v| v.parse::<u64>().ok()))
            .unwrap_or(0);
        tracing::info!("lock: idle delay {}", if idle_s == 0 { "never".into() } else { format!("{idle_s} s") });
        let mut lock = Lock {
            entering: false,
            pin: String::new(),
            pressed: None,
            checking: Default::default(),
            wake,
            message: Label::new(16.0, [1.0, 1.0, 1.0, 0.7]),
            keys: KEYS.iter().map(|_| Label::new(28.0, [1.0, 1.0, 1.0, 0.95])).collect(),
            key_bg: crate::grid::rounded(KEY, KEY, KEY / 2.0, [30, 30, 30, 30]),
            key_on: crate::grid::rounded(KEY, KEY, KEY / 2.0, [77, 77, 77, 77]),
            dot: crate::grid::rounded(12.0, 12.0, 6.0, [240, 240, 240, 240]),
            erase: crate::quick::symbolic("/usr/share/icons/Adwaita/symbolic/ui/edit-clear-symbolic.svg", 28)
                .or_else(|| crate::quick::symbolic("/usr/share/icons/Adwaita/symbolic/actions/edit-clear-symbolic.svg", 28)),
            locked: false,
            blank: false,
            fading: None,
            grab: None,
            lift: 0.0,
            fonts: thin.zip(regular),
            time: Label::new(110.0, [1.0, 1.0, 1.0, 0.9]),
            date: Label::new(22.0, [1.0, 1.0, 1.0, 0.6]),
            hint: Label::new(15.0, [1.0, 1.0, 1.0, 0.45]),
            hint_base: "Проведите вверх, чтобы разблокировать".into(),
            notice_until: 0,
            minute: -1,
            id: Id::new(),
            last_touch_ns: hybris_hwc::now_ns(),
            idle_ns: idle_s * 1_000_000_000,
            relit: false,
            unlocked: None,
        };
        if let Some((_, regular)) = &lock.fonts {
            for (l, k) in lock.keys.iter_mut().zip(KEYS) {
                l.set(regular, k);
            }
            lock.message.set(regular, "Введите PIN");
        }
        lock.refresh();
        lock
    }

    /// The hint, when nothing passing is said in its place.
    pub fn set_hint(&mut self, text: &str) {
        if self.hint_base == text {
            return;
        }
        self.hint_base = text.to_owned();
        if let (Some((_, regular)), 0) = (&self.fonts, self.notice_until) {
            self.hint.set(regular, text);
        }
    }

    /// Something said in the hint's place for 2 s.
    pub fn notice(&mut self, text: &str) {
        if let Some((_, regular)) = &self.fonts {
            self.hint.set(regular, text);
            self.notice_until = hybris_hwc::now_ns() + 2_000_000_000;
        }
    }

    /// The notice's time is up: the hint is back; whether it changed.
    pub fn notice_done(&mut self, now_ns: u64) -> bool {
        if self.notice_until == 0 || now_ns < self.notice_until {
            return false;
        }
        self.notice_until = 0;
        if let Some((_, regular)) = &self.fonts {
            self.hint.set(regular, &self.hint_base);
        }
        true
    }

    fn say(&mut self, text: &str) {
        if let Some((_, regular)) = &self.fonts {
            self.message.set(regular, text);
        }
    }

    /// Key `i`'s rect on the right panel, logical px.
    fn key_rect(i: usize) -> Rectangle<f64, smithay::utils::Logical> {
        let p = layout::panels()[1];
        let w = 3.0 * KEY + 2.0 * KEY_GAP;
        let x0 = p.loc.x as f64 + (p.size.w as f64 - w) / 2.0;
        let (col, row) = ((i % 3) as f64, (i / 3) as f64);
        Rectangle::new((x0 + col * (KEY + KEY_GAP), PAD_TOP + row * (KEY + KEY_GAP)).into(), (KEY, KEY).into())
    }

    fn key_at(x: f64, y: f64) -> Option<usize> {
        (0..KEYS.len()).find(|&i| Self::key_rect(i).contains((x, y)))
    }

    /// A key of the pad let go.
    fn key(&mut self, i: usize) {
        let checking = self.checking.lock().unwrap().is_some();
        if checking {
            return;
        }
        match KEYS[i] {
            "←" => {
                self.pin.pop();
            }
            "OK" => self.submit(),
            d => {
                if self.pin.len() < MAX_PIN {
                    self.pin.push_str(d);
                }
            }
        }
    }

    /// The PIN to PAM, on a thread.
    fn submit(&mut self) {
        if self.pin.is_empty() {
            return;
        }
        let pin = std::mem::take(&mut self.pin);
        *self.checking.lock().unwrap() = Some(None);
        self.say("Проверка…");
        let (slot, wake) = (self.checking.clone(), self.wake.clone());
        std::thread::spawn(move || {
            let ok = crate::pam::check(&pin);
            drop(pin);
            *slot.lock().unwrap() = Some(Some(ok));
            wake.ping();
        });
    }

    /// A check's answer, if it came: unlocked, or told so.
    pub fn poll(&mut self) -> bool {
        let answer = { *self.checking.lock().unwrap() };
        match answer {
            Some(Some(ok)) => {
                *self.checking.lock().unwrap() = None;
                if ok {
                    tracing::info!("lock: unlocked");
                    self.fading = Some(hybris_hwc::now_ns());
                    self.unlocked = self.fading;
                    self.entering = false;
                    self.say("Введите PIN");
                } else {
                    self.say("Неверный PIN");
                }
                true
            }
            _ => false,
        }
    }

    /// The time brought up to the minute; whether it changed.
    pub fn refresh(&mut self) -> bool {
        let Some((thin, regular)) = &self.fonts else { return false };
        let now = local_time();
        let minute = now.tm_hour * 60 + now.tm_min;
        if minute == self.minute {
            return false;
        }
        self.minute = minute;
        self.time.set(thin, &format!("{}:{:02}", now.tm_hour, now.tm_min));
        self.date.set(regular, &date_line(&now));
        if self.notice_until == 0 {
            self.hint.set(regular, &self.hint_base);
        }
        true
    }

    /// Whether the lock screen takes touches: locked, and not fading.
    pub fn holds_screen(&self) -> bool {
        self.locked && self.fading.is_none()
    }

    /// Screen off and locked, or on again.
    pub fn power_key(&mut self) {
        if self.blank {
            self.set_blank(false);
        } else {
            self.lock_now();
            self.set_blank(true);
        }
    }

    pub fn lock_now(&mut self) {
        if !self.locked {
            tracing::info!("lock: locked");
        }
        self.locked = true;
        self.fading = None;
        self.lift = 0.0;
        self.entering = false;
        self.pin.clear();
        self.say("Введите PIN");
        self.refresh();
    }

    /// Unlocked from outside (logind's Unlock: the fingerprint reader,
    /// `loginctl unlock-session`): the lock screen fades as after a PIN.
    pub fn unlock(&mut self) {
        if !self.locked || self.fading.is_some() {
            return;
        }
        tracing::info!("lock: unlocked from outside");
        self.fading = Some(hybris_hwc::now_ns());
        self.unlocked = self.fading;
        self.entering = false;
        self.pin.clear();
    }

    pub fn set_blank(&mut self, blank: bool) {
        if blank == self.blank {
            return;
        }
        self.blank = blank;
        let ok = hybris_hwc::set_display_power(!blank);
        tracing::info!("lock: screen {}{}", if blank { "off" } else { "on" }, if ok { "" } else { " (hwcomposer refused)" });
        if !blank {
            self.last_touch_ns = hybris_hwc::now_ns();
            self.refresh();
            self.relit = true;
        }
    }

    /// When an unlock began, once: the dock rises after it.
    pub fn take_unlocked(&mut self) -> Option<u64> {
        self.unlocked.take()
    }

    /// Whether the display was lit again since last asked: it wants whole
    /// frames.
    pub fn relit(&mut self) -> bool {
        std::mem::take(&mut self.relit)
    }

    /// The idle delay passed without a touch: lock and blank.
    pub fn idle(&mut self, now_ns: u64) -> bool {
        if self.idle_ns == 0 || self.blank || now_ns.saturating_sub(self.last_touch_ns) < self.idle_ns {
            return false;
        }
        self.lock_now();
        self.set_blank(true);
        true
    }

    pub fn down(&mut self, slot: TouchSlot, x: f64, y: f64, time_us: u64) {
        self.pressed = if self.entering { Self::key_at(x, y) } else { None };
        self.grab = Some((slot, y, (time_us, y), 0.0));
    }

    pub fn holds(&self, slot: TouchSlot) -> bool {
        self.grab.is_some_and(|g| g.0 == slot)
    }

    pub fn motion(&mut self, slot: TouchSlot, y: f64, time_us: u64) {
        if let Some(g) = self.grab.as_mut().filter(|g| g.0 == slot) {
            let dt = time_us.saturating_sub(g.2 .0) as f64 / 1000.0;
            if dt > 0.0 {
                g.3 = g.3 * 0.4 + (y - g.2 .1) / dt * 0.6;
            }
            g.2 = (time_us, y);
            if (g.1 - y).abs() > 12.0 {
                self.pressed = None;
            }
            if !self.entering {
                self.lift = (g.1 - y).max(0.0);
            }
        }
    }

    /// The finger lets go: a swipe up brings the PIN pad, a swipe down on
    /// it puts it away, a tap on a key types.
    pub fn up(&mut self, slot: TouchSlot) {
        let Some(g) = self.grab.take_if(|g| g.0 == slot) else { return };
        if self.entering {
            if let Some(i) = self.pressed.take() {
                self.key(i);
            } else if g.2 .1 - g.1 > UNLOCK || g.3 > FLING {
                self.entering = false;
                self.pin.clear();
            }
            return;
        }
        if self.lift > UNLOCK || g.3 < -FLING {
            self.entering = true;
        }
        self.lift = 0.0;
    }

    /// After a frame: whether the lock screen still wants frames.
    pub fn settle(&mut self, frame_ns: u64) -> bool {
        match self.fading {
            Some(t) if frame_ns >= t + FADE_NS => {
                self.locked = false;
                self.fading = None;
                self.lift = 0.0;
                true
            }
            Some(_) => true,
                None => self.grab.is_some() || self.checking.lock().unwrap().is_some(),
        }
    }

    pub fn warm_up(&self, renderer: &mut GlesRenderer) -> usize {
        [&self.time, &self.date, &self.hint, &self.message]
            .into_iter()
            .chain(self.keys.iter())
            .filter(|l| MemoryRenderBufferRenderElement::from_buffer(renderer, (0.0, 0.0), &l.buffer, None, None, None, Kind::Unspecified).is_ok())
            .count()
    }

    pub fn elements(&self, renderer: &mut GlesRenderer, frame_ns: u64) -> Vec<ShellElement> {
        if !self.locked {
            return Vec::new();
        }
        let alpha = match self.fading {
            Some(t) => 1.0 - (frame_ns.saturating_sub(t) as f32 / FADE_NS as f32).clamp(0.0, 1.0),
            None => 1.0 - (self.lift / 400.0).clamp(0.0, 0.5) as f32,
        };
        let mut out = Vec::new();
        // One way to put a texture: logical px, the lock screen's alpha.
        let mut put = |out: &mut Vec<ShellElement>, b: &smithay::backend::renderer::element::memory::MemoryRenderBuffer, x: f64, y: f64| {
            if let Ok(e) = MemoryRenderBufferRenderElement::from_buffer(renderer, ((x * SCALE as f64).round(), (y * SCALE as f64).round()), b, Some(alpha), None, None, Kind::Unspecified) {
                out.push(ShellElement::Text(e));
            }
        };
        let panels = layout::panels();
        let left = panels[0];
        if self.entering {
            let right = panels[1];
            // What is typed, as dots; what is going on, under them.
            let n = self.pin.len() as f64;
            let dots_w = n * 12.0 + (n - 1.0).max(0.0) * 10.0;
            for i in 0..self.pin.len() {
                put(&mut out, &self.dot, right.loc.x as f64 + (right.size.w as f64 - dots_w) / 2.0 + i as f64 * 22.0, DOTS_Y);
            }
            put(&mut out, &self.message.buffer, right.loc.x as f64 + (right.size.w - self.message.extent.w) as f64 / 2.0, DOTS_Y + 36.0);
            for (i, l) in self.keys.iter().enumerate() {
                let r = Self::key_rect(i);
                match (&self.erase, KEYS[i]) {
                    (Some(e), "←") => put(&mut out, e, r.loc.x + (KEY - 28.0) / 2.0, r.loc.y + (KEY - 28.0) / 2.0),
                    _ => put(&mut out, &l.buffer, r.loc.x + (KEY - l.extent.w as f64) / 2.0, r.loc.y + (KEY - l.extent.h as f64) / 2.0),
                }
                put(&mut out, if self.pressed == Some(i) { &self.key_on } else { &self.key_bg }, r.loc.x, r.loc.y);
            }
        }
        let rise = -(self.lift * 0.5);
        let cx = |l: &Label, p: smithay::utils::Rectangle<i32, smithay::utils::Logical>| (p.loc.x + (p.size.w - l.extent.w) / 2) as f64;
        put(&mut out, &self.time.buffer, cx(&self.time, left), 150.0 + rise);
        put(&mut out, &self.date.buffer, cx(&self.date, left), 154.0 + self.time.extent.h as f64 + rise);
        if !self.entering {
            for p in panels {
                put(&mut out, &self.hint.buffer, cx(&self.hint, p), (p.size.h - 60) as f64 + rise);
            }
        }
        let (w, h) = layout::LAYOUT;
        let rect = Rectangle::<i32, Physical>::from_size((w * SCALE, h * SCALE).into());
        let commit = CommitCounter::from((alpha * 1000.0) as usize);
        out.push(ShellElement::Solid(SolidColorRenderElement::new(self.id.clone(), rect, commit, [0.0, 0.0, 0.0, alpha], Kind::Unspecified)));
        out
    }
}
