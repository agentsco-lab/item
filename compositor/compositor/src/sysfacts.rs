//! The system screen's page, read and drawn on a thread of its own
//! (sysscreen.rs shows it, on frosted glass): rows, not cards (#109, as
//! sfduo-system-screen had them, thinned out 2026-10-03).
//!
//! - **The head**: the greeting and the date.
//! - **Battery** (#115): the level, and what it is doing (UPower's
//!   estimate).
//! - **Next up** (#118): Clocks' next alarm and how long until it rings; the
//!   calendar's next event (phosh's calendar server, which reads Evolution's
//!   calendars: a thread keeps the week's events as it tells them).
//! - **Weather** (#120): for the first city chosen in GNOME Weather, from
//!   met.no: now, the high and low of the next 24 hours, the next hours in
//!   one quiet line; asked when the last answer is over half an hour old.
//! - **Connections** (#117): the Wi-Fi network, its signal, band and speed
//!   (nmcli); the operator, the technology, the signal and whether data is
//!   on (mmcli); today's data, Wi-Fi and mobile apart - the interfaces count
//!   from boot, so a ledger keeps the counts the day started with
//!   (`~/.local/state/item/traffic`), brought up to date with the samples
//!   and written once a minute.
//! - **Storage**: free on the system image and /userdata, red under 1 GB.
//! - **Update**: only when a newer port is out (GitHub, asked at most once
//!   a day while the page is open, the answer kept in
//!   `~/.local/state/item/update`).
//! - **Details**, shut until tapped (#116, #119): the power and the
//!   battery's temperature, the clusters' clocks and the hottest core
//!   (sampled every 10 s while the screen is lit), memory, the hinge's
//!   angle (org.sfduo.Posture), the uptime, the port's version, Droidian
//!   and the kernel.
//!
//! Each read draws the whole page into one picture, taller than the panel
//! (it scrolls), handed to the loop to show with where Details' row is.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use resvg::tiny_skia;
use smithay::reexports::calloop::ping::Ping;

use crate::layout::{self, SCALE};
use crate::shade::date_line;
use crate::text::Font;

const LOAD_EVERY: Duration = Duration::from_secs(10);
const LOAD_SPAN_S: f64 = 600.0;
const WEATHER_EVERY: Duration = Duration::from_secs(1800);
const UPDATE_EVERY_S: u64 = 86_400;
const RELEASES: &str = "https://api.github.com/repos/agentsco-lab/surfaceduo-droidian/releases/latest";
const WEATHER_HOURS: [i64; 6] = [0, 2, 4, 6, 8, 10];

/// What the page is asked for.
pub enum Job {
    /// Read everything and draw the page.
    Read,
    /// The screen lit or dark: load samples or none.
    Display(bool),
    /// Details opened or shut: read and drawn so.
    Details(bool),
}

/// A drawn page: premultiplied RGBA, its size in physical px, and where
/// Details' row is (logical y, top and bottom).
pub type Page = (Vec<u8>, i32, i32, (f32, f32));

const INK: [f32; 4] = [0.95, 0.95, 0.96, 1.0];
const DIM: [f32; 4] = [0.66, 0.68, 0.72, 1.0];
const RED: [f32; 4] = [0.96, 0.45, 0.42, 1.0];

fn run(cmd: &str, args: &[&str]) -> String {
    std::process::Command::new(cmd).args(args).output().map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default()
}

/// A page from the web (the phone has wget, not curl), or nothing.
fn fetch(url: &str, accept: &str) -> String {
    run("wget", &["-q", "-T", "10", "-O", "-", "-U", "item-shell/0.1 github.com/agentsco-lab/item", "--header", &format!("Accept: {accept}"), url])
}

fn read(path: &str) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

fn state_dir() -> PathBuf {
    std::env::var("XDG_STATE_HOME").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".local/state")).join("item")
}

fn now_s() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Bytes as a person reads them: 1.7 GB, 72 GB, 640 MB, 12 kB.
fn size(n: f64) -> String {
    let gb = n / 1e9;
    if gb >= 10.0 {
        format!("{gb:.0} GB")
    } else if gb >= 1.0 {
        format!("{gb:.1} GB")
    } else if n >= 1e6 {
        format!("{:.0} MB", n / 1e6)
    } else {
        format!("{:.0} kB", n / 1e3)
    }
}

/// The CPU's use, the little cores and the big ones apart.
struct Sampler {
    clusters: Vec<(String, Vec<usize>)>,
    last: Option<Vec<(u64, u64)>>,
    /// (when, little 0..1, big 0..1).
    samples: Vec<(Instant, f64, f64)>,
}

impl Sampler {
    fn new() -> Sampler {
        let mut clusters: Vec<(String, Vec<usize>)> = std::fs::read_dir("/sys/devices/system/cpu/cpufreq")
            .map(|d| d.flatten().filter_map(|e| {
                let name = e.file_name().into_string().ok()?;
                let cpus: Vec<usize> = read(&format!("/sys/devices/system/cpu/cpufreq/{name}/related_cpus")).split_whitespace().filter_map(|c| c.parse().ok()).collect();
                (!cpus.is_empty()).then_some((name, cpus))
            }).collect())
            .unwrap_or_default();
        clusters.sort_by_key(|c| c.1[0]);
        Sampler { clusters, last: None, samples: Vec::new() }
    }

    fn stat() -> Vec<(u64, u64)> {
        let mut out = Vec::new();
        for line in read("/proc/stat").lines() {
            let Some(rest) = line.strip_prefix("cpu") else { continue };
            if !rest.starts_with(|c: char| c.is_ascii_digit()) {
                continue;
            }
            let mut f = rest.split_whitespace();
            let i: usize = f.next().and_then(|n| n.parse().ok()).unwrap_or(0);
            let v: Vec<u64> = f.filter_map(|n| n.parse().ok()).collect();
            let idle = v.get(3).copied().unwrap_or(0) + v.get(4).copied().unwrap_or(0);
            let total: u64 = v.iter().sum();
            if out.len() <= i {
                out.resize(i + 1, (0, 0));
            }
            out[i] = (total - idle, total);
        }
        out
    }

    fn sample(&mut self) {
        let now = Self::stat();
        if let (Some(last), false) = (&self.last, self.clusters.is_empty()) {
            let use_of = |cpus: &[usize]| {
                let (mut busy, mut total) = (0u64, 0u64);
                for &c in cpus {
                    if let (Some(a), Some(b)) = (last.get(c), now.get(c)) {
                        busy += b.0.saturating_sub(a.0);
                        total += b.1.saturating_sub(a.1);
                    }
                }
                if total > 0 { busy as f64 / total as f64 } else { 0.0 }
            };
            let little = use_of(&self.clusters[0].1);
            let big: Vec<usize> = self.clusters[1..].iter().flat_map(|c| c.1.clone()).collect();
            let t = Instant::now();
            self.samples.push((t, little, use_of(&big)));
            self.samples.retain(|s| t.duration_since(s.0).as_secs_f64() <= LOAD_SPAN_S);
        }
        self.last = Some(now);
    }
}

/// Today's data, Wi-Fi and mobile apart, carried over restarts.
struct Ledger {
    day: String,
    boot: String,
    base: [u64; 2],
    carried: [u64; 2],
    last: [u64; 2],
}

impl Ledger {
    fn path() -> PathBuf {
        state_dir().join("traffic")
    }

    /// Bytes in and out since boot: Wi-Fi, mobile (rmnet_data*: rmnet_ipa0
    /// under them would count it twice).
    fn counters() -> [u64; 2] {
        let mut out = [0u64; 2];
        for line in read("/proc/net/dev").lines().skip(2) {
            let Some((name, rest)) = line.split_once(':') else { continue };
            let f: Vec<u64> = rest.split_whitespace().filter_map(|v| v.parse().ok()).collect();
            if f.len() < 9 {
                continue;
            }
            let both = f[0] + f[8];
            let name = name.trim();
            if name.starts_with("wlan") {
                out[0] += both;
            } else if name.starts_with("rmnet_data") {
                out[1] += both;
            }
        }
        out
    }

    fn load() -> Option<Ledger> {
        let s = read(Ledger::path().to_str()?);
        let w: Vec<&str> = s.split_whitespace().collect();
        let n = |i: usize| w.get(i).and_then(|v| v.parse().ok());
        Some(Ledger { day: w.first()?.to_string(), boot: w.get(1)?.to_string(), base: [n(2)?, n(3)?], carried: [n(4)?, n(5)?], last: [n(6)?, n(7)?] })
    }

    fn save(&self) {
        let p = Ledger::path();
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let _ = std::fs::write(&p, format!("{} {} {} {} {} {} {} {}\n", self.day, self.boot, self.base[0], self.base[1], self.carried[0], self.carried[1], self.last[0], self.last[1]));
    }

    /// Up to now: today's Wi-Fi and mobile.
    fn tally(this: &mut Option<Ledger>, save: bool) -> [u64; 2] {
        let day = { let t = crate::shade::local_time(); format!("{}-{:02}-{:02}", t.tm_year + 1900, t.tm_mon + 1, t.tm_mday) };
        let boot = read("/proc/sys/kernel/random/boot_id").trim().to_owned();
        let now = Self::counters();
        if this.is_none() {
            *this = Ledger::load();
        }
        let led = match this.take() {
            // A new day starts from what the counters say now.
            Some(l) if l.day == day => l,
            _ => Ledger { day, boot: boot.clone(), base: now, carried: [0, 0], last: now },
        };
        let mut led = led;
        if led.boot != boot {
            // Restarted today: what the old boot counted since the base is kept.
            for i in 0..2 {
                led.carried[i] += led.last[i].saturating_sub(led.base[i]);
            }
            led.boot = boot;
            led.base = [0, 0];
        }
        led.last = now;
        if save {
            led.save();
        }
        let today = [0, 1].map(|i| led.carried[i] + now[i].saturating_sub(led.base[i]));
        *this = Some(led);
        today
    }
}

/// What the page shows.
#[derive(Default)]
struct Facts {
    level: u32,
    status: String,
    power_w: f64,
    temp_c: f64,
    to_empty_s: i64,
    to_full_s: i64,
    name: Option<String>,
    load_line: String,
    memory: Option<(f64, f64, f64)>,
    storage: Vec<(&'static str, f64, f64)>,
    wifi: String,
    mobile: String,
    today: [u64; 2],
    alarm: String,
    event: String,
    hinge: String,
    uptime: String,
    port: String,
    system: String,
    weather: Option<Weather>,
    weather_note: String,
}

#[derive(Clone)]
struct Weather {
    city: String,
    now: (f64, String, String),
    high_low: Option<(f64, f64)>,
    /// (when, temperature, icon) for the strip.
    hours: Vec<(i64, f64, String)>,
}

/// The battery, from the kernel and UPower.
fn battery(f: &mut Facts) {
    let file = |n: &str| read(&format!("/sys/class/power_supply/battery/{n}")).trim().to_owned();
    let num = |n: &str| file(n).parse::<f64>().unwrap_or(0.0);
    let prop = |name: &str| {
        let out = run("busctl", &["--system", "get-property", "org.freedesktop.UPower", "/org/freedesktop/UPower/devices/DisplayDevice", "org.freedesktop.UPower.Device", name]);
        out.split_whitespace().nth(1).and_then(|v| v.parse::<i64>().ok()).unwrap_or(0)
    };
    f.level = num("capacity") as u32;
    f.status = file("status");
    f.power_w = (num("current_now") * num("voltage_now")).abs() / 1e12;
    f.temp_c = num("temp") / 10.0;
    f.to_empty_s = prop("TimeToEmpty");
    f.to_full_s = prop("TimeToFull");
}

/// The clusters' clocks now and the hottest core.
fn load_line(sampler: &Sampler, temps: &[String]) -> String {
    let names = ["Little", "Big", "Prime"];
    let mut parts: Vec<String> = sampler
        .clusters
        .iter()
        .enumerate()
        .filter_map(|(i, (policy, _))| {
            let khz: f64 = read(&format!("/sys/devices/system/cpu/cpufreq/{policy}/scaling_cur_freq")).trim().parse().ok()?;
            Some(format!("{} {:.1} GHz", names.get(i).copied().unwrap_or(policy), khz / 1e6))
        })
        .collect();
    let hottest = temps.iter().filter_map(|p| read(p).trim().parse::<f64>().ok()).fold(f64::MIN, f64::max);
    if hottest > f64::MIN {
        parts.push(format!("{:.0} °C", hottest / 1000.0));
    }
    parts.join(" · ")
}

fn memory() -> Option<(f64, f64, f64)> {
    let mut info = std::collections::HashMap::new();
    for line in read("/proc/meminfo").lines() {
        if let Some((k, v)) = line.split_once(':') {
            if let Some(n) = v.split_whitespace().next().and_then(|n| n.parse::<f64>().ok()) {
                info.insert(k.to_owned(), n * 1024.0);
            }
        }
    }
    let total = *info.get("MemTotal")?;
    let free = info.get("MemAvailable").copied().unwrap_or(0.0);
    let swap = info.get("SwapTotal").copied().unwrap_or(0.0) - info.get("SwapFree").copied().unwrap_or(0.0);
    Some((total - free, total, swap))
}

fn storage() -> Vec<(&'static str, f64, f64)> {
    [("System", "/"), ("Data", "/userdata")]
        .into_iter()
        .filter_map(|(name, path)| {
            let c = std::ffi::CString::new(path).ok()?;
            let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
            if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
                return None;
            }
            let total = st.f_blocks as f64 * st.f_frsize as f64;
            let free = st.f_bavail as f64 * st.f_frsize as f64;
            (total > 0.0).then_some((name, free, total))
        })
        .collect()
}

/// nmcli's terse fields, its escaped colons kept.
fn terse(line: &str) -> Vec<String> {
    let mut out = vec![String::new()];
    let mut esc = false;
    for c in line.chars() {
        match (esc, c) {
            (true, c) => {
                out.last_mut().unwrap().push(c);
                esc = false;
            }
            (false, '\\') => esc = true,
            (false, ':') => out.push(String::new()),
            (false, c) => out.last_mut().unwrap().push(c),
        }
    }
    out
}

fn network(f: &mut Facts) {
    let devices = run("nmcli", &["-t", "-f", "TYPE,STATE", "dev"]);
    let state = |kind: &str| devices.lines().map(terse).find(|t| t.first().map(String::as_str) == Some(kind)).and_then(|t| t.get(1).cloned()).unwrap_or_default();
    let wifi = state("wifi");
    f.wifi = if wifi.starts_with("connected") {
        let list = run("nmcli", &["-t", "-f", "ACTIVE,SSID,SIGNAL,FREQ,RATE", "dev", "wifi", "list", "--rescan", "no"]);
        match list.lines().map(terse).find(|t| t.first().map(String::as_str) == Some("yes")) {
            Some(t) => {
                let ssid = t.get(1).filter(|s| !s.is_empty()).cloned().unwrap_or_else(|| "Hidden network".into());
                let mhz: f64 = t.get(3).and_then(|v| v.split_whitespace().next()?.parse().ok()).unwrap_or(0.0);
                let mut parts = vec![ssid, format!("{}%", t.get(2).cloned().unwrap_or_default()), if mhz >= 4900.0 { "5 GHz".into() } else { "2.4 GHz".into() }];
                if let Some(rate) = t.get(4).and_then(|v| v.split_whitespace().next()?.parse::<u32>().ok()).filter(|r| *r > 0) {
                    parts.push(format!("{rate} Mb/s"));
                }
                parts.join(" · ")
            }
            None => "Not connected".into(),
        }
    } else if wifi.starts_with("unavailable") || run("nmcli", &["radio", "wifi"]).trim() == "disabled" {
        "Off".into()
    } else {
        "Not connected".into()
    };
    let modem = run("mmcli", &["-m", "any", "-K"]);
    let key = |k: &str| modem.lines().find(|l| l.split(':').next().is_some_and(|n| n.trim() == k)).and_then(|l| l.split_once(':')).map(|(_, v)| v.trim().to_owned()).filter(|v| v != "--");
    f.mobile = match key("modem.generic.state").as_deref() {
        None => "No modem".into(),
        Some("registered" | "connected" | "connecting" | "disconnecting") => {
            let tech = modem.lines().filter(|l| l.contains("access-technologies.value")).filter_map(|l| l.split_once(':').map(|(_, v)| v.trim().to_owned())).collect::<Vec<_>>();
            let best = ["5gnr", "lte", "umts", "hspa", "edge", "gprs", "gsm"].iter().find(|t| tech.iter().any(|x| x.contains(*t))).map(|t| match *t {
                "5gnr" => "5G",
                "lte" => "LTE",
                "umts" | "hspa" => "3G",
                _ => "2G",
            });
            let mut parts = vec![key("modem.3gpp.operator-name").unwrap_or_else(|| "Mobile network".into())];
            parts.extend(best.map(str::to_owned));
            parts.push(format!("{}%", key("modem.generic.signal-quality.value").unwrap_or_else(|| "0".into())));
            let gsm = state("gsm");
            parts.push(if gsm.starts_with("connected") { "data on".into() } else if gsm.starts_with("connecting") { "data connecting".into() } else { "data off".into() });
            parts.join(" · ")
        }
        Some(_) => "No service".into(),
    };
}

/// Clocks' next alarm: its gsettings value is a list of dicts (hour,
/// minute, active, days 0 Monday .. 6 Sunday, name).
/// The week's events by id - what, when it starts and ends (s) - as phosh's
/// calendar server tells them.
type Events = Arc<Mutex<std::collections::HashMap<String, (String, i64, i64)>>>;

const CALENDAR: &str = "mobi.phosh.Shell.CalendarServer";

/// A thread asking phosh's calendar server for the week ahead (again every
/// hour) and keeping what it tells.
fn calendar() -> Events {
    let events: Events = Default::default();
    let e = events.clone();
    let _ = std::thread::Builder::new().name("calendar".into()).spawn(move || {
        let Ok(bus) = zbus::blocking::Connection::session() else { return };
        let rule = zbus::MatchRule::builder().msg_type(zbus::message::Type::Signal).interface(CALENDAR).map(|b| b.build());
        let Ok(rule) = rule else { return };
        let Ok(signals) = zbus::blocking::MessageIterator::for_match_rule(rule, &bus, Some(64)) else { return };
        // The asking, on a thread of its own: the signals come here.
        let asker = bus.clone();
        let _ = std::thread::Builder::new().name("calendar ask".into()).spawn(move || loop {
            let now = now_s() as i64;
            let _ = asker.call_method(Some(CALENDAR), "/mobi/phosh/Shell/CalendarServer", Some(CALENDAR), "SetTimeRange", &(now - 86_400, now + 8 * 86_400, true));
            std::thread::sleep(Duration::from_secs(3600));
        });
        type Event = (String, String, i64, i64, std::collections::HashMap<String, zbus::zvariant::OwnedValue>);
        for msg in signals.flatten() {
            let member = msg.header().member().map(|m| m.to_string()).unwrap_or_default();
            match member.as_str() {
                "EventsAddedOrUpdated" => {
                    if let Ok(list) = msg.body().deserialize::<Vec<Event>>() {
                        let mut map = e.lock().unwrap();
                        for (id, summary, start, end, _) in list {
                            map.insert(id, (summary, start, end));
                        }
                    }
                }
                "EventsRemoved" => {
                    if let Ok(ids) = msg.body().deserialize::<Vec<String>>() {
                        let mut map = e.lock().unwrap();
                        for id in ids {
                            map.remove(&id);
                        }
                    }
                }
                _ => {}
            }
        }
    });
    events
}

/// The next event: under way, or the first to start.
fn next_event(events: &Events, twelve: bool) -> String {
    let now = now_s() as i64;
    let map = events.lock().unwrap();
    let on = map.values().filter(|(_, s, e)| *s <= now && *e > now).min_by_key(|(_, _, e)| *e);
    if let Some((what, _, end)) = on {
        return format!("{what} · now, till {}", clock_text(*end, twelve));
    }
    match map.values().filter(|(_, s, _)| *s > now).min_by_key(|(_, s, _)| *s) {
        Some((what, start, _)) => format!("{what} · {} {} · {}", clock_text(*start, twelve), day_text(*start), until_text(*start)),
        None => "Nothing this week".into(),
    }
}

fn next_alarm(twelve: bool) -> String {
    match next_alarm_at() {
        None => "No alarm set".into(),
        Some((at, name)) => {
            let text = format!("{} {} · {}", clock_text(at, twelve), day_text(at), until_text(at));
            if !name.is_empty() && name != "Alarm" { format!("{name} · {text}") } else { text }
        }
    }
}

/// When Clocks' next alarm rings (s since the epoch), and its name.
pub fn next_alarm_at() -> Option<(i64, String)> {
    let text = run("gsettings", &["get", "org.gnome.clocks", "alarms"]);
    let num = |d: &str, k: &str| -> Option<i64> {
        let i = d.find(&format!("'{k}': <"))? + k.len() + 5;
        d[i..].split(|c: char| !c.is_ascii_digit() && c != '-').find(|s| !s.is_empty())?.parse().ok()
    };
    let mut best: Option<(i64, String)> = None;
    let now = now_s() as i64;
    for d in text.split('{').skip(1) {
        if d.contains("'active': <false>") {
            continue;
        }
        let (Some(hour), Some(minute)) = (num(d, "hour"), num(d, "minute")) else { continue };
        let days: Vec<i64> = d.find("'days': <[").map(|i| d[i + 10..].split(']').next().unwrap_or("").split(',').filter_map(|v| v.trim().parse().ok()).collect()).unwrap_or_default();
        let name = d.find("'name': <'").map(|i| d[i + 10..].split('\'').next().unwrap_or("").to_owned()).unwrap_or_default();
        for ahead in 0..8i64 {
            let t = now + ahead * 86_400;
            let mut tm: libc::tm = unsafe { std::mem::zeroed() };
            unsafe { libc::localtime_r(&(t as libc::time_t), &mut tm) };
            tm.tm_hour = hour as i32;
            tm.tm_min = minute as i32;
            tm.tm_sec = 0;
            tm.tm_isdst = -1;
            let at = unsafe { libc::mktime(&mut tm) } as i64;
            // tm_wday: 0 Sunday; Clocks' days: 0 Monday.
            let wday = (tm.tm_wday as i64 + 6) % 7;
            if at <= now || (!days.is_empty() && !days.contains(&wday)) {
                continue;
            }
            if best.as_ref().is_none_or(|b| at < b.0) {
                best = Some((at, name.clone()));
            }
            break;
        }
    }
    best
}

fn local(t: i64) -> libc::tm {
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&(t as libc::time_t), &mut tm) };
    tm
}

fn clock_text(t: i64, twelve: bool) -> String {
    let tm = local(t);
    if twelve {
        let h = if tm.tm_hour % 12 == 0 { 12 } else { tm.tm_hour % 12 };
        format!("{h}:{:02} {}", tm.tm_min, if tm.tm_hour < 12 { "AM" } else { "PM" })
    } else {
        format!("{}:{:02}", tm.tm_hour, tm.tm_min)
    }
}

/// today, tomorrow, or the weekday.
fn day_text(t: i64) -> String {
    let (a, b) = (local(now_s() as i64), local(t));
    let day = |tm: &libc::tm| (tm.tm_year as i64) * 400 + tm.tm_yday as i64;
    match day(&b) - day(&a) {
        0 => "today".into(),
        1 => "tomorrow".into(),
        _ => ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"][b.tm_wday as usize].into(),
    }
}

fn until_text(t: i64) -> String {
    let left = t - now_s() as i64;
    if left < 60 {
        "now".into()
    } else if left < 3600 {
        format!("in {} min", left / 60)
    } else if left < 86_400 {
        let (h, m) = (left / 3600, (left / 60) % 60);
        if m > 0 { format!("in {h} h {m} min") } else { format!("in {h} h") }
    } else {
        let d = (left as f64 / 86_400.0).round() as i64;
        format!("in {d} day{}", if d > 1 { "s" } else { "" })
    }
}

fn uptime_text(s: f64) -> String {
    let minutes = (s / 60.0) as i64;
    let (days, minutes) = (minutes / 1440, minutes % 1440);
    let (hours, minutes) = (minutes / 60, minutes % 60);
    if days > 0 {
        format!("{days} day{} {hours} h", if days > 1 { "s" } else { "" })
    } else if hours > 0 {
        format!("{hours} h {minutes} min")
    } else {
        format!("{minutes} min")
    }
}

fn version_parts(v: &str) -> Vec<u32> {
    v.trim_start_matches('v').split('.').filter_map(|p| p.parse().ok()).collect()
}

/// The port's version, and GitHub's latest when it is due (once a day).
fn port(asked: &mut bool) -> String {
    let version = read("/usr/share/sfduo/version").trim().to_owned();
    let path = state_dir().join("update");
    let kept = read(path.to_str().unwrap_or(""));
    let mut lines = kept.lines();
    let checked: u64 = lines.next().and_then(|l| l.parse().ok()).unwrap_or(0);
    let mut latest = lines.next().unwrap_or("").to_owned();
    if !*asked && now_s().saturating_sub(checked) > UPDATE_EVERY_S {
        *asked = true;
        let reply = fetch(RELEASES, "application/vnd.github+json");
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&reply) {
            if let Some(tag) = v.get("tag_name").and_then(|t| t.as_str()) {
                latest = tag.to_owned();
                if let Some(d) = path.parent() {
                    let _ = std::fs::create_dir_all(d);
                }
                let _ = std::fs::write(&path, format!("{}\n{latest}\n", now_s()));
            }
        }
    }
    if version.is_empty() {
        return "?".into();
    }
    if !latest.is_empty() && version_parts(&latest) > version_parts(&version) {
        format!("{version} · {} is out", latest.trim_start_matches('v'))
    } else if !latest.is_empty() {
        format!("{version} · up to date")
    } else {
        version
    }
}

fn system_line() -> String {
    let os = read("/etc/os-release");
    let field = |k: &str| os.lines().find_map(|l| l.strip_prefix(&format!("{k}="))).map(|v| v.trim_matches('"').to_owned());
    let name = field("NAME").unwrap_or_else(|| "Linux".into());
    let number = field("VERSION_ID").or_else(|| field("VERSION")).unwrap_or_default();
    let kernel = read("/proc/sys/kernel/osrelease").trim().split("-microsoft").next().unwrap_or("").to_owned();
    format!("{name} {number} · kernel {kernel}").replace("  ", " ")
}

fn hinge(bus: &Option<zbus::blocking::Connection>) -> String {
    let Some(bus) = bus else { return String::new() };
    let get = |p: &str| -> Option<zbus::zvariant::OwnedValue> {
        bus.call_method(Some("org.sfduo.Posture"), "/org/sfduo/Posture", Some("org.freedesktop.DBus.Properties"), "Get", &("org.sfduo.Posture", p)).ok()?.body().deserialize().ok()
    };
    let Some(angle) = get("Angle").and_then(|v| f64::try_from(v).ok()) else {
        tracing::debug!("system: no hinge from org.sfduo.Posture");
        return "—".into();
    };
    let posture = get("Posture").and_then(|v| String::try_from(v).ok()).unwrap_or_default();
    let name = match posture.as_str() {
        "closed" => Some("closed"),
        "laptop" => Some("laptop"),
        "flat" => Some("flat"),
        "folded" => Some("folded back"),
        _ => None,
    };
    format!("{angle:.0}°{}", name.map(|n| format!(" · {n}")).unwrap_or_default())
}

/// Days since 1970 of a civil date (Howard Hinnant's).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// "2026-10-01T18:00:00Z" as a time.
fn iso_time(s: &str) -> Option<i64> {
    let n = |a: usize, b: usize| s.get(a..b)?.parse::<i64>().ok();
    Some(days_from_civil(n(0, 4)?, n(5, 7)?, n(8, 10)?) * 86_400 + n(11, 13)? * 3600 + n(14, 16)? * 60 + n(17, 19)?)
}

/// met.no's symbol: what it is called, and Adwaita's icon for it.
fn sky(symbol: &str) -> (&'static str, String) {
    let night = symbol.ends_with("_night");
    let (text, icon) = if symbol.contains("thunder") {
        ("Thunder", "weather-storm")
    } else if symbol.contains("snow") || symbol.contains("sleet") {
        (if symbol.contains("sleet") { "Sleet" } else { "Snow" }, "weather-snow")
    } else if symbol.contains("showers") {
        ("Showers", "weather-showers-scattered")
    } else if symbol.contains("rain") {
        ("Rain", "weather-showers")
    } else if symbol.starts_with("fog") {
        ("Fog", "weather-fog")
    } else if symbol.starts_with("cloudy") {
        ("Cloudy", "weather-overcast")
    } else if symbol.starts_with("partlycloudy") {
        ("Partly cloudy", if night { "weather-few-clouds-night" } else { "weather-few-clouds" })
    } else if symbol.starts_with("fair") {
        ("Mostly clear", if night { "weather-few-clouds-night" } else { "weather-few-clouds" })
    } else {
        ("Clear", if night { "weather-clear-night" } else { "weather-clear" })
    };
    (text, icon.to_owned())
}

/// The weather for GNOME Weather's first city, from met.no.
fn weather() -> Result<Weather, String> {
    let locations = run("gsettings", &["get", "org.gnome.Weather", "locations"]);
    let city = locations.split('\'').nth(1).filter(|c| !c.is_empty()).ok_or("Choose a city in Weather")?.to_owned();
    // The first coordinates, in radians: "[(0.879..., 0.531...)]".
    let i = locations.find("[(").ok_or("Choose a city in Weather")?;
    let pair: Vec<f64> = locations[i + 2..].split(')').next().unwrap_or("").split(',').filter_map(|v| v.trim().parse().ok()).collect();
    let (lat, lon) = (pair.first().ok_or("No place")?.to_degrees(), pair.get(1).ok_or("No place")?.to_degrees());
    let url = format!("https://api.met.no/weatherapi/locationforecast/2.0/compact?lat={lat:.4}&lon={lon:.4}");
    let reply = fetch(&url, "application/json");
    let v: serde_json::Value = serde_json::from_str(&reply).map_err(|_| "No weather now")?;
    let series = v.pointer("/properties/timeseries").and_then(|s| s.as_array()).ok_or("No weather now")?;
    let now = now_s() as i64;
    let mut points: Vec<(i64, f64, String)> = Vec::new();
    for e in series {
        let Some(t) = e.get("time").and_then(|t| t.as_str()).and_then(iso_time) else { continue };
        let Some(temp) = e.pointer("/data/instant/details/air_temperature").and_then(|t| t.as_f64()) else { continue };
        let symbol = e.pointer("/data/next_1_hours/summary/symbol_code").or_else(|| e.pointer("/data/next_6_hours/summary/symbol_code")).and_then(|s| s.as_str()).unwrap_or("").to_owned();
        if t >= now - 1800 {
            points.push((t, temp, symbol));
        }
    }
    let first = points.first().ok_or("No weather now")?.clone();
    let day: Vec<f64> = points.iter().filter(|p| p.0 <= now + 86_400).map(|p| p.1).collect();
    let high_low = (!day.is_empty()).then(|| (day.iter().cloned().fold(f64::MIN, f64::max), day.iter().cloned().fold(f64::MAX, f64::min)));
    let hours = WEATHER_HOURS
        .iter()
        .filter_map(|ahead| points.iter().min_by_key(|p| (p.0 - (now + ahead * 3600)).abs()).map(|p| (p.0, p.1, sky(&p.2).1)))
        .collect();
    let (text, icon) = sky(&first.2);
    Ok(Weather { city, now: (first.1, text.to_owned(), icon), high_low, hours })
}

/// The worker: samples while lit, reads and draws when asked.
/// The weather now for the desktop's clock: the temperature, what it is
/// called, Adwaita's icon for it.
pub type Now = Arc<Mutex<Option<(f64, String, String)>>>;

pub fn worker(rx: Receiver<Job>, page: Arc<Mutex<Option<Page>>>, now: Now, wake: Ping) {
    let thin = Font::load(&["/usr/share/fonts/truetype/lato/Lato-Light.ttf", "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf"]);
    let regular = Font::load(&["/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf", "/usr/share/fonts/truetype/lato/Lato-Regular.ttf"]);
    let Some(fonts) = thin.zip(regular) else { return };
    let bus = zbus::blocking::Connection::session().ok();
    let temps: Vec<String> = std::fs::read_dir("/sys/class/thermal")
        .map(|d| d.flatten().filter_map(|e| {
            let p = e.path();
            let kind = read(p.join("type").to_str()?);
            let kind = kind.trim();
            (kind.starts_with("cpu") && kind.ends_with("-usr")).then(|| p.join("temp").to_string_lossy().into_owned())
        }).collect())
        .unwrap_or_default();
    let mut sampler = Sampler::new();
    let mut ledger: Option<Ledger> = None;
    let name = crate::sysscreen::first_name();
    let system = system_line();
    let mut weather_cache: Option<(Instant, Result<Weather, String>)> = None;
    let events = calendar();
    let mut asked_update = false;
    let mut details = false;
    let (mut lit, mut samples) = (true, 0u32);
    let mut next_sample = Instant::now();
    loop {
        let wait = if lit { next_sample.saturating_duration_since(Instant::now()) } else { Duration::from_secs(3600) };
        let job = match rx.recv_timeout(wait) {
            Ok(Job::Details(open)) => {
                details = open;
                Ok(Job::Read)
            }
            other => other,
        };
        match job {
            Ok(Job::Details(_)) => {}
            Ok(Job::Display(on)) => {
                if on && !lit {
                    // A span of darkness is not one long sample.
                    sampler.last = None;
                    next_sample = Instant::now();
                }
                lit = on;
            }
            Ok(Job::Read) => {
                let t = Instant::now();
                let mut f = Facts { name: name.clone(), system: system.clone(), ..Default::default() };
                battery(&mut f);
                f.load_line = load_line(&sampler, &temps);
                f.memory = memory();
                f.storage = storage();
                network(&mut f);
                f.today = Ledger::tally(&mut ledger, false);
                let twelve = run("gsettings", &["get", "org.gnome.desktop.interface", "clock-format"]).contains("12h");
                f.alarm = next_alarm(twelve);
                f.event = next_event(&events, twelve);
                f.hinge = hinge(&bus);
                f.uptime = uptime_text(read("/proc/uptime").split_whitespace().next().and_then(|v| v.parse().ok()).unwrap_or(0.0));
                f.port = port(&mut asked_update);
                if weather_cache.as_ref().is_none_or(|(at, w)| at.elapsed() > WEATHER_EVERY || (w.is_err() && at.elapsed() > Duration::from_secs(120))) {
                    weather_cache = Some((Instant::now(), weather()));
                }
                match &weather_cache {
                    Some((_, Ok(w))) => {
                        *now.lock().unwrap() = Some(w.now.clone());
                        f.weather = Some(w.clone());
                    }
                    Some((_, Err(e))) => f.weather_note = e.clone(),
                    None => {}
                }
                if let Some(p) = draw(&fonts, &f, twelve, details) {
                    tracing::debug!("system: read and drawn in {:.0} ms", t.elapsed().as_secs_f64() * 1e3);
                    *page.lock().unwrap() = Some(p);
                    wake.ping();
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                sampler.sample();
                samples += 1;
                // The ledger kept up, written once a minute.
                Ledger::tally(&mut ledger, samples % 6 == 0);
                next_sample = Instant::now() + LOAD_EVERY;
            }
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
}

/// An estimate as item rounds it: to 10 minutes, from 5 hours to hours.
fn hours(s: i64) -> Option<String> {
    if s <= 0 || s > 48 * 3600 {
        return None;
    }
    let min = ((s as f64 / 60.0) / 10.0).round() as i64 * 10;
    Some(if min >= 300 {
        format!("~{} h", (min as f64 / 60.0).round() as i64)
    } else if min >= 60 {
        format!("~{} h {} min", min / 60, min % 60)
    } else {
        format!("~{} min", min.max(10))
    })
}

fn rounded_path(x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<tiny_skia::Path> {
    let k = r * 0.5523;
    let mut pb = tiny_skia::PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    pb.cubic_to(x + w - r + k, y, x + w, y + r - k, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.cubic_to(x + w, y + h - r + k, x + w - r + k, y + h, x + w - r, y + h);
    pb.line_to(x + r, y + h);
    pb.cubic_to(x + r - k, y + h, x, y + h - r + k, x, y + h - r);
    pb.line_to(x, y + r);
    pb.cubic_to(x, y + r - k, x + r - k, y, x + r, y);
    pb.close();
    pb.finish()
}

/// The page, on frosted glass (sysscreen.rs draws the glass): the head, the
/// battery, then what the day holds - next
/// up, the weather, the connections, the storage - plain rows under small
/// titles, no cards; the rest (load, memory, the hinge, uptime, the port,
/// the system) under Details, opened by a tap. Returns the page and where
/// Details' row is (logical y, top and bottom).
fn draw(fonts: &(Font, Font), f: &Facts, twelve: bool, details: bool) -> Option<Page> {
    let (thin, regular) = fonts;
    let s = SCALE as f32;
    let panel = layout::panels()[0];
    let pw = panel.size.w as f32;
    // Tall enough for everything; cut to what it took after.
    let tall = 2200.0f32;
    let mut c = tiny_skia::Pixmap::new((pw * s) as u32, (tall * s) as u32)?;
    let text = |c: &mut tiny_skia::Pixmap, font: &Font, t: &str, size: f32, color: [f32; 4], x: f32, y: f32, align: i32, max_w: f32| -> (f32, f32) {
        if t.is_empty() {
            return (0.0, 0.0);
        }
        // Too long for its room: cut, with an ellipsis.
        let mut t = t.to_owned();
        let mut raster = font.rasterize(&t, size, color);
        while raster.1 as f32 / s > max_w && t.chars().count() > 2 {
            t = t.chars().take(t.chars().count() - 2).collect::<String>() + "…";
            raster = font.rasterize(&t, size, color);
        }
        let (rgba, w, h) = raster;
        if let Some(px) = tiny_skia::IntSize::from_wh(w as u32, h as u32).and_then(|sz| tiny_skia::Pixmap::from_vec(rgba, sz)) {
            let x = match align {
                0 => (pw * s - w as f32) / 2.0,
                1 => x * s - w as f32,
                _ => x * s,
            };
            c.draw_pixmap(x as i32, (y * s) as i32, px.as_ref(), &tiny_skia::PixmapPaint::default(), tiny_skia::Transform::identity(), None);
        }
        (w as f32 / s, h as f32 / s)
    };
    let fill = |c: &mut tiny_skia::Pixmap, x: f32, y: f32, w: f32, h: f32, r: f32, rgba: [u8; 4]| {
        let mut p = tiny_skia::Paint::default();
        p.set_color_rgba8(rgba[0], rgba[1], rgba[2], rgba[3]);
        p.anti_alias = true;
        if let Some(path) = rounded_path(x * s, y * s, w * s, h * s, r * s) {
            c.fill_path(&path, &p, tiny_skia::FillRule::Winding, tiny_skia::Transform::identity(), None);
        }
    };
    let icon = |c: &mut tiny_skia::Pixmap, name: &str, x: f32, y: f32, size: f32| {
        let path = format!("/usr/share/icons/Adwaita/symbolic/status/{name}-symbolic.svg");
        if let Some(mut pm) = crate::apps::icon_pixmap(&path, size as i32) {
            for px in pm.pixels_mut() {
                let a = px.alpha() as f32 / 255.0;
                let ch = |v: f32| (v * 255.0 * a) as u8;
                *px = tiny_skia::PremultipliedColorU8::from_rgba(ch(INK[0]), ch(INK[1]), ch(INK[2]), px.alpha()).unwrap_or(*px);
            }
            c.draw_pixmap((x * s) as i32, (y * s) as i32, pm.as_ref(), &tiny_skia::PixmapPaint::default(), tiny_skia::Transform::identity(), None);
        }
    };
    let (cx, inner) = (40.0f32, pw - 80.0);
    let now = crate::shade::local_time();

    // The head: the greeting and the date (the time is the desktop's).
    let part = crate::sysscreen::part_of_day(now.tm_hour);
    let greeting = match &f.name {
        Some(n) => format!("{part}, {n}"),
        None => part.to_owned(),
    };
    text(&mut c, regular, &greeting, 26.0, INK, cx, 72.0, -1, inner);
    text(&mut c, regular, &date_line(&now), 16.0, DIM, cx, 108.0, -1, inner);

    // The battery: its level large, what it is doing.
    let mut y = 170.0f32;
    let (lw, lh) = text(&mut c, thin, &format!("{}%", f.level), 64.0, INK, cx - 3.0, y, -1, inner);
    let doing = match f.status.as_str() {
        "Charging" => match hours(f.to_full_s) {
            Some(t) => format!("Charging · full in {t}"),
            None => "Charging".to_owned(),
        },
        "Full" | "Not charging" => "Charged".to_owned(),
        _ => match hours(f.to_empty_s) {
            Some(t) => format!("{t} left"),
            None => String::new(),
        },
    };
    text(&mut c, regular, &doing, 16.0, DIM, cx + lw + 16.0, y + lh - 30.0, -1, inner - lw - 16.0);
    y += lh + 30.0;

    // A part: a hairline, its title small and dim.
    let section = |c: &mut tiny_skia::Pixmap, title: &str, y: &mut f32| {
        fill(c, cx, *y, inner, 1.0, 0.0, [255, 255, 255, 22]);
        text(c, regular, title, 13.0, DIM, cx, *y + 18.0, -1, inner);
        *y += 50.0;
    };
    // A row: its name on the left, its value on the right.
    let row = |c: &mut tiny_skia::Pixmap, name: &str, value: &str, color: [f32; 4], y: &mut f32| {
        let (nw, _) = text(c, regular, name, 16.0, INK, cx, *y, -1, inner);
        text(c, regular, value, 16.0, color, cx + inner, *y, 1, inner - nw - 16.0);
        *y += 34.0;
    };

    section(&mut c, "Next up", &mut y);
    row(&mut c, "Alarm", &f.alarm, DIM, &mut y);
    row(&mut c, "Event", &f.event, DIM, &mut y);
    y += 14.0;

    section(&mut c, "Weather", &mut y);
    match &f.weather {
        Some(w) => {
            icon(&mut c, &w.now.2, cx, y + 2.0, 24.0);
            text(&mut c, regular, &format!("{:.0}° · {}", w.now.0, w.now.1), 18.0, INK, cx + 36.0, y, -1, inner - 160.0);
            text(&mut c, regular, &w.city, 14.0, DIM, cx + inner, y + 3.0, 1, 150.0);
            y += 34.0;
            if let Some((hi, lo)) = w.high_low {
                text(&mut c, regular, &format!("High {hi:.0}° · low {lo:.0}°"), 14.0, DIM, cx, y, -1, inner);
                y += 30.0;
            }
            // The next hours, one quiet line: when and how warm.
            let col = inner / WEATHER_HOURS.len() as f32;
            for (i, (t, temp, _)) in w.hours.iter().enumerate() {
                let mid = cx + col * i as f32 + col / 2.0;
                let tm = local(*t);
                let when = if i == 0 { "Now".to_owned() } else if twelve { format!("{} {}", if tm.tm_hour % 12 == 0 { 12 } else { tm.tm_hour % 12 }, if tm.tm_hour < 12 { "AM" } else { "PM" }) } else { format!("{}:00", tm.tm_hour) };
                let ww = regular.rasterize(&when, 12.0, DIM).1 as f32 / s;
                text(&mut c, regular, &when, 12.0, DIM, mid - ww / 2.0, y, -1, col);
                let tt = format!("{temp:.0}°");
                let tw = regular.rasterize(&tt, 15.0, INK).1 as f32 / s;
                text(&mut c, regular, &tt, 15.0, INK, mid - tw / 2.0, y + 20.0, -1, col);
            }
            y += 56.0;
        }
        None => {
            text(&mut c, regular, if f.weather_note.is_empty() { "Asking met.no…" } else { &f.weather_note }, 16.0, DIM, cx, y, -1, inner);
            y += 34.0;
        }
    }
    y += 14.0;

    section(&mut c, "Connections", &mut y);
    row(&mut c, "Wi-Fi", &f.wifi, DIM, &mut y);
    row(&mut c, "Mobile", &f.mobile, DIM, &mut y);
    row(&mut c, "Today", &format!("Wi-Fi {} · mobile {}", size(f.today[0] as f64), size(f.today[1] as f64)), DIM, &mut y);
    y += 14.0;

    section(&mut c, "Storage", &mut y);
    for (name, free, _) in &f.storage {
        row(&mut c, name, &format!("{} free", size(*free)), if *free < 1e9 { RED } else { DIM }, &mut y);
    }
    // A newer port, only when there is one.
    if f.port.contains(" is out") {
        y += 14.0;
        section(&mut c, "Update", &mut y);
        row(&mut c, "Port", &f.port, INK, &mut y);
    }
    y += 14.0;

    // Details: shut, one row to tap; open, the rest.
    fill(&mut c, cx, y, inner, 1.0, 0.0, [255, 255, 255, 22]);
    let details_top = y;
    text(&mut c, regular, "Details", 15.0, DIM, cx, y + 20.0, -1, inner);
    text(&mut c, regular, if details { "Hide" } else { "Show" }, 15.0, DIM, cx + inner, y + 20.0, 1, inner);
    y += 60.0;
    let details_bottom = y;
    if details {
        row(&mut c, "Power", &format!("{:.1} W · {:.0} °C", f.power_w, f.temp_c), DIM, &mut y);
        row(&mut c, "Load", &f.load_line, DIM, &mut y);
        if let Some((used, total, swap)) = f.memory {
            let mut m = format!("{} of {}", size(used), size(total));
            if swap >= 50e6 {
                m += &format!(" · swap {}", size(swap));
            }
            row(&mut c, "Memory", &m, DIM, &mut y);
        }
        row(&mut c, "Hinge", &f.hinge, DIM, &mut y);
        row(&mut c, "Uptime", &f.uptime, DIM, &mut y);
        if !f.port.contains(" is out") {
            row(&mut c, "Port", &f.port, DIM, &mut y);
        }
        row(&mut c, "System", &f.system, DIM, &mut y);
        y += 10.0;
    }
    // Cut to what it took, and room to scroll the last row above the dock.
    let used = ((y + 160.0) * s) as u32;
    let (w, h) = (c.width(), used.min(c.height()));
    let data = c.data()[..(w * h * 4) as usize].to_vec();
    Some((data, w as i32, h as i32, (details_top, details_bottom)))
}
