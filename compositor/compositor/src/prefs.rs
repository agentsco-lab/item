//! The keyboard's preferences, as item Settings on the phone writes them
//! (apps/settings/src/keyboard.rs), one small file each in
//! `~/.config/item/`, read again while item runs: the directory's mtime is
//! looked at once a second (Settings writes a file whole and renames it
//! into place, which touches the directory), and the files are read when
//! it changed.
//!
//! - `keyboard-langs`: the languages the keyboard's language key goes
//!   through, "en ru" (default) or "en";
//! - `keyboard-auto`: "on" (default) or "off" - whether the keyboard rises
//!   by itself for a text field (state.rs's text_field);
//! - `haptic`: "on" (default) or "off" - the tick under a finger
//!   (haptic.rs).
//! `keyboard` (item or stevia) is item-switch's and read at the start.

use std::time::SystemTime;

use crate::kbd::Lang;

pub struct Prefs {
    pub langs: Vec<Lang>,
    pub auto: bool,
    pub haptic: bool,
    dir_mtime: Option<SystemTime>,
    looked_at: u64,
}

fn dir() -> String {
    format!("{}/.config/item", std::env::var("HOME").unwrap_or_default())
}

/// The session's private directory ($XDG_RUNTIME_DIR, the user's alone)
/// for what the compositor writes or reads at run time - screenshots,
/// frames, the camera's notes; never /tmp, which every user reads.
pub fn runtime_dir() -> String {
    // SAFETY: getuid takes nothing and cannot fail.
    std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| format!("/run/user/{}", unsafe { libc::getuid() }))
}

/// The file `name` in item's configuration directory, for the pieces
/// that keep a thing of their own there.
pub fn path(name: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(dir()).join(name)
}

fn read(name: &str) -> Option<String> {
    std::fs::read_to_string(format!("{}/{name}", dir())).ok().map(|s| s.trim().to_owned())
}

/// `value` into the file `name`, whole (a temporary renamed over it), as
/// item Settings writes it; the directory's mtime moves, poll() sees it.
pub fn write(name: &str, value: &str) {
    let dir = dir();
    let _ = std::fs::create_dir_all(&dir);
    let tmp = format!("{dir}/.{name}.new");
    if std::fs::write(&tmp, format!("{value}\n")).is_ok() {
        let _ = std::fs::rename(&tmp, format!("{dir}/{name}"));
    }
}

fn on(name: &str) -> bool {
    !read(name).is_some_and(|v| v == "off")
}

/// Whether the shell may reach the network by itself (the system screen's
/// weather from met.no and the port's latest release from GitHub): off
/// unless `~/.config/item/online` says `on` - item sends nothing anywhere
/// unless the owner said so (item Settings' switch).
pub fn online() -> bool {
    read("online").is_some_and(|v| v == "on")
}

/// Whether the frame turns as the sensors say the phone is held
/// (`turn` = "sensors"), or by the dock's turn button alone (the default:
/// the accelerometer and the gyroscope are then not read at all).
pub fn turn_by_sensors() -> bool {
    read("turn").is_some_and(|v| v == "sensors")
}

fn langs() -> Vec<Lang> {
    let mut out: Vec<Lang> = read("keyboard-langs")
        .unwrap_or_else(|| "en ru".into())
        .split_whitespace()
        .filter_map(|w| match w {
            "en" => Some(Lang::En),
            "ru" => Some(Lang::Ru),
            _ => None,
        })
        .collect();
    if out.is_empty() {
        out.push(Lang::En);
    }
    out
}

impl Prefs {
    pub fn load() -> Prefs {
        let p = Prefs { langs: langs(), auto: on("keyboard-auto"), haptic: on("haptic"), dir_mtime: std::fs::metadata(dir()).and_then(|m| m.modified()).ok(), looked_at: 0 };
        tracing::info!("prefs: languages {:?}, keyboard by itself {}, haptic {}", p.langs, p.auto, p.haptic);
        p
    }

    /// Once a second: whether the files changed and were read again.
    pub fn poll(&mut self, now_ns: u64) -> bool {
        if now_ns.saturating_sub(self.looked_at) < 1_000_000_000 {
            return false;
        }
        self.looked_at = now_ns;
        let mtime = std::fs::metadata(dir()).and_then(|m| m.modified()).ok();
        if mtime == self.dir_mtime {
            return false;
        }
        self.dir_mtime = mtime;
        let fresh = Prefs::load();
        let changed = fresh.langs != self.langs || fresh.auto != self.auto || fresh.haptic != self.haptic;
        self.langs = fresh.langs;
        self.auto = fresh.auto;
        self.haptic = fresh.haptic;
        changed
    }
}
