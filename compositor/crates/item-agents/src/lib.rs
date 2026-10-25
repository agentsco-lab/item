//! What item-agents and item-sync share: the paths, the devices file, the
//! ssh options, reading files that may be missing, the local time, and the
//! shell's quoting (Python's shlex.quote, which the scripts used).

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

pub fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}

fn xdg(var: &str, fallback: &str) -> PathBuf {
    std::env::var_os(var).filter(|v| !v.is_empty()).map(PathBuf::from).unwrap_or_else(|| home().join(fallback))
}

pub fn config_dir() -> PathBuf {
    xdg("XDG_CONFIG_HOME", ".config")
}

pub fn state_dir() -> PathBuf {
    xdg("XDG_STATE_HOME", ".local/state")
}

/// `~/.config/item/devices`.
pub fn devices_file() -> PathBuf {
    config_dir().join("item/devices")
}

/// A file's text, or nothing.
pub fn read(path: impl AsRef<Path>) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

/// A device of `~/.config/item/devices`: NAME USER@HOST [USER@HOST...] [notify].
#[derive(Clone, Debug, PartialEq)]
pub struct Device {
    pub name: String,
    pub hosts: Vec<String>,
    pub notify: bool,
}

pub fn parse_devices(text: &str) -> Vec<Device> {
    text.lines()
        .filter_map(|line| {
            let w: Vec<&str> = line.split_whitespace().collect();
            if w.is_empty() || w[0].starts_with('#') {
                return None;
            }
            Some(Device { name: w[0].to_owned(), hosts: w[1..].iter().filter(|h| h.contains('@')).map(|h| h.to_string()).collect(), notify: w[1..].contains(&"notify") })
        })
        .collect()
}

pub fn devices() -> Vec<Device> {
    parse_devices(&read(devices_file()))
}

/// The ssh options both use: no prompt, a short wait, a new host's key
/// taken, quiet.
pub fn ssh_args(connect_timeout: u32) -> Vec<String> {
    ["-o", "BatchMode=yes", "-o", &format!("ConnectTimeout={connect_timeout}"), "-o", "StrictHostKeyChecking=accept-new", "-o", "LogLevel=ERROR"].iter().map(|s| s.to_string()).collect()
}

/// A command's output if it ended within `timeout` (killed after), with
/// stdin closed.
pub fn output_within(mut cmd: Command, timeout: Duration) -> Option<std::process::Output> {
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().ok()?;
    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return child.wait_with_output().ok(),
            Ok(None) if start.elapsed() < timeout => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

/// The machine's name (os.uname().nodename).
pub fn nodename() -> String {
    // SAFETY: uname fills the struct it is given.
    unsafe {
        let mut u: libc::utsname = std::mem::zeroed();
        if libc::uname(&mut u) != 0 {
            return String::new();
        }
        std::ffi::CStr::from_ptr(u.nodename.as_ptr()).to_string_lossy().into_owned()
    }
}

pub fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// The local time as strftime's `format` has it.
pub fn local_time(format: &str) -> String {
    // SAFETY: localtime_r and strftime with valid buffers.
    unsafe {
        let t: libc::time_t = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&t, &mut tm);
        let f = std::ffi::CString::new(format).unwrap_or_default();
        let mut buf = [0u8; 64];
        let n = libc::strftime(buf.as_mut_ptr().cast(), buf.len(), f.as_ptr(), &tm);
        String::from_utf8_lossy(&buf[..n]).into_owned()
    }
}

/// Seconds since the epoch of a UTC "YYYY-MM-DDTHH:MM:SS".
pub fn utc_seconds(ts: &str) -> Option<i64> {
    let n = |r: std::ops::Range<usize>| ts.get(r).and_then(|v| v.parse::<i64>().ok());
    let (y, m, d, hh, mm, ss) = (n(0..4)?, n(5..7)?, n(8..10)?, n(11..13)?, n(14..16)?, n(17..19)?);
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) || ts.as_bytes().get(10) != Some(&b'T') {
        return None;
    }
    // Days from the civil date (Howard Hinnant's).
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    Some(days * 86400 + hh * 3600 + mm * 60 + ss)
}

/// A word for a POSIX shell, as Python's shlex.quote makes it.
pub fn shell_quote(s: &str) -> String {
    if s.is_empty() {
        return "''".into();
    }
    if s.chars().all(|c| c.is_ascii_alphanumeric() || "_@%+=:,./-".contains(c)) {
        return s.to_owned();
    }
    format!("'{}'", s.replace('\'', "'\"'\"'"))
}

/// `~` and `~/...` made the home's.
pub fn expand_user(p: &str) -> PathBuf {
    if p == "~" {
        home()
    } else if let Some(rest) = p.strip_prefix("~/") {
        home().join(rest)
    } else {
        PathBuf::from(p)
    }
}

/// A path made absolute against the current directory, `.` and `..` taken
/// out (Python's os.path.abspath).
pub fn abspath(p: &Path) -> PathBuf {
    let joined = if p.is_absolute() { p.to_path_buf() } else { std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/")).join(p) };
    let mut out = PathBuf::from("/");
    for c in joined.components() {
        match c {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::Normal(n) => out.push(n),
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoting_as_shlex() {
        assert_eq!(shell_quote(""), "''");
        assert_eq!(shell_quote("plain-word_1.2/x@y"), "plain-word_1.2/x@y");
        assert_eq!(shell_quote("it's $(x)"), "'it'\"'\"'s $(x)'");
        assert_eq!(shell_quote("a b"), "'a b'");
    }

    #[test]
    fn devices_parsed() {
        let d = parse_devices("# c\nduo droidian@duo.local droidian@100.1.2.3 notify\nlaptop me@pc\n\n");
        assert_eq!(d.len(), 2);
        assert_eq!(d[0], Device { name: "duo".into(), hosts: vec!["droidian@duo.local".into(), "droidian@100.1.2.3".into()], notify: true });
        assert!(!d[1].notify);
    }

    #[test]
    fn utc() {
        assert_eq!(utc_seconds("1970-01-01T00:00:00.000Z"), Some(0));
        assert_eq!(utc_seconds("2026-10-25T13:45:00Z"), Some(1792935900));
        assert_eq!(utc_seconds("nonsense"), None);
    }

    #[test]
    fn paths() {
        assert_eq!(abspath(Path::new("/a/b/../c/./d")), PathBuf::from("/a/c/d"));
        assert_eq!(expand_user("~"), home());
    }
}
