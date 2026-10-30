//! The first setup, once: welcome, Wi-Fi, the PIN (the face taken as it is
//! typed), a finger, all set - the first thing the owner of a fresh phone
//! sees, drawn as item/grid draws its table (the owner's, 2026-10-29/30):
//! the left panel says what and why, the right one is where you act.
//!
//! - **Wi-Fi**, as Android's setup asks first: the networks NetworkManager
//!   sees on the right, the strongest first, the one joined marked; a tap
//!   joins it (`nmcli device wifi connect`), NetworkManager asking for its
//!   password through item's own agent (nm.rs) in the system's dialog, over
//!   the setup. Skippable; not shown at all when a Wi-Fi network is known
//!   already (item/grid gives the phone the computer's at an install).
//!   Joined, the clock is set from the network (systemd-timesyncd, by
//!   itself) while the rest of the setup goes on. Whether item may go online
//!   by itself is not asked here: the system screen's weather asks, where
//!   it is wanted (the owner, 2026-10-30).
//! - **The PIN.** Droidian's images come with 1234, found by trying it with
//!   PAM at the start: then a new PIN of four digits is asked for twice and
//!   set, else the one there is asked for once. There is no OK: a PIN is
//!   taken at its length (the one there's as last unlocked with, OK while
//!   that is unknown). pam_unix's "obscure" turns down a PIN of
//!   fewer than eight digits when the user changes it, so the new one is set
//!   as root: `sudo -S chpasswd`, sudo taking the PIN there is as the user's
//!   password. The login keyring's password follows it through
//!   gnome-keyring's ChangeWithMasterPassword, so that after the next boot
//!   the PIN still opens it. The PIN is never logged.
//! - **A finger**, through Droidian's reader daemon (fingerprint.rs).
//! - **Done**: `~/.config/item/setup-done` is written, and the view pulls
//!   back: the table's squares become the wallpaper's pixels.
//!
//! **The table** (table.frag, drawn by the output): dark paper, a grid of
//! squares over both panels as one, a lamp under it whose light comes
//! through the slits between the squares in item's accent, going along
//! the setup's way left to right across both panels (way()). The words are
//! a letter a square, lowercase, as few as will do (squares.rs): a title a
//! letter on 2x2 squares, typed a square at a time and erased the same
//! way; a tap takes words on the right ("› begin"), the networks are rows
//! of them. A moment worth it (the welcome, all set) is zoomed on: the
//! words erased, the squares grown, the words typed in them again.
//!
//! **The drop.** item's water drop, clear, carries the setup through and
//! goes where the user is to act, bending the table under it: it grows out
//! of a point on the right and breathes; stands small over the networks
//! and over the PIN pad, whose keys are drops born of it; for the finger it
//! grows into a ring by the reader with a fingerprint inside, whose ridges
//! come from the middle out and light up, one touch after another; at the
//! end it is the dock's size. A move takes 0.4-0.8 s by how far it goes,
//! along a slight arc.
//!
//! **The end.** At "› start" the words go, the squares take the wallpaper's
//! colour under their middles and grow small until a square is a pixel
//! (the lines going as they do, the light out): the table is the desktop's
//! picture, and goes; the dock is born of the drop.
//!
//! **A finger, touch by touch.** Each touch the reader takes lights the
//! print further, flashes it and the ring, buzzes, and says what next on
//! the left; one it could not take shakes the print and says why, in amber.
//! The last lights the print whole, and a moment after the setup moves on.
//!
//! Colour only as light, and where it means something: the accent in the
//! lamp and the print, green for a finger taken whole, amber for a touch
//! to do again, red for a wrong PIN (the pad's).
//!
//! `SETUP=1` shows it though it was done; `SETUP_DRY=1` changes nothing (no
//! PIN set, no finger enrolled, nothing written), to try the screens: there
//! the reader only identifies, and each touch counts as taken. With
//! `SETUP_AUTO=1` too, the touches come by themselves (a second apart, the
//! third not taken), for scripted runs. `SETUP_DRY=pin` leaves the PIN and
//! the marker alone but enrolls the finger for real.

use std::sync::{Arc, Mutex};

use smithay::backend::renderer::element::memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement};
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::utils::Rectangle;

use crate::layout::{self, SCALE};
use crate::pinpad::{ease, PinPad, Press};
use crate::shade::ShellElement;
use crate::squares::{self, Grid, Line, DIM, LIT};
use crate::text::Font;

const DEFAULT_PIN: &str = "1234";
const MIN_PIN: usize = 4;
/// A new PIN's length: typed to it, it is taken (no OK key).
const NEW_PIN_LEN: usize = 4;
/// The reader's arrow coming in after the words.
const WORDS_IN_NS: u64 = 280_000_000;
/// The circle's moves, by how far: from 0.4 s, 0.45 ms more a px, up to
/// 0.8 s; its arc, of the way across; its breath on the welcome.
const MOVE_MIN_NS: f64 = 400_000_000.0;
const MOVE_PER_PX_NS: f64 = 450_000.0;
const MOVE_MAX_NS: f64 = 800_000_000.0;
const ARC: f64 = 0.12;
/// The print: its ridges coming (after the circle is nearly there), its
/// light following the reader, a touch's flash, the pause after the last.
const PRINT_IN_NS: f64 = 700_000_000.0;
const LIT_TAU_NS: f64 = 260_000_000.0;
const FLASH_NS: f64 = 700_000_000.0;
const LAST_PAUSE_NS: u64 = 900_000_000;
/// SETUP_DRY's touches for a whole finger.
const DRY_TOUCHES: i32 = 8;
/// The setup going at the end (the table's squares becoming the
/// wallpaper's pixels, Setup::table), its drop handed to the dock after;
/// its words gone first.
const PULL_NS: u64 = 2_250_000_000;
const WORDS_GO_NS: f64 = 250_000_000.0;
/// Its parts, ms from "› start": from when, how long - the wallpaper's
/// colour coming into the squares, the pull back, the table going.
const PULL_WAVE: (f64, f64) = (120.0, 900.0);
const PULL_ZOOM: (f64, f64) = (550.0, 1550.0);
const PULL_FADE: (f64, f64) = (2050.0, 200.0);
/// The table in from black at the start; the lamp from one place on the
/// way to the next; where its way begins and ends (of a panel's width, in
/// from the outer edges) and its height (of the panels').
const TABLE_IN_NS: u64 = 600_000_000;
const LAMP_NS: u64 = 1_200_000_000;
const LAMP_IN: f64 = 0.15;
const LAMP_Y: f64 = 0.62;
/// The end's drop: the dock's height.
const LAST_DROP_R: f64 = 36.0;
/// The words a tap takes, on the right: the row nearest this.
const BUTTON_Y: f64 = 640.0;
/// The welcome's drop (and the end's) on the right, over its words; the
/// PIN's step mark and the finger's ring on the right too, the ring level
/// with the reader.
const WELCOME_Y: f64 = 400.0;
/// The drop over the PIN pad.
const PIN_DROP_R: f64 = 26.0;
/// A digit pressed: the drop alive, calming over this.
const POKE_NS: f64 = 1.4e9;
const PIN_MARK_Y: f64 = 120.0;
const PRINT_R: f64 = 92.0;
/// The reader, under the power key (lock.rs's POWER_Y).
const READER_Y: f64 = 455.0;
/// The networks' rows on the right: from the row nearest here down, a
/// row between, this many at most; scanned again this often while shown.
const NETS_Y: f64 = 170.0;
const NETS_MAX: usize = 7;
const RESCAN_NS: u64 = 8_000_000_000;
const ARROW_ICON: &str = "/usr/share/icons/Adwaita/symbolic/actions/go-next-symbolic.svg";

const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
/// The print's light: item's accent (accent.rs).
fn accent() -> [f32; 4] {
    crate::accent::get_f()
}
const GREEN: [f32; 4] = [0.180, 0.761, 0.494, 1.0];
const AMBER: [f32; 4] = [0.965, 0.729, 0.290, 1.0];

#[derive(Clone, Copy, PartialEq, Debug)]
enum Step {
    Welcome,
    /// The networks: one joined, or skipped.
    Wifi,
    /// Finding out whether the PIN is Droidian's.
    Checking,
    PinNew,
    PinAgain,
    PinCurrent,
    /// The PIN being checked or set.
    Saving,
    Finger,
    /// A finger is known already: on, or another.
    FingerKnown,
    Done,
}

fn marker() -> std::path::PathBuf {
    crate::prefs::path("setup-done")
}

/// Whether the first setup is to be shown.
pub fn wanted() -> bool {
    std::env::var_os("SETUP").is_some() || std::env::var_os("SETUP_DRY").is_some() || !marker().exists()
}

/// The keyring's password from `old` to `new` (gnome-keyring's
/// ChangeWithMasterPassword, secrets in a plain session).
fn change_keyring(old: &str, new: &str) -> Result<(), String> {
    use zbus::zvariant::{ObjectPath, OwnedObjectPath, Value};
    let conn = crate::bus::session().map_err(|e| e.to_string())?;
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
    // The password is set: the step is done whatever the keyring says. The
    // login keyring takes the new PIN with the old one, or with none (the
    // port's sfduo-keyring-open makes it without a password on a fresh
    // phone); refusing both, it keeps what it had and is said so - the
    // prompter (keyring.rs) answers its prompts with the PIN that unlocks.
    if let Err(e) = change_keyring(old, new).or_else(|_| change_keyring("", new)) {
        tracing::warn!("setup: the PIN is set; the keyring kept its own ({e})");
    }
    Ok(())
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
    /// As a drop: how full of coral it is (the progress), from its foot.
    pub fill: f64,
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
            fill: mix(self.fill, to.fill),
        }
    }
}

/// A Wi-Fi network as nmcli lists it.
#[derive(Clone, Debug, PartialEq)]
struct Net {
    ssid: String,
    signal: u8,
    secure: bool,
    joined: bool,
}

/// nmcli's terse line split at its unescaped colons (`\:` is a colon in a
/// name, `\\` a backslash).
fn terse(line: &str) -> Vec<String> {
    let mut out = vec![String::new()];
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                if let Some(n) = chars.next() {
                    out.last_mut().unwrap().push(n);
                }
            }
            ':' => out.push(String::new()),
            c => out.last_mut().unwrap().push(c),
        }
    }
    out
}

/// The networks in reach, the joined first, then by signal; each name once.
fn scan() -> Vec<Net> {
    let out = std::process::Command::new("nmcli").args(["-t", "-f", "IN-USE,SSID,SIGNAL,SECURITY", "device", "wifi", "list", "--rescan", "auto"]).output();
    let Ok(out) = out else { return Vec::new() };
    let mut nets: Vec<Net> = Vec::new();
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let f = terse(line);
        let [used, ssid, signal, security] = &f[..] else { continue };
        if ssid.is_empty() {
            continue;
        }
        let net = Net { ssid: ssid.clone(), signal: signal.parse().unwrap_or(0), secure: !security.is_empty() && security != "--", joined: used == "*" };
        match nets.iter_mut().find(|n| n.ssid == net.ssid) {
            Some(n) => {
                n.signal = n.signal.max(net.signal);
                n.joined |= net.joined;
            }
            None => nets.push(net),
        }
    }
    nets.sort_by(|a, b| b.joined.cmp(&a.joined).then(b.signal.cmp(&a.signal)));
    nets.truncate(NETS_MAX);
    nets
}

/// A network's row (its words are a line of the table's).
struct NetRow {
    net: Net,
}

/// A step's words: its title (a word a row), what it is about, the lines
/// at the bottom, the words a tap takes on the right (and a second).
struct StepWords(Vec<&'static str>, Vec<String>, Vec<String>, Option<String>, Option<String>);

/// The way the setup has come, 0 to 1 (the lamp under the table goes
/// along it, left to right across both panels).
fn way(step: Step) -> f64 {
    match step {
        Step::Welcome => 0.0,
        Step::Wifi => 0.25,
        Step::Checking | Step::PinNew | Step::PinAgain | Step::PinCurrent | Step::Saving => 0.5,
        Step::Finger | Step::FingerKnown => 0.75,
        Step::Done => 1.0,
    }
}

/// The table as a frame has it (output.rs, table.frag, light.frag): its
/// squares' side and where its square (0, 0) is; the lamp under it, how far
/// its light reaches, how bright; the wallpaper's colour coming into the
/// squares in a wave out from `centre` (the drop), as far as `wave`, `soft`
/// wide at its front; how much of it is shown. Logical px, the table's (the
/// hinge taken out).
#[derive(Clone, Copy, Debug)]
pub struct Table {
    pub s: f64,
    pub o: (f64, f64),
    pub lamp: (f64, f64),
    pub reach: f64,
    pub glow: f64,
    pub centre: (f64, f64),
    pub wave: f64,
    pub soft: f64,
    pub alpha: f64,
}

pub struct Setup {
    pub active: bool,
    dry: bool,
    /// The finger's step dry too (SETUP_DRY=pin: only the PIN and the
    /// marker are left alone, a finger is enrolled for real).
    dry_finger: bool,
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
    /// Whether NetworkManager knows a Wi-Fi network already.
    wifi_known: Arc<Mutex<Option<bool>>>,
    saved: Arc<Mutex<Option<Result<String, String>>>>,
    wake: smithay::reexports::calloop::ping::Ping,
    /// A PIN that is the user's now, for the keyring's prompts (keyring.rs).
    pin_for_keyring: Option<String>,
    /// CV ID (face.rs): whether it is on, and whether the face of the one
    /// typing the PIN is taken yet; the PIN accepted, not yet told.
    cvid: bool,
    face_taken: bool,
    pin_done: bool,
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
    /// The words on the table (squares.rs), each typed from its time; those
    /// going, erased since when.
    lines: Vec<Line>,
    going: Vec<(Line, u64)>,
    /// The squares' zoom: from, to, since when (it moves once the words
    /// before it are erased).
    zoom: (f64, f64, u64),
    /// The lamp under the table: from, to (of the whole way), since when.
    lamp: (f64, f64, u64),
    /// Words were laid once (the first come at their zoom, not zoomed to).
    shown_once: bool,
    /// What next, on the left while a finger is taken, and its colour.
    note: (String, [f32; 4]),
    orb: Orb,
    /// The circle's move: from where, since when, how long.
    from: Orb,
    moved_at: u64,
    /// The pad's keys are out of the drop.
    pad_split: bool,
    /// When a digit was last pressed: the drop comes alive.
    poked: u64,
    /// The drop itself touched, not yet told to the water.
    drop_poked: Option<u64>,
    move_ns: f64,
    orb_ns: u64,
    started: u64,
    arrow: Option<MemoryRenderBuffer>,
    /// Wi-Fi: the last scan's answer (from a thread), when it was asked,
    /// the rows shown; the network being joined, and the answer.
    scanned: Arc<Mutex<Option<Vec<Net>>>>,
    scan_at: u64,
    nets: Vec<NetRow>,
    joining: Option<String>,
    join_failed: Option<String>,
    joined_answer: Arc<Mutex<Option<(String, Result<(), String>)>>>,
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
            dry_finger: std::env::var_os("SETUP_DRY").is_some_and(|v| v != "pin"),
            step: Step::Welcome,
            leaving: None,
            handed: None,
            pad: {
                let mut pad = PinPad::new();
                pad.water();
                pad
            },
            first: String::new(),
            is_default: Default::default(),
            wifi_known: Default::default(),
            saved: Default::default(),
            wake,
            pin_for_keyring: None,
            cvid: std::env::var_os("CVID").is_some(),
            face_taken: false,
            pin_done: false,
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
            lines: Vec::new(),
            going: Vec::new(),
            zoom: (1.0, 1.0, 0),
            lamp: (0.0, 0.0, 0),
            shown_once: false,
            note: (String::new(), LIT),
            orb: Orb { x: 0.0, y: WELCOME_Y, r: 0.0, thickness: 1.0, progress: 0.0, segments: 0.0, base: WHITE, accent: [1.0; 4], print: 0.0, lit: 0.0, flash: 0.0, fill: 0.0 },
            from: Orb { x: 0.0, y: WELCOME_Y, r: 0.0, thickness: 1.0, progress: 0.0, segments: 0.0, base: WHITE, accent: [1.0; 4], print: 0.0, lit: 0.0, flash: 0.0, fill: 0.0 },
            moved_at: 0,
            pad_split: false,
            poked: 0,
            drop_poked: None,
            move_ns: MOVE_MAX_NS,
            orb_ns: 0,
            started: 0,
            arrow: tint([240, 240, 240], ARROW_ICON, 32.0),
            scanned: Default::default(),
            scan_at: 0,
            nets: Vec::new(),
            joining: None,
            join_failed: None,
            joined_answer: Default::default(),
        }
    }

    /// Shown now: the welcome first, and whether the PIN is Droidian's found
    /// out meanwhile.
    pub fn start(&mut self, fingers: usize) {
        self.active = true;
        self.fingers = fingers;
        self.started = hybris_hwc::now_ns();
        let right = layout::panels()[1];
        // The drop grows out of a point in the right panel's middle, where
        // you act (the left one says).
        self.orb.x = right.loc.x as f64 + right.size.w as f64 / 2.0;
        self.orb.r = 0.0;
        self.from = self.orb;
        self.moved_at = self.started;
        self.move_ns = MOVE_MAX_NS;
        self.orb_ns = self.started;
        tracing::info!("setup: shown{}", if self.dry_finger { " (dry: nothing is changed)" } else if self.dry { " (dry but the finger: the PIN is left alone)" } else { "" });
        let slot = self.is_default.clone();
        let wake = self.wake.clone();
        std::thread::spawn(move || {
            let default = crate::pam::check(DEFAULT_PIN);
            *slot.lock().unwrap() = Some(default);
            wake.ping();
        });
        let known = self.wifi_known.clone();
        std::thread::spawn(move || {
            let out = std::process::Command::new("nmcli").args(["-t", "-f", "TYPE", "connection", "show"]).output();
            *known.lock().unwrap() = Some(out.is_ok_and(|o| String::from_utf8_lossy(&o.stdout).lines().any(|l| l == "802-11-wireless")));
        });
        self.go(Step::Welcome);
    }

    /// Whether prompts and the reader wait: the setup is the phone's.
    /// CV ID: the camera wanted, unseen, for the face of the one typing the
    /// PIN - the owner.
    pub fn wants_face(&self) -> bool {
        self.cvid && self.holds_screen() && !self.face_taken && matches!(self.step, Step::PinNew | Step::PinAgain | Step::PinCurrent | Step::Saving | Step::Finger | Step::FingerKnown)
    }

    /// The face is taken (face.rs): the camera no longer wanted.
    pub fn face_taken(&mut self) {
        self.face_taken = true;
    }

    /// The PIN, once accepted (since last asked): the face taken while it
    /// was typed is the owner's, kept with it (face.rs).
    pub fn take_pin_done(&mut self) -> Option<String> {
        std::mem::take(&mut self.pin_done).then(|| self.pin_for_keyring.clone()).flatten()
    }

    pub fn holds_screen(&self) -> bool {
        self.active && self.leaving.is_none()
    }

    /// The step's words, lowercase, as few as will do: its title (a word a
    /// row, a letter on 2x2 squares), what it is about, a line at the
    /// bottom, and the words a tap takes on the right (the second under
    /// the first).
    fn words(&self) -> StepWords {
        let s = |v: &str| v.to_owned();
        let cvid = |mut foot: Vec<String>| {
            if self.cvid {
                foot.push(s("your face, as you type, is kept for cv id"));
            }
            foot
        };
        // Saving: the words of the PIN being saved.
        let pin_new = self.step == Step::PinNew || self.step == Step::PinAgain || (self.step == Step::Saving && !self.first.is_empty());
        match self.step {
            Step::Welcome => StepWords(vec!["hello"], vec![s("a minute, and this duo is yours")], Vec::new(), Some(s("› begin")), None),
            Step::Wifi => StepWords(vec!["wi-fi"], vec![s("pick a network")], vec![s("the clock is set from it")], Some(s(if self.joined().is_some() { "› next" } else { "› skip" })), None),
            // The PIN found out in a moment: its title only (words typed
            // then erased at once flickered).
            Step::Checking => StepWords(vec!["pin"], Vec::new(), Vec::new(), None, None),
            _ if pin_new => StepWords(vec!["pin"], vec![s("four new digits")], cvid(vec![s("it opens the phone after a restart")]), None, None),
            Step::PinCurrent | Step::Saving => StepWords(vec!["pin"], vec![s("the one you use now")], cvid(vec![s("it opens the phone after a restart")]), None, None),
            Step::Finger => StepWords(vec!["finger"], vec![s("touch the power key, lift, touch again")], Vec::new(), Some(s("› later")), None),
            Step::FingerKnown => {
                let known = if self.fingers == 1 { s("one is added: it opens the phone") } else { format!("{} are added: they open the phone", self.fingers) };
                StepWords(vec!["finger"], vec![known], Vec::new(), Some(s("› go on")), Some(s("› add another")))
            }
            Step::Done => StepWords(vec!["all", "set"], Vec::new(), Vec::new(), Some(s("› start")), None),
            _ => StepWords(Vec::new(), Vec::new(), Vec::new(), None, None),
        }
    }

    /// The table's words laid out for the step as it stands: the left
    /// panel says, the right is where you act. A moment worth it (the
    /// welcome, all set) is zoomed on.
    fn lay(&mut self) {
        let Some((thin, _)) = &self.fonts else { return };
        let StepWords(title, detail, foot, go, two) = self.words();
        let zoom = if matches!(self.step, Step::Welcome | Step::Done) { squares::zoom_for(&title) } else { 1.0 };
        let g = Grid::at(zoom);
        let panel = layout::panels()[0];
        let (w, h) = (panel.size.w as f64, panel.size.h as f64);
        let mut out = Vec::new();
        // The left panel: from the title's square, a square spare at the right.
        let wide = (((w - g.o.0) / g.s).floor() as usize).saturating_sub(2).max(4);
        let mut row = squares::TITLE_ROW;
        for t in &title {
            out.push(Line::new(thin, t, 1.0, row, 2.0, zoom, LIT, None));
            row += 2.0;
        }
        row += 1.0;
        for l in detail.iter().flat_map(|d| squares::wrap(d, wide)) {
            out.push(Line::new(thin, &l, 1.0, row, 1.0, zoom, DIM, None));
            row += 1.0;
        }
        if self.step == Step::Finger && !self.note.0.is_empty() {
            row += 1.0;
            for l in squares::wrap(&self.note.0, wide) {
                out.push(Line::new(thin, &l, 1.0, row, 1.0, zoom, self.note.1, None));
                row += 1.0;
            }
        }
        let foot: Vec<String> = foot.iter().flat_map(|f| squares::wrap(f, wide)).collect();
        let mut r = ((h - g.o.1) / g.s).floor() - 2.0 - foot.len() as f64;
        for l in &foot {
            out.push(Line::new(thin, l, 1.0, r, 1.0, zoom, DIM, None));
            r += 1.0;
        }
        // The right panel: the networks, then the words a tap takes, from
        // its first whole square (and one in, if there is room).
        let cols = g.cols(1);
        let first = g.first_col(1);
        if self.step == Step::Wifi {
            let room = cols.saturating_sub(3).max(4);
            let top = g.row_at(NETS_Y);
            for (i, n) in self.nets.iter().enumerate() {
                let joining = self.joining.as_deref() == Some(n.net.ssid.as_str());
                let failed = self.join_failed.as_deref() == Some(n.net.ssid.as_str());
                let mut name: String = n.net.ssid.to_lowercase().chars().take(room).collect();
                if joining {
                    name.push_str(" …");
                }
                let color = if n.net.joined || joining { LIT } else if failed { AMBER } else { DIM };
                out.push(Line::new(thin, &name, first + 1.0, top + 2.0 * i as f64, 1.0, zoom, color, Some(format!("net:{i}"))));
            }
        }
        let button_row = g.row_at(BUTTON_Y);
        for (i, (word, tag)) in [(go, "go"), (two, "two")].into_iter().enumerate() {
            if let Some(word) = word {
                let n = word.chars().count();
                let col = first + if n + 2 <= cols { 1.0 } else { 0.0 };
                out.push(Line::new(thin, &word, col, button_row + 2.0 * i as f64, 1.0, zoom, LIT, Some(tag.to_owned())));
            }
        }
        self.reconcile(out, zoom);
    }

    /// The new words in: the same words in the same place stay as they
    /// are; the others are erased, letter by letter from their end, and the
    /// new typed after them, one line after another. A new zoom erases
    /// all, zooms, then types.
    fn reconcile(&mut self, mut new: Vec<Line>, zoom: f64) {
        let now = hybris_hwc::now_ns();
        let erase = |l: &Line| l.letters() as u64 * squares::ERASE_NS;
        let old = std::mem::take(&mut self.lines);
        let mut start = now;
        if (zoom - self.zoom.1).abs() > 1e-6 {
            let erased = old.iter().map(erase).max().unwrap_or(0);
            self.going.extend(old.into_iter().map(|l| (l, now)));
            let from = self.zoom_now(now);
            self.zoom = (from, zoom, now + erased);
            start = now + erased + if self.lines_shown_once() { squares::ZOOM_NS } else { 0 };
            if !self.lines_shown_once() {
                // The first words: no zoom to see, at the zoom at once.
                self.zoom = (zoom, zoom, now);
            }
        } else {
            let mut kept = vec![false; new.len()];
            for l in old {
                match new.iter().position(|n| n.same(&l)) {
                    Some(i) if !kept[i] => {
                        kept[i] = true;
                        new[i].at = l.at;
                    }
                    _ => {
                        start = start.max(now + erase(&l));
                        self.going.push((l, now));
                    }
                }
            }
            for (i, l) in new.iter_mut().enumerate() {
                if !kept[i] {
                    l.at = u64::MAX;
                }
            }
        }
        for l in new.iter_mut().filter(|l| l.at == 0 || l.at == u64::MAX) {
            l.at = start;
            start = l.done_at() + squares::TYPE_NS;
        }
        self.lines = new;
        self.shown_once = true;
    }

    fn lines_shown_once(&self) -> bool {
        self.shown_once
    }

    /// The squares' zoom at a frame.
    fn zoom_now(&self, frame_ns: u64) -> f64 {
        let (from, to, at) = self.zoom;
        if frame_ns <= at {
            return from;
        }
        if frame_ns >= at + squares::ZOOM_NS {
            return to;
        }
        let k = ease((frame_ns - at) as f64 / squares::ZOOM_NS as f64);
        from + (to - from) * k
    }

    /// What next, said on the left while a finger is taken.
    fn note(&mut self, text: &str, color: [f32; 4]) {
        self.note = (text.to_lowercase(), color);
        self.lay();
    }

    /// When the right panel's things come in: after the left's words.
    fn right_at(&self) -> u64 {
        let now = hybris_hwc::now_ns();
        self.lines.iter().filter(|l| l.tag.is_none()).map(Line::done_at).max().unwrap_or(now).min(now + 1_500_000_000)
    }

    fn go(&mut self, step: Step) {
        tracing::info!("setup: {step:?}");
        let was = self.step;
        self.step = step;
        // No OK: a new PIN is taken at its length, the one there at the
        // length it was last unlocked with (with OK while that is unknown).
        self.pad.auto_len = match step {
            Step::PinNew | Step::PinAgain => Some(NEW_PIN_LEN),
            Step::PinCurrent => crate::lock::pin_len(),
            _ => None,
        };
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
        // The pad's keys: born of the drop once it stands over the pad;
        // gathered back into it before it goes on, which waits for them.
        let pad_step = |s: Step| matches!(s, Step::Checking | Step::PinNew | Step::PinAgain | Step::PinCurrent | Step::Saving);
        if matches!(step, Step::PinNew | Step::PinCurrent) && !self.pad_split {
            self.pad_split = true;
            self.pad.split(now.max(self.moved_at + self.move_ns as u64));
        }
        if pad_step(was) && !pad_step(step) && self.pad_split {
            self.pad_split = false;
            self.pad.gather(now);
            self.moved_at = now + crate::pinpad::SPLIT_NS;
        }
        // The lamp on along the way; the words laid for the step (before
        // the pad: its keys come after the words).
        let from = self.lamp_now(now);
        self.lamp = (from, way(step), now);
        if step == Step::Finger {
            self.note = (String::new(), LIT);
        }
        self.lay();
        match step {
            Step::Wifi => self.scan_now(),
            Step::PinNew => {
                let at = self.right_at();
                self.pad.show_at(at);
                self.pad.say("Enter a new 4-digit PIN");
            }
            Step::PinAgain => {
                self.pad.show();
                self.pad.say("Enter it again");
            }
            Step::PinCurrent => {
                let at = self.right_at();
                self.pad.show_at(at);
                self.pad.say("Enter PIN");
            }
            Step::Saving => self.pad.say("Saving…"),
            Step::Finger => {
                self.progress = 0;
                self.lit = 0.0;
                (self.touched_at, self.poor_at, self.last_at) = (None, None, None);
                if self.dry_finger && std::env::var_os("SETUP_AUTO").is_some() {
                    self.auto = Some((hybris_hwc::now_ns() + 2_000_000_000, 0));
                }
                self.enrolling = true;
            }
            Step::FingerKnown => self.lit = 1.0,
            _ => {}
        }
    }

    /// Where the lamp is along the way at a frame (0 to 1).
    fn lamp_now(&self, frame_ns: u64) -> f64 {
        let (from, to, at) = self.lamp;
        let k = ease(frame_ns.saturating_sub(at) as f64 / LAMP_NS as f64);
        from + (to - from) * k
    }

    /// The table this frame: the lamp along the way, breathing (lit
    /// whole, still, when all is set); the squares' zoom; at the end the
    /// squares taking the wallpaper's colour and growing small until they
    /// are its pixels, the light going out, then the table gone - the
    /// desktop under it the same picture (the owner's, 2026-10-29: "as if
    /// our squares were these pixels").
    pub fn table(&self, frame_ns: u64) -> Option<Table> {
        if !self.active {
            return None;
        }
        let panel = layout::panels()[0];
        let (w, h) = (panel.size.w as f64, panel.size.h as f64);
        let square = squares::square();
        let way = self.lamp_now(frame_ns);
        let lamp = ((LAMP_IN + way * (2.0 - 2.0 * LAMP_IN)) * w, LAMP_Y * h);
        let t = frame_ns.saturating_sub(self.started) as f64 / 1e9;
        // All set: the light grows over the whole table and stills, along
        // with the lamp's last move (it jumped).
        let done = if self.step == Step::Done { ease(frame_ns.saturating_sub(self.lamp.2) as f64 / LAMP_NS as f64) } else { 0.0 };
        let mut glow = 1.0 + (0.85 + 0.15 * (t * 2.4).sin() - 1.0) * (1.0 - done);
        let reach = square * (4.5 + (13.0 - 4.5) * done);
        let g = Grid::at(self.zoom_now(frame_ns));
        let (mut s, mut o) = (g.s, g.o);
        // The drop, in the table: the end's wave and pull start from it.
        let centre = squares::to_table(self.orb.x, self.orb.y);
        let (mut wave, soft) = (0.0, 2.5 * square);
        // In from black at the start.
        let mut alpha = ease(frame_ns.saturating_sub(self.started) as f64 / TABLE_IN_NS as f64);
        if let Some(at) = self.leaving {
            let ms = frame_ns.saturating_sub(at) as f64 / 1e6;
            glow *= 1.0 - ease(ms / 600.0);
            // The wallpaper's colour into the squares, out from the drop
            // across the whole table.
            let far = [(0.0, 0.0), (2.0 * w, 0.0), (0.0, h), (2.0 * w, h)].iter().map(|c: &(f64, f64)| (c.0 - centre.0).hypot(c.1 - centre.1)).fold(0.0, f64::max) + soft;
            wave = far * ease((ms - PULL_WAVE.0) / PULL_WAVE.1);
            // Then the view pulls back from the drop, the squares smaller
            // and smaller - evenly to the eye (by the log) - until one is a
            // physical pixel: the wallpaper itself.
            let small = 1.0 / (s * SCALE as f64);
            let f = (small.ln() * ease((ms - PULL_ZOOM.0) / PULL_ZOOM.1)).exp();
            s *= f;
            o = (centre.0 + (o.0 - centre.0) * f, centre.1 + (o.1 - centre.1) * f);
            alpha = 1.0 - ease((ms - PULL_FADE.0) / PULL_FADE.1);
        }
        Some(Table { s, o, lamp, reach, glow, centre, wave, soft, alpha })
    }

    /// Where the circle goes for the step, and how it looks.
    fn orb_target(&self, frame_ns: u64) -> Orb {
        let panels = layout::panels();
        let middle = |i: usize| panels[i].loc.x as f64 + panels[i].size.w as f64 / 2.0;
        let dim = |a: f32| [a, a, a, a];
        let orb = Orb { x: middle(1), y: WELCOME_Y, r: 52.0, thickness: 1.0, progress: 0.0, segments: 0.0, base: dim(0.92), accent: accent(), print: 0.0, lit: self.lit, flash: 0.0, fill: 0.0 };
        match self.step {
            // Still, waiting to be touched (it breathed in a loop, its seam
            // seen).
            Step::Welcome => orb,
            // A small drop over the networks, empty.
            Step::Wifi => Orb { x: middle(1), y: PIN_MARK_Y - 10.0, r: PIN_DROP_R, ..orb },
            // A drop over the pad, clear (no colour in the water: the
            // owner's, 2026-10-29).
            Step::Checking | Step::PinNew | Step::PinAgain | Step::PinCurrent | Step::Saving => Orb { x: middle(1), y: PIN_MARK_Y, r: PIN_DROP_R, ..orb },
            Step::Finger => {
                // A drop by the reader with the print inside; a touch
                // swells it a little, and it lets go.
                let flash = self.flash(frame_ns);
                // The whole finger taken: the print turns green.
                let green = self.last_at.map(|t| ease(frame_ns.saturating_sub(t) as f64 / 300_000_000.0) as f32).unwrap_or(0.0);
                let a = accent();
                let accent = std::array::from_fn(|i| a[i] + (GREEN[i] - a[i]) * green);
                Orb { x: middle(1), y: READER_Y, r: PRINT_R + 6.0 * flash, base: dim(0.2), accent, print: 1.0, flash, ..orb }
            }
            // The known finger: the print whole in the drop, lit.
            Step::FingerKnown => Orb { x: middle(1), y: READER_Y, r: PRINT_R, base: dim(0.2), print: 1.0, lit: 1.0, ..orb },
            // A drop again, the dock's size: it goes on to be the dock.
            Step::Done => Orb { r: LAST_DROP_R, ..orb },
        }
    }

    /// The pad's keys as drops, born of the circle's drop or gathering back
    /// into it; whether that drop is drawn with them.
    pub fn pad_drops(&self, frame_ns: u64) -> Option<(Vec<crate::pinpad::Group>, bool)> {
        if !self.active || self.leaving.is_some() {
            return None;
        }
        self.pad.drops(frame_ns, (self.orb.x, self.orb.y), self.orb.r)
    }

    /// How alive the drop is: a little at rest,
    /// more for a while after a touch, a digit or a move.
    pub fn life(&self, frame_ns: u64) -> f64 {
        let base: f64 = 0.15;
        let since = |t: u64| if t == 0 { f64::MAX } else { frame_ns.saturating_sub(t) as f64 };
        let poke = (-since(self.poked) / (POKE_NS / 3.0)).exp();
        let moved = (-since(self.moved_at) / (POKE_NS / 2.0)).exp() * 0.7;
        base.max(poke).max(moved).max(self.flash(frame_ns))
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
        // Gone at the start: the dock is born of it then.
        (self.active && self.leaving.is_none() && self.orb.r > 0.3).then_some(self.orb)
    }

    /// Whether it is going (start pressed): the dock, born, over its table.
    pub fn leaving(&self) -> bool {
        self.active && self.leaving.is_some()
    }

    /// Answers from the threads, and the made-up reader; whether anything
    /// changed.
    pub fn poll(&mut self, fingerprint: &crate::fingerprint::Fingerprint) -> bool {
        if !self.active {
            return false;
        }
        let mut changed = false;
        if self.step == Step::Wifi {
            changed |= self.poll_wifi();
        }
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
                    if !self.dry {
                        crate::lock::remember_pin_len(pin.len());
                    }
                    tracing::info!("setup: the PIN {}", if self.dry { "kept (dry)" } else { "is set" });
                    self.pin_for_keyring = Some(pin);
                    self.pin_done = true;
                    if self.fingers > 0 {
                        self.go(Step::FingerKnown);
                    } else {
                        self.go(Step::Finger);
                        if !self.dry_finger {
                            fingerprint.enroll(&format!("finger {}", self.fingers + 1));
                        }
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
        self.holds_screen() && self.dry_finger && self.step == Step::Finger && self.enrolling
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
        self.note(next, LIT);
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
            Event::Identified | Event::NotRecognized if self.dry_finger => {
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
                if !self.dry_finger {
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
                self.enroll_again = !self.dry_finger;
            }
            _ => {}
        }
    }

    /// The PIN that is the user's now, once: the keyring's prompts take it.
    pub fn take_pin(&mut self) -> Option<String> {
        self.pin_for_keyring.take()
    }

    /// The network joined, as the last scan saw it.
    fn joined(&self) -> Option<String> {
        self.nets.iter().find(|r| r.net.joined).map(|r| r.net.ssid.clone())
    }

    /// The networks looked for again, on a thread.
    fn scan_now(&mut self) {
        self.scan_at = hybris_hwc::now_ns();
        let (slot, wake) = (self.scanned.clone(), self.wake.clone());
        std::thread::spawn(move || {
            let nets = scan();
            *slot.lock().unwrap() = Some(nets);
            wake.ping();
        });
    }

    /// `ssid` joined, on a thread: NetworkManager asks for its password
    /// through item's agent if it needs one (the dialog over the setup).
    fn join(&mut self, ssid: &str) {
        tracing::info!("setup: joining a Wi-Fi network");
        self.joining = Some(ssid.to_owned());
        self.join_failed = None;
        self.refresh_rows();
        if self.dry {
            *self.joined_answer.lock().unwrap() = Some((ssid.to_owned(), Err("dry".into())));
            self.wake.ping();
            return;
        }
        let (slot, wake, ssid) = (self.joined_answer.clone(), self.wake.clone(), ssid.to_owned());
        std::thread::spawn(move || {
            let out = std::process::Command::new("nmcli").args(["--wait", "60", "device", "wifi", "connect", &ssid]).output();
            let result = match out {
                Ok(o) if o.status.success() => Ok(()),
                Ok(o) => Err(String::from_utf8_lossy(&o.stderr).trim().to_owned()),
                Err(e) => Err(e.to_string()),
            };
            *slot.lock().unwrap() = Some((ssid, result));
            wake.ping();
        });
    }

    /// The Wi-Fi step's answers: a scan, a join; scanned again now and then.
    fn poll_wifi(&mut self) -> bool {
        let mut changed = false;
        let scanned = self.scanned.lock().unwrap().take();
        if let Some(nets) = scanned {
            let was = self.joined();
            let keep: Vec<Net> = nets;
            self.nets = keep.into_iter().map(|net| NetRow { net }).collect();
            self.refresh_rows();
            if was.is_none() && self.joined().is_some() && self.joining.is_none() {
                self.relabel_button();
            }
            changed = true;
        }
        let answer = self.joined_answer.lock().unwrap().take();
        if let Some((ssid, result)) = answer {
            self.joining = None;
            match result {
                Ok(()) => {
                    tracing::info!("setup: Wi-Fi joined");
                    for r in &mut self.nets {
                        r.net.joined = r.net.ssid == ssid;
                    }
                    // The clock: systemd-timesyncd tries as the network
                    // changes, by itself.
                }
                Err(e) => {
                    tracing::warn!("setup: Wi-Fi not joined: {}", e.lines().next().unwrap_or(""));
                    self.join_failed = Some(ssid);
                }
            }
            self.refresh_rows();
            self.relabel_button();
            changed = true;
        }
        if self.joining.is_none() && hybris_hwc::now_ns() >= self.scan_at + RESCAN_NS {
            self.scan_now();
        }
        changed
    }

    /// The button's word as the network stands (next once joined), the
    /// rows' words: the table's words laid again.
    fn relabel_button(&mut self) {
        self.lay();
    }

    fn refresh_rows(&mut self) {
        self.lay();
    }

    /// Whether the system's dialog may come over the setup: a Wi-Fi
    /// network's password, asked while one is joined.
    pub fn wants_wifi_dialog(&self) -> bool {
        self.holds_screen() && self.step == Step::Wifi
    }

    /// The words a tap at (`x`, `y`) takes, typed: their tag (a button, a
    /// network) - a square round them taking it too.
    fn tapped(&self, x: f64, y: f64) -> Option<String> {
        let now = hybris_hwc::now_ns();
        self.lines.iter().filter(|l| l.tag.is_some() && l.typed(now) > 0).find_map(|l| {
            let r = l.rect();
            let pad = Grid::at(l.zoom).s / 2.0;
            let hit = x >= r.loc.x - pad && x < r.loc.x + r.size.w + pad && y >= r.loc.y - pad && y < r.loc.y + r.size.h + pad;
            hit.then(|| l.tag.clone().unwrap_or_default())
        })
    }

    fn has_pad(&self) -> bool {
        matches!(self.step, Step::PinNew | Step::PinAgain | Step::PinCurrent | Step::Saving)
    }

    pub fn down(&mut self, x: f64, y: f64) {
        if self.has_pad() && self.step != Step::Saving {
            self.pad.down(x, y);
            self.poked = hybris_hwc::now_ns();
            return;
        }
        // The drop touched where it stands still (the welcome, all set):
        // it plays the touch - gives, wobbles back, comes alive a moment -
        // and stays where it is (the owner, 2026-10-30).
        let still = matches!(self.step, Step::Welcome | Step::Done) && self.leaving.is_none();
        if still && (x - self.orb.x).hypot(y - self.orb.y) < self.orb.r * 1.4 {
            let now = hybris_hwc::now_ns();
            self.poked = now;
            self.drop_poked = Some(now);
            crate::fingerprint::buzz("button-pressed");
        }
    }

    /// The drop touched, for the dock's water to play it (once).
    pub fn take_poke(&mut self) -> Option<u64> {
        self.drop_poked.take()
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
        if self.leaving.is_some() {
            return;
        }
        let tag = self.tapped(x, y);
        if let Some(i) = tag.as_deref().and_then(|t| t.strip_prefix("net:")).and_then(|i| i.parse::<usize>().ok()).filter(|_| self.step == Step::Wifi) {
            if let Some(net) = self.nets.get(i).map(|r| r.net.clone()) {
                if !net.joined && self.joining.is_none() {
                    self.join(&net.ssid);
                }
            }
            return;
        }
        if self.step == Step::FingerKnown && tag.as_deref() == Some("two") {
            tracing::info!("setup: another finger");
            self.go(Step::Finger);
            if !self.dry_finger {
                fingerprint.enroll(&format!("finger {}", self.fingers + 1));
            }
            return;
        }
        if tag.as_deref() != Some("go") {
            return;
        }
        match self.step {
            Step::FingerKnown => self.go(Step::Done),
            // A Wi-Fi network known already (item/grid's, at the install):
            // no networks to pick.
            Step::Welcome if *self.wifi_known.lock().unwrap() == Some(true) => {
                tracing::info!("setup: a Wi-Fi network is known; its step left out");
                self.go(Step::Checking);
            }
            Step::Welcome => self.go(Step::Wifi),
            Step::Wifi => {
                tracing::info!("setup: Wi-Fi {}", if self.joined().is_some() { "joined" } else { "skipped" });
                self.go(Step::Checking);
            }
            Step::Finger => {
                // Later: the reader stops.
                tracing::info!("setup: the finger later");
                self.enrolling = false;
                if !self.dry_finger {
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
        // The warm-ups item-session held back while the setup was up.
        let _ = std::process::Command::new("systemctl").args(["--user", "start", "--no-block", "item-warm.service", "item-chromium.service"]).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn();
        // The words erased, the squares become the wallpaper's pixels
        // (table()); the drop falls at once, and the dock is born of it
        // (dock.rs) as the view pulls back (the owner's, 2026-10-30).
        let now = hybris_hwc::now_ns();
        self.going.extend(std::mem::take(&mut self.lines).into_iter().map(|l| (l, now)));
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
            // (The words are erased: what else the setup shows fades.)
            Some(t) => 1.0 - ease(frame_ns.saturating_sub(t) as f64 / WORDS_GO_NS),
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
            if frame_ns >= t + PULL_NS {
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
        if matches!(self.step, Step::Finger | Step::FingerKnown) {
            let since = frame_ns as f64 - (self.moved_at as f64 + self.move_ns * 0.7);
            self.orb.print = ease(since / PRINT_IN_NS);
            if let Some(t) = self.poor_at {
                self.orb.x += crate::pinpad::shake(frame_ns.saturating_sub(t));
            }
        }
        // The words erased go; the table's lamp breathes all along.
        self.going.retain(|(l, t)| frame_ns < t + l.letters() as u64 * squares::ERASE_NS);
        true
    }

    pub fn warm_up(&self, renderer: &mut GlesRenderer) -> usize {
        self.arrow
            .iter()
            .chain(self.pad.buffers())
            .filter(|b| MemoryRenderBufferRenderElement::from_buffer(renderer, (0.0, 0.0), b, None, None, None, Kind::Unspecified).is_ok())
            .count()
    }

    /// The setup's words on its table (the output draws the table and the
    /// drop: table.frag, the dock's water), going first at the end.
    pub fn elements(&self, renderer: &mut GlesRenderer, frame_ns: u64) -> Vec<ShellElement> {
        if !self.active {
            return Vec::new();
        }
        let fade = self.shown(frame_ns);
        let right = layout::panels()[1];
        let mut out = if self.has_pad() { self.pad.elements(renderer, frame_ns, 0.0) } else { Vec::new() };
        // (The words are erased at the end, not faded: only the arrow fades.)
        let mut put = |out: &mut Vec<ShellElement>, b: &MemoryRenderBuffer, x: f64, y: f64, alpha: f64, src: Option<Rectangle<f64, smithay::utils::Logical>>| {
            if alpha <= 0.001 {
                return;
            }
            let size = src.map(|s| s.size.to_i32_round());
            if let Ok(e) = MemoryRenderBufferRenderElement::from_buffer(renderer, ((x * SCALE as f64).round(), (y * SCALE as f64).round()), b, Some(alpha as f32), src, size, Kind::Unspecified) {
                out.push(ShellElement::Text(e));
            }
        };
        // A line as far as `n` of its letters, at the zoom it is laid for
        // (none while the squares move).
        let zoom = self.zoom_now(frame_ns);
        let mut line = |out: &mut Vec<ShellElement>, l: &Line, n: usize| {
            if n == 0 || (l.zoom - zoom).abs() > 1e-6 {
                return;
            }
            let r = l.rect();
            let side = r.size.h;
            let src = Rectangle::<f64, smithay::utils::Logical>::from_size((side * n as f64, side).into());
            put(out, &l.buffer, r.loc.x, r.loc.y, 1.0, Some(src));
        };
        for (l, t) in &self.going {
            let gone = (frame_ns.saturating_sub(*t) / squares::ERASE_NS) as usize;
            line(&mut out, l, l.letters().saturating_sub(gone));
        }
        for l in &self.lines {
            line(&mut out, l, l.typed(frame_ns));
        }
        // A finger: an arrow bobbing at the reader.
        if let (Some(arrow), true, Step::Finger) = (&self.arrow, self.enrolling, self.step) {
            let a = ease(frame_ns.saturating_sub(self.right_at()) as f64 / WORDS_IN_NS as f64);
            let bob = 8.0 * (frame_ns as f64 / 1e9 * std::f64::consts::TAU / 1.2).sin().abs();
            put(&mut out, arrow, (right.loc.x + right.size.w) as f64 - 60.0 + bob, READER_Y - 16.0, a * fade, None);
        }
        out
    }
}
