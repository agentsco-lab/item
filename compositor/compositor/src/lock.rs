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
//! - On the left half under the status line, what is playing (MPRIS) with
//!   its buttons, which work locked; under it the last three
//!   notifications, their app and title only - what they say is for after
//!   the unlock.

use smithay::backend::renderer::element::memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement};
use smithay::backend::renderer::element::solid::SolidColorRenderElement;
use smithay::backend::renderer::element::{Id, Kind};
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::renderer::utils::CommitCounter;
use smithay::backend::input::TouchSlot;
use smithay::utils::{Physical, Rectangle};

use crate::layout::{self, SCALE};
use resvg::tiny_skia;
use crate::shade::{local_time, ShellElement};
use crate::pinpad::{ease, shake, PinPad, Press, SHAKE_NS};
use crate::text::{Font, Label};

/// The mark's flash at a known finger.
const FLASH_NS: u64 = 300_000_000;
/// The mark pulses this long after the lock screen shows, then rests.
const PULSE_FOR_NS: u64 = 12_000_000_000;
const PULSE_PERIOD_NS: u64 = 1_800_000_000;
/// How dark the wallpaper is under the lock screen.
const SHADE: f32 = 0.75;
/// Unknown fingers in a row before the PIN pad comes.
const FAILS_FOR_PIN: u32 = 5;
/// A swipe up this far brings the pad, or a fling up.
const UNLOCK: f64 = 120.0;
const FLING: f64 = 0.5;

/// The fingerprint mark: its size, and the height of the power key's middle
/// on the right panel's right edge (logical px).
const MARK: f64 = 48.0;
const POWER_Y: f64 = 455.0;
/// The glow across from the reader: its size, and how far in from the
/// panel's edge its middle is (half of it past the edge, hugging it).
const SENSOR_GLOW: f64 = 110.0;
const SENSOR_IN: f64 = 8.0;
/// The mark comes up over this.
const MARK_IN_NS: u64 = 400_000_000;
const GLOW: f64 = 124.0;
/// Where the left panel talks to you: the middle of its lower part.
const TALK_FROM_FOOT: i32 = 170;

/// The clock line, as the desktop's (clock.rs): its size, its distance from
/// the top, the room between the time and the date, its end in from the
/// panel's right edge.
const CLOCK_SIZE: f32 = 36.0;
const CLOCK_TOP: f64 = 72.0;
const CLOCK_BETWEEN: i32 = 14;
const CLOCK_RIGHT: f64 = 40.0;

pub struct Lock {
    /// The PIN pad is up (pinpad.rs).
    entering: bool,
    pad: PinPad,
    /// A check running on its thread, and its answer when it comes.
    checking: std::sync::Arc<std::sync::Mutex<Option<Option<bool>>>>,
    /// The PIN that unlocked, for the keyring's prompts (keyring.rs).
    verified: std::sync::Arc<std::sync::Mutex<Option<String>>>,
    wake: smithay::reexports::calloop::ping::Ping,
    /// The fingerprint mark, white and red.
    mark: Option<MemoryRenderBuffer>,
    mark_red: Option<MemoryRenderBuffer>,
    /// The mark a little larger, for a soft shine under it.
    /// The mark in the accent, for a known finger (drawn again when the
    /// accent changes).
    mark_lit: std::cell::RefCell<Option<(u64, MemoryRenderBuffer)>>,
    glow: Option<MemoryRenderBuffer>,
    /// The status line under the date: the battery and the network.
    battery_icon: Option<MemoryRenderBuffer>,
    battery_label: Label,
    net_icon: Option<MemoryRenderBuffer>,
    net_label: Label,
    status_icons: (String, &'static str),
    pub locked: bool,
    pub blank: bool,
    /// The first lock after a boot: the PIN, and a greeting.
    after_boot: bool,
    /// Fingers enrolled with the reader; none, and there is no mark.
    fingers: usize,
    /// Unknown fingers in a row.
    fails: u32,
    /// When the doors began to open, how they open (door.rs), and where the
    /// unlock was touched (logical px), where a wave starts.
    fading: Option<u64>,
    touched_at: (f64, f64),
    door: crate::door::Style,
    /// When the mark last shook or flashed.
    mark_shake: Option<u64>,
    mark_flash: Option<u64>,
    /// When the lock screen last came up or was lit: the mark pulses after.
    shown_at: u64,
    grab: Option<(TouchSlot, f64, (u64, f64), f64)>,
    /// How far the finger has lifted the lock screen, logical px.
    lift: f64,
    fonts: Option<(Font, Font)>,
    /// The clock's: bold for the time, medium for the date (as the
    /// desktop's, clock.rs), and the accent it is drawn in.
    clock_fonts: Option<(Font, Font)>,
    clock_accent: u64,
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
    /// The last notifications (app, title) and their labels.
    notes: Vec<(String, String)>,
    note_labels: Vec<(Label, Label)>,
    /// What is playing (title, artist, playing) and its labels; its card,
    /// a notification's, and the buttons' icons (previous, play, pause,
    /// next).
    media: Option<(String, String, bool)>,
    media_labels: (Label, Label),
    card: MemoryRenderBuffer,
    note_card: MemoryRenderBuffer,
    media_icons: [Option<MemoryRenderBuffer>; 4],
}

/// The lock screen's cards: their width and heights, logical px.
const CARD_W: f64 = 520.0;
const MEDIA_H: f64 = 76.0;
const NOTE_H: f64 = 56.0;
const CARDS_GAP: f64 = 10.0;
const CARDS_TOP: f64 = 30.0;
const MEDIA_BUTTON: f64 = 48.0;

/// A symbolic icon in one colour, `size` logical px.
/// A soft round glow, `size` logical px across: the colour strongest in the
/// middle, gone at the rim.
pub(crate) fn soft_glow(size: f64, rgba: [u8; 4]) -> MemoryRenderBuffer {
    use smithay::backend::allocator::Fourcc;
    let px = (size as f32 * SCALE as f32).round() as u32;
    let mut pixmap = tiny_skia::Pixmap::new(px, px).expect("pixmap");
    let c = px as f32 / 2.0;
    let stop = |at: f32, a: f32| tiny_skia::GradientStop::new(at, tiny_skia::Color::from_rgba8(rgba[0], rgba[1], rgba[2], (a * 255.0) as u8));
    let shader = tiny_skia::RadialGradient::new(
        tiny_skia::Point::from_xy(c, c),
        tiny_skia::Point::from_xy(c, c),
        c,
        vec![stop(0.0, 0.8), stop(0.35, 0.5), stop(0.7, 0.16), stop(1.0, 0.0)],
        tiny_skia::SpreadMode::Pad,
        tiny_skia::Transform::identity(),
    );
    if let (Some(shader), Some(rect)) = (shader, tiny_skia::Rect::from_xywh(0.0, 0.0, px as f32, px as f32)) {
        let paint = tiny_skia::Paint { shader, anti_alias: true, ..Default::default() };
        pixmap.fill_rect(rect, &paint, tiny_skia::Transform::identity(), None);
    }
    MemoryRenderBuffer::from_slice(pixmap.data(), Fourcc::Abgr8888, (px as i32, px as i32), SCALE, smithay::utils::Transform::Normal, None)
}

pub(crate) fn tinted(path: &str, size: i32, rgb: [u8; 3]) -> Option<MemoryRenderBuffer> {
    let mut pixmap = crate::apps::icon_pixmap(path, size)?;
    for px in pixmap.pixels_mut() {
        let a = px.alpha() as u16;
        let c = |v: u8| ((v as u16 * a) / 255) as u8;
        *px = resvg::tiny_skia::PremultipliedColorU8::from_rgba(c(rgb[0]), c(rgb[1]), c(rgb[2]), a as u8).unwrap_or(*px);
    }
    let n = pixmap.width() as i32;
    Some(MemoryRenderBuffer::from_slice(pixmap.data(), smithay::backend::allocator::Fourcc::Abgr8888, (n, n), SCALE, smithay::utils::Transform::Normal, None))
}

/// A soft white glow, `size` logical px across.
fn glow(size: f64) -> Option<MemoryRenderBuffer> {
    use resvg::tiny_skia;
    let px = (size * SCALE as f64) as u32;
    let mut pixmap = tiny_skia::Pixmap::new(px, px)?;
    let c = px as f32 / 2.0;
    let shader = tiny_skia::RadialGradient::new(
        tiny_skia::Point::from_xy(c, c),
        tiny_skia::Point::from_xy(c, c),
        c,
        vec![
            tiny_skia::GradientStop::new(0.0, tiny_skia::Color::from_rgba8(255, 255, 255, 120)),
            tiny_skia::GradientStop::new(0.55, tiny_skia::Color::from_rgba8(255, 255, 255, 70)),
            tiny_skia::GradientStop::new(0.75, tiny_skia::Color::from_rgba8(255, 255, 255, 22)),
            tiny_skia::GradientStop::new(1.0, tiny_skia::Color::from_rgba8(255, 255, 255, 0)),
        ],
        tiny_skia::SpreadMode::Pad,
        tiny_skia::Transform::identity(),
    )?;
    let mut paint = tiny_skia::Paint::default();
    paint.shader = shader;
    pixmap.fill_rect(tiny_skia::Rect::from_xywh(0.0, 0.0, px as f32, px as f32)?, &paint, tiny_skia::Transform::identity(), None);
    Some(MemoryRenderBuffer::from_slice(pixmap.data(), smithay::backend::allocator::Fourcc::Abgr8888, (px as i32, px as i32), SCALE, smithay::utils::Transform::Normal, None))
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
            pad: {
                let mut pad = PinPad::new();
                pad.water_bloom();
                pad
            },
            checking: Default::default(),
            verified: Default::default(),
            wake,
            mark: None,
            mark_red: Some(soft_glow(SENSOR_GLOW, [235, 80, 70, 255])),
            mark_lit: std::cell::RefCell::new(None),
            glow: glow(GLOW),
            battery_icon: None,
            battery_label: Label::new(15.0, [1.0, 1.0, 1.0, 0.65]),
            net_icon: None,
            net_label: Label::new(15.0, [1.0, 1.0, 1.0, 0.65]),
            status_icons: (String::new(), ""),
            locked: false,
            blank: false,
            after_boot: false,
            fingers: 0,
            fails: 0,
            fading: None,
            touched_at: (0.0, 0.0),
            door: crate::door::Style::default(),
            mark_shake: None,
            mark_flash: None,
            shown_at: 0,
            grab: None,
            lift: 0.0,
            fonts: thin.zip(regular),
            clock_fonts: Font::load(&["/usr/share/fonts/truetype/lato/Lato-Bold.ttf", "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf"])
                .zip(Font::load(&["/usr/share/fonts/truetype/lato/Lato-Medium.ttf", "/usr/share/fonts/truetype/lato/Lato-Regular.ttf"])),
            clock_accent: 0,
            time: Label::new(CLOCK_SIZE, [1.0; 4]),
            date: Label::new(CLOCK_SIZE, [1.0; 4]),
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
            notes: Vec::new(),
            note_labels: Vec::new(),
            media: None,
            media_labels: (Label::new(17.0, [1.0, 1.0, 1.0, 0.92]), Label::new(14.0, [1.0, 1.0, 1.0, 0.6])),
            card: crate::grid::rounded(CARD_W, MEDIA_H, 18.0, [22, 22, 22, 22]),
            note_card: crate::grid::rounded(CARD_W, NOTE_H, 16.0, [18, 18, 18, 18]),
            media_icons: ["media-skip-backward", "media-playback-start", "media-playback-pause", "media-skip-forward"].map(|n| crate::quick::symbolic(&format!("/usr/share/icons/Adwaita/symbolic/actions/{n}-symbolic.svg"), 24)),
        };
        if let Some((_, regular)) = &lock.fonts {
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
        self.pad.show();
        self.minute = -1;
        self.refresh();
    }

    /// How many fingers the reader knows: with none there is no mark.
    pub fn set_fingers(&mut self, n: usize) {
        self.fingers = n;
        self.set_hint(if n > 0 { "" } else { "Swipe up to unlock" });
    }

    /// The status line's facts.
    pub fn set_status(&mut self, f: &crate::status::Facts) {
        let Some((_, regular)) = &self.fonts else { return };
        self.battery_label.set(regular, &format!("{}%", f.battery));
        // The network as its icon alone, as in the shade.
        self.net_label.set(regular, "");
        let icons = (f.battery_icon(), f.net_icon);
        if icons != self.status_icons {
            let path = |n: &str| format!("/usr/share/icons/Adwaita/symbolic/status/{n}.svg");
            self.battery_icon = crate::quick::symbolic(&path(&icons.0), 16);
            self.net_icon = crate::quick::symbolic(&path(icons.1), 16);
            self.status_icons = icons;
        }
    }

    /// The last notifications, newest first: their app and title. Returns
    /// whether they changed.
    pub fn set_notes(&mut self, notes: Vec<(String, String)>) -> bool {
        let notes: Vec<(String, String)> = notes.into_iter().take(3).collect();
        if notes == self.notes {
            return false;
        }
        let Some((_, regular)) = &self.fonts else { return false };
        self.note_labels = notes
            .iter()
            .map(|(app, title)| {
                let mut a = Label::new(13.0, [1.0, 1.0, 1.0, 0.55]);
                a.set(regular, &cut(app, 40));
                let mut t = Label::new(16.0, [1.0, 1.0, 1.0, 0.9]);
                t.set(regular, &cut(title, 46));
                (a, t)
            })
            .collect();
        self.notes = notes;
        true
    }

    /// What is playing: title, artist, playing. Returns whether it changed.
    pub fn set_media(&mut self, media: Option<(String, String, bool)>) -> bool {
        if media == self.media {
            return false;
        }
        let Some((_, regular)) = &self.fonts else { return false };
        if let Some((title, artist, _)) = &media {
            self.media_labels.0.set(regular, &cut(title, 34));
            self.media_labels.1.set(regular, &cut(artist, 40));
        }
        self.media = media;
        true
    }

    /// The clock line's height.
    fn clock_h(&self) -> f64 {
        self.time.extent.h.max(self.date.extent.h) as f64
    }

    /// The media card's place on the left panel (logical px), under the
    /// status line.
    fn media_rect(&self) -> Rectangle<f64, smithay::utils::Logical> {
        let left = layout::panels()[0];
        let y = CLOCK_TOP + CARDS_TOP;
        Rectangle::new((left.loc.x as f64 + (left.size.w as f64 - CARD_W) / 2.0, y).into(), (CARD_W, MEDIA_H).into())
    }

    /// A media button's place: previous, play or pause, next, at the card's
    /// right.
    fn media_button_rect(&self, i: usize) -> Rectangle<f64, smithay::utils::Logical> {
        let r = self.media_rect();
        let x = r.loc.x + r.size.w - 12.0 - (3 - i) as f64 * MEDIA_BUTTON;
        Rectangle::new((x, r.loc.y + (MEDIA_H - MEDIA_BUTTON) / 2.0).into(), (MEDIA_BUTTON, MEDIA_BUTTON).into())
    }

    /// The player's button under a touch on the lock screen, if any.
    pub fn media_at(&self, x: f64, y: f64) -> Option<crate::quick::MediaButton> {
        if self.media.is_none() || !self.holds_screen() || self.blank {
            return None;
        }
        let buttons = [crate::quick::MediaButton::Previous, crate::quick::MediaButton::PlayPause, crate::quick::MediaButton::Next];
        (0..3).find(|&i| self.media_button_rect(i).contains((x, y))).map(|i| buttons[i])
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
            self.pad.show();
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
        self.pad.say(text);
    }

    /// The PIN to PAM, on a thread.
    fn submit(&mut self) {
        if self.checking.lock().unwrap().is_some() {
            return;
        }
        let pin = self.pad.take();
        if pin.is_empty() {
            return;
        }
        *self.checking.lock().unwrap() = Some(None);
        self.say("Checking…");
        let (slot, wake, verified) = (self.checking.clone(), self.wake.clone(), self.verified.clone());
        std::thread::spawn(move || {
            let ok = crate::pam::check(&pin);
            if ok {
                *verified.lock().unwrap() = Some(pin);
            } else {
                let mut pin = pin;
                unsafe { pin.as_bytes_mut().fill(0) };
            }
            *slot.lock().unwrap() = Some(Some(ok));
            wake.ping();
        });
    }

    /// The PIN that has just unlocked, once.
    pub fn take_verified_pin(&self) -> Option<String> {
        self.verified.lock().unwrap().take()
    }

    /// A check's answer, if it came: unlocked, or told so.
    pub fn poll(&mut self) -> bool {
        let answer = { *self.checking.lock().unwrap() };
        match answer {
            Some(Some(ok)) => {
                *self.checking.lock().unwrap() = None;
                if ok {
                    tracing::info!("lock: unlocked");
                    self.touched_at = PinPad::ok_centre();
                    self.open_doors();
                } else {
                    self.say("Wrong PIN");
                    self.pad.shake();
                    crate::fingerprint::buzz("bell-terminal");
                }
                true
            }
            _ => false,
        }
    }

    fn open_doors(&mut self) {
        self.door = crate::door::Style::read();
        tracing::info!("lock: doors {:?}, {} ms", self.door.mode, self.door.ns / 1_000_000);
        self.fading = Some(hybris_hwc::now_ns());
        self.unlocked = self.fading;
        // The rest is reset when the doors are open: the picture they turn
        // is the lock screen as it was.
    }

    /// The time brought up to the minute; whether it changed.
    pub fn refresh(&mut self) -> bool {
        let Some((_, regular)) = &self.fonts else { return false };
        let now = local_time();
        let minute = now.tm_hour * 60 + now.tm_min;
        let accent = crate::accent::version();
        if minute == self.minute && accent == self.clock_accent {
            return false;
        }
        if accent != self.clock_accent {
            self.clock_accent = accent;
            let mut c = crate::accent::get_f();
            c[3] = 0.95;
            self.time.set_color(c);
            self.date.set_color(c);
        }
        self.minute = minute;
        if let Some((bold, medium)) = &self.clock_fonts {
            self.time.set(bold, &format!("{}:{:02}", now.tm_hour, now.tm_min));
            self.date.set(medium, &crate::clock::short_date(&now));
        }
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
        self.pad.clear();
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
        self.touched_at = Self::mark_centre();
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
    /// Dimmed, the screen about to go dark for idleness.
    pub fn dimming(&self, now_ns: u64) -> bool {
        self.idle_ns != 0 && !self.blank && now_ns.saturating_sub(self.last_touch_ns) + crate::idle::DIM_NS >= self.idle_ns
    }

    pub fn idle(&mut self, now_ns: u64) -> bool {
        if self.idle_ns == 0 || self.blank || now_ns.saturating_sub(self.last_touch_ns) < self.idle_ns {
            return false;
        }
        self.lock_now();
        self.set_blank(true);
        true
    }

    pub fn down(&mut self, slot: TouchSlot, x: f64, y: f64, time_us: u64) {
        if self.entering {
            self.pad.down(x, y);
        }
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
                self.pad.slide();
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
            let tapped = self.pad.pressing();
            if let Some(press) = self.pad.up() {
                if press == Press::Ok {
                    self.submit();
                }
            } else if !tapped && !self.after_boot && (g.2 .1 - g.1 > UNLOCK || g.3 > FLING) {
                self.entering = false;
                self.pad.clear();
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
            if frame_ns >= t + self.door.ns {
                self.locked = false;
                self.fading = None;
                self.lift = 0.0;
                if self.after_boot {
                    // Opened with the PIN since the phone booted: a session
                    // again (the compositor restarted) locks as any lock.
                    let _ = std::fs::write(boot_marker(), "");
                }
                self.after_boot = false;
                self.entering = false;
                self.fails = 0;
                self.pad.clear();
            }
            return true;
        }
        if !self.locked || self.blank {
            return false;
        }
        let moving = |t: Option<u64>, d: u64| t.is_some_and(|t| frame_ns < t + d);
        self.grab.is_some()
            || self.checking.lock().unwrap().is_some()
            || (self.entering && self.pad.moving(frame_ns))
            || moving(self.mark_shake, SHAKE_NS)
            || moving(self.mark_flash, FLASH_NS)
            || moving(Some(self.shown_at), MARK_IN_NS)
            || (self.fingers > 0 && !self.entering && !self.after_boot && frame_ns < self.shown_at + PULSE_FOR_NS)
    }

    pub fn warm_up(&self, renderer: &mut GlesRenderer) -> usize {
        [&self.time, &self.date, &self.hint, &self.greeting, &self.status]
            .into_iter()
            .map(|l| &l.buffer)
            .chain(self.pad.buffers())
            .chain(self.mark.iter())
            .chain(self.mark_red.iter())
            .chain(self.glow.iter())
            .filter(|b| MemoryRenderBufferRenderElement::from_buffer(renderer, (0.0, 0.0), b, None, None, None, Kind::Unspecified).is_ok())
            .count()
    }

    fn mark_centre() -> (f64, f64) {
        let right = layout::panels()[1];
        ((right.loc.x + right.size.w) as f64 - 18.0 - MARK / 2.0, POWER_Y)
    }

    /// Where the unlock was touched: a wave starts there.
    pub fn touched_at(&self) -> (f64, f64) {
        self.touched_at
    }

    /// The doors turning in depth (not sliding): their style, how far open
    /// (0..1), and since when - the output draws them from a picture of the
    /// lock screen (door.rs).
    pub fn turning(&self, frame_ns: u64) -> Option<(crate::door::Style, f64, u64)> {
        let t = self.fading?;
        (self.door.mode != crate::door::Mode::Slide).then(|| (self.door, ease(frame_ns.saturating_sub(t) as f64 / self.door.ns as f64), t))
    }

    pub fn elements(&self, renderer: &mut GlesRenderer, frame_ns: u64) -> Vec<ShellElement> {
        if !self.locked || self.turning(frame_ns).is_some() {
            return Vec::new();
        }
        // The doors sliding: each half out to its own side.
        let open = self.fading.map(|t| ease(frame_ns.saturating_sub(t) as f64 / self.door.ns as f64)).unwrap_or(0.0);
        self.draw(renderer, frame_ns, open)
    }

    /// The lock screen as it stands, for the picture the doors turn.
    pub fn picture(&self, renderer: &mut GlesRenderer, frame_ns: u64) -> Vec<ShellElement> {
        self.draw(renderer, frame_ns, 0.0)
    }

    fn draw(&self, renderer: &mut GlesRenderer, frame_ns: u64, open: f64) -> Vec<ShellElement> {
        let panels = layout::panels();
        let (left, right) = (panels[0], panels[1]);
        let middle = (left.loc.x + left.size.w + right.loc.x) as f64 / 2.0;
        let (w, h) = layout::LAYOUT;
        let left_dx = -middle * open;
        let right_dx = (w as f64 - middle) * open;
        // The pad, with the right half.
        let mut out = if self.entering { self.pad.elements(renderer, frame_ns, right_dx) } else { Vec::new() };
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
            // Drawn above (pinpad.rs).
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
            // It comes up softly where the lock screen shows.
            let appear = ease(frame_ns.saturating_sub(self.shown_at) as f64 / MARK_IN_NS as f64);
            let alpha = (flash.map(|f| 0.9 + 0.1 * f).unwrap_or(pulse) * appear) as f32;
            let mx = (right.loc.x + right.size.w) as f64 - 18.0 - MARK;
            let my = POWER_Y - MARK / 2.0;
            // No picture of a finger: a soft glow in the accent hugging the
            // edge across from the reader, breathing with the pulse, brighter
            // a moment at a known finger, red and shaking at an unknown one.
            let _ = (mx, my);
            let glow = if shaking {
                self.mark_red.clone()
            } else {
                let mut l = self.mark_lit.borrow_mut();
                if l.as_ref().is_none_or(|(v, _)| *v != crate::accent::version()) {
                    *l = Some((crate::accent::version(), soft_glow(SENSOR_GLOW, crate::accent::get())));
                }
                l.as_ref().map(|(_, b)| b.clone())
            };
            if let Some(glow) = glow {
                let (gx, gy) = ((right.loc.x + right.size.w) as f64 - SENSOR_IN - SENSOR_GLOW / 2.0, POWER_Y - SENSOR_GLOW / 2.0);
                let a = if shaking { 1.0 } else { flash.map(|f| 0.75 + 0.25 * f as f32).unwrap_or(alpha) };
                put(&mut out, &glow, gx, gy, right_dx + sx, a);
            }
        }

        // The left half: who and what. The time, the date, the status line
        // under them; after a boot a greeting; low down, what is said to you.
        let rise = -(self.lift * 0.5);
        // The clock as the desktop's: one line in the accent near the top of
        // the right panel, set to its right edge (where the desktop's
        // stands); the status line under it, as set.
        let top = CLOCK_TOP;
        let clock_panel = layout::panels()[1];
        let end = (clock_panel.loc.x + clock_panel.size.w) as f64 - CLOCK_RIGHT;
        let mut x = end - (self.time.extent.w + CLOCK_BETWEEN + self.date.extent.w) as f64;
        put(&mut out, &self.time.buffer, x, top + rise, right_dx, 1.0);
        x += (self.time.extent.w + CLOCK_BETWEEN) as f64;
        put(&mut out, &self.date.buffer, x, top + rise, right_dx, 1.0);
        let y = top + self.clock_h() + 12.0;
        // The status line: [battery] 84%   [network], centred.
        let gap = 6.0;
        let item = |icon: &Option<MemoryRenderBuffer>, l: &Label| if icon.is_some() { 16.0 + gap } else { 0.0 } + l.extent.w as f64;
        let (bw, nw) = (item(&self.battery_icon, &self.battery_label), item(&self.net_icon, &self.net_label));
        if self.battery_label.extent.w > 0 {
            let mut x = end - (bw + 22.0 + nw);
            let ly = y + rise;
            let icon_y = ly + (self.battery_label.extent.h as f64 - 16.0) / 2.0;
            for (icon, l) in [(&self.battery_icon, &self.battery_label), (&self.net_icon, &self.net_label)] {
                if let Some(i) = icon {
                    put(&mut out, i, x, icon_y, right_dx, 0.75);
                    x += 16.0 + gap;
                }
                put(&mut out, &l.buffer, x, ly, right_dx, 1.0);
                x += l.extent.w as f64 + 22.0;
            }
        }
        // The left panel: after a boot the greeting at the top, else the
        // cards.
        if self.after_boot {
            put(&mut out, &self.greeting.buffer, cx(&self.greeting, left), CLOCK_TOP + 60.0 + rise, left_dx, 1.0);
        }
        // What is playing, and the last notifications, in cards.
        if !self.after_boot {
            let mut cy = self.media_rect().loc.y + rise;
            let cx0 = self.media_rect().loc.x;
            if let Some((_, _, playing)) = &self.media {
                put(&mut out, &self.media_labels.0.buffer, cx0 + 18.0, cy + 14.0, left_dx, 1.0);
                put(&mut out, &self.media_labels.1.buffer, cx0 + 18.0, cy + 16.0 + self.media_labels.0.extent.h as f64, left_dx, 1.0);
                let icons = [&self.media_icons[0], if *playing { &self.media_icons[2] } else { &self.media_icons[1] }, &self.media_icons[3]];
                for (i, icon) in icons.into_iter().enumerate() {
                    if let Some(icon) = icon {
                        let b = self.media_button_rect(i);
                        put(&mut out, icon, b.loc.x + (MEDIA_BUTTON - 24.0) / 2.0, b.loc.y + rise + (MEDIA_BUTTON - 24.0) / 2.0, left_dx, 0.9);
                    }
                }
                put(&mut out, &self.card, cx0, cy, left_dx, 1.0);
                cy += MEDIA_H + CARDS_GAP;
            }
            for (app, title) in &self.note_labels {
                if cy + NOTE_H > (left.size.h - TALK_FROM_FOOT) as f64 - 20.0 {
                    break;
                }
                put(&mut out, &app.buffer, cx0 + 18.0, cy + 8.0, left_dx, 1.0);
                put(&mut out, &title.buffer, cx0 + 18.0, cy + 10.0 + app.extent.h as f64, left_dx, 1.0);
                put(&mut out, &self.note_card, cx0, cy, left_dx, 1.0);
                cy += NOTE_H + CARDS_GAP;
            }
        }
        // Talking: a passing notice, else the hint, else after a boot why
        // the PIN.
        let talk_y = (left.size.h - TALK_FROM_FOOT) as f64;
        if self.notice_until != 0 || !self.hint_base.is_empty() {
            put(&mut out, &self.hint.buffer, cx(&self.hint, left), talk_y, left_dx, 1.0);
        } else if self.after_boot {
            put(&mut out, &self.status.buffer, cx(&self.status, left), talk_y, left_dx, 1.0);
        }

        // The two halves' dark over the wallpaper (the output's, each half's
        // moving with it), meeting over the hinge.
        let m = (middle * SCALE as f64).round() as i32;
        let lx = (left_dx * SCALE as f64).round() as i32;
        let rx = (right_dx * SCALE as f64).round() as i32;
        let commit = CommitCounter::from((open * 1000.0) as usize);
        let lrect = Rectangle::<i32, Physical>::new((lx, 0).into(), (m, h * SCALE).into());
        let rrect = Rectangle::<i32, Physical>::new((m + rx, 0).into(), (w * SCALE - m, h * SCALE).into());
        out.push(ShellElement::Solid(SolidColorRenderElement::new(self.id.clone(), lrect, commit, [0.0, 0.0, 0.0, SHADE], Kind::Unspecified)));
        out.push(ShellElement::Solid(SolidColorRenderElement::new(self.id_right.clone(), rrect, commit, [0.0, 0.0, 0.0, SHADE], Kind::Unspecified)));
        out
    }

    /// Where the two halves stand while it is drawn (as doors, how far
    /// out each has gone, logical px); None while it is not, or goes as a
    /// picture (door.rs).
    pub fn doors(&self, frame_ns: u64) -> Option<(f64, f64)> {
        if !self.locked || self.turning(frame_ns).is_some() {
            return None;
        }
        let open = self.fading.map(|t| ease(frame_ns.saturating_sub(t) as f64 / self.door.ns as f64)).unwrap_or(0.0);
        Some(Self::door_dx(open))
    }

    fn door_dx(open: f64) -> (f64, f64) {
        let panels = layout::panels();
        let middle = (panels[0].loc.x + panels[0].size.w + panels[1].loc.x) as f64 / 2.0;
        let w = layout::LAYOUT.0 as f64;
        (-middle * open, (w - middle) * open)
    }

    /// The PIN pad's keys as drops, moved with the right half by `dx`.
    pub fn pad_drops(&self, frame_ns: u64, dx: f64) -> Option<Vec<crate::pinpad::Group>> {
        if !self.entering {
            return None;
        }
        let (groups, _) = self.pad.drops(frame_ns, (0.0, 0.0), 0.0)?;
        Some(
            groups
                .into_iter()
                .map(|mut g| {
                    for d in &mut g.drops {
                        d.0 .0 += dx;
                    }
                    g
                })
                .collect(),
        )
    }
}

/// `text` cut to `n` characters, an ellipsis after.
fn cut(text: &str, n: usize) -> String {
    if text.chars().count() <= n {
        text.to_owned()
    } else {
        format!("{}…", text.chars().take(n - 1).collect::<String>())
    }
}

/// Kept in the user's runtime directory (memory, gone at a reboot) once the
/// phone has been opened with the PIN since it booted.
fn boot_marker() -> std::path::PathBuf {
    let dir = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".into());
    std::path::Path::new(&dir).join("item-opened")
}

/// Whether the phone has been opened with the PIN since it booted: if so,
/// a new session locks as any lock (the reader on), not as after a boot.
pub fn opened_since_boot() -> bool {
    boot_marker().exists()
}
