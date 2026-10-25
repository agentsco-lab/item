//! item-sync: the black box. What Claude Code is working on here goes to
//! the phone as it changes, so a session can go on there when this machine
//! loses its power (Kyiv, 2026): the transcripts (~/.claude/projects) and
//! the folders being worked in, to the same paths on the phone. On the way
//! back, transcripts the phone wrote on come to this machine (--update: the
//! newer copy wins, nothing is deleted). In Rust since 2026-10-26 (it was a
//! Python script): the same commands and configuration; the folders
//! watched through inotify itself, no inotifywait.
//!
//!   item-sync            run on: a sync at the start, then on each change
//!                        (3 s after the last), at least once a minute; the
//!                        phone's transcripts back every 10 min
//!   item-sync once       one sync now
//!   item-sync setup      the user unit written and started (systemd --user)
//!
//! ~/.config/item/sync, a line each:
//!   to USER@HOST [USER@HOST...]   where to (else the devices marked notify)
//!   path DIR                      a folder to keep on the phone (the folders
//!                                 of running sessions go in any case)
//!   skip DIR                      a folder never sent
//!   exclude PATTERN               besides the build trees and secrets' files

use std::collections::HashMap;
use std::ffi::CString;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use item_agents::*;
use serde_json::Value;

const USAGE: &str = "item-sync [run | once | setup]";
/// Build trees and caches, and the files secrets usually live in: those
/// never travel. Claude Code's transcripts travel whole.
const EXCLUDES: &[&str] = &[
    "target", "node_modules", "sysroot", "out", ".cache", "__pycache__", "dist", "*.img", "*.img.zst", "*.zst", ".venv", "venv", ".env", ".env.*", "*.pem", "*.key", "*.p12", "*.pfx", "id_rsa*", "id_ed25519*",
    "id_ecdsa*", ".netrc", "*.kdbx", "credentials*.json",
];
const DEBOUNCE: Duration = Duration::from_secs(3);
const AT_LEAST: Duration = Duration::from_secs(60);
const BACK_EVERY: Duration = Duration::from_secs(600);

fn log(msg: &str) {
    println!("{} {msg}", local_time("%H:%M:%S"));
}

fn ssh_command() -> String {
    format!("ssh {}", ssh_args(4).join(" "))
}

struct Config {
    to: Vec<String>,
    paths: Vec<PathBuf>,
    excludes: Vec<String>,
    skips: Vec<PathBuf>,
}

fn config() -> Config {
    let mut c = Config { to: Vec::new(), paths: Vec::new(), excludes: EXCLUDES.iter().map(|s| s.to_string()).collect(), skips: Vec::new() };
    for line in read(config_dir().join("item/sync")).lines() {
        let w: Vec<&str> = line.split_whitespace().collect();
        match w.as_slice() {
            [] => {}
            [first, ..] if first.starts_with('#') => {}
            ["to", hosts @ ..] => c.to.extend(hosts.iter().filter(|h| h.contains('@')).map(|h| h.to_string())),
            ["path", dir, ..] => c.paths.push(abspath(&expand_user(dir))),
            ["exclude", pat, ..] => c.excludes.push(pat.to_string()),
            ["skip", dir, ..] => c.skips.push(abspath(&expand_user(dir))),
            _ => {}
        }
    }
    if c.to.is_empty() {
        for d in devices().into_iter().filter(|d| d.notify) {
            c.to.extend(d.hosts);
        }
    }
    c
}

/// item-agents beside this program first (a user unit's PATH has no
/// ~/.local/bin: the sessions were not seen, 2026-10-26), else on the PATH.
fn agents_program() -> PathBuf {
    std::env::current_exe().ok().and_then(|me| me.parent().map(|d| d.join("item-agents"))).filter(|p| p.is_file()).unwrap_or_else(|| PathBuf::from("item-agents"))
}

/// item-agents' sessions here.
fn agents_list() -> Vec<Value> {
    let mut c = Command::new(agents_program());
    c.arg("list");
    output_within(c, Duration::from_secs(10)).and_then(|o| serde_json::from_slice::<Vec<Value>>(&o.stdout).ok()).unwrap_or_default()
}

fn running(s: &Value) -> bool {
    s.get("running").and_then(Value::as_bool).unwrap_or(false)
}

/// The folders of the running sessions - but the ones skipped, and the
/// home itself.
fn session_dirs(skips: &[PathBuf]) -> Vec<PathBuf> {
    let home = home();
    agents_list()
        .iter()
        .filter(|s| running(s))
        .filter_map(|s| s.get("cwd").and_then(Value::as_str).filter(|c| !c.is_empty()).map(expand_user))
        .filter(|d| *d != home && !skips.iter().any(|s| d == s || d.starts_with(s)))
        .collect()
}

/// The ids of the sessions running here: their transcripts are being
/// written, a copy from the phone must not land over them.
fn running_ids() -> Vec<String> {
    agents_list().iter().filter(|s| running(s)).filter_map(|s| s.get("id").and_then(Value::as_str).filter(|i| !i.is_empty()).map(str::to_owned)).collect()
}

/// The phone: the first address that answers, kept until it fails.
struct Target {
    hosts: Vec<String>,
    host: Option<String>,
}

impl Target {
    fn pick(&mut self) -> Option<String> {
        if self.host.is_none() {
            self.host = self.hosts.iter().find(|h| {
                let mut c = Command::new("ssh");
                c.args(ssh_args(4)).arg(h.as_str()).arg("true");
                output_within(c, Duration::from_secs(12)).is_some_and(|o| o.status.success())
            }).cloned();
        }
        self.host.clone()
    }

    fn failed(&mut self) {
        self.host = None;
    }
}

fn rsync(src: &str, dst: &str, excludes: &[String], extra: &[String]) -> Result<(), String> {
    let mut c = Command::new("rsync");
    c.args(["-a", "--update", "--timeout=60", "-e", &ssh_command()]);
    c.args(excludes.iter().map(|e| format!("--exclude={e}")));
    c.args(extra).arg(src).arg(dst);
    let out = c.stdin(std::process::Stdio::null()).output().map_err(|e| format!("rsync: {e}"))?;
    match out.status.code() {
        Some(0) | Some(24) => Ok(()),
        code => {
            let err = String::from_utf8_lossy(&out.stderr);
            Err(err.trim().lines().last().map(str::to_owned).unwrap_or_else(|| format!("rsync {code:?}")))
        }
    }
}

fn sync_once(target: &mut Target, paths: &[PathBuf], excludes: &[String]) -> bool {
    let Some(host) = target.pick() else {
        log("the phone does not answer");
        return false;
    };
    let t = Instant::now();
    let projects = format!("{}/", home().join(".claude/projects").display());
    let result = (|| {
        // The transcripts, into the phone owner's home (the folder made
        // first: a fresh phone has no ~/.claude, and rsync makes only the
        // last part of a path).
        rsync(&projects, &format!("{host}:.claude/projects/"), &[], &["--rsync-path=mkdir -p .claude/projects && rsync".to_owned()])?;
        // The folders, to the same absolute paths (made on the phone first).
        for p in paths.iter().filter(|p| p.is_dir()) {
            let p = p.to_string_lossy();
            let p = p.trim_end_matches('/');
            let quoted = serde_json::to_string(p).unwrap_or_default();
            rsync(&format!("{p}/"), &format!("{host}:{p}/"), excludes, &[format!("--rsync-path=mkdir -p {quoted} && rsync")])?;
        }
        Ok::<(), String>(())
    })();
    match result {
        Ok(()) => {
            log(&format!("to {host}: transcripts and {} folder(s) in {:.1} s", paths.len(), t.elapsed().as_secs_f64()));
            true
        }
        Err(e) => {
            log(&format!("{host}: {e}"));
            target.failed();
            false
        }
    }
}

fn sync_back(target: &mut Target) {
    let Some(host) = target.pick() else { return };
    let keep: Vec<String> = running_ids().into_iter().map(|id| format!("{id}.jsonl")).collect();
    let projects = format!("{}/", home().join(".claude/projects").display());
    // (The phone's folder made if it is not there yet: nothing to bring, no
    // error.)
    match rsync(&format!("{host}:.claude/projects/"), &projects, &keep, &["--rsync-path=mkdir -p .claude/projects && rsync".to_owned()]) {
        Ok(()) => log(&format!("from {host}: transcripts back ({} running here left alone)", keep.len())),
        Err(e) => {
            log(&format!("{host}: back: {e}"));
            target.failed();
        }
    }
}

/// The folders watched, recursively, through inotify: a change is a write
/// closed, a file made, removed or moved; the excluded paths (the
/// patterns, and .git's objects) neither watched nor told.
struct Watch {
    fd: i32,
    dirs: HashMap<i32, PathBuf>,
    skip: regex::Regex,
}

const EVENTS: u32 = libc::IN_CLOSE_WRITE | libc::IN_CREATE | libc::IN_DELETE | libc::IN_MOVED_TO | libc::IN_MOVED_FROM;

impl Watch {
    fn new(roots: &[PathBuf], excludes: &[String]) -> Option<Watch> {
        let alts: Vec<String> = excludes.iter().map(|e| regex::escape(e).replace(r"\*", ".*")).chain([r"\.git/objects".to_owned()]).collect();
        let skip = regex::Regex::new(&format!("({})", alts.join("|"))).ok()?;
        // SAFETY: inotify_init1 takes flags.
        let fd = unsafe { libc::inotify_init1(libc::IN_CLOEXEC | libc::IN_NONBLOCK) };
        if fd < 0 {
            return None;
        }
        let mut w = Watch { fd, dirs: HashMap::new(), skip };
        for r in roots {
            w.add_tree(r);
        }
        Some(w)
    }

    fn add_tree(&mut self, dir: &Path) {
        if self.skip.is_match(&dir.to_string_lossy()) {
            return;
        }
        let Ok(c) = CString::new(dir.as_os_str().as_encoded_bytes()) else { return };
        // SAFETY: a valid C string and the inotify fd.
        let wd = unsafe { libc::inotify_add_watch(self.fd, c.as_ptr(), EVENTS | libc::IN_ONLYDIR) };
        if wd < 0 {
            return;
        }
        self.dirs.insert(wd, dir.to_path_buf());
        if let Ok(rd) = std::fs::read_dir(dir) {
            for e in rd.flatten() {
                if e.file_type().is_ok_and(|t| t.is_dir()) {
                    self.add_tree(&e.path());
                }
            }
        }
    }

    /// Whether a change came within `timeout` (new folders watched as they
    /// come); None: the watch is broken.
    fn wait(&mut self, timeout: Duration) -> Option<bool> {
        let mut pfd = libc::pollfd { fd: self.fd, events: libc::POLLIN, revents: 0 };
        // SAFETY: one pollfd.
        let n = unsafe { libc::poll(&mut pfd, 1, timeout.as_millis().min(i32::MAX as u128) as i32) };
        if n < 0 {
            return (std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted).then_some(false);
        }
        if n == 0 {
            return Some(false);
        }
        let mut buf = [0u8; 16384];
        let mut changed = false;
        loop {
            // SAFETY: reading into our buffer.
            let len = unsafe { libc::read(self.fd, buf.as_mut_ptr().cast(), buf.len()) };
            if len <= 0 {
                break;
            }
            let mut at = 0usize;
            let head = std::mem::size_of::<libc::inotify_event>();
            while at + head <= len as usize {
                // SAFETY: the kernel wrote whole events into the buffer.
                let ev: libc::inotify_event = unsafe { std::ptr::read_unaligned(buf.as_ptr().add(at).cast()) };
                let name_bytes = &buf[at + head..at + head + ev.len as usize];
                let name = String::from_utf8_lossy(name_bytes.split(|b| *b == 0).next().unwrap_or(&[])).into_owned();
                at += head + ev.len as usize;
                let Some(dir) = self.dirs.get(&ev.wd).cloned() else { continue };
                let path = dir.join(&name);
                if self.skip.is_match(&path.to_string_lossy()) {
                    continue;
                }
                if ev.mask & libc::IN_ISDIR != 0 && ev.mask & (libc::IN_CREATE | libc::IN_MOVED_TO) != 0 {
                    self.add_tree(&path);
                }
                if ev.mask & EVENTS != 0 {
                    changed = true;
                }
            }
        }
        Some(changed)
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        // SAFETY: our fd.
        unsafe { libc::close(self.fd) };
    }
}

fn sorted_paths(c: &Config) -> Vec<PathBuf> {
    let mut p: Vec<PathBuf> = c.paths.iter().cloned().chain(session_dirs(&c.skips)).collect();
    p.sort();
    p.dedup();
    p
}

fn run_on() -> ! {
    let c = config();
    if c.to.is_empty() {
        eprintln!("nowhere to sync to: `to USER@HOST` in {}, or a device marked notify in {}", config_dir().join("item/sync").display(), devices_file().display());
        std::process::exit(1);
    }
    let mut target = Target { hosts: c.to.clone(), host: None };
    let mut paths = sorted_paths(&c);
    sync_back(&mut target);
    sync_once(&mut target, &paths, &c.excludes);
    let mut last_back = Instant::now();
    loop {
        let dirs: Vec<PathBuf> = std::iter::once(home().join(".claude/projects")).chain(paths.iter().filter(|p| p.is_dir()).cloned()).collect();
        let Some(mut w) = Watch::new(&dirs, &c.excludes) else {
            log("inotify: not available; a sync each minute");
            std::thread::sleep(AT_LEAST);
            sync_once(&mut target, &paths, &c.excludes);
            continue;
        };
        log(&format!("watching {} folders", dirs.len()));
        let mut pending: Option<Instant> = None;
        let mut last_sync = Instant::now();
        // Watched until the set of folders changes (a session in a new one).
        loop {
            let timeout = pending.map(|p| DEBOUNCE.saturating_sub(p.elapsed()).max(Duration::from_millis(100))).unwrap_or(Duration::from_secs(1));
            match w.wait(timeout) {
                None => break,
                Some(true) => {
                    pending = Some(Instant::now());
                    continue;
                }
                Some(false) => {}
            }
            let due = pending.is_some_and(|p| p.elapsed() >= DEBOUNCE) || last_sync.elapsed() >= AT_LEAST;
            if due {
                let new_paths = sorted_paths(&c);
                sync_once(&mut target, &new_paths, &c.excludes);
                last_sync = Instant::now();
                pending = None;
                if new_paths != paths {
                    paths = new_paths;
                    break;
                }
            }
            if last_back.elapsed() >= BACK_EVERY {
                sync_back(&mut target);
                last_back = Instant::now();
            }
        }
    }
}

fn setup() {
    let unit_dir = config_dir().join("systemd/user");
    let _ = std::fs::create_dir_all(&unit_dir);
    let me = std::env::current_exe().map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|_| "item-sync".into());
    let unit = format!(
        "[Unit]\nDescription=item-sync: Claude Code's transcripts and folders to the phone as they change\nAfter=network-online.target\n\n[Service]\nExecStart={me}\nRestart=always\nRestartSec=10\n\n[Install]\nWantedBy=default.target\n"
    );
    if let Err(e) = std::fs::write(unit_dir.join("item-sync.service"), unit) {
        eprintln!("item-sync.service: {e}");
        std::process::exit(1);
    }
    let _ = Command::new("systemctl").args(["--user", "daemon-reload"]).status();
    let _ = Command::new("systemctl").args(["--user", "enable", "--now", "item-sync.service"]).status();
    println!("item-sync.service: on (journalctl --user -u item-sync -f)");
}

fn main() {
    let arg = std::env::args().nth(1).unwrap_or_else(|| "run".into());
    match arg.as_str() {
        "run" => run_on(),
        "once" => {
            let c = config();
            let mut target = Target { hosts: c.to.clone(), host: None };
            sync_once(&mut target, &sorted_paths(&c), &c.excludes);
        }
        "setup" => setup(),
        _ => {
            eprintln!("{USAGE}");
            std::process::exit(1);
        }
    }
}
