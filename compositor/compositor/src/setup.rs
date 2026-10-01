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
//! - **A finger**, through Droidian's reader daemon (fingerprint.rs): the
//!   mark on the left fills as the reader takes the finger; "Later" skips it.
//! - **Done**: `~/.config/item/setup-done` is written, and the setup fades.
//!
//! `SETUP=1` shows it though it was done; `SETUP_DRY=1` changes nothing (no
//! PIN set, no finger enrolled, the reader's progress made up, nothing
//! written), to try the screens.

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
/// A step's things coming in, and the setup going.
const STEP_IN_NS: u64 = 300_000_000;
const OUT_NS: u64 = 500_000_000;
/// The welcome's two halves meeting over the hinge.
const MEET_NS: u64 = 900_000_000;
const DISC: f64 = 120.0;
const BUTTON_W: f64 = 240.0;
const BUTTON_H: f64 = 56.0;
const BIG_MARK: f64 = 168.0;
/// The reader, under the power key (lock.rs's POWER_Y).
const READER_Y: f64 = 455.0;
const MARK_ICON: &str = "/usr/share/icons/Adwaita/symbolic/devices/auth-fingerprint-symbolic.svg";
const ARROW_ICON: &str = "/usr/share/icons/Adwaita/symbolic/actions/go-next-symbolic.svg";

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

pub struct Setup {
    pub active: bool,
    dry: bool,
    step: Step,
    step_since: u64,
    started: u64,
    leaving: Option<u64>,
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
    /// SETUP_DRY's made-up reader: when its next 20 % come.
    dry_next: u64,
    enrolling: bool,
    /// The fingers enrolled, for the lock's mark.
    pub fingers: usize,
    fonts: Option<(Font, Font)>,
    title: Label,
    lines: Vec<Label>,
    button: Label,
    note: Label,
    button_bg: MemoryRenderBuffer,
    disc: MemoryRenderBuffer,
    mark_dim: Option<MemoryRenderBuffer>,
    mark_lit: Option<MemoryRenderBuffer>,
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
        Setup {
            active: false,
            dry: std::env::var_os("SETUP_DRY").is_some(),
            step: Step::Welcome,
            step_since: 0,
            started: 0,
            leaving: None,
            pad: PinPad::new(),
            first: String::new(),
            is_default: Default::default(),
            saved: Default::default(),
            wake,
            pin_for_keyring: None,
            progress: 0,
            dry_next: 0,
            enrolling: false,
            fingers: 0,
            fonts: thin.zip(regular),
            title: Label::new(44.0, [1.0, 1.0, 1.0, 0.95]),
            lines: Vec::new(),
            button: Label::new(19.0, [1.0, 1.0, 1.0, 1.0]),
            note: Label::new(16.0, [1.0, 1.0, 1.0, 0.6]),
            button_bg: crate::grid::rounded(BUTTON_W, BUTTON_H, BUTTON_H / 2.0, [0x35, 0x84, 0xe4, 255]),
            disc: crate::grid::rounded(DISC, DISC, DISC / 2.0, [235, 235, 235, 235]),
            mark_dim: tint([70, 74, 80], MARK_ICON, BIG_MARK),
            mark_lit: tint([240, 240, 240], MARK_ICON, BIG_MARK),
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

    fn text(&mut self, title: &str, lines: &[&str], button: &str) {
        let Some((thin, regular)) = &self.fonts else { return };
        self.title.set(thin, title);
        self.lines = lines
            .iter()
            .map(|l| {
                let mut label = Label::new(19.0, [1.0, 1.0, 1.0, 0.7]);
                label.set(regular, l);
                label
            })
            .collect();
        self.button.set(regular, button);
        self.note.set(regular, "");
    }

    fn note(&mut self, text: &str) {
        if let Some((_, regular)) = &self.fonts {
            self.note.set(regular, text);
        }
    }

    fn go(&mut self, step: Step) {
        tracing::info!("setup: {step:?}");
        self.step = step;
        self.step_since = hybris_hwc::now_ns();
        match step {
            Step::Welcome => self.text("Welcome", &["Let's set up your Duo.", "It takes a minute."], "Get started"),
            Step::Checking => self.text("Your PIN", &["One moment…"], ""),
            Step::PinNew => {
                self.text("Create a PIN", &["Your PIN unlocks the phone after a restart,", "and whenever your finger isn't recognized."], "");
                self.pad.show();
                self.pad.say("Enter a new PIN");
            }
            Step::PinAgain => {
                self.pad.show();
                self.pad.say("Enter it again");
            }
            Step::PinCurrent => {
                self.text("Your PIN", &["Enter the PIN you use now.", "It unlocks the phone after a restart."], "");
                self.pad.show();
                self.pad.say("Enter PIN");
            }
            Step::Saving => self.pad.say("Saving…"),
            Step::Finger => {
                self.text("Add a fingerprint", &["Touch the sensor under the power key,", "lifting your finger and moving it", "a little each time."], "Later");
                self.progress = 0;
                self.dry_next = hybris_hwc::now_ns() + 1_500_000_000;
                self.enrolling = true;
            }
            Step::Done => self.text("All set", &["Touch the sensor to unlock,", "or swipe up for your PIN."], "Start"),
        }
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
        if self.step == Step::Finger && self.enrolling && self.dry && hybris_hwc::now_ns() >= self.dry_next {
            self.dry_next = hybris_hwc::now_ns() + 1_200_000_000;
            self.reader(crate::fingerprint::Event::EnrollProgress((self.progress + 20).min(100)));
            if self.progress >= 100 {
                self.reader(crate::fingerprint::Event::Enrolled);
            }
            changed = true;
        }
        changed
    }

    /// What the reader says while a finger is enrolled.
    pub fn reader(&mut self, event: crate::fingerprint::Event) {
        use crate::fingerprint::Event;
        if self.step != Step::Finger || !self.enrolling {
            return;
        }
        match event {
            Event::EnrollProgress(p) => {
                self.progress = p.clamp(0, 100);
                let text = format!("{}%", self.progress);
                self.note(&text);
            }
            Event::Enrolled => {
                self.enrolling = false;
                self.fingers += 1;
                self.progress = 100;
                self.go(Step::Done);
            }
            Event::EnrollFailed => {
                self.note("Let's try again: touch the sensor");
                self.progress = 0;
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
        self.leaving = Some(hybris_hwc::now_ns());
    }

    /// After a frame: whether it still moves.
    pub fn settle(&mut self, frame_ns: u64) -> bool {
        if !self.active {
            return false;
        }
        if let Some(t) = self.leaving {
            if frame_ns >= t + OUT_NS {
                self.active = false;
                self.leaving = None;
            }
            return true;
        }
        frame_ns < self.started + MEET_NS + STEP_IN_NS
            || frame_ns < self.step_since + STEP_IN_NS
            || (self.has_pad() && self.pad.moving(frame_ns))
            // The arrow to the reader bobs while a finger is awaited.
            || self.step == Step::Finger
    }

    pub fn warm_up(&self, renderer: &mut GlesRenderer) -> usize {
        [&self.button_bg, &self.disc]
            .into_iter()
            .chain(self.mark_dim.iter())
            .chain(self.mark_lit.iter())
            .chain(self.arrow.iter())
            .chain(self.pad.buffers())
            .filter(|b| MemoryRenderBufferRenderElement::from_buffer(renderer, (0.0, 0.0), b, None, None, None, Kind::Unspecified).is_ok())
            .count()
    }

    pub fn elements(&self, renderer: &mut GlesRenderer, frame_ns: u64) -> Vec<ShellElement> {
        if !self.active {
            return Vec::new();
        }
        let panels = layout::panels();
        let (left, right) = (panels[0], panels[1]);
        let middle = (left.loc.x + left.size.w + right.loc.x) as f64 / 2.0;
        let out_alpha = self.leaving.map(|t| 1.0 - ease(frame_ns.saturating_sub(t) as f64 / OUT_NS as f64)).unwrap_or(1.0);
        let step_alpha = ease(frame_ns.saturating_sub(self.step_since) as f64 / STEP_IN_NS as f64);
        let meet = ease(frame_ns.saturating_sub(self.started) as f64 / MEET_NS as f64);
        // The words come after the halves have met, on the first screen.
        let words = if self.step == Step::Welcome {
            ease(frame_ns.saturating_sub(self.started + MEET_NS * 2 / 3) as f64 / STEP_IN_NS as f64)
        } else {
            step_alpha
        };
        let mut out = if self.has_pad() { self.pad.elements(renderer, frame_ns, 0.0) } else { Vec::new() };
        let mut put = |out: &mut Vec<ShellElement>, b: &MemoryRenderBuffer, x: f64, y: f64, alpha: f64, src: Option<Rectangle<f64, smithay::utils::Logical>>| {
            let size = src.map(|s| s.size.to_i32_round());
            if let Ok(e) = MemoryRenderBufferRenderElement::from_buffer(renderer, ((x * SCALE as f64).round(), (y * SCALE as f64).round()), b, Some((alpha * out_alpha) as f32), src, size, Kind::Unspecified) {
                out.push(ShellElement::Text(e));
            }
        };
        let cx = |w: i32, p: Rectangle<i32, smithay::utils::Logical>| (p.loc.x + (p.size.w - w) / 2) as f64;

        // The left panel: what and why.
        let ty = if self.step == Step::Welcome { 330.0 } else { 140.0 };
        put(&mut out, &self.title.buffer, cx(self.title.extent.w, left), ty, words, None);
        let mut y = ty + self.title.extent.h as f64 + 22.0;
        for l in &self.lines {
            put(&mut out, &l.buffer, cx(l.extent.w, left), y, words, None);
            y += l.extent.h as f64 + 6.0;
        }

        // The welcome: two halves of a disc coming to meet over the hinge.
        if self.step == Step::Welcome {
            let r = DISC / 2.0;
            let reach = 420.0 * (1.0 - meet);
            let half = |x0: f64| Some(Rectangle::new((x0, 0.0).into(), (r, DISC).into()));
            put(&mut out, &self.disc, middle - r - reach, 170.0, meet, half(0.0));
            put(&mut out, &self.disc, middle + reach, 170.0, meet, half(r));
        }

        // A finger: the mark on the left filling from below as it is taken;
        // on the right, an arrow bobbing at the reader.
        if self.step == Step::Finger {
            let (mx, my) = (cx(BIG_MARK as i32, left), y + 40.0);
            // Topmost first: the lit part over the dim mark.
            if let (Some(lit), true) = (&self.mark_lit, self.progress > 0) {
                let h = BIG_MARK * self.progress as f64 / 100.0;
                put(&mut out, lit, mx, my + BIG_MARK - h, step_alpha, Some(Rectangle::new((0.0, BIG_MARK - h).into(), (BIG_MARK, h).into())));
            }
            if let Some(dim) = &self.mark_dim {
                put(&mut out, dim, mx, my, step_alpha, None);
            }
            put(&mut out, &self.note.buffer, cx(self.note.extent.w, left), my + BIG_MARK + 24.0, step_alpha, None);
            if let Some(arrow) = &self.arrow {
                let bob = 8.0 * (frame_ns as f64 / 1e9 * std::f64::consts::TAU / 1.2).sin().abs();
                put(&mut out, arrow, (right.loc.x + right.size.w) as f64 - 60.0 + bob, READER_Y - 16.0, step_alpha, None);
            }
        }

        // The right panel's button.
        if matches!(self.step, Step::Welcome | Step::Finger | Step::Done) {
            let b = Self::button_rect();
            put(&mut out, &self.button.buffer, b.loc.x + (BUTTON_W - self.button.extent.w as f64) / 2.0, b.loc.y + (BUTTON_H - self.button.extent.h as f64) / 2.0, words, None);
            put(&mut out, &self.button_bg, b.loc.x, b.loc.y, words, None);
        }

        // Black over everything else.
        let (w, h) = layout::LAYOUT;
        let rect = Rectangle::<i32, Physical>::from_size((w * SCALE, h * SCALE).into());
        out.push(ShellElement::Solid(SolidColorRenderElement::new(self.id.clone(), rect, CommitCounter::from((out_alpha * 1000.0) as usize), [0.0, 0.0, 0.0, out_alpha as f32], Kind::Unspecified)));
        out
    }
}
