//! The phone's sleep, as logind says it (`PrepareForSleep`): what woke it.
//!
//! After a resume `/sys/power/pm_wakeup_irq` holds the interrupt that woke
//! the SoC, named in `/proc/interrupts`. The power key's (pon_kpdpwr) lights
//! the screen: the press that woke the phone is spent on waking it and
//! never arrives as a key. The modem's is a call or a message - the call
//! screen lights itself. Wi-Fi's (a packet) leaves the screen dark. The
//! port's sleep hook that pressed KEY_WAKEUP after every resume does not
//! under item.
//!
//! It also puts the phone to sleep (`tick`): locked, the screen dark, no
//! call, nothing playing, nothing holding the screen - after 30 s, or 3
//! after something else woke it (a Wi-Fi or modem packet, healthd's minute
//! alarm: with 15 s it was awake most of a night - over a thousand such
//! wakes, 300 mA - while Android sleeps again at once). The kernel may
//! refuse (the USB cable in, a wakeup on its way): a refusal that comes
//! back as a resume is tried again 3 s later, one without a resume 20 s
//! later. NO_SUSPEND=1 keeps it awake.
//!
//! Clocks' alarms are GLib timers: asleep, nothing wakes the phone for them,
//! and an alarm rang when something else woke it - minutes late, or not at
//! all. So before asking to sleep it sets the kernel's alarm (a timerfd on
//! CLOCK_REALTIME_ALARM, the RTC under it) for Clocks' next alarm and for a
//! snoozed one, a few seconds early: woken, Clocks rings within a second.
//! It needs CAP_WAKE_ALARM (session-run.sh gives it).
//!
//! While the screen is lit it holds a kernel wakelock (`awake`), as Android
//! does: the lid opened while systemd-sleep was on its way to sleep lit the
//! screen, and the phone went to sleep 0.2 s later all the same (or slept
//! and woke again: a blink). Held, the kernel gives the sleep up. It needs
//! CAP_BLOCK_SUSPEND and the android_wakelock group (session-run.sh).
//!
//! The wakelock does not stop a sleep already on its way: systemd-sleep
//! writes /sys/power/state without the wakeup_count handshake, so a lock
//! taken after it began is not looked at. And the lid's own interrupt is
//! spent by then - it reached item as the lid's event. Opened in that
//! moment, the phone slept on with the lid open and dark until a Wi-Fi
//! packet woke it (2026-10-05 11:28; it reset on that resume). So the lid
//! opened on the way into sleep also sets the kernel's alarm a second away
//! (`wake_soon`): the alarm timer refuses a sleep with an alarm under 2 s
//! (the sleep given up), or, if the sleep is past that, wakes it.

use std::sync::{Arc, Mutex};

use smithay::reexports::calloop::ping::Ping;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Woken {
    PowerKey,
    Modem,
    Wifi,
    Other,
}

/// What woke the phone, from the interrupt's name.
fn woken_by() -> (Woken, String) {
    let irq = std::fs::read_to_string("/sys/power/pm_wakeup_irq").ok().and_then(|s| s.trim().parse::<u32>().ok());
    let Some(irq) = irq else { return (Woken::Other, "unknown".into()) };
    let name = std::fs::read_to_string("/proc/interrupts")
        .unwrap_or_default()
        .lines()
        .find(|l| l.trim_start().split(':').next().and_then(|n| n.trim().parse::<u32>().ok()) == Some(irq))
        .and_then(|l| l.split_whitespace().last().map(str::to_owned))
        .unwrap_or_default();
    let kind = if name.contains("kpdpwr") {
        Woken::PowerKey
    } else if name.contains("modem") || name.contains("smp2p") || name.contains("ipa") {
        Woken::Modem
    } else if name.contains("WLAN") || name.contains("wlan") {
        Woken::Wifi
    } else {
        Woken::Other
    };
    (kind, format!("irq {irq} {name}"))
}

const DARK_NS: u64 = 30_000_000_000;
const AGAIN_NS: u64 = 3_000_000_000;
const RETRY_NS: u64 = 20_000_000_000;
/// Woken this long before an alarm.
const EARLY_S: i64 = 5;
/// Clocks' snooze.
pub const SNOOZE_S: i64 = 10 * 60;

static TIMER: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(-1);
static WAKELOCK: std::sync::OnceLock<(Option<std::fs::File>, Option<std::fs::File>)> = std::sync::OnceLock::new();

/// The kernel's alarm, a timerfd on CLOCK_REALTIME_ALARM, made first thing
/// (main.rs); then CAP_WAKE_ALARM is let go of as an ambient capability, so
/// the session and its apps do not get it.
pub fn prepare() {
    let fd = unsafe { libc::timerfd_create(libc::CLOCK_REALTIME_ALARM, libc::TFD_CLOEXEC | libc::TFD_NONBLOCK) };
    if fd < 0 {
        tracing::warn!("sleep: no wake alarm: {}", std::io::Error::last_os_error());
    }
    TIMER.store(fd, std::sync::atomic::Ordering::Relaxed);
    let open = |p: &str| std::fs::OpenOptions::new().write(true).open(p).map_err(|e| tracing::warn!("sleep: {p}: {e}")).ok();
    let _ = WAKELOCK.set((open("/sys/power/wake_lock"), open("/sys/power/wake_unlock")));
    unsafe { libc::prctl(libc::PR_CAP_AMBIENT, libc::PR_CAP_AMBIENT_CLEAR_ALL as libc::c_ulong, 0, 0, 0) };
}

/// The kernel's alarm set for `at` (s since the epoch), or off.
fn set_alarm(fd: i32, at: Option<i64>) {
    if fd < 0 {
        return;
    }
    let mut spec: libc::itimerspec = unsafe { std::mem::zeroed() };
    if let Some(at) = at {
        spec.it_value.tv_sec = at as libc::time_t;
    }
    if unsafe { libc::timerfd_settime(fd, libc::TFD_TIMER_ABSTIME, &spec, std::ptr::null_mut()) } < 0 {
        tracing::warn!("sleep: wake alarm not set: {}", std::io::Error::last_os_error());
    }
}

fn now_s() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

pub struct Sleep {
    woken: Arc<Mutex<Option<Woken>>>,
    /// Since when it may sleep, and whether it was just woken (sooner).
    may_since: Option<u64>,
    just_woken: bool,
    /// Not asked again before then.
    next_try: u64,
    timer: i32,
    /// A snoozed alarm's time (s since the epoch).
    snoozed: Arc<Mutex<Option<i64>>>,
    going: Arc<std::sync::atomic::AtomicBool>,
}

impl Sleep {
    pub fn new(wake: Ping) -> Sleep {
        let woken: Arc<Mutex<Option<Woken>>> = Default::default();
        let w = woken.clone();
        let going: Arc<std::sync::atomic::AtomicBool> = Default::default();
        let g = going.clone();
        std::thread::Builder::new()
            .name("sleep".into())
            .spawn(move || {
                let Ok(bus) = zbus::blocking::Connection::system() else { return };
                let rule = zbus::MatchRule::builder()
                    .msg_type(zbus::message::Type::Signal)
                    .interface("org.freedesktop.login1.Manager")
                    .and_then(|b| b.member("PrepareForSleep"))
                    .map(|b| b.build());
                let Ok(rule) = rule else { return };
                let Ok(signals) = zbus::blocking::MessageIterator::for_match_rule(rule, &bus, Some(8)) else { return };
                for msg in signals.flatten() {
                    let Ok(going) = msg.body().deserialize::<bool>() else { continue };
                    g.store(going, std::sync::atomic::Ordering::Relaxed);
                    if going {
                        tracing::info!("sleep: going to sleep");
                        continue;
                    }
                    let (kind, what) = woken_by();
                    tracing::info!("sleep: woken by {what} ({kind:?})");
                    *w.lock().unwrap() = Some(kind);
                    wake.ping();
                }
            })
            .expect("sleep thread");
        Sleep { woken, may_since: None, just_woken: false, next_try: 0, timer: TIMER.load(std::sync::atomic::Ordering::Relaxed), snoozed: Default::default(), going }
    }

    /// On the way into sleep (logind's PrepareForSleep, until it says the
    /// phone is back): the screen lit now would go dark with the phone.
    pub fn going(&self) -> bool {
        self.going.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Whether the phone may sleep now; asks logind to put it to sleep once
    /// it has been so long enough. Returns whether it asked.
    pub fn tick(&mut self, now_ns: u64, may: bool) -> bool {
        if !may || std::env::var_os("NO_SUSPEND").is_some() {
            self.may_since = None;
            return false;
        }
        let since = *self.may_since.get_or_insert(now_ns);
        let delay = if self.just_woken { AGAIN_NS } else { DARK_NS };
        if now_ns < since + delay || now_ns < self.next_try {
            return false;
        }
        self.next_try = now_ns + RETRY_NS;
        self.just_woken = false;
        tracing::info!("sleep: asking to sleep");
        let (timer, snoozed) = (self.timer, self.snoozed.clone());
        std::thread::spawn(move || {
            // Woken for the next alarm, Clocks' or a snoozed one.
            let now = now_s();
            let mut s = snoozed.lock().unwrap();
            if s.is_some_and(|t| t <= now) {
                *s = None;
            }
            let at = crate::sysfacts::next_alarm_at().map(|a| a.0).into_iter().chain(*s).min().map(|t| (t - EARLY_S).max(now + 1));
            drop(s);
            set_alarm(timer, at);
            if let Some(at) = at {
                tracing::info!("sleep: woken for an alarm in {} s", at - now);
            }
            let Ok(bus) = zbus::blocking::Connection::system() else { return };
            if let Err(e) = bus.call_method(Some("org.freedesktop.login1"), "/org/freedesktop/login1", Some("org.freedesktop.login1.Manager"), "Suspend", &(false)) {
                tracing::warn!("sleep: refused: {e}");
            }
        });
        true
    }

    /// The screen lit: the kernel's sleep held off (or let be).
    pub fn awake(&self, on: bool) {
        use std::io::Write;
        let Some((lock, unlock)) = WAKELOCK.get() else { return };
        let Some(mut f) = (if on { lock } else { unlock }).as_ref() else { return };
        match f.write_all(b"item") {
            Ok(()) => tracing::info!("sleep: wakelock {}", if on { "held" } else { "let go" }),
            Err(e) => tracing::warn!("sleep: wakelock: {e}"),
        }
    }

    /// The kernel's alarm a second away: the sleep on its way given up, or
    /// woken from at once (the lid opened as it went: see the top). The
    /// alarm for Clocks is set again before the next sleep.
    pub fn wake_soon(&self) {
        set_alarm(self.timer, Some(now_s() + 1));
        tracing::info!("sleep: woken again in a second");
    }

    /// An alarm snoozed now: woken for it again.
    pub fn snoozed(&self) {
        *self.snoozed.lock().unwrap() = Some(now_s() + SNOOZE_S);
    }

    /// What woke the phone, once, after a resume.
    pub fn take_woken(&mut self, now_ns: u64) -> Option<Woken> {
        let w = self.woken.lock().unwrap().take()?;
        // Awake again: asleep again sooner, from now (the last asking is over).
        self.may_since = Some(now_ns);
        self.next_try = 0;
        self.just_woken = true;
        self.next_try = 0;
        Some(w)
    }
}
