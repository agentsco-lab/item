//! Locking, blanking and the power key; the lock screen
//! (docs/LOCK-AND-SETUP.md).
//!
//! - The power key (qpnp_pon, KEY_POWER) with the screen on blanks it - the
//!   display's hwc2 power mode OFF, panel and backlight dark, no frames - and
//!   locks. With the screen off it lights it again, locked.
//! - The session's idle delay (org.gnome.desktop.session idle-delay, read at
//!   start; 0: never) does the same after that long without a touch.
//! - The lock screen is the first setup's table (setup.rs, squares.rs), its
//!   squares the wallpaper's colour, dimmed: the wallpaper seen as squares
//!   (the owner's, 2026-10-31). The left panel says who and what, in the
//!   setup's words - a letter a square, lowercase: the time as a title, the
//!   date and the battery under it; after a boot a greeting; what is
//!   playing, with its buttons as words a tap takes (they work locked); the
//!   last notifications, their app and title only - what they say is for
//!   after the unlock; low down, what is said to you. The words are there
//!   at once as the screen lights (it is seen too often to type them), and
//!   typed as they change.
//! - The right panel is where you act. The setup's drop stands at its edge
//!   across from the power key, which holds the reader, a lamp under it: a
//!   known finger flashes it and unlocks (fingerprint.rs), an unknown one
//!   shakes it, coral; after five, or a swipe up at any time, the drop goes
//!   to the panel's middle and becomes the PIN pad, as in the setup
//!   (pinpad.rs). After a boot the pad is open at once: the first PIN also
//!   opens the login keyring (PAM's phosh service, pam.rs).
//! - Unlocked, the lock screen goes as the doors say (door.rs).
//! - The volume keys (gpio-keys) set the volume, locked or not.

use smithay::backend::renderer::element::memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement};
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::input::TouchSlot;

use crate::layout::{self, SCALE};
use crate::shade::{local_time, ShellElement};
use crate::pinpad::{ease, shake, PinPad, Press, SHAKE_NS};
use crate::squares::{self, Grid, Line, Table, DIM, LIT};
use crate::text::Font;

/// Said low on the left once a face is known.
const FACE_OK: &str = "it's you, swipe up";
/// A face opens the lock only this long after the PIN was last given.
const FACE_PIN_FOR: std::time::Duration = std::time::Duration::from_secs(48 * 3600);
/// The drop's flash at a known finger, its coral at an unknown one.
const FLASH_NS: u64 = 300_000_000;
/// The drop and its lamp breathe this long after the lock screen shows,
/// then rest.
const PULSE_FOR_NS: u64 = 12_000_000_000;
const PULSE_PERIOD_NS: u64 = 1_800_000_000;
/// How much of the wallpaper's colour is in the squares.
const WALL: f64 = 0.45;
/// Into pixels (door.rs), ms from the unlock: the drop gone; the squares
/// the wallpaper's whole colour; the view pulled back (from, for); the
/// table faded (from, for) - the whole within door::PIXELS_NS.
const DROP_GOES_MS: f64 = 300.0;
const PULL_COLOUR_MS: f64 = 400.0;
const PULL_ZOOM: (f64, f64) = (100.0, 1250.0);
const PULL_FADE: (f64, f64) = (1150.0, 450.0);
/// The brightest a square gets (luma): the dim words are
/// 0.47 grey, a light sky under them took them away (2026-10-31).
const TOP: f64 = 0.2;
/// Unknown fingers in a row before the PIN pad comes.
const FAILS_FOR_PIN: u32 = 5;
/// A swipe up this far brings the pad, or a fling up.
const UNLOCK: f64 = 120.0;
const FLING: f64 = 0.5;
/// The power key's middle on the right panel's right edge (logical px):
/// the drop stands across from it, this far in, this big.
const POWER_Y: f64 = 455.0;
const DROP_IN: f64 = 56.0;
const DROP_R: f64 = 26.0;
/// The drop in the pad's middle, the keys to be (the setup's), and its
/// way there and back.
const PAD_DROP_R: f64 = 40.0;
const MOVE_NS: f64 = 450_000_000.0;
/// How alive the water is: breathing, at rest.
const LIFE_PULSE: f64 = 0.3;
const LIFE_REST: f64 = 0.15;
/// The lamp's reach, in squares.
const REACH: f64 = 4.5;
/// The words on the left: from the title's row, wrapped this wide.
const WIDE: usize = 16;
/// A line's letters at most (cut after, with an ellipsis).
const CUT: usize = 16;

pub struct Lock {
    /// The PIN pad is up (pinpad.rs).
    entering: bool,
    pad: PinPad,
    /// A check running on its thread, and its answer when it comes.
    checking: std::sync::Arc<std::sync::Mutex<Option<Option<bool>>>>,
    /// The PIN that unlocked, for the keyring's prompts (keyring.rs).
    verified: std::sync::Arc<std::sync::Mutex<Option<String>>>,
    wake: smithay::reexports::calloop::ping::Ping,
    pub locked: bool,
    pub blank: bool,
    /// The first lock after a boot: the PIN, and a greeting.
    after_boot: bool,
    /// When the PIN was last given (wall time: sleep counts); a face opens
    /// the lock only within FACE_PIN_FOR of it.
    pin_at: std::time::SystemTime,
    /// Fingers enrolled with the reader; none, and the drop waits for a
    /// swipe up.
    fingers: usize,
    /// Unknown fingers in a row.
    fails: u32,
    /// When the doors began to open, how they open (door.rs), and where the
    /// unlock was touched (logical px), where a wave starts.
    fading: Option<u64>,
    touched_at: (f64, f64),
    door: crate::door::Style,
    /// When the drop last shook or flashed.
    mark_shake: Option<u64>,
    mark_flash: Option<u64>,
    /// When the lock screen last came up or was lit: the drop breathes
    /// after.
    shown_at: u64,
    /// The drop's way: to the pad (true) or back to the reader, since when.
    drop_way: (bool, u64),
    grab: Option<(TouchSlot, f64, (u64, f64), f64)>,
    /// How far the finger has lifted the lock screen, logical px.
    lift: f64,
    /// The words' thin face (the setup's).
    font: Option<Font>,
    /// The PIN pad put away, its drops gathering back (pinpad.rs).
    pad_leaving: bool,
    /// The words on the table (squares.rs).
    words: squares::Words,
    /// What the left panel says: the time and the date, the status line,
    /// the greeting.
    time: String,
    date: String,
    status: String,
    greeting: String,
    /// What is said low down: the hint, a passing notice in its place
    /// until when, what the pad says while it is up.
    hint_base: String,
    notice: String,
    notice_until: u64,
    said: String,
    /// A face known (face.rs): unlocked, not opened - the doors wait for a
    /// swipe up or a finger, so the phone is not opened before it is in
    /// hand. Said in the hint's place until then.
    face_ok: bool,
    /// Face Unlock turned on: the face taken as the PIN is typed, kept
    /// with it (face.rs).
    face_enrol: bool,
    /// The PIN's length, once known (pin_len): typed to it, the PIN is
    /// checked without OK; and the length just sent to be checked.
    pin_len: Option<usize>,
    sent_len: usize,
    /// The PIN sent without OK: wrong, OK is wanted again (the PIN may
    /// have changed length) until one is accepted.
    auto_sent: bool,
    minute: i32,
    /// When the last touch came, for the idle delay.
    pub last_touch_ns: u64,
    pub idle_ns: u64,
    /// The display was lit again since last asked.
    relit: bool,
    /// When the last unlock began, not yet asked for.
    unlocked: Option<u64>,
    /// The last notifications (app, title).
    notes: Vec<(String, String)>,
    /// What is playing (title, artist, playing).
    media: Option<(String, String, bool)>,
}

/// A symbolic icon in one colour, `size` logical px.
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

/// Where the PIN's length is kept - the length alone, never the PIN - so a
/// PIN typed to it is checked without OK, as Android's auto-confirm.
fn pin_len_path() -> std::path::PathBuf {
    crate::prefs::path("pin-length")
}

pub fn pin_len() -> Option<usize> {
    std::fs::read_to_string(pin_len_path()).ok()?.trim().parse().ok().filter(|n| (4..=16).contains(n))
}

/// A PIN accepted, or set (setup.rs): its length kept.
pub fn remember_pin_len(n: usize) {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let path = pin_len_path();
    if let Some(d) = path.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&path) {
        let _ = f.write_all(n.to_string().as_bytes());
    }
}

impl Lock {
    pub fn new(wake: smithay::reexports::calloop::ping::Ping) -> Lock {
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
                pad.water();
                pad
            },
            checking: Default::default(),
            verified: Default::default(),
            wake,
            locked: false,
            blank: false,
            after_boot: false,
            pin_at: std::time::SystemTime::now(),
            fingers: 0,
            fails: 0,
            fading: None,
            touched_at: (0.0, 0.0),
            door: crate::door::Style::default(),
            mark_shake: None,
            mark_flash: None,
            shown_at: 0,
            drop_way: (false, 0),
            grab: None,
            lift: 0.0,
            font: Font::load(&["/usr/share/fonts/truetype/lato/Lato-Light.ttf", "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf"]),
            pad_leaving: false,
            words: Default::default(),
            time: String::new(),
            date: String::new(),
            status: String::new(),
            greeting: String::new(),
            hint_base: "swipe up".into(),
            notice: String::new(),
            notice_until: 0,
            said: String::new(),
            face_ok: false,
            face_enrol: false,
            pin_len: pin_len(),
            sent_len: 0,
            auto_sent: false,
            minute: -1,
            last_touch_ns: hybris_hwc::now_ns(),
            idle_ns: idle_s * 1_000_000_000,
            relit: false,
            unlocked: None,
            notes: Vec::new(),
            media: None,
        };
        lock.pad.say("");
        lock.refresh();
        lock
    }

    /// The first lock after a boot: the PIN pad open, a greeting, no reader
    /// (a finger cannot open the keyring).
    pub fn after_boot(&mut self) {
        self.after_boot = true;
        self.lock_now();
        self.minute = -1;
        self.refresh();
    }

    /// How many fingers the reader knows: with none the drop waits for a
    /// swipe up.
    pub fn set_fingers(&mut self, n: usize) {
        self.fingers = n;
        self.set_hint(if n > 0 { "" } else { "swipe up" });
    }

    /// The status line's facts: the battery.
    pub fn set_status(&mut self, f: &crate::status::Facts) {
        // The network's name is not shown: it says where the phone is.
        let status = format!("{} %", f.battery);
        if status != self.status {
            self.status = status;
            self.lay(false);
        }
    }

    /// The last notifications, newest first: their app and title. Returns
    /// whether they changed.
    pub fn set_notes(&mut self, notes: Vec<(String, String)>) -> bool {
        let notes: Vec<(String, String)> = notes.into_iter().take(3).collect();
        if notes == self.notes {
            return false;
        }
        self.notes = notes;
        self.lay(false);
        true
    }

    /// What is playing: title, artist, playing. Returns whether it changed.
    pub fn set_media(&mut self, media: Option<(String, String, bool)>) -> bool {
        if media == self.media {
            return false;
        }
        self.media = media;
        self.lay(false);
        true
    }

    /// The player's button under a touch on the lock screen, if any: its
    /// words.
    pub fn media_at(&self, x: f64, y: f64) -> Option<crate::quick::MediaButton> {
        if self.media.is_none() || !self.holds_screen() || self.blank {
            return None;
        }
        match self.words.tapped(x, y)?.as_str() {
            "media:0" => Some(crate::quick::MediaButton::Previous),
            "media:1" => Some(crate::quick::MediaButton::PlayPause),
            "media:2" => Some(crate::quick::MediaButton::Next),
            _ => None,
        }
    }

    /// The words laid on the left panel's squares: there at once
    /// (`instant`, as the screen lights), else the changed ones erased and
    /// typed.
    fn lay(&mut self, instant: bool) {
        let Some(font) = &self.font else { return };
        let g = Grid::at(1.0);
        let h = layout::panels()[0].size.h as f64;
        let mut out = Vec::new();
        let mut row = squares::TITLE_ROW;
        out.push(Line::new(font, &self.time, 1.0, row, 2.0, 1.0, LIT, None));
        row += 3.0;
        for l in [&self.date, &self.status] {
            if !l.is_empty() {
                out.push(Line::new(font, l, 1.0, row, 1.0, 1.0, DIM, None));
                row += 1.0;
            }
        }
        row += 1.0;
        // What is said, low down; the rest stops a row above it.
        let foot = squares::wrap(&self.talk(), WIDE);
        let foot_row = ((h - g.o.1) / g.s).floor() - 2.0 - foot.len() as f64;
        if self.after_boot {
            for l in squares::wrap(&self.greeting, WIDE) {
                out.push(Line::new(font, &l, 1.0, row, 1.0, 1.0, LIT, None));
                row += 1.0;
            }
        } else {
            if let Some((title, artist, playing)) = &self.media {
                out.push(Line::new(font, &cut(&title.to_lowercase(), CUT), 1.0, row, 1.0, 1.0, LIT, None));
                row += 1.0;
                if !artist.is_empty() {
                    out.push(Line::new(font, &cut(&artist.to_lowercase(), CUT), 1.0, row, 1.0, 1.0, DIM, None));
                    row += 1.0;
                }
                row += 1.0;
                let play = if *playing { "pause" } else { "play" };
                for (i, (word, col)) in [("‹", 1.0), (play, 4.0), ("›", 11.0)].into_iter().enumerate() {
                    out.push(Line::new(font, word, col, row, 1.0, 1.0, LIT, Some(format!("media:{i}"))));
                }
                row += 2.0;
            }
            for (app, title) in &self.notes {
                if row + 1.0 >= foot_row - 1.0 {
                    break;
                }
                out.push(Line::new(font, &cut(&app.to_lowercase(), CUT), 1.0, row, 1.0, 1.0, DIM, None));
                out.push(Line::new(font, &cut(&title.to_lowercase(), CUT), 1.0, row + 1.0, 1.0, 1.0, LIT, None));
                row += 3.0;
            }
        }
        for (i, l) in foot.iter().enumerate() {
            out.push(Line::new(font, l, 1.0, foot_row + i as f64, 1.0, 1.0, DIM, None));
        }
        if instant {
            self.words.set(out, 1.0);
        } else {
            self.words.reconcile(out, 1.0);
        }
    }

    /// What is said low on the left: a passing notice, else what the pad
    /// says while it is up, else a face's word, else the hint, else after a
    /// boot why the PIN.
    fn talk(&self) -> String {
        if self.notice_until != 0 {
            self.notice.clone()
        } else if self.entering && !self.said.is_empty() {
            self.said.clone()
        } else if self.face_ok {
            FACE_OK.to_owned()
        } else if !self.hint_base.is_empty() && !self.entering {
            self.hint_base.clone()
        } else if self.after_boot {
            "pin after a restart".to_owned()
        } else {
            String::new()
        }
    }

    /// Whether the reader should listen: locked, lit, not right after a
    /// boot, with a finger to know.
    pub fn wants_reader(&self) -> bool {
        self.holds_screen() && !self.blank && !self.after_boot && self.fingers > 0
    }

    /// Whether a face would open it now (face.rs): as for a finger, and not
    /// once the PIN is asked for.
    pub fn wants_face(&self) -> bool {
        let pin_lately = self.pin_at.elapsed().map_or(true, |d| d < FACE_PIN_FOR);
        !self.face_enrol && crate::face::enrolled() && self.holds_screen() && !self.blank && !self.after_boot && self.fails < FAILS_FOR_PIN && pin_lately
    }

    /// The reader did not know a finger.
    pub fn fingerprint_failed(&mut self) {
        self.fails += 1;
        self.mark_shake = Some(hybris_hwc::now_ns());
        if self.fails >= FAILS_FOR_PIN && !self.entering {
            self.say("your pin");
            self.pad_in();
        } else {
            self.notice("not this finger");
        }
    }

    /// The pad comes: the drop goes to its middle and becomes its keys.
    fn pad_in(&mut self) {
        if !self.entering {
            let now = hybris_hwc::now_ns();
            self.entering = true;
            self.pad_leaving = false;
            self.pad.auto_len = self.pin_len;
            self.pad.show();
            self.pad.say("");
            self.drop_way = (true, now);
            self.pad.split(now + MOVE_NS as u64);
            self.lay(false);
        }
    }

    /// The hint, when nothing passing is said in its place.
    fn set_hint(&mut self, text: &str) {
        if self.hint_base != text {
            self.hint_base = text.to_owned();
            self.lay(false);
        }
    }

    /// The face's word given up (locked again, dark): the usual hint back.
    fn face_forgotten(&mut self) {
        if std::mem::take(&mut self.face_ok) {
            self.lay(false);
        }
    }

    /// Something said low on the left for 2 s.
    pub fn notice(&mut self, text: &str) {
        self.notice = text.to_lowercase();
        self.notice_until = hybris_hwc::now_ns() + 2_000_000_000;
        self.lay(false);
    }

    /// The notice's time is up: the hint is back; whether it changed.
    pub fn notice_done(&mut self, now_ns: u64) -> bool {
        if self.notice_until == 0 || now_ns < self.notice_until {
            return false;
        }
        self.notice_until = 0;
        self.lay(false);
        true
    }

    /// What the pad says, low on the left while it is up.
    fn say(&mut self, text: &str) {
        if self.said != text {
            self.said = text.to_owned();
            self.lay(false);
        }
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
        self.sent_len = pin.len();
        *self.checking.lock().unwrap() = Some(None);
        self.say("checking");
        let (slot, wake, verified) = (self.checking.clone(), self.wake.clone(), self.verified.clone());
        std::thread::spawn(move || {
            let ok = crate::pam::check(&pin);
            if ok {
                *verified.lock().unwrap() = Some(pin);
            } else {
                let mut pin = pin;
                // SAFETY: zeroing the bytes keeps the String valid UTF-8.
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
                    self.pin_at = std::time::SystemTime::now();
                    if self.pin_len != Some(self.sent_len) {
                        self.pin_len = Some(self.sent_len);
                        remember_pin_len(self.sent_len);
                    }
                    self.touched_at = self.pad.middle();
                    self.open_doors();
                } else {
                    self.say("not this one");
                    if self.auto_sent {
                        self.pin_len = None;
                        self.pad.auto_len = None;
                    }
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
        // Into pixels: the words typed away as the view pulls back (laid
        // again at the next lock).
        if self.door.mode == crate::door::Mode::Pixels {
            self.words.erase_all();
        }
        // The rest is reset when the doors are open: the picture they turn
        // is the lock screen as it was.
    }

    /// The time brought up to the minute; whether it changed.
    pub fn refresh(&mut self) -> bool {
        let now = local_time();
        let minute = now.tm_hour * 60 + now.tm_min;
        if minute == self.minute {
            return false;
        }
        self.minute = minute;
        self.time = format!("{}:{:02}", now.tm_hour, now.tm_min);
        self.date = crate::clock::short_date(&now).to_lowercase();
        if self.after_boot {
            let part = crate::sysscreen::part_of_day(now.tm_hour).to_lowercase();
            self.greeting = match crate::sysscreen::first_name() {
                Some(n) => format!("{part}, {}", n.to_lowercase()),
                None => part,
            };
        }
        self.lay(false);
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
        self.face_forgotten();
        self.entering = self.after_boot;
        self.pad_leaving = false;
        self.pad.clear();
        self.fails = 0;
        self.said.clear();
        self.notice_until = 0;
        // After a boot the pad stands at once, born of the drop already
        // in its middle; else the drop is at the reader.
        self.drop_way = (self.entering, 0);
        if self.entering {
            self.pad.auto_len = self.pin_len;
            self.pad.show();
            self.pad.say("");
            self.pad.split(hybris_hwc::now_ns());
        } else {
            self.pad.gather(0);
        }
        self.refresh();
        self.lay(true);
    }

    /// Face Unlock turned on: the PIN asked for, the face taken meanwhile.
    pub fn enrol_face(&mut self) {
        self.face_enrol = true;
        self.pad_in();
        self.say("your pin, to add your face");
    }

    /// Whether the face is being taken with the PIN.
    pub fn enrolling_face(&self) -> bool {
        self.face_enrol && self.locked
    }

    /// The PIN given while the face was taken: the enrolling over.
    pub fn take_face_enrol(&mut self) -> bool {
        std::mem::take(&mut self.face_enrol)
    }

    /// A known face (face.rs): unlocked, said so - no picture of it. The
    /// doors open with a swipe up or a finger on the reader.
    pub fn unlock_face(&mut self) {
        if !self.locked || self.fading.is_some() || self.face_ok {
            return;
        }
        tracing::info!("lock: a known face: unlocked, waiting for a swipe");
        self.face_ok = true;
        self.mark_flash = Some(hybris_hwc::now_ns());
        self.notice_until = 0;
        self.lay(false);
    }

    /// Unlocked from outside (logind's Unlock: the fingerprint reader,
    /// `loginctl unlock-session`): the doors open as after a PIN.
    pub fn unlock(&mut self) {
        if !self.locked || self.fading.is_some() {
            return;
        }
        tracing::info!("lock: unlocked from outside");
        self.mark_flash = Some(hybris_hwc::now_ns());
        self.touched_at = Self::reader_drop();
        self.open_doors();
    }

    pub fn set_blank(&mut self, blank: bool) {
        if blank == self.blank {
            return;
        }
        self.blank = blank;
        if blank {
            self.face_forgotten();
        }
        let ok = hybris_hwc::set_display_power(!blank);
        tracing::info!("lock: screen {}{}", if blank { "off" } else { "on" }, if ok { "" } else { " (hwcomposer refused)" });
        if !blank {
            self.last_touch_ns = hybris_hwc::now_ns();
            self.shown_at = self.last_touch_ns;
            self.refresh();
            self.lay(true);
            self.relit = true;
        }
    }

    /// When an unlock began, once: the dock rises after it.
    pub fn take_unlocked(&mut self) -> Option<u64> {
        self.unlocked.take()
    }

    /// How long after the unlock the dock comes in: into pixels, as the
    /// table fades (sooner it came in under the table, and was there at
    /// once as it went).
    pub fn dock_after_ns(&self) -> u64 {
        if self.door.mode == crate::door::Mode::Pixels { (PULL_FADE.0 * 1e6) as u64 } else { 250_000_000 }
    }

    /// Whether the display was lit again since last asked: it wants whole
    /// frames.
    pub fn relit(&mut self) -> bool {
        std::mem::take(&mut self.relit)
    }

    /// Dimmed, the screen about to go dark for idleness.
    pub fn dimming(&self, now_ns: u64) -> bool {
        self.idle_ns != 0 && !self.blank && now_ns.saturating_sub(self.last_touch_ns) + crate::idle::DIM_NS >= self.idle_ns
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
                // OK, or the PIN's length reached: checked.
                if press == Press::Ok {
                    self.auto_sent = self.pad.auto_len.is_some();
                    self.submit();
                }
            } else if !tapped && !self.after_boot && (g.2 .1 - g.1 > UNLOCK || g.3 > FLING) {
                // Put away as it came: the keys gather back into rows, the
                // rows into the drop, which goes back to the reader.
                let now = hybris_hwc::now_ns();
                self.entering = false;
                self.pad.gather(now);
                self.pad_leaving = true;
                self.drop_way = (false, now + crate::pinpad::SPLIT_NS);
                self.said.clear();
                self.lay(false);
            }
            return;
        }
        if self.lift > UNLOCK || g.3 < -FLING {
            if self.face_ok {
                // Unlocked by a face: the swipe opens it.
                tracing::info!("lock: opened by a swipe, the face known");
                self.touched_at = Self::reader_drop();
                self.open_doors();
            } else {
                self.pad_in();
            }
        }
        self.lift = 0.0;
    }

    /// After a frame: whether the lock screen still wants frames.
    pub fn settle(&mut self, frame_ns: u64) -> bool {
        let words = self.words.settle(frame_ns);
        // The pad put away: drawn till its drops have gathered and gone.
        if self.pad_leaving {
            if self.pad.drops(frame_ns, (0.0, 0.0), 0.0, 0.0).is_some() {
                return true;
            }
            self.pad_leaving = false;
            self.pad.clear();
        }
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
        words
            || self.grab.is_some()
            || self.checking.lock().unwrap().is_some()
            || (self.entering && self.pad.moving(frame_ns))
            || moving(Some(self.drop_way.1), MOVE_NS as u64)
            || moving(self.mark_shake, SHAKE_NS)
            || moving(self.mark_flash, FLASH_NS)
            || (!self.entering && frame_ns < self.shown_at + PULSE_FOR_NS)
    }

    pub fn warm_up(&self, renderer: &mut GlesRenderer) -> usize {
        self.words
            .lines
            .iter()
            .map(|l| &l.buffer)
            .chain(self.pad.buffers())
            .filter(|b| MemoryRenderBufferRenderElement::from_buffer(renderer, (0.0, 0.0), b, None, None, None, Kind::Unspecified).is_ok())
            .count()
    }

    /// The drop at the reader: across from the power key, in from the edge.
    fn reader_drop() -> (f64, f64) {
        let right = layout::panels()[1];
        ((right.loc.x + right.size.w) as f64 - DROP_IN, POWER_Y)
    }

    /// The drop now (its middle and radius): at the reader, at the pad's
    /// middle, or on its way between.
    fn drop_now(&self, frame_ns: u64) -> ((f64, f64), f64) {
        let (to_pad, at) = self.drop_way;
        let k = if frame_ns <= at { 0.0 } else { ease(((frame_ns - at) as f64 / MOVE_NS).min(1.0)) };
        let k = if to_pad { k } else { 1.0 - k };
        let (a, b) = (Self::reader_drop(), self.pad.middle());
        let lerp = |a: f64, b: f64| a + (b - a) * k;
        ((lerp(a.0, b.0), lerp(a.1, b.1)), lerp(DROP_R, PAD_DROP_R))
    }

    /// How alive the drop is: breathing a while after the screen lights,
    /// then at rest; a known finger's flash.
    pub fn life(&self, frame_ns: u64) -> f64 {
        let since = |t: Option<u64>| t.map(|t| frame_ns.saturating_sub(t));
        let flash = since(self.mark_flash).filter(|t| *t < FLASH_NS).map(|t| 1.0 - t as f64 / FLASH_NS as f64).unwrap_or(0.0);
        let base = if frame_ns < self.shown_at + PULSE_FOR_NS { LIFE_PULSE } else { LIFE_REST };
        base.max(flash)
    }

    /// The breath, 0..1, while the drop breathes; 0 after.
    fn breath(&self, frame_ns: u64) -> f64 {
        if frame_ns >= self.shown_at + PULSE_FOR_NS {
            return 0.0;
        }
        let u = (frame_ns.saturating_sub(self.shown_at) % PULSE_PERIOD_NS) as f64 / PULSE_PERIOD_NS as f64;
        0.5 - 0.5 * (u * std::f64::consts::TAU).cos()
    }

    /// The drop for the output to draw (the setup's water), moved with the
    /// right half by `dx`: shaking and coral a moment at an unknown finger.
    pub fn orb(&self, frame_ns: u64, dx: f64) -> Option<crate::setup::Orb> {
        if !self.locked || self.blank {
            return None;
        }
        let ((x, y), r) = self.drop_now(frame_ns);
        // Into pixels: the drop goes first.
        let r = r * (1.0 - ease(self.pulling(frame_ns).unwrap_or(0.0) / DROP_GOES_MS));
        if r < 0.5 {
            return None;
        }
        let shaking = self.mark_shake.map(|t| frame_ns.saturating_sub(t)).filter(|t| *t < SHAKE_NS);
        let sx = shaking.map(shake).unwrap_or(0.0);
        let coral = shaking.map(|t| 1.0 - t as f64 / SHAKE_NS as f64).unwrap_or(0.0);
        Some(crate::setup::Orb::water(x + dx + sx, y, r, coral))
    }

    /// The PIN pad's keys as drops, born of the drop, moved with the right
    /// half by `dx`; whether the drop is drawn with them.
    pub fn pad_drops(&self, frame_ns: u64, dx: f64) -> Option<(Vec<crate::pinpad::Group>, bool)> {
        if (!self.entering && !self.pad_leaving) || self.pulling(frame_ns).is_some() {
            return None;
        }
        let ((x, y), r) = self.drop_now(frame_ns);
        let (groups, with) = self.pad.drops(frame_ns, (x, y), r, self.life(frame_ns))?;
        let groups = groups
            .into_iter()
            .map(|mut g| {
                for d in &mut g.drops {
                    d.0 .0 += dx;
                }
                g
            })
            .collect();
        Some((groups, with))
    }

    /// The table under it all: the squares the wallpaper's colour, dimmed;
    /// the lamp under the drop, breathing with it a while.
    pub fn table(&self, frame_ns: u64) -> Option<Table> {
        if !self.locked {
            return None;
        }
        let g = Grid::at(1.0);
        let ((x, y), _) = self.drop_now(frame_ns);
        let lamp = squares::to_table(x, y);
        let mut glow = 0.85 + 0.15 * self.breath(frame_ns) + 0.3 * (self.life(frame_ns) - LIFE_REST).max(0.0);
        let mut t = Table { s: g.s, o: g.o, lamp, reach: REACH * g.s, glow, centre: (0.0, 0.0), wave: 1e5, soft: 1.0, alpha: 1.0, light: crate::setup::LAMP_GREEN, wall: WALL, top: TOP };
        // Into pixels (as the setup ends): the squares take the wallpaper's
        // whole colour and the light goes out; the view pulls back from
        // where it was touched, the squares smaller and smaller - evenly to
        // the eye (by the log) - until one is a physical pixel: the
        // wallpaper, which the desktop under it is.
        if let Some(ms) = self.pulling(frame_ns) {
            let k = ease(ms / PULL_COLOUR_MS);
            t.wall = WALL + (1.0 - WALL) * k;
            t.top = TOP + (1.0 - TOP) * k;
            glow *= 1.0 - k;
            t.glow = glow;
            let centre = squares::to_table(self.touched_at.0, self.touched_at.1);
            let small = 1.0 / (t.s * layout::SCALE as f64);
            // Gently in and out (a sine; the cubic started and stopped hard).
            let z = ((ms - PULL_ZOOM.0) / PULL_ZOOM.1).clamp(0.0, 1.0);
            let f = (small.ln() * (0.5 - 0.5 * (z * std::f64::consts::PI).cos())).exp();
            t.s *= f;
            t.o = (centre.0 + (t.o.0 - centre.0) * f, centre.1 + (t.o.1 - centre.1) * f);
            t.alpha = 1.0 - ease((ms - PULL_FADE.0) / PULL_FADE.1);
        }
        Some(t)
    }

    /// Going into pixels (door.rs): for how long (ms).
    fn pulling(&self, frame_ns: u64) -> Option<f64> {
        let t = self.fading.filter(|_| self.door.mode == crate::door::Mode::Pixels)?;
        Some(frame_ns.saturating_sub(t) as f64 / 1e6)
    }

    /// How far the halves have slid out (0..1): only the slide moves them.
    fn slid(&self, frame_ns: u64) -> f64 {
        match self.fading {
            Some(t) if self.door.mode == crate::door::Mode::Slide => ease(frame_ns.saturating_sub(t) as f64 / self.door.ns as f64),
            _ => 0.0,
        }
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
        (!matches!(self.door.mode, crate::door::Mode::Slide | crate::door::Mode::Pixels)).then(|| (self.door, ease(frame_ns.saturating_sub(t) as f64 / self.door.ns as f64), t))
    }

    pub fn elements(&self, renderer: &mut GlesRenderer, frame_ns: u64) -> Vec<ShellElement> {
        if !self.locked || self.turning(frame_ns).is_some() {
            return Vec::new();
        }
        // The doors sliding: each half out to its own side.
        self.draw(renderer, frame_ns, self.slid(frame_ns))
    }

    /// The lock screen as it stands, for the picture the doors turn.
    pub fn picture(&self, renderer: &mut GlesRenderer, frame_ns: u64) -> Vec<ShellElement> {
        self.draw(renderer, frame_ns, 0.0)
    }

    /// Its words and the pad's dots (the output draws the table, the drop
    /// and the keys' water under them).
    fn draw(&self, renderer: &mut GlesRenderer, frame_ns: u64, open: f64) -> Vec<ShellElement> {
        let (left_dx, right_dx) = Self::door_dx(open);
        let mut out = if self.entering { self.pad.elements(renderer, frame_ns, right_dx) } else { Vec::new() };
        out.extend(self.words.elements(renderer, frame_ns, left_dx));
        out
    }

    /// Whether the doors are opening (an unlock under way).
    pub fn opening(&self) -> bool {
        self.fading.is_some()
    }

    /// Where the two halves stand while it is drawn (as doors, how far
    /// out each has gone, logical px); None while it is not, or goes as a
    /// picture (door.rs).
    pub fn doors(&self, frame_ns: u64) -> Option<(f64, f64)> {
        if !self.locked || self.turning(frame_ns).is_some() {
            return None;
        }
        Some(Self::door_dx(self.slid(frame_ns)))
    }

    fn door_dx(open: f64) -> (f64, f64) {
        let panels = layout::panels();
        let middle = (panels[0].loc.x + panels[0].size.w + panels[1].loc.x) as f64 / 2.0;
        let w = layout::size().0 as f64;
        (-middle * open, (w - middle) * open)
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
