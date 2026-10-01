//! The first setup, once (docs/LOCK-AND-SETUP.md): welcome, the PIN, a
//! finger, all set. The left panel says what and why, the right one is where
//! you act.
//!
//! - **The PIN.** Droidian's images come with 1234, found by trying it with
//!   PAM at the start: then a new PIN is asked for twice and set, else the
//!   one there is asked for once. pam_unix's "obscure" turns down a PIN of
//!   fewer than eight digits when the user changes it, so the new one is set
//!   as root: `sudo -S chpasswd`, sudo taking the PIN there is as the user's
//!   password. The login keyring's password follows it through
//!   gnome-keyring's ChangeWithMasterPassword, so that after the next boot
//!   the PIN still opens it. The PIN is never logged.
//! - **A finger**, through Droidian's reader daemon (fingerprint.rs).
//! - **Done**: `~/.config/item/setup-done` is written, and the setup goes
//!   in a wave from its circle.
//!
//! **Motion.** One circle carries the setup through (orb.frag), and goes
//! where the user is to act: on the welcome it grows out of a point on the
//! left and breathes; at "Get started" it rolls over the hinge to the right,
//! shrinking to the PIN's step mark above the pad; for the finger it grows
//! into a ring by the reader with a fingerprint inside, whose ridges come
//! from the middle out and light up, one touch after another; at the end it
//! rolls back to the left, closing green, and the wave that opens the
//! desktop starts from it. A move takes 0.4-0.8 s by how far it goes, along
//! a slight arc. Words leave quickly (180 ms, drifting up), and the next
//! come in after them, one after another (280 ms each, rising 12 px, 60 ms
//! apart); the right panel follows the left by 80 ms.
//!
//! **A finger, touch by touch.** Each touch the reader takes lights the
//! print further, flashes it and the ring, buzzes, and says what next on
//! the left; one it could not take shakes the print and says why, in amber.
//! The last lights the print whole, and a moment after the setup moves on.
//!
//! Colours only where they mean something: coral for progress and the
//! buttons, green for done, amber for a touch to do again, red for a wrong
//! PIN.
//!
//! `SETUP=1` shows it though it was done; `SETUP_DRY=1` changes nothing (no
//! PIN set, no finger enrolled, nothing written), to try the screens: there
//! the reader only identifies, and each touch counts as taken. With
//! `SETUP_AUTO=1` too, the touches come by themselves (a second apart, the
//! third not taken), for scripted runs.

use std::sync::{Arc, Mutex};

use smithay::backend::renderer::element::memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement};
use smithay::backend::renderer::element::solid::SolidColorRenderElement;
use smithay::backend::renderer::element::{Id, Kind};
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::renderer::utils::CommitCounter;
use smithay::utils::{Physical, Rectangle};

use crate::layout::{self, SCALE};
use crate::pinpad::{ease, PinPad, Press};
use crate::shade::ShellElement;
use crate::text::{Font, Label};

const DEFAULT_PIN: &str = "1234";
const MIN_PIN: usize = 4;
/// Words: the old ones going, the new ones coming one after another, the
/// right panel after the left.
const WORDS_OUT_NS: u64 = 180_000_000;
const WORDS_IN_NS: u64 = 280_000_000;
const WORDS_GAP_NS: u64 = 60_000_000;
const RIGHT_AFTER_NS: u64 = 80_000_000;
const RISE: f64 = 12.0;
/// The circle's moves, by how far: from 0.4 s, 0.45 ms more a px, up to
/// 0.8 s; its arc, of the way across; its breath on the welcome.
const MOVE_MIN_NS: f64 = 400_000_000.0;
const MOVE_PER_PX_NS: f64 = 450_000.0;
const MOVE_MAX_NS: f64 = 800_000_000.0;
const ARC: f64 = 0.12;
const BREATH_NS: f64 = 3_200_000_000.0;
/// The print: its ridges coming (after the circle is nearly there), its
/// light following the reader, a touch's flash, the pause after the last.
const PRINT_IN_NS: f64 = 700_000_000.0;
const LIT_TAU_NS: f64 = 260_000_000.0;
const FLASH_NS: f64 = 700_000_000.0;
const LAST_PAUSE_NS: u64 = 900_000_000;
/// SETUP_DRY's touches for a whole finger.
const DRY_TOUCHES: i32 = 8;
/// The setup going at the end, its drop handed to the dock.
const LEAVE_NS: u64 = 600_000_000;
/// The end's drop: the dock's height.
const LAST_DROP_R: f64 = 36.0;
const BUTTON_W: f64 = 240.0;
const BUTTON_H: f64 = 56.0;
/// The welcome's circle (and the end's) on the left; the PIN's step mark
/// and the finger's ring on the right, the ring level with the reader.
const WELCOME_Y: f64 = 250.0;
/// How dark the wallpaper is under the setup.
const SHADE: f32 = 0.5;
const PIN_MARK_Y: f64 = 120.0;
const PRINT_R: f64 = 104.0;
/// The reader, under the power key (lock.rs's POWER_Y).
const READER_Y: f64 = 455.0;
const ARROW_ICON: &str = "/usr/share/icons/Adwaita/symbolic/actions/go-next-symbolic.svg";

const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
/// The buttons: a soft coral.
const CORAL: [u8; 4] = [0xf0, 0x8a, 0x7b, 255];
/// Progress, as the buttons: coral.
const ACCENT: [f32; 4] = [0.941, 0.541, 0.482, 1.0];
const GREEN: [f32; 4] = [0.180, 0.761, 0.494, 1.0];
const AMBER: [f32; 4] = [0.965, 0.729, 0.290, 1.0];
const NOTE: [f32; 4] = [1.0, 1.0, 1.0, 0.85];

#[derive(Clone, Copy, PartialEq, Debug)]
enum Step {
    Welcome,
    /// Finding out whether the PIN is Droidian's.
    Checking,
    PinNew,
    PinAgain,
    PinCurrent,
    /// The PIN being checked or set.
    Saving,
    Finger,
    Done,
}

fn marker() -> std::path::PathBuf {
    std::path::Path::new(&std::env::var("HOME").unwrap_or_default()).join(".config/item/setup-done")
}

/// Whether the first setup is to be shown.
pub fn wanted() -> bool {
    std::env::var_os("SETUP").is_some() || std::env::var_os("SETUP_DRY").is_some() || !marker().exists()
}

/// The keyring's password from `old` to `new` (gnome-keyring's
/// ChangeWithMasterPassword, secrets in a plain session).
fn change_keyring(old: &str, new: &str) -> Result<(), String> {
    use zbus::zvariant::{ObjectPath, OwnedObjectPath, Value};
    let conn = zbus::blocking::Connection::session().map_err(|e| e.to_string())?;
    let (_, session): (zbus::zvariant::OwnedValue, OwnedObjectPath) = conn
        .call_method(Some("org.freedesktop.secrets"), "/org/freedesktop/secrets", Some("org.freedesktop.Secret.Service"), "OpenSession", &("plain", Value::from("")))
        .and_then(|r| r.body().deserialize())
        .map_err(|e| e.to_string())?;
    let secret = |s: &str| (session.clone(), Vec::<u8>::new(), s.as_bytes().to_vec(), "text/plain");
    let collection = ObjectPath::try_from("/org/freedesktop/secrets/collection/login").unwrap();
    conn.call_method(
        Some("org.freedesktop.secrets"),
        "/org/freedesktop/secrets",
        Some("org.gnome.keyring.InternalUnsupportedGuiltRiddenInterface"),
        "ChangeWithMasterPassword",
        &(collection, secret(old), secret(new)),
    )
    .map(|_| ())
    .map_err(|e| e.to_string())
}

/// The user's password to `new` as root, sudo taking `old`.
fn set_pin(old: &str, new: &str) -> Result<(), String> {
    use std::io::Write;
    let user = std::env::var("USER").map_err(|e| e.to_string())?;
    let mut child = std::process::Command::new("sudo")
        .args(["-S", "-p", "", "/usr/sbin/chpasswd"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = write!(stdin, "{old}\n{user}:{new}\n");
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_owned());
    }
    change_keyring(old, new).map_err(|e| format!("the keyring: {e}"))
}

/// The circle: where it is and what it looks like (orb.frag's uniforms),
/// logical px.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Orb {
    pub x: f64,
    pub y: f64,
    pub r: f64,
    pub thickness: f64,
    pub progress: f64,
    pub segments: f64,
    pub base: [f32; 4],
    pub accent: [f32; 4],
    /// The fingerprint inside: how much of it is shown, lit, flashing.
    pub print: f64,
    pub lit: f64,
    pub flash: f64,
}

impl Orb {
    /// `k` of the way from this to `to`.
    fn lerp(&self, to: &Orb, k: f64) -> Orb {
        let mix = |a: f64, b: f64| a + (b - a) * k;
        let mix4 = |a: [f32; 4], b: [f32; 4]| std::array::from_fn(|i| a[i] + (b[i] - a[i]) * k as f32);
        Orb {
            x: mix(self.x, to.x),
            y: mix(self.y, to.y),
            r: mix(self.r, to.r),
            thickness: mix(self.thickness, to.thickness),
            progress: mix(self.progress, to.progress),
            segments: if k < 0.5 { self.segments } else { to.segments },
            base: mix4(self.base, to.base),
            accent: mix4(self.accent, to.accent),
            print: mix(self.print, to.print),
            lit: to.lit,
            flash: to.flash,
        }
    }
}

/// A step's words, and when they come in.
#[derive(Clone)]
struct Words {
    title: Label,
    lines: Vec<Label>,
    button: Label,
    at: u64,
    /// Where the title stands.
    top: f64,
}

pub struct Setup {
    pub active: bool,
    dry: bool,
    step: Step,
    leaving: Option<u64>,
    /// The drop as it was when the setup went, for the dock to take: when,
    /// its middle and radius.
    handed: Option<(u64, (f64, f64), f64)>,
    pad: PinPad,
    /// The new PIN, between its two entries.
    first: String,
    /// Answers from threads: whether the PIN is Droidian's; whether the PIN
    /// was set (or checked).
    is_default: Arc<Mutex<Option<bool>>>,
    saved: Arc<Mutex<Option<Result<String, String>>>>,
    wake: smithay::reexports::calloop::ping::Ping,
    /// A PIN that is the user's now, for the keyring's prompts (keyring.rs).
    pin_for_keyring: Option<String>,
    progress: i32,
    /// The print's light as shown, following `progress`.
    lit: f64,
    /// The last touch taken, the last not taken; the last of all, after
    /// which the setup moves on; the reader to be asked again.
    touched_at: Option<u64>,
    poor_at: Option<u64>,
    last_at: Option<u64>,
    enroll_again: bool,
    /// SETUP_AUTO's touches: the next, and how many came.
    auto: Option<(u64, u32)>,
    enrolling: bool,
    /// The fingers enrolled, for the lock's mark.
    pub fingers: usize,
    fonts: Option<(Font, Font)>,
    words: Words,
    /// The step before's words going, since when.
    old: Option<(Words, u64)>,
    /// What next, on the left while a finger is taken; the one before
    /// going, since when.
    note: Label,
    note_at: u64,
    old_note: Option<(Label, u64)>,
    orb: Orb,
    /// The circle's move: from where, since when, how long.
    from: Orb,
    moved_at: u64,
    move_ns: f64,
    orb_ns: u64,
    started: u64,
    button_bg: MemoryRenderBuffer,
    arrow: Option<MemoryRenderBuffer>,
    id: Id,
}

impl Setup {
    pub fn new(wake: smithay::reexports::calloop::ping::Ping) -> Setup {
        let thin = Font::load(&["/usr/share/fonts/truetype/lato/Lato-Light.ttf", "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf"]);
        let regular = Font::load(&["/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf", "/usr/share/fonts/truetype/lato/Lato-Regular.ttf"]);
        let tint = |rgb: [u8; 3], path: &str, size: f64| {
            let mut pixmap = crate::apps::icon_pixmap(path, size as i32)?;
            for px in pixmap.pixels_mut() {
                let a = px.alpha() as u16;
                let c = |v: u8| ((v as u16 * a) / 255) as u8;
                *px = resvg::tiny_skia::PremultipliedColorU8::from_rgba(c(rgb[0]), c(rgb[1]), c(rgb[2]), a as u8).unwrap_or(*px);
            }
            let n = pixmap.width() as i32;
            Some(MemoryRenderBuffer::from_slice(pixmap.data(), smithay::backend::allocator::Fourcc::Abgr8888, (n, n), SCALE, smithay::utils::Transform::Normal, None))
        };
        let blank = Words { title: Label::new(44.0, WHITE), lines: Vec::new(), button: Label::new(19.0, WHITE), at: 0, top: 0.0 };
        Setup {
            active: false,
            dry: std::env::var_os("SETUP_DRY").is_some(),
            step: Step::Welcome,
            leaving: None,
            handed: None,
            pad: PinPad::new(),
            first: String::new(),
            is_default: Default::default(),
            saved: Default::default(),
            wake,
            pin_for_keyring: None,
            progress: 0,
            lit: 0.0,
            touched_at: None,
            poor_at: None,
            last_at: None,
            enroll_again: false,
            auto: None,
            enrolling: false,
            fingers: 0,
            fonts: thin.zip(regular),
            words: blank,
            old: None,
            note: Label::new(19.0, NOTE),
            note_at: 0,
            old_note: None,
            orb: Orb { x: 0.0, y: WELCOME_Y, r: 0.0, thickness: 1.0, progress: 0.0, segments: 0.0, base: WHITE, accent: ACCENT, print: 0.0, lit: 0.0, flash: 0.0 },
            from: Orb { x: 0.0, y: WELCOME_Y, r: 0.0, thickness: 1.0, progress: 0.0, segments: 0.0, base: WHITE, accent: ACCENT, print: 0.0, lit: 0.0, flash: 0.0 },
            moved_at: 0,
            move_ns: MOVE_MAX_NS,
            orb_ns: 0,
            started: 0,
            button_bg: crate::grid::rounded(BUTTON_W, BUTTON_H, BUTTON_H / 2.0, CORAL),
            arrow: tint([240, 240, 240], ARROW_ICON, 32.0),
            id: Id::new(),
        }
    }

    /// Shown now: the welcome first, and whether the PIN is Droidian's found
    /// out meanwhile.
    pub fn start(&mut self, fingers: usize) {
        self.active = true;
        self.fingers = fingers;
        self.started = hybris_hwc::now_ns();
        let left = layout::panels()[0];
        // The circle grows out of a point in the left panel's middle.
        self.orb.x = left.loc.x as f64 + left.size.w as f64 / 2.0;
        self.orb.r = 0.0;
        self.from = self.orb;
        self.moved_at = self.started;
        self.move_ns = MOVE_MAX_NS;
        self.orb_ns = self.started;
        tracing::info!("setup: shown{}", if self.dry { " (dry: nothing is changed)" } else { "" });
        let slot = self.is_default.clone();
        let wake = self.wake.clone();
        std::thread::spawn(move || {
            let default = crate::pam::check(DEFAULT_PIN);
            *slot.lock().unwrap() = Some(default);
            wake.ping();
        });
        self.go(Step::Welcome);
    }

    /// Whether prompts and the reader wait: the setup is the phone's.
    pub fn holds_screen(&self) -> bool {
        self.active && self.leaving.is_none()
    }

    /// A step's words: the old ones go, these come after them.
    fn text(&mut self, title: &str, lines: &[&str], button: &str, top: f64) {
        let Some((thin, regular)) = &self.fonts else { return };
        let now = hybris_hwc::now_ns();
        let changed = self.words.title.extent.w > 0 && (title != "" || !self.words.lines.is_empty());
        if changed {
            self.old = Some((self.words.clone(), now));
        }
        let mut words = Words { title: Label::new(44.0, [1.0, 1.0, 1.0, 0.95]), lines: Vec::new(), button: Label::new(19.0, WHITE), at: now + if changed { WORDS_OUT_NS - 20_000_000 } else { 0 }, top };
        words.title.set(thin, title);
        words.lines = lines
            .iter()
            .map(|l| {
                let mut label = Label::new(19.0, [1.0, 1.0, 1.0, 0.7]);
                label.set(regular, l);
                label
            })
            .collect();
        words.button.set(regular, button);
        self.words = words;
        self.note.set(regular, "");
        self.old_note = None;
    }

    /// What next, said on the left: the one before goes, this comes.
    fn note(&mut self, text: &str, color: [f32; 4]) {
        let Some((_, regular)) = &self.fonts else { return };
        let now = hybris_hwc::now_ns();
        let mut label = Label::new(19.0, color);
        label.set(regular, text);
        let old = std::mem::replace(&mut self.note, label);
        if old.extent.w > 0 {
            self.old_note = Some((old, now));
        }
        self.note_at = now + if self.old_note.is_some() { WORDS_OUT_NS / 2 } else { 0 };
    }

    /// When the right panel's things come in: after the left's.
    fn right_at(&self) -> u64 {
        self.words.at + RIGHT_AFTER_NS + WORDS_GAP_NS * (1 + self.words.lines.len() as u64)
    }

    fn go(&mut self, step: Step) {
        tracing::info!("setup: {step:?}");
        let was = self.step;
        self.step = step;
        // The circle moves from where it is to the step's place, longer the
        // further it goes.
        let now = hybris_hwc::now_ns();
        if step != Step::Welcome {
            let to = self.orb_target(now);
            let far = (to.x - self.orb.x).hypot(to.y - self.orb.y);
            self.from = self.orb;
            self.moved_at = now;
            self.move_ns = (MOVE_MIN_NS + far * MOVE_PER_PX_NS).min(MOVE_MAX_NS);
        }
        match step {
            Step::Welcome => self.text("Hello", &["Let's make this Duo yours:", "a PIN, a fingerprint, and you're in."], "Get started", WELCOME_Y + 90.0),
            Step::Checking => self.text("Your PIN", &["One moment…"], "", 150.0),
            Step::PinNew => {
                if was != Step::PinAgain && was != Step::Saving {
                    self.text("Create a PIN", &["Your PIN unlocks the phone after a restart,", "and whenever your finger isn't recognized."], "", 150.0);
                }
                let at = self.right_at();
                self.pad.show_at(at);
                self.pad.say("Enter a new PIN");
            }
            Step::PinAgain => {
                self.pad.show();
                self.pad.say("Enter it again");
            }
            Step::PinCurrent => {
                if was != Step::Saving {
                    self.text("Your PIN", &["Enter the PIN you use now.", "It unlocks the phone after a restart."], "", 150.0);
                }
                let at = self.right_at();
                self.pad.show_at(at);
                self.pad.say("Enter PIN");
            }
            Step::Saving => self.pad.say("Saving…"),
            Step::Finger => {
                self.text("Add a fingerprint", &["Touch the sensor under the power key.", "Lift your finger after each touch,", "and move it a little each time."], "Later", 150.0);
                self.progress = 0;
                self.lit = 0.0;
                (self.touched_at, self.poor_at, self.last_at) = (None, None, None);
                if self.dry && std::env::var_os("SETUP_AUTO").is_some() {
                    self.auto = Some((hybris_hwc::now_ns() + 2_000_000_000, 0));
                }
                self.enrolling = true;
            }
            Step::Done => self.text("All set", &["Touch the sensor to unlock,", "or swipe up for your PIN."], "Start", WELCOME_Y + 90.0),
        }
    }

    /// Where the circle goes for the step, and how it looks.
    fn orb_target(&self, frame_ns: u64) -> Orb {
        let panels = layout::panels();
        let middle = |i: usize| panels[i].loc.x as f64 + panels[i].size.w as f64 / 2.0;
        let dim = |a: f32| [a, a, a, a];
        let orb = Orb { x: middle(0), y: WELCOME_Y, r: 52.0, thickness: 1.0, progress: 0.0, segments: 0.0, base: dim(0.92), accent: ACCENT, print: 0.0, lit: self.lit, flash: 0.0 };
        match self.step {
            Step::Welcome => {
                let t = frame_ns.saturating_sub(self.started) as f64;
                let breath = 1.0 + 0.025 * (t / BREATH_NS * std::f64::consts::TAU).sin();
                Orb { r: 52.0 * breath, ..orb }
            }
            Step::Checking | Step::PinNew | Step::PinAgain | Step::PinCurrent | Step::Saving => {
                let lit = match self.step {
                    Step::PinAgain | Step::Saving => 2.0 / 3.0,
                    _ => 1.0 / 3.0,
                };
                Orb { x: middle(1), y: PIN_MARK_Y, r: 14.0, thickness: 0.34, progress: lit, segments: 3.0, base: dim(0.22), ..orb }
            }
            Step::Finger => {
                // A touch: the ring swells a little and lets go.
                let flash = self.flash(frame_ns);
                // The whole finger taken: the print turns green.
                let green = self.last_at.map(|t| ease(frame_ns.saturating_sub(t) as f64 / 300_000_000.0) as f32).unwrap_or(0.0);
                let accent = std::array::from_fn(|i| ACCENT[i] + (GREEN[i] - ACCENT[i]) * green);
                Orb { x: middle(1), y: READER_Y, r: PRINT_R + 5.0 * flash, thickness: 0.022, base: dim(0.2), accent, print: 1.0, flash, ..orb }
            }
            // A drop again, the dock's size: it goes on to be the dock.
            Step::Done => Orb { r: LAST_DROP_R, ..orb },
        }
    }

    /// A touch's flash, 1 fading to 0.
    fn flash(&self, frame_ns: u64) -> f64 {
        let Some(t) = self.touched_at else { return 0.0 };
        let k = (frame_ns.saturating_sub(t) as f64 / FLASH_NS).min(1.0);
        (1.0 - k) * (1.0 - k)
    }

    /// The circle now, for the output to draw (orb.frag), with the setup's
    /// alpha.
    pub fn orb(&self) -> Option<Orb> {
        (self.active && self.leaving.is_none() && self.orb.r > 0.3).then_some(self.orb)
    }

    /// Answers from the threads, and the made-up reader; whether anything
    /// changed.
    pub fn poll(&mut self, fingerprint: &crate::fingerprint::Fingerprint) -> bool {
        if !self.active {
            return false;
        }
        let mut changed = false;
        if self.step == Step::Checking {
            let found = *self.is_default.lock().unwrap();
            if let Some(default) = found {
                tracing::info!("setup: the PIN is {}", if default { "Droidian's, to be changed" } else { "the user's own" });
                self.go(if default { Step::PinNew } else { Step::PinCurrent });
                changed = true;
            }
        }
        let saved = self.saved.lock().unwrap().take();
        if let Some(result) = saved {
            changed = true;
            match result {
                Ok(pin) => {
                    tracing::info!("setup: the PIN {}", if self.dry { "kept (dry)" } else { "is set" });
                    self.pin_for_keyring = Some(pin);
                    self.go(Step::Finger);
                    if !self.dry {
                        fingerprint.enroll(&format!("finger {}", self.fingers + 1));
                    }
                }
                Err(e) => {
                    tracing::warn!("setup: the PIN: {e}");
                    let back = if self.first.is_empty() { Step::PinCurrent } else { Step::PinNew };
                    self.first.clear();
                    self.go(back);
                    self.pad.say(if back == Step::PinCurrent { "Wrong PIN" } else { "Couldn't set the PIN" });
                    self.pad.shake();
                    crate::fingerprint::buzz("bell-terminal");
                }
            }
        }
        if self.step == Step::Finger && self.enroll_again {
            self.enroll_again = false;
            fingerprint.enroll(&format!("finger {}", self.fingers + 1));
        }
        if let Some((at, n)) = self.auto.filter(|_| self.step == Step::Finger && self.enrolling) {
            if hybris_hwc::now_ns() >= at {
                self.auto = Some((at + 1_000_000_000, n + 1));
                self.reader(if n == 2 { crate::fingerprint::Event::Poor(crate::fingerprint::Poor::Partial) } else { crate::fingerprint::Event::Identified });
                changed = true;
            }
        }
        // The last touch's light seen, on.
        if self.step == Step::Finger && self.last_at.is_some_and(|t| hybris_hwc::now_ns() >= t + LAST_PAUSE_NS) {
            self.last_at = None;
            self.go(Step::Done);
            changed = true;
        }
        changed
    }

    /// Whether the setup wants the reader to identify: SETUP_DRY's finger
    /// step, each touch counted as taken.
    pub fn wants_reader(&self) -> bool {
        self.holds_screen() && self.dry && self.step == Step::Finger && self.enrolling
    }

    /// A touch the reader took, `progress` % of the finger now.
    fn touch_taken(&mut self, progress: i32) {
        let now = hybris_hwc::now_ns();
        self.progress = progress.clamp(0, 100);
        self.touched_at = Some(now);
        self.poor_at = None;
        if self.progress == 100 {
            return;
        }
        crate::fingerprint::buzz("button-pressed");
        let next = match self.progress {
            0..=29 => "Good. Lift, and touch again",
            30..=59 => "Keep going",
            60..=84 => "Now the edges of your finger",
            _ => "Almost there",
        };
        self.note(next, NOTE);
    }

    /// What the reader says while a finger is enrolled.
    pub fn reader(&mut self, event: crate::fingerprint::Event) {
        use crate::fingerprint::Event;
        if self.step != Step::Finger || !self.enrolling {
            return;
        }
        match event {
            Event::EnrollProgress(p) if p > self.progress && p < 100 => self.touch_taken(p),
            // SETUP_DRY: any touch is taken.
            Event::Identified | Event::NotRecognized if self.dry => {
                let p = (self.progress + 100 / DRY_TOUCHES + 1).min(100);
                if p < 100 {
                    self.touch_taken(p);
                } else {
                    self.reader(Event::Enrolled);
                }
            }
            Event::Enrolled => {
                // The print lights whole, green, and a moment after the
                // setup moves on.
                self.enrolling = false;
                if !self.dry {
                    self.fingers += 1;
                }
                self.touch_taken(100);
                self.note("Done", GREEN);
                self.last_at = Some(hybris_hwc::now_ns());
            }
            Event::Poor(poor) => {
                use crate::fingerprint::Poor;
                self.poor_at = Some(hybris_hwc::now_ns());
                let why = match poor {
                    Poor::Partial => "Cover the whole sensor",
                    Poor::Insufficient => "Press a little firmer",
                    Poor::Dirty => "Clean the sensor, then try again",
                    Poor::TooFast => "Hold your finger a moment longer",
                    Poor::TooSlow => "Lift your finger a bit sooner",
                };
                self.note(why, AMBER);
            }
            Event::EnrollFailed => {
                // The reader gave up (a while with no finger): it is asked
                // again, from the start.
                self.note("Let's start again: touch the sensor", AMBER);
                self.progress = 0;
                self.touched_at = None;
                self.enroll_again = !self.dry;
            }
            _ => {}
        }
    }

    /// The PIN that is the user's now, once: the keyring's prompts take it.
    pub fn take_pin(&mut self) -> Option<String> {
        self.pin_for_keyring.take()
    }

    fn button_rect() -> Rectangle<f64, smithay::utils::Logical> {
        let right = layout::panels()[1];
        Rectangle::new(((right.loc.x as f64 + (right.size.w as f64 - BUTTON_W) / 2.0), 640.0).into(), (BUTTON_W, BUTTON_H).into())
    }

    fn has_pad(&self) -> bool {
        matches!(self.step, Step::PinNew | Step::PinAgain | Step::PinCurrent | Step::Saving)
    }

    pub fn down(&mut self, x: f64, y: f64) {
        if self.has_pad() && self.step != Step::Saving {
            self.pad.down(x, y);
        }
    }

    pub fn motion(&mut self) {
        self.pad.slide();
    }

    /// A tap let go at `(x, y)`.
    pub fn up(&mut self, x: f64, y: f64, fingerprint: &crate::fingerprint::Fingerprint) {
        if self.has_pad() {
            if self.pad.up() == Some(Press::Ok) {
                let pin = self.pad.take();
                self.pin_entered(pin);
            }
            return;
        }
        if !Self::button_rect().contains((x, y)) {
            return;
        }
        match self.step {
            Step::Welcome => self.go(Step::Checking),
            Step::Finger => {
                // Later: the reader stops.
                tracing::info!("setup: the finger later");
                self.enrolling = false;
                if !self.dry {
                    fingerprint.stop_enroll();
                }
                self.go(Step::Done);
            }
            Step::Done => self.finish(),
            _ => {}
        }
    }

    fn pin_entered(&mut self, pin: String) {
        match self.step {
            Step::PinNew if pin.len() < MIN_PIN => {
                self.pad.say("Use at least 4 digits");
                self.pad.shake();
            }
            Step::PinNew => {
                self.first = pin;
                self.go(Step::PinAgain);
            }
            Step::PinAgain if pin != self.first => {
                self.first.clear();
                self.go(Step::PinNew);
                self.pad.say("The PINs didn't match");
                self.pad.shake();
                crate::fingerprint::buzz("bell-terminal");
            }
            Step::PinAgain => {
                self.go(Step::Saving);
                let (slot, wake, dry) = (self.saved.clone(), self.wake.clone(), self.dry);
                std::thread::spawn(move || {
                    // Dry: the PIN stays Droidian's, and is the one to give
                    // the keyring.
                    let result = if dry { Ok(DEFAULT_PIN.to_owned()) } else { set_pin(DEFAULT_PIN, &pin).map(|_| pin) };
                    *slot.lock().unwrap() = Some(result);
                    wake.ping();
                });
            }
            Step::PinCurrent => {
                self.go(Step::Saving);
                self.pad.say("Checking…");
                let (slot, wake) = (self.saved.clone(), self.wake.clone());
                std::thread::spawn(move || {
                    let result = if crate::pam::check(&pin) { Ok(pin) } else { Err("refused".into()) };
                    *slot.lock().unwrap() = Some(result);
                    wake.ping();
                });
            }
            _ => {}
        }
    }

    fn finish(&mut self) {
        tracing::info!("setup: done");
        if !self.dry {
            let path = marker();
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = std::fs::write(&path, "");
        }
        let now = hybris_hwc::now_ns();
        self.leaving = Some(now);
        self.handed = Some((now, (self.orb.x, self.orb.y), self.orb.r));
    }

    /// The drop the setup went with, once: the dock is born of it.
    pub fn take_drop(&mut self) -> Option<(u64, (f64, f64), f64)> {
        self.handed.take()
    }

    /// How much of the setup is still there: 1, fading to 0 as it goes.
    pub fn shown(&self, frame_ns: u64) -> f64 {
        match self.leaving {
            Some(t) => 1.0 - ease(frame_ns.saturating_sub(t) as f64 / LEAVE_NS as f64),
            None if self.active => 1.0,
            None => 0.0,
        }
    }

    /// After a frame: whether it still moves.
    pub fn settle(&mut self, frame_ns: u64) -> bool {
        if !self.active {
            return false;
        }
        if let Some(t) = self.leaving {
            if frame_ns >= t + LEAVE_NS {
                self.active = false;
                self.leaving = None;
            }
            return true;
        }
        // The print's light follows the reader.
        let dt = frame_ns.saturating_sub(self.orb_ns) as f64;
        self.orb_ns = frame_ns;
        let lit_to = self.progress as f64 / 100.0;
        self.lit += (lit_to - self.lit) * (1.0 - (-dt / LIT_TAU_NS).exp());
        if (self.lit - lit_to).abs() < 0.001 {
            self.lit = lit_to;
        }
        // The circle goes from where it was to its place (which may move on
        // its own meanwhile), along a slight arc.
        let target = self.orb_target(frame_ns);
        let k = (frame_ns.saturating_sub(self.moved_at) as f64 / self.move_ns).min(1.0);
        let e = ease(k);
        self.orb = self.from.lerp(&target, e);
        self.orb.y -= ARC * (target.x - self.from.x).abs() * (std::f64::consts::PI * e).sin();
        // The print's ridges come once the circle is nearly there; a touch
        // not taken shakes it.
        if self.step == Step::Finger {
            let since = frame_ns as f64 - (self.moved_at as f64 + self.move_ns * 0.7);
            self.orb.print = ease(since / PRINT_IN_NS);
            if let Some(t) = self.poor_at {
                self.orb.x += crate::pinpad::shake(frame_ns.saturating_sub(t));
            }
        }
        let notes_moving = frame_ns < self.note_at + WORDS_IN_NS || self.old_note.as_ref().is_some_and(|(_, t)| frame_ns < t + WORDS_OUT_NS);
        if self.old_note.as_ref().is_some_and(|(_, t)| frame_ns >= t + WORDS_OUT_NS) {
            self.old_note = None;
        }
        let words_moving = notes_moving || frame_ns < self.right_at() + WORDS_IN_NS + 200_000_000 || self.old.as_ref().is_some_and(|(_, t)| frame_ns < t + WORDS_OUT_NS);
        if self.old.as_ref().is_some_and(|(_, t)| frame_ns >= t + WORDS_OUT_NS) {
            self.old = None;
        }
        words_moving
            || k < 1.0
            || self.lit != lit_to
            || self.flash(frame_ns) > 0.0
            || self.poor_at.is_some_and(|t| frame_ns < t + crate::pinpad::SHAKE_NS)
            || self.step == Step::Welcome
            || (self.has_pad() && self.pad.moving(frame_ns))
            // The arrow to the reader bobs while a finger is awaited.
            || self.step == Step::Finger
    }

    pub fn warm_up(&self, renderer: &mut GlesRenderer) -> usize {
        [&self.button_bg]
            .into_iter()
            .chain(self.arrow.iter())
            .chain(self.pad.buffers())
            .filter(|b| MemoryRenderBufferRenderElement::from_buffer(renderer, (0.0, 0.0), b, None, None, None, Kind::Unspecified).is_ok())
            .count()
    }

    /// The setup as it stands, fading as it goes; the circle is the
    /// output's to draw (orb.frag, the dock's water).
    pub fn elements(&self, renderer: &mut GlesRenderer, frame_ns: u64) -> Vec<ShellElement> {
        if !self.active {
            return Vec::new();
        }
        let fade = self.shown(frame_ns);
        let panels = layout::panels();
        let (left, right) = (panels[0], panels[1]);
        let mut out = if self.has_pad() { self.pad.elements(renderer, frame_ns, 0.0) } else { Vec::new() };
        let mut put = |out: &mut Vec<ShellElement>, b: &MemoryRenderBuffer, x: f64, y: f64, alpha: f64, src: Option<Rectangle<f64, smithay::utils::Logical>>| {
            let alpha = alpha * fade;
            if alpha <= 0.001 {
                return;
            }
            let size = src.map(|s| s.size.to_i32_round());
            if let Ok(e) = MemoryRenderBufferRenderElement::from_buffer(renderer, ((x * SCALE as f64).round(), (y * SCALE as f64).round()), b, Some(alpha as f32), src, size, Kind::Unspecified) {
                out.push(ShellElement::Text(e));
            }
        };
        let cx = |w: i32, p: Rectangle<i32, smithay::utils::Logical>| (p.loc.x + (p.size.w - w) / 2) as f64;
        // A thing coming in at `at`: its alpha and how far below it still is.
        let coming = |at: u64| {
            let k = ease(frame_ns.saturating_sub(at) as f64 / WORDS_IN_NS as f64);
            let k = if frame_ns < at { 0.0 } else { k };
            (k, RISE * (1.0 - k))
        };

        // The words going: quickly, drifting up.
        if let Some((old, t)) = &self.old {
            let k = ease(frame_ns.saturating_sub(*t) as f64 / WORDS_OUT_NS as f64);
            let (alpha, dy) = (1.0 - k, -8.0 * k);
            put(&mut out, &old.title.buffer, cx(old.title.extent.w, left), old.top + dy, alpha, None);
            let mut y = old.top + old.title.extent.h as f64 + 22.0;
            for l in &old.lines {
                put(&mut out, &l.buffer, cx(l.extent.w, left), y + dy, alpha, None);
                y += l.extent.h as f64 + 6.0;
            }
            if old.button.extent.w > 0 {
                let b = Self::button_rect();
                put(&mut out, &old.button.buffer, b.loc.x + (BUTTON_W - old.button.extent.w as f64) / 2.0, b.loc.y + (BUTTON_H - old.button.extent.h as f64) / 2.0 + dy, alpha, None);
                put(&mut out, &self.button_bg, b.loc.x, b.loc.y + dy, alpha, None);
            }
        }

        // The words coming: the title, then each line, 60 ms apart.
        let w = &self.words;
        let (a, dy) = coming(w.at);
        put(&mut out, &w.title.buffer, cx(w.title.extent.w, left), w.top + dy, a, None);
        let mut y = w.top + w.title.extent.h as f64 + 22.0;
        for (i, l) in w.lines.iter().enumerate() {
            let (a, dy) = coming(w.at + WORDS_GAP_NS * (i as u64 + 1));
            put(&mut out, &l.buffer, cx(l.extent.w, left), y + dy, a, None);
            y += l.extent.h as f64 + 6.0;
        }

        // A finger: what next under the words, the one before going; the
        // print is the circle's (orb.frag). On the right, an arrow bobbing
        // at the reader.
        if self.step == Step::Finger {
            let note_y = y + 34.0;
            if let Some((old, t)) = &self.old_note {
                let k = ease(frame_ns.saturating_sub(*t) as f64 / WORDS_OUT_NS as f64);
                put(&mut out, &old.buffer, cx(old.extent.w, left), note_y - 8.0 * k, 1.0 - k, None);
            }
            let (a, dy) = coming(self.note_at);
            put(&mut out, &self.note.buffer, cx(self.note.extent.w, left), note_y + dy, a, None);
            if let (Some(arrow), true) = (&self.arrow, self.enrolling) {
                let (a, _) = coming(self.right_at());
                let bob = 8.0 * (frame_ns as f64 / 1e9 * std::f64::consts::TAU / 1.2).sin().abs();
                put(&mut out, arrow, (right.loc.x + right.size.w) as f64 - 60.0 + bob, READER_Y - 16.0, a, None);
            }
        }

        // The right panel's button, after the left's words.
        if w.button.extent.w > 0 {
            let (a, dy) = coming(self.right_at());
            let b = Self::button_rect();
            put(&mut out, &w.button.buffer, b.loc.x + (BUTTON_W - w.button.extent.w as f64) / 2.0, b.loc.y + (BUTTON_H - w.button.extent.h as f64) / 2.0 + dy, a, None);
            put(&mut out, &self.button_bg, b.loc.x, b.loc.y + dy, a, None);
        }

        // The wallpaper through, darkened, over everything else (the output
        // puts the wallpaper under it).
        let (lw, lh) = layout::LAYOUT;
        let rect = Rectangle::<i32, Physical>::from_size((lw * SCALE, lh * SCALE).into());
        out.push(ShellElement::Solid(SolidColorRenderElement::new(self.id.clone(), rect, CommitCounter::from((fade * 1000.0) as usize), [0.0, 0.0, 0.0, SHADE * fade as f32], Kind::Unspecified)));
        out
    }
}
