//! Locking, blanking and the power key.
//!
//! - The power key (qpnp_pon, KEY_POWER) with the screen on blanks it - the
//!   display's hwc2 power mode OFF, panel and backlight dark, no frames - and
//!   locks. With the screen off it lights it again, locked.
//! - The session's idle delay (org.gnome.desktop.session idle-delay, read at
//!   start; 0: never) does the same after that long without a touch.
//! - The lock screen is drawn by the compositor over everything: the time
//!   and the date on the left panel, a line on how to unlock on both. A
//!   swipe up anywhere unlocks, and the lock screen fades out over 300 ms,
//!   as item's (patches/phosh/0005). No PIN yet: that is PAM's, a step of
//!   its own.
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

pub struct Lock {
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
    minute: i32,
    id: Id,
    /// When the last touch came, for the idle delay.
    pub last_touch_ns: u64,
    pub idle_ns: u64,
    /// The display was lit again since last asked.
    relit: bool,
}

impl Lock {
    pub fn new() -> Lock {
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
            locked: false,
            blank: false,
            fading: None,
            grab: None,
            lift: 0.0,
            fonts: thin.zip(regular),
            time: Label::new(110.0, [1.0, 1.0, 1.0, 0.9]),
            date: Label::new(22.0, [1.0, 1.0, 1.0, 0.6]),
            hint: Label::new(15.0, [1.0, 1.0, 1.0, 0.45]),
            minute: -1,
            id: Id::new(),
            last_touch_ns: hybris_hwc::now_ns(),
            idle_ns: idle_s * 1_000_000_000,
            relit: false,
        };
        lock.refresh();
        lock
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
        self.hint.set(regular, "Проведите вверх, чтобы разблокировать");
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
        self.refresh();
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

    pub fn down(&mut self, slot: TouchSlot, y: f64, time_us: u64) {
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
            self.lift = (g.1 - y).max(0.0);
        }
    }

    /// The finger lets go: unlocked if it went far or fast enough up.
    pub fn up(&mut self, slot: TouchSlot) {
        let Some(g) = self.grab.take_if(|g| g.0 == slot) else { return };
        if self.lift > UNLOCK || g.3 < -FLING {
            tracing::info!("lock: unlocked");
            self.fading = Some(hybris_hwc::now_ns());
        } else {
            self.lift = 0.0;
        }
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
            None => self.grab.is_some(),
        }
    }

    pub fn warm_up(&self, renderer: &mut GlesRenderer) -> usize {
        [&self.time, &self.date, &self.hint]
            .iter()
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
        let rise = -(self.lift * 0.5) as i32;
        let mut out = Vec::new();
        let mut push = |label: &Label, x: i32, y: i32| {
            let loc = ((x * SCALE) as f64, ((y + rise) * SCALE) as f64);
            if let Ok(e) = MemoryRenderBufferRenderElement::from_buffer(renderer, loc, &label.buffer, Some(alpha), None, None, Kind::Unspecified) {
                out.push(ShellElement::Text(e));
            }
        };
        let panels = layout::panels();
        let left = panels[0];
        push(&self.time, left.loc.x + (left.size.w - self.time.extent.w) / 2, 150);
        push(&self.date, left.loc.x + (left.size.w - self.date.extent.w) / 2, 150 + self.time.extent.h + 4);
        for p in panels {
            push(&self.hint, p.loc.x + (p.size.w - self.hint.extent.w) / 2, p.size.h - 60);
        }
        let (w, h) = layout::LAYOUT;
        let rect = Rectangle::<i32, Physical>::from_size((w * SCALE, h * SCALE).into());
        let commit = CommitCounter::from((alpha * 1000.0) as usize);
        out.push(ShellElement::Solid(SolidColorRenderElement::new(self.id.clone(), rect, commit, [0.0, 0.0, 0.0, alpha], Kind::Unspecified)));
        out
    }
}
