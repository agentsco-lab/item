//! Locking, blanking and the power key; the lock screen
//! (docs/LOCK-AND-SETUP.md).
//!
//! - The power key (qpnp_pon, KEY_POWER) with the screen on blanks it - the
//!   display's hwc2 power mode OFF, panel and backlight dark, no frames - and
//!   locks. With the screen off it lights it again, locked.
//! - The session's idle delay (org.gnome.desktop.session idle-delay, read at
//!   start; 0: never) does the same after that long without a touch.
//! - The lock screen, over everything, on two panels with two roles: the
//!   left one says who and what (the time, the date; after a boot a
//!   greeting), the right one is where you act. After a boot the PIN pad is
//!   open there at once: the first PIN also opens the login keyring (PAM's
//!   phosh service, pam.rs). Otherwise a fingerprint mark stands at the right
//!   edge, across from the power key that holds the reader; a known finger
//!   unlocks (fingerprint.rs), an unknown one shakes it red; after five, or
//!   a swipe up at any time, the PIN pad slides in.
//! - Unlocked, the two halves open as doors: the left one slides out left,
//!   the right one right, over 450 ms.
//! - The volume keys (gpio-keys) set the volume, locked or not.

use smithay::backend::renderer::element::memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement};
use smithay::backend::renderer::element::solid::SolidColorRenderElement;
use smithay::backend::renderer::element::{Id, Kind};
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::renderer::utils::CommitCounter;
use smithay::backend::input::TouchSlot;
use smithay::utils::{Physical, Rectangle};

use crate::layout::{self, SCALE};
use crate::shade::{date_line, local_time, ShellElement};
use crate::text::{Font, Label};

/// The doors opening.
const DOOR_NS: u64 = 450_000_000;
/// The PIN pad coming in.
const PAD_IN_NS: u64 = 200_000_000;
/// A shake: the pad at a wrong PIN, the mark at an unknown finger.
const SHAKE_NS: u64 = 420_000_000;
/// The mark's flash at a known finger.
const FLASH_NS: u64 = 300_000_000;
/// The mark pulses this long after the lock screen shows, then rests.
const PULSE_FOR_NS: u64 = 12_000_000_000;
const PULSE_PERIOD_NS: u64 = 1_800_000_000;
/// Unknown fingers in a row before the PIN pad comes.
const FAILS_FOR_PIN: u32 = 5;
/// A swipe up this far brings the pad, or a fling up.
const UNLOCK: f64 = 120.0;
const FLING: f64 = 0.5;

/// The PIN pad's keys, row by row.
const KEYS: [&str; 12] = ["1", "2", "3", "4", "5", "6", "7", "8", "9", "←", "0", "OK"];
const KEY: f64 = 76.0;
const KEY_GAP: f64 = 22.0;
const PAD_TOP: f64 = 330.0;
const DOTS_Y: f64 = 220.0;
const MAX_PIN: usize = 16;
/// The fingerprint mark: its size, and the height of the power key's middle
/// on the right panel's right edge (logical px).
const MARK: f64 = 48.0;
const POWER_Y: f64 = 380.0;
const MARK_ICON: &str = "/usr/share/icons/Adwaita/symbolic/devices/auth-fingerprint-symbolic.svg";

pub struct Lock {
    /// The PIN pad is up (since when), and what is typed.
    entering: bool,
    entering_since: u64,
    pin: String,
    pressed: Option<usize>,
    /// A check running on its thread, and its answer when it comes.
    checking: std::sync::Arc<std::sync::Mutex<Option<Option<bool>>>>,
    wake: smithay::reexports::calloop::ping::Ping,
    message: Label,
    keys: Vec<Label>,
    key_bg: MemoryRenderBuffer,
    key_on: MemoryRenderBuffer,
    dot: MemoryRenderBuffer,
    /// The backspace key's icon (the fonts have no arrow).
    erase: Option<MemoryRenderBuffer>,
    /// The fingerprint mark, white and red.
    mark: Option<MemoryRenderBuffer>,
    mark_red: Option<MemoryRenderBuffer>,
    pub locked: bool,
    pub blank: bool,
    /// The first lock after a boot: the PIN, and a greeting.
    after_boot: bool,
    /// Fingers enrolled with the reader; none, and there is no mark.
    fingers: usize,
    /// Unknown fingers in a row.
    fails: u32,
    /// When the doors began to open.
    fading: Option<u64>,
    /// When the pad last shook, the mark last shook or flashed.
    pad_shake: Option<u64>,
    mark_shake: Option<u64>,
    mark_flash: Option<u64>,
    /// When the lock screen last came up or was lit: the mark pulses after.
    shown_at: u64,
    grab: Option<(TouchSlot, f64, (u64, f64), f64)>,
    /// How far the finger has lifted the lock screen, logical px.
    lift: f64,
    fonts: Option<(Font, Font)>,
    time: Label,
    date: Label,
    greeting: Label,
    status: Label,
    hint: Label,
    /// The hint, and a passing notice in its place until when.
    hint_base: String,
    notice_until: u64,
    minute: i32,
    id: Id,
    id_right: Id,
    /// When the last touch came, for the idle delay.
    pub last_touch_ns: u64,
    pub idle_ns: u64,
    /// The display was lit again since last asked.
    relit: bool,
    /// When the last unlock began, not yet asked for.
    unlocked: Option<u64>,
}

/// A symbolic icon in one colour, `size` logical px.
fn tinted(path: &str, size: i32, rgb: [u8; 3]) -> Option<MemoryRenderBuffer> {
    let mut pixmap = crate::apps::icon_pixmap(path, size)?;
    for px in pixmap.pixels_mut() {
        let a = px.alpha() as u16;
        let c = |v: u8| ((v as u16 * a) / 255) as u8;
        *px = resvg::tiny_skia::PremultipliedColorU8::from_rgba(c(rgb[0]), c(rgb[1]), c(rgb[2]), a as u8).unwrap_or(*px);
    }
    let n = pixmap.width() as i32;
    Some(MemoryRenderBuffer::from_slice(pixmap.data(), smithay::backend::allocator::Fourcc::Abgr8888, (n, n), SCALE, smithay::utils::Transform::Normal, None))
}

/// Ease in and out (cubic).
fn ease(k: f64) -> f64 {
    let k = k.clamp(0.0, 1.0);
    if k < 0.5 { 4.0 * k * k * k } else { 1.0 - (-2.0 * k + 2.0).powi(3) / 2.0 }
}

/// A damped shake's offset, logical px, `t` ns into it.
fn shake(t: u64) -> f64 {
    if t >= SHAKE_NS {
        return 0.0;
    }
    let u = t as f64 / SHAKE_NS as f64;
    14.0 * (1.0 - u) * (u * std::f64::consts::TAU * 3.5).sin()
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
            entering_since: 0,
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
            mark: tinted(MARK_ICON, MARK as i32, [240, 240, 240]),
            mark_red: tinted(MARK_ICON, MARK as i32, [235, 80, 70]),
            locked: false,
            blank: false,
            after_boot: false,
            fingers: 0,
            fails: 0,
            fading: None,
            pad_shake: None,
            mark_shake: None,
            mark_flash: None,
            shown_at: 0,
            grab: None,
            lift: 0.0,
            fonts: thin.zip(regular),
            time: Label::new(110.0, [1.0, 1.0, 1.0, 0.9]),
            date: Label::new(22.0, [1.0, 1.0, 1.0, 0.6]),
            greeting: Label::new(26.0, [1.0, 1.0, 1.0, 0.9]),
            status: Label::new(16.0, [1.0, 1.0, 1.0, 0.55]),
            hint: Label::new(15.0, [1.0, 1.0, 1.0, 0.5]),
            hint_base: "Swipe up to unlock".into(),
            notice_until: 0,
            minute: -1,
            id: Id::new(),
            id_right: Id::new(),
            last_touch_ns: hybris_hwc::now_ns(),
            idle_ns: idle_s * 1_000_000_000,
            relit: false,
            unlocked: None,
        };
        if let Some((_, regular)) = &lock.fonts {
            for (l, k) in lock.keys.iter_mut().zip(KEYS) {
                l.set(regular, k);
            }
            lock.message.set(regular, "Enter PIN");
            lock.status.set(regular, "Enter your PIN after a restart");
        }
        lock.refresh();
        lock
    }

    /// The first lock after a boot: the PIN pad open, a greeting, no reader
    /// (a finger cannot open the keyring).
    pub fn after_boot(&mut self) {
        self.after_boot = true;
        self.lock_now();
        self.entering_since = hybris_hwc::now_ns();
        self.minute = -1;
        self.refresh();
    }

    /// How many fingers the reader knows: with none there is no mark.
    pub fn set_fingers(&mut self, n: usize) {
        self.fingers = n;
        self.set_hint(if n > 0 { "Touch the power key" } else { "Swipe up to unlock" });
    }

    /// Whether the reader should listen: locked, lit, not right after a
    /// boot, with a finger to know.
    pub fn wants_reader(&self) -> bool {
        self.holds_screen() && !self.blank && !self.after_boot && self.fingers > 0
    }

    /// The reader did not know a finger.
    pub fn fingerprint_failed(&mut self) {
        self.fails += 1;
        self.mark_shake = Some(hybris_hwc::now_ns());
        if self.fails >= FAILS_FOR_PIN && !self.entering {
            self.say("Use your PIN");
            self.pad_in();
        } else {
            self.notice("Not recognized");
        }
    }

    fn pad_in(&mut self) {
        if !self.entering {
            self.entering = true;
            self.entering_since = hybris_hwc::now_ns();
        }
    }

    /// The hint, when nothing passing is said in its place.
    fn set_hint(&mut self, text: &str) {
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
        self.say("Checking…");
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
                    self.open_doors();
                } else {
                    self.say("Wrong PIN");
                    self.pad_shake = Some(hybris_hwc::now_ns());
                    crate::fingerprint::buzz("bell-terminal");
                }
                true
            }
            _ => false,
        }
    }

    fn open_doors(&mut self) {
        self.fading = Some(hybris_hwc::now_ns());
        self.unlocked = self.fading;
        self.after_boot = false;
        self.fails = 0;
        self.pin.clear();
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
        if self.after_boot {
            let part = crate::sysscreen::part_of_day(now.tm_hour);
            let greeting = match crate::sysscreen::first_name() {
                Some(n) => format!("{part}, {n}"),
                None => part.to_owned(),
            };
            self.greeting.set(regular, &greeting);
        }
        if self.notice_until == 0 {
            self.hint.set(regular, &self.hint_base);
        }
        true
    }

    /// Whether the lock screen takes touches: locked, and not opening.
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
            self.shown_at = hybris_hwc::now_ns();
        }
        self.locked = true;
        self.fading = None;
        self.lift = 0.0;
        self.entering = self.after_boot;
        self.pin.clear();
        self.fails = 0;
        self.say("Enter PIN");
        self.refresh();
    }

    /// Unlocked from outside (logind's Unlock: the fingerprint reader,
    /// `loginctl unlock-session`): the doors open as after a PIN.
    pub fn unlock(&mut self) {
        if !self.locked || self.fading.is_some() {
            return;
        }
        tracing::info!("lock: unlocked from outside");
        self.mark_flash = Some(hybris_hwc::now_ns());
        self.entering = false;
        self.open_doors();
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
            self.shown_at = self.last_touch_ns;
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
    /// it puts it away (not after a boot), a tap on a key types.
    pub fn up(&mut self, slot: TouchSlot) {
        let Some(g) = self.grab.take_if(|g| g.0 == slot) else { return };
        if self.entering {
            if let Some(i) = self.pressed.take() {
                self.key(i);
            } else if !self.after_boot && (g.2 .1 - g.1 > UNLOCK || g.3 > FLING) {
                self.entering = false;
                self.pin.clear();
            }
            return;
        }
        if self.lift > UNLOCK || g.3 < -FLING {
            self.pad_in();
        }
        self.lift = 0.0;
    }

    /// After a frame: whether the lock screen still wants frames.
    pub fn settle(&mut self, frame_ns: u64) -> bool {
        if let Some(t) = self.fading {
            if frame_ns >= t + DOOR_NS {
                self.locked = false;
                self.fading = None;
                self.lift = 0.0;
            }
            return true;
        }
        if !self.locked || self.blank {
            return false;
        }
        let moving = |t: Option<u64>, d: u64| t.is_some_and(|t| frame_ns < t + d);
        self.grab.is_some()
            || self.checking.lock().unwrap().is_some()
            || moving(Some(self.entering_since), PAD_IN_NS)
            || moving(self.pad_shake, SHAKE_NS)
            || moving(self.mark_shake, SHAKE_NS)
            || moving(self.mark_flash, FLASH_NS)
            || (self.fingers > 0 && !self.entering && !self.after_boot && frame_ns < self.shown_at + PULSE_FOR_NS)
    }

    pub fn warm_up(&self, renderer: &mut GlesRenderer) -> usize {
        [&self.time, &self.date, &self.hint, &self.message, &self.greeting, &self.status]
            .into_iter()
            .map(|l| &l.buffer)
            .chain(self.keys.iter().map(|l| &l.buffer))
            .chain(self.mark.iter())
            .chain(self.mark_red.iter())
            .filter(|b| MemoryRenderBufferRenderElement::from_buffer(renderer, (0.0, 0.0), b, None, None, None, Kind::Unspecified).is_ok())
            .count()
    }

    pub fn elements(&self, renderer: &mut GlesRenderer, frame_ns: u64) -> Vec<ShellElement> {
        if !self.locked {
            return Vec::new();
        }
        let panels = layout::panels();
        let (left, right) = (panels[0], panels[1]);
        // The doors: each half slides out to its own side.
        let open = self.fading.map(|t| ease(frame_ns.saturating_sub(t) as f64 / DOOR_NS as f64)).unwrap_or(0.0);
        let middle = (left.loc.x + left.size.w + right.loc.x) as f64 / 2.0;
        let (w, h) = layout::LAYOUT;
        let left_dx = -middle * open;
        let right_dx = (w as f64 - middle) * open;
        let mut out = Vec::new();
        // One way to put a texture: logical px, moved with its half.
        let mut put = |out: &mut Vec<ShellElement>, b: &MemoryRenderBuffer, x: f64, y: f64, dx: f64, alpha: f32| {
            if let Ok(e) = MemoryRenderBufferRenderElement::from_buffer(renderer, (((x + dx) * SCALE as f64).round(), (y * SCALE as f64).round()), b, Some(alpha), None, None, Kind::Unspecified) {
                out.push(ShellElement::Text(e));
            }
        };
        let cx = |l: &Label, p: Rectangle<i32, smithay::utils::Logical>| (p.loc.x + (p.size.w - l.extent.w) / 2) as f64;
        let since = |t: Option<u64>| t.map(|t| frame_ns.saturating_sub(t));

        // The right half: where you act.
        if self.entering {
            let k = (frame_ns.saturating_sub(self.entering_since) as f64 / PAD_IN_NS as f64).clamp(0.0, 1.0);
            let alpha = ease(k) as f32;
            let rise = 40.0 * (1.0 - ease(k));
            let sx = since(self.pad_shake).map(shake).unwrap_or(0.0);
            let dx = right_dx + sx;
            // What is typed, as dots; what is going on, under them.
            let n = self.pin.len() as f64;
            let dots_w = n * 12.0 + (n - 1.0).max(0.0) * 10.0;
            for i in 0..self.pin.len() {
                put(&mut out, &self.dot, right.loc.x as f64 + (right.size.w as f64 - dots_w) / 2.0 + i as f64 * 22.0, DOTS_Y + rise, dx, alpha);
            }
            put(&mut out, &self.message.buffer, cx(&self.message, right), DOTS_Y + 36.0 + rise, dx, alpha);
            for (i, l) in self.keys.iter().enumerate() {
                let r = Self::key_rect(i);
                match (&self.erase, KEYS[i]) {
                    (Some(e), "←") => put(&mut out, e, r.loc.x + (KEY - 28.0) / 2.0, r.loc.y + (KEY - 28.0) / 2.0 + rise, dx, alpha),
                    _ => put(&mut out, &l.buffer, r.loc.x + (KEY - l.extent.w as f64) / 2.0, r.loc.y + (KEY - l.extent.h as f64) / 2.0 + rise, dx, alpha),
                }
                put(&mut out, if self.pressed == Some(i) { &self.key_on } else { &self.key_bg }, r.loc.x, r.loc.y + rise, dx, alpha);
            }
        } else if self.fingers > 0 {
            // The mark across from the power key: pulsing a while after it
            // shows, flashing at a known finger, shaking red at an unknown one.
            let shaking = since(self.mark_shake).is_some_and(|t| t < SHAKE_NS);
            let sx = since(self.mark_shake).map(shake).unwrap_or(0.0);
            let flash = since(self.mark_flash).filter(|t| *t < FLASH_NS).map(|t| 1.0 - t as f64 / FLASH_NS as f64);
            let pulse = if frame_ns < self.shown_at + PULSE_FOR_NS {
                let u = (frame_ns.saturating_sub(self.shown_at) % PULSE_PERIOD_NS) as f64 / PULSE_PERIOD_NS as f64;
                0.55 + 0.35 * (0.5 - 0.5 * (u * std::f64::consts::TAU).cos())
            } else {
                0.7
            };
            let alpha = flash.map(|f| 0.9 + 0.1 * f).unwrap_or(pulse) as f32;
            let mx = (right.loc.x + right.size.w) as f64 - 18.0 - MARK;
            let my = POWER_Y - MARK / 2.0;
            let icon = if shaking { self.mark_red.as_ref() } else { self.mark.as_ref() };
            if let Some(icon) = icon {
                put(&mut out, icon, mx, my, right_dx + sx, alpha);
            }
            // What to do, beside it, right-aligned to the mark.
            put(&mut out, &self.hint.buffer, mx - 14.0 - self.hint.extent.w as f64, POWER_Y - self.hint.extent.h as f64 / 2.0, right_dx, 1.0);
        } else {
            put(&mut out, &self.hint.buffer, cx(&self.hint, right), (right.size.h - 60) as f64, right_dx, 1.0);
        }

        // The left half: who and what.
        let rise = -(self.lift * 0.5);
        if self.after_boot {
            put(&mut out, &self.time.buffer, cx(&self.time, left), 130.0 + rise, left_dx, 1.0);
            let y = 134.0 + self.time.extent.h as f64;
            put(&mut out, &self.date.buffer, cx(&self.date, left), y + rise, left_dx, 1.0);
            put(&mut out, &self.greeting.buffer, cx(&self.greeting, left), y + 90.0 + rise, left_dx, 1.0);
            put(&mut out, &self.status.buffer, cx(&self.status, left), (left.size.h - 70) as f64, left_dx, 1.0);
        } else {
            put(&mut out, &self.time.buffer, cx(&self.time, left), 150.0 + rise, left_dx, 1.0);
            put(&mut out, &self.date.buffer, cx(&self.date, left), 154.0 + self.time.extent.h as f64 + rise, left_dx, 1.0);
        }

        // The two halves' black, meeting over the hinge.
        let m = (middle * SCALE as f64).round() as i32;
        let lx = (left_dx * SCALE as f64).round() as i32;
        let rx = (right_dx * SCALE as f64).round() as i32;
        let commit = CommitCounter::from((open * 1000.0) as usize);
        let lrect = Rectangle::<i32, Physical>::new((lx, 0).into(), (m, h * SCALE).into());
        let rrect = Rectangle::<i32, Physical>::new((m + rx, 0).into(), (w * SCALE - m, h * SCALE).into());
        out.push(ShellElement::Solid(SolidColorRenderElement::new(self.id.clone(), lrect, commit, [0.0, 0.0, 0.0, 1.0], Kind::Unspecified)));
        out.push(ShellElement::Solid(SolidColorRenderElement::new(self.id_right.clone(), rrect, commit, [0.0, 0.0, 0.0, 1.0], Kind::Unspecified)));
        out
    }
}
