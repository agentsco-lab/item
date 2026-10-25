//! item-agents: Claude Code sessions on this machine and on the others
//! (~/.config/item/devices), as item's system screen shows them, and the
//! ways to reach them (sysfacts.rs, kbd.rs's Agent entry). In Rust since
//! 2026-10-26 (it was a Python script): the same commands, the same JSON,
//! the same hook and its traps.
//!
//!   item-agents list              the sessions here, as JSON
//!   item-agents devices           this machine and the others with theirs, JSON
//!   item-agents hook              from Claude Code's hooks (settings.json): what
//!                                 the session is doing, kept for `list`; the
//!                                 devices marked `notify` told when it waits
//!   item-agents setup             the hooks written into ~/.claude/settings.json
//!   item-agents run [DIR] [--resume ID]
//!                                 claude inside tmux (so a window may close
//!                                 and another, from another machine, attach);
//!                                 with no folder, the warm home session
//!   item-agents warm              the home session started in the background
//!   item-agents open DEVICE [TMUX [ID [CWD]]]
//!                                 a session attached (tmux) or resumed (its
//!                                 id, in its folder), here ("-") or over ssh
//!
//! The hook also keeps the agent from the phone's traps (the port's
//! SAFETY.md, item/grid's rules): a PreToolUse hook refuses a command that
//! reads the GPIO debug file (the phone resets at once), writes a block
//! device or flashes (no EDL loader: a bad write bricks the Duo), stops or
//! restarts the vendor composer, or wipes the root.

use std::collections::{HashMap, HashSet};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use item_agents::*;
use serde_json::{json, Map, Value};

const USAGE: &str = "item-agents list | devices | hook | setup | run [DIR] [--resume ID] | warm | open DEVICE [TMUX [ID [CWD]]]";

fn state_root() -> PathBuf {
    state_dir().join("item/agents")
}

/// Claude Code's folder for a working directory: the path with every
/// character but letters and digits as a dash.
fn project_dir(cwd: &str) -> String {
    cwd.chars().map(|c| if c.is_alphanumeric() { c } else { '-' }).collect()
}

/// "claude-opus-4-5-..." as "Opus 4.5...".
fn model_name(mid: &str) -> String {
    let parts: Vec<&str> = mid.split('-').collect();
    if parts.len() < 2 || parts[0] != "claude" {
        return mid.to_owned();
    }
    let mut c = parts[1].chars();
    let name: String = c.next().map(|f| f.to_uppercase().collect::<String>() + &c.as_str().to_lowercase()).unwrap_or_default();
    let version: Vec<&str> = parts[2..].iter().filter(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit())).copied().collect();
    if version.is_empty() {
        name
    } else {
        format!("{name} {}", version.join("."))
    }
}

/// A transcript's facts: the model, the turns, the tokens, the last line's
/// time.
fn transcript(path: &str) -> Option<Map<String, Value>> {
    if path.is_empty() {
        return None;
    }
    let text = std::fs::read_to_string(path).ok()?;
    let (mut model, mut turns, mut tin, mut tout, mut last) = (String::new(), 0u64, 0u64, 0u64, 0i64);
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        if let Some(ts) = v.get("timestamp").and_then(Value::as_str) {
            if let Some(t) = ts.get(..19).and_then(utc_seconds) {
                last = t;
            }
        }
        if v.get("type").and_then(Value::as_str) != Some("assistant") {
            continue;
        }
        let m = v.get("message").cloned().unwrap_or(Value::Null);
        if let Some(id) = m.get("model").and_then(Value::as_str).filter(|s| !s.is_empty()) {
            model = model_name(id);
        }
        if let Some(u) = m.get("usage").filter(|u| u.is_object()) {
            let n = |k: &str| u.get(k).and_then(Value::as_u64).unwrap_or(0);
            turns += 1;
            tin += n("input_tokens") + n("cache_read_input_tokens") + n("cache_creation_input_tokens");
            tout += n("output_tokens");
        }
    }
    let mut t = Map::new();
    t.insert("model".into(), json!(model));
    t.insert("turns".into(), json!(turns));
    t.insert("tokens_in".into(), json!(tin));
    t.insert("tokens_out".into(), json!(tout));
    t.insert("last".into(), json!(last));
    Some(t)
}

/// tmux's panes: the session name of each pane's shell, by pid.
fn tmux_panes() -> HashMap<i64, String> {
    let mut c = Command::new("tmux");
    c.args(["list-panes", "-a", "-F", "#{session_name} #{pane_pid}"]);
    let Some(out) = output_within(c, Duration::from_secs(3)) else { return HashMap::new() };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| {
            let p: Vec<&str> = l.split_whitespace().collect();
            (p.len() == 2).then(|| p[1].parse::<i64>().ok().map(|pid| (pid, p[0].to_owned()))).flatten()
        })
        .collect()
}

/// The fields of /proc/PID/stat after the command's name.
fn stat_fields(pid: i64) -> Vec<String> {
    let s = read(format!("/proc/{pid}/stat"));
    s.rsplit_once(')').map(|(_, rest)| rest.split_whitespace().map(str::to_owned).collect()).unwrap_or_default()
}

fn parent(pid: i64) -> i64 {
    stat_fields(pid).get(1).and_then(|v| v.parse().ok()).unwrap_or(0)
}

struct Proc {
    cwd: String,
    since: i64,
    tmux: Option<String>,
}

/// Claude Code's processes: their folder, seconds run, the tmux session
/// they are in.
fn processes() -> Vec<Proc> {
    let panes = tmux_panes();
    let uptime: f64 = read("/proc/uptime").split_whitespace().next().and_then(|v| v.parse().ok()).unwrap_or(0.0);
    // SAFETY: sysconf takes a constant.
    let ticks = unsafe { libc::sysconf(libc::_SC_CLK_TCK) }.max(1) as f64;
    let mut out = Vec::new();
    let Ok(dir) = std::fs::read_dir("/proc") else { return out };
    for e in dir.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let Ok(pid) = name.parse::<i64>() else { continue };
        if read(format!("/proc/{pid}/comm")).trim() != "claude" {
            continue;
        }
        let cwd = std::fs::read_link(format!("/proc/{pid}/cwd")).map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
        let started = stat_fields(pid).get(19).and_then(|v| v.parse::<f64>().ok()).map(|t| t / ticks).unwrap_or(uptime);
        // The tmux session: a pane's shell among its ancestors.
        let (mut p, mut tmux) = (pid, None);
        for _ in 0..12 {
            if let Some(s) = panes.get(&p) {
                tmux = Some(s.clone());
                break;
            }
            p = parent(p);
            if p <= 1 {
                break;
            }
        }
        out.push(Proc { cwd, since: (uptime - started).max(0.0) as i64, tmux });
    }
    out
}

/// What the hooks last said of each session, by its id.
fn states() -> Vec<(String, Value)> {
    let mut out = Vec::new();
    let Ok(dir) = std::fs::read_dir(state_root()) else { return out };
    for e in dir.flatten() {
        let n = e.file_name().to_string_lossy().into_owned();
        if let Some(id) = n.strip_suffix(".json") {
            if let Ok(v) = serde_json::from_str::<Value>(&read(e.path())) {
                out.push((id.to_owned(), v));
            }
        }
    }
    out
}

fn s_of<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

fn i_of(v: &Value, k: &str) -> i64 {
    v.get(k).and_then(Value::as_i64).unwrap_or(0)
}

fn mtime(p: &Path) -> Option<f64> {
    std::fs::metadata(p).ok()?.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok().map(|d| d.as_secs_f64())
}

/// A folder's transcripts (`*.jsonl`) by their time, the newest last.
fn transcripts_in(dir: &Path) -> Vec<(f64, String)> {
    let mut files: Vec<(f64, String)> = std::fs::read_dir(dir)
        .map(|d| d.flatten().filter_map(|e| {
            let n = e.file_name().to_string_lossy().into_owned();
            n.ends_with(".jsonl").then(|| mtime(&e.path()).map(|t| (t, n))).flatten()
        }).collect())
        .unwrap_or_default();
    files.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    files
}

/// The sessions here: each a process (or a session the hooks saw end less
/// than a day ago, or a transcript synced from elsewhere), with its state
/// and its transcript's facts.
fn list_sessions() -> Vec<Map<String, Value>> {
    let home = home().to_string_lossy().into_owned();
    let short = |p: &str| if p.starts_with(&home) { format!("~{}", &p[home.len()..]) } else { p.to_owned() };
    let all_states = states();
    let mut by_cwd: HashMap<String, Vec<(String, Value)>> = HashMap::new();
    for (id, st) in &all_states {
        by_cwd.entry(s_of(st, "cwd").to_owned()).or_default().push((id.clone(), st.clone()));
    }
    let mut sessions: Vec<Map<String, Value>> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let merge = |s: &mut Map<String, Value>, t: Option<Map<String, Value>>| {
        if let Some(t) = t {
            s.extend(t);
        }
    };
    for pr in processes() {
        let mut s = Map::new();
        s.insert("cwd".into(), json!(short(&pr.cwd)));
        s.insert("running".into(), json!(true));
        s.insert("since".into(), json!(pr.since));
        s.insert("tmux".into(), json!(pr.tmux));
        s.insert("state".into(), json!("running"));
        s.insert("message".into(), json!(""));
        s.insert("at".into(), json!(0));
        s.insert("id".into(), json!(""));
        // The newest hook state for this folder is this session's.
        let mut cands = by_cwd.get(&pr.cwd).cloned().unwrap_or_default();
        cands.sort_by_key(|(_, st)| std::cmp::Reverse(i_of(st, "at")));
        let mut found = false;
        for (id, st) in cands {
            if seen.insert(id.clone()) {
                s.insert("id".into(), json!(id));
                s.insert("state".into(), json!(st.get("state").and_then(Value::as_str).unwrap_or("running")));
                s.insert("message".into(), json!(s_of(&st, "message")));
                s.insert("at".into(), json!(i_of(&st, "at")));
                merge(&mut s, transcript(s_of(&st, "transcript")));
                found = true;
                break;
            }
        }
        if !found {
            // No hook yet: the newest transcript of the folder.
            let d = home_path(&home).join(".claude/projects").join(project_dir(&pr.cwd));
            if let Some((_, f)) = transcripts_in(&d).pop() {
                s.insert("id".into(), json!(f.trim_end_matches(".jsonl")));
                merge(&mut s, transcript(&d.join(&f).to_string_lossy()));
            }
        }
        sessions.push(s);
    }
    // Sessions the hooks saw that no longer run: kept a day, to resume.
    let now = now();
    for (id, st) in &all_states {
        if seen.contains(id) || s_of(st, "state") == "gone" || now - i_of(st, "at") > 86400 {
            continue;
        }
        let mut s = Map::new();
        s.insert("cwd".into(), json!(short(s_of(st, "cwd"))));
        s.insert("running".into(), json!(false));
        s.insert("since".into(), json!(0));
        s.insert("tmux".into(), Value::Null);
        s.insert("state".into(), json!("stopped"));
        s.insert("message".into(), json!(s_of(st, "message")));
        s.insert("at".into(), json!(i_of(st, "at")));
        s.insert("id".into(), json!(id));
        merge(&mut s, transcript(s_of(st, "transcript")));
        sessions.push(s);
    }
    // Transcripts synced here from another machine (item-sync) and not yet
    // run here: a session to go on with, from its folder, when that
    // machine is dark. Whose it is: the device whose user's home its folder
    // is under (origin), else this machine's.
    let mut homes: Vec<(String, String)> = Vec::new();
    for dev in devices() {
        for h in &dev.hosts {
            let user_home = format!("/home/{}", h.split('@').next().unwrap_or(""));
            if !homes.iter().any(|(k, _)| *k == user_home) {
                homes.push((user_home, dev.name.clone()));
            }
        }
    }
    let known: HashSet<String> = sessions.iter().filter_map(|s| s.get("id").and_then(Value::as_str).map(str::to_owned)).collect();
    let projects = home_path(&home).join(".claude/projects");
    let dirs: Vec<PathBuf> = std::fs::read_dir(&projects).map(|d| d.flatten().map(|e| e.path()).collect()).unwrap_or_default();
    for d in dirs {
        let Some((mtime, f)) = transcripts_in(&d).pop() else { continue };
        let sid = f.trim_end_matches(".jsonl").to_owned();
        if known.contains(&sid) || now as f64 - mtime > 7.0 * 86400.0 {
            continue;
        }
        let cwd = read(d.join(&f))
            .lines()
            .filter_map(|l| serde_json::from_str::<Value>(l).ok())
            .find_map(|v| v.get("cwd").and_then(Value::as_str).filter(|c| !c.is_empty()).map(str::to_owned))
            .unwrap_or_default();
        if cwd.is_empty() {
            continue;
        }
        let mut origin = homes.iter().find(|(h, _)| cwd == *h || cwd.starts_with(&format!("{h}/"))).map(|(_, n)| n.clone()).unwrap_or_default();
        if !origin.is_empty() && cwd.starts_with(&format!("{home}/")) {
            origin.clear();
        }
        let mut s = Map::new();
        s.insert("cwd".into(), json!(short(&cwd)));
        s.insert("running".into(), json!(false));
        s.insert("since".into(), json!(0));
        s.insert("tmux".into(), Value::Null);
        s.insert("state".into(), json!("synced"));
        s.insert("message".into(), json!(""));
        s.insert("at".into(), json!(mtime as i64));
        s.insert("id".into(), json!(sid));
        s.insert("synced".into(), json!(true));
        s.insert("origin".into(), json!(origin));
        merge(&mut s, transcript(&d.join(&f).to_string_lossy()));
        sessions.push(s);
    }
    sessions.sort_by_key(|s| (!s.get("running").and_then(Value::as_bool).unwrap_or(false), std::cmp::Reverse(s.get("at").and_then(Value::as_i64).unwrap_or(0))));
    sessions
}

fn home_path(home: &str) -> PathBuf {
    PathBuf::from(home)
}

/// A device's sessions over ssh, from the first address that answers.
fn remote_list(dev: &Device) -> Option<(String, Value)> {
    for host in &dev.hosts {
        let mut c = Command::new("ssh");
        // Not on a non-interactive shell's PATH when it is the user's own.
        c.args(ssh_args(3)).arg(host).arg("item-agents list 2>/dev/null || ~/.local/bin/item-agents list");
        let Some(out) = output_within(c, Duration::from_secs(8)) else { continue };
        if out.status.success() {
            return Some((host.clone(), serde_json::from_slice(&out.stdout).unwrap_or(json!([]))));
        }
    }
    None
}

/// This machine with its own sessions; each device with its own when it
/// answers, else with the copies synced here of its sessions.
fn cmd_devices() {
    let mine = list_sessions();
    let own: Vec<&Map<String, Value>> = mine.iter().filter(|s| s.get("origin").and_then(Value::as_str).unwrap_or("").is_empty()).collect();
    let mut out = vec![json!({"name": nodename(), "here": true, "online": true, "host": "", "sessions": own})];
    for dev in devices() {
        let (host, sessions) = match remote_list(&dev) {
            Some((h, s)) => (Some(h), s),
            None => (None, json!(mine.iter().filter(|s| s.get("origin").and_then(Value::as_str) == Some(dev.name.as_str())).collect::<Vec<_>>())),
        };
        out.push(json!({"name": dev.name, "here": false, "online": host.is_some(), "host": host.unwrap_or_default(), "sessions": sessions}));
    }
    println!("{}", Value::Array(out));
}

/// The phone's traps: a command matching one is refused, with the reason.
const TRAPS: &[(&str, &str)] = &[
    (r"/sys/kernel/debug/gpio", "reading /sys/kernel/debug/gpio resets the Duo at once (the port's SAFETY.md)"),
    (r"(^|[\s;&|])dd\s[^|;]*of=/dev/", "dd onto a device: the Duo has no EDL loader, a bad write bricks it"),
    (r"(^|[;&|]\s*|\$\(\s*)(sudo\s+)?(mkfs|wipefs|sgdisk|parted|fdisk)\b", "writing a partition table or a filesystem on the Duo: no way back without EDL"),
    (r">\s*/dev/(block|sd|mmcblk)", "writing a block device: the Duo has no EDL loader"),
    (r"fastboot\s+(flash|erase|format|oem)", "flashing from the agent: item/grid flashes, by its rules (RAM boot first, one change per boot)"),
    (r"systemctl\s+(stop|restart|kill)\s+.*(hwcomposer|android-service|lxc@android)", "stopping or restarting the vendor composer resets the phone about one time in three"),
    (r"rm\s+(-[a-zA-Z]*r[a-zA-Z]*\s+)+(/|/\*|/userdata|/home)(\s|$)", "wiping the root or userdata"),
    (r"(^|[;&|]\s*|\$\(\s*)(sudo\s+)?(reboot|poweroff|halt|shutdown)\b", "restarting the phone ends the session: ask the owner"),
];

/// The trap a command falls into, if any.
fn trap(cmd: &str) -> Option<&'static str> {
    TRAPS.iter().find(|(p, _)| regex::Regex::new(p).map(|r| r.is_match(cmd)).unwrap_or(false)).map(|(_, why)| *why)
}

/// PreToolUse: a Bash command into a trap of the phone's is refused (exit
/// 2, the reason to Claude Code).
fn guard(ev: &Value) {
    if s_of(ev, "tool_name") != "Bash" {
        return;
    }
    let cmd = ev.get("tool_input").and_then(|t| t.get("command")).and_then(Value::as_str).unwrap_or("");
    if let Some(why) = trap(cmd) {
        let shown: String = cmd.chars().take(200).collect();
        eprintln!("item-agents: refused: {why}. Command: {shown}");
        std::process::exit(2);
    }
}

/// Claude Code's hook: the event on stdin (session_id, cwd,
/// transcript_path, hook_event_name, message...).
fn cmd_hook() {
    let mut input = String::new();
    let _ = std::io::Read::read_to_string(&mut std::io::stdin(), &mut input);
    let Ok(ev) = serde_json::from_str::<Value>(&input) else { return };
    let sid = s_of(&ev, "session_id");
    if sid.is_empty() {
        return;
    }
    let name = s_of(&ev, "hook_event_name");
    if name == "PreToolUse" {
        guard(&ev);
    }
    let state = match name {
        "SessionStart" => "running",
        "UserPromptSubmit" | "PreToolUse" => "working",
        "Notification" => "waiting",
        "Stop" => "done",
        "SessionEnd" => "gone",
        _ => return,
    };
    // An id is a file's name here: no path in it.
    if sid.contains('/') || sid.starts_with('.') {
        return;
    }
    let root = state_root();
    let _ = std::fs::create_dir_all(&root);
    let path = root.join(format!("{sid}.json"));
    let old: Value = serde_json::from_str(&read(&path)).unwrap_or(json!({}));
    let keep = |k: &str, old_k: &str| ev.get(k).cloned().unwrap_or_else(|| old.get(old_k).cloned().unwrap_or(json!("")));
    let st = json!({
        "state": state,
        "message": if state == "waiting" { ev.get("message").cloned().unwrap_or(json!("")) } else { json!("") },
        "at": now(),
        "cwd": keep("cwd", "cwd"),
        "transcript": keep("transcript_path", "transcript"),
    });
    let tmp = root.join(format!("{sid}.json.new"));
    if std::fs::write(&tmp, st.to_string()).is_ok() {
        let _ = std::fs::rename(&tmp, &path);
    }
    // The devices marked notify told, so the phone calls when the agent
    // waits (their notify-send needs the session bus).
    if state == "waiting" || state == "done" {
        let cwd = s_of(&st, "cwd");
        let folder = Path::new(cwd.trim_end_matches('/')).file_name().map(|f| f.to_string_lossy().into_owned()).filter(|f| !f.is_empty()).unwrap_or_else(|| "~".into());
        let title = format!("{folder}: {}", if state == "waiting" { "Claude Code is waiting" } else { "Claude Code is done" });
        let message = s_of(&st, "message");
        let body = if !message.is_empty() { message } else if state == "waiting" { "" } else { "The turn is yours" };
        let me = nodename();
        for dev in devices().into_iter().filter(|d| d.notify) {
            for host in &dev.hosts {
                // Quoted for the shell: a $(...) or a backtick would
                // otherwise run on the notified machine.
                let cmd = format!(
                    "export DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/$(id -u)/bus; notify-send -a {} -i item-agent {} {}",
                    shell_quote(&format!("Agent on {me}")),
                    shell_quote(&title),
                    shell_quote(body)
                );
                let mut c = Command::new("ssh");
                c.args(ssh_args(3)).arg(host).arg(cmd);
                if output_within(c, Duration::from_secs(6)).is_some_and(|o| o.status.success()) {
                    break;
                }
            }
        }
    }
}

const HOOK_EVENTS: [&str; 6] = ["SessionStart", "UserPromptSubmit", "PreToolUse", "Notification", "Stop", "SessionEnd"];

/// The hooks into ~/.claude/settings.json, the rest of it kept; the file
/// left as it is when they are all there.
fn cmd_setup() {
    let path = home().join(".claude/settings.json");
    let mut s: Value = serde_json::from_str(&read(&path)).unwrap_or(json!({}));
    if !s.is_object() {
        s = json!({});
    }
    let mut added = false;
    {
        let hooks = s.as_object_mut().unwrap().entry("hooks").or_insert(json!({}));
        if !hooks.is_object() {
            *hooks = json!({});
        }
        for ev in HOOK_EVENTS {
            let entries = hooks.as_object_mut().unwrap().entry(ev).or_insert(json!([]));
            if !entries.is_array() {
                *entries = json!([]);
            }
            let has = entries.as_array().unwrap().iter().any(|e| e.get("hooks").and_then(Value::as_array).is_some_and(|hs| hs.iter().any(|h| s_of(h, "command").contains("item-agents hook"))));
            if !has {
                entries.as_array_mut().unwrap().push(json!({"hooks": [{"type": "command", "command": "item-agents hook"}]}));
                added = true;
            }
        }
    }
    if added {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let tmp = path.with_extension("json.new");
        let text = serde_json::to_string_pretty(&s).unwrap_or_default() + "\n";
        if std::fs::write(&tmp, text).and_then(|_| std::fs::rename(&tmp, &path)).is_err() {
            eprintln!("{}: not written", path.display());
            std::process::exit(1);
        }
    }
    println!("{}: item's hooks in", path.display());
}

fn claude_path() -> String {
    let own = home().join(".local/bin/claude");
    if own.exists() {
        own.to_string_lossy().into_owned()
    } else {
        "claude".into()
    }
}

fn tmux_has(name: &str) -> bool {
    Command::new("tmux").args(["has-session", "-t", name]).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status().is_ok_and(|s| s.success())
}

/// Replaced by `program` (exec), or out with why not.
fn exec(program: &str, args: &[String]) -> ! {
    let e = Command::new(program).args(args).exec();
    eprintln!("item-agents: {program}: {e}");
    std::process::exit(1);
}

/// claude inside tmux, attached: a new session named after the folder and
/// the minute, or the session resumed.
fn cmd_run(args: &[String]) -> ! {
    let home = home();
    let mut cwd = home.clone();
    let mut resume: Option<String> = None;
    let mut rest: Vec<String> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--resume" && i + 1 < args.len() {
            resume = Some(args[i + 1].clone());
            i += 2;
        } else if expand_user(&args[i]).is_dir() {
            cwd = abspath(&expand_user(&args[i]));
            i += 1;
        } else {
            rest.push(args[i].clone());
            i += 1;
        }
    }
    // Home, nothing to resume: the warm session, if it is up.
    if cwd == home && resume.is_none() && rest.is_empty() && tmux_has("home") {
        exec("tmux", &["attach".into(), "-t".into(), "home".into()]);
    }
    let name = if cwd == home && resume.is_none() {
        "home".to_owned()
    } else {
        let base = cwd.file_name().map(|f| f.to_string_lossy().into_owned()).filter(|f| !f.is_empty()).unwrap_or_else(|| "home".into());
        format!("{base}-{}", local_time("%H%M"))
    };
    let mut a: Vec<String> = vec!["new-session".into(), "-A".into(), "-s".into(), name, "-c".into(), cwd.to_string_lossy().into_owned(), claude_path()];
    if let Some(r) = resume {
        a.push("--resume".into());
        a.push(r);
    }
    a.extend(rest);
    exec("tmux", &a);
}

/// The home session started detached, unless it is up.
fn cmd_warm() {
    if tmux_has("home") {
        return;
    }
    let _ = Command::new("tmux").args(["new-session", "-d", "-s", "home", "-c"]).arg(home()).arg(claude_path()).status();
    println!("home: warm");
}

fn die(msg: &str) -> ! {
    eprintln!("{msg}");
    std::process::exit(1);
}

/// Attach to a session: `open DEVICE TMUX ID CWD` ("-" for this machine,
/// no tmux session, no id): attached to its tmux session, else resumed from
/// its transcript in its folder, here or over ssh.
fn cmd_open(args: &[String]) -> ! {
    let Some(dev_name) = args.first() else { die("item-agents open DEVICE [TMUX [ID [CWD]]]") };
    let opt = |i: usize| args.get(i).filter(|a| *a != "-").cloned();
    let tmux = opt(1);
    let sid = opt(2);
    let cwd = args.get(3).cloned().unwrap_or_else(|| "~".into());
    let there = if let Some(t) = &tmux {
        format!("tmux attach -t {}", shell_quote(t))
    } else if let Some(id) = &sid {
        format!(
            "item-agents run {c} --resume {i} 2>/dev/null || ~/.local/bin/item-agents run {c} --resume {i}",
            c = shell_quote(&cwd),
            i = shell_quote(id)
        )
    } else {
        die("nothing to open: no tmux session and no id");
    };
    if dev_name == "-" || *dev_name == nodename() {
        if let Some(t) = tmux {
            exec("tmux", &["attach".into(), "-t".into(), t]);
        }
        cmd_run(&[cwd, "--resume".into(), sid.unwrap_or_default()]);
    }
    for dev in devices().into_iter().filter(|d| d.name == *dev_name) {
        for host in &dev.hosts {
            let mut c = Command::new("ssh");
            c.args(ssh_args(3)).arg(host).arg("true");
            if output_within(c, Duration::from_secs(8)).is_some_and(|o| o.status.success()) {
                exec("ssh", &["-t".into(), "-o".into(), "StrictHostKeyChecking=accept-new".into(), "-o".into(), "LogLevel=ERROR".into(), host.clone(), there.clone()]);
            }
        }
        die(&format!("{dev_name}: not reachable"));
    }
    die(&format!("{dev_name}: not in {}", devices_file().display()));
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str).unwrap_or("list") {
        "list" => println!("{}", Value::Array(list_sessions().into_iter().map(Value::Object).collect())),
        "devices" => cmd_devices(),
        "hook" => cmd_hook(),
        "setup" => cmd_setup(),
        "run" => cmd_run(&args[1..]),
        "warm" => cmd_warm(),
        "open" => cmd_open(&args[1..]),
        _ => die(USAGE),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(project_dir("/home/me/a.b_c"), "-home-me-a-b-c");
        assert_eq!(model_name("claude-opus-4-5-20251101"), "Opus 4.5.20251101");
        assert_eq!(model_name("claude-fable-5-1"), "Fable 5.1");
        assert_eq!(model_name("gpt-4"), "gpt-4");
    }

    #[test]
    fn traps() {
        for (cmd, trapped) in [
            ("cat /sys/kernel/debug/gpio", true),
            ("sudo dd if=x.img of=/dev/sda bs=4M", true),
            ("dd if=/dev/zero of=/tmp/x", false),
            ("mkfs.ext4 /dev/sda5", true),
            ("echo mkfs is a word", false),
            ("ls; sudo wipefs -a /dev/sdb", true),
            ("echo x > /dev/block/sda1", true),
            ("echo x > /dev/null", false),
            ("fastboot flash boot_a boot.img", true),
            ("fastboot devices", false),
            ("sudo systemctl restart android-service@hwcomposer", true),
            ("systemctl restart item", false),
            ("rm -rf /", true),
            ("rm -rf /home", true),
            ("rm -rf /home/me/tmp", false),
            ("sudo reboot", true),
            ("cd x && poweroff", true),
            ("echo $(shutdown -h now)", true),
            ("grep -n reboot file.rs", false),
            ("itemgrid reboot", false),
        ] {
            assert_eq!(trap(cmd).is_some(), trapped, "{cmd}");
        }
    }
}
