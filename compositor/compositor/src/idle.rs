//! Idleness: when the user last did something, who keeps the screen on, and
//! gsd-power told, as mutter and phosh tell it.
//!
//! - The screen blanks after the session's idle delay (lock.rs), dimmed for
//!   DIM_NS before; the delay is read again when the setting changes.
//! - It stays on while something asks it to: a window inhibiting idleness
//!   (Wayland's idle-inhibit, a video playing), or an inhibitor of
//!   gnome-session's (`IsInhibited` with the idle flag, asked on a thread
//!   every 10 s).
//! - `org.gnome.Mutter.IdleMonitor` on the session bus, phosh's job: the
//!   idle time, and watches that fire when the user has been idle so long or
//!   comes back - gsd-power's dimming and its going to sleep by the user's
//!   settings work from it.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Dimmed this long before the screen goes dark.
pub const DIM_NS: u64 = 7_000_000_000;
const PATH: &str = "/org/gnome/Mutter/IdleMonitor/Core";

#[derive(Default)]
struct Watches {
    next: u32,
    /// Idle watches: their interval (ms), whether fired this idle stretch.
    idle: HashMap<u32, (u64, bool)>,
    /// Watches for the user's coming back.
    active: Vec<u32>,
}

struct Monitor {
    last_active_ns: Arc<AtomicU64>,
    watches: Arc<Mutex<Watches>>,
}

fn idle_ms(last: &AtomicU64) -> u64 {
    hybris_hwc::now_ns().saturating_sub(last.load(Ordering::Relaxed)) / 1_000_000
}

#[zbus::interface(name = "org.gnome.Mutter.IdleMonitor")]
impl Monitor {
    fn get_idletime(&self) -> u64 {
        idle_ms(&self.last_active_ns)
    }

    fn add_idle_watch(&self, interval: u64) -> u32 {
        let mut w = self.watches.lock().unwrap();
        w.next += 1;
        let id = w.next;
        w.idle.insert(id, (interval, false));
        id
    }

    fn add_user_active_watch(&self) -> u32 {
        let mut w = self.watches.lock().unwrap();
        w.next += 1;
        let id = w.next;
        w.active.push(id);
        id
    }

    fn remove_watch(&self, id: u32) {
        let mut w = self.watches.lock().unwrap();
        w.idle.remove(&id);
        w.active.retain(|a| *a != id);
    }

    #[zbus(signal)]
    async fn watch_fired(emitter: &zbus::object_server::SignalEmitter<'_>, id: u32) -> zbus::Result<()>;
}

pub struct Idle {
    /// When the user last did something (ns).
    pub last_active_ns: Arc<AtomicU64>,
    /// gnome-session has an idle inhibitor.
    session_inhibited: Arc<AtomicBool>,
    /// The session's idle delay (ns; 0 never).
    delay_ns: Arc<AtomicU64>,
}

impl Idle {
    pub fn new() -> Idle {
        let last = Arc::new(AtomicU64::new(hybris_hwc::now_ns()));
        let inhibited = Arc::new(AtomicBool::new(false));
        let delay = Arc::new(AtomicU64::new(300_000_000_000));
        let watches: Arc<Mutex<Watches>> = Default::default();
        let monitor = Monitor { last_active_ns: last.clone(), watches: watches.clone() };
        let conn = zbus::blocking::connection::Builder::session()
            .and_then(|b| b.name("org.gnome.Mutter.IdleMonitor"))
            .and_then(|b| b.serve_at(PATH, monitor))
            .and_then(|b| b.build());
        match &conn {
            Ok(_) => tracing::info!("idle: the idle monitor"),
            Err(e) => tracing::warn!("idle: no idle monitor: {e}"),
        }
        // The watches fired as the idle time crosses them; the session's
        // inhibitors and the idle delay read every 10 s.
        let (l, i, d) = (last.clone(), inhibited.clone(), delay.clone());
        std::thread::Builder::new()
            .name("idle".into())
            .spawn(move || {
                let session = zbus::blocking::Connection::session().ok();
                let mut was_idle_ms = 0u64;
                let mut tick = 0u32;
                loop {
                    let idle = idle_ms(&l);
                    if let Ok(conn) = &conn {
                        let mut fire = Vec::new();
                        {
                            let mut w = watches.lock().unwrap();
                            for (id, (interval, fired)) in w.idle.iter_mut() {
                                if idle >= *interval && !*fired {
                                    *fired = true;
                                    fire.push(*id);
                                } else if idle < *interval {
                                    *fired = false;
                                }
                            }
                            // Back after being idle: the active watches fire, once.
                            if idle < was_idle_ms && was_idle_ms > 500 {
                                fire.append(&mut w.active);
                            }
                        }
                        for id in fire {
                            let _ = conn.emit_signal(None::<&str>, PATH, "org.gnome.Mutter.IdleMonitor", "WatchFired", &(id));
                        }
                    }
                    was_idle_ms = idle;
                    if tick % 40 == 0 {
                        if let Some(s) = &session {
                            let inhibited = s
                                .call_method(Some("org.gnome.SessionManager"), "/org/gnome/SessionManager", Some("org.gnome.SessionManager"), "IsInhibited", &(8u32))
                                .ok()
                                .and_then(|r| r.body().deserialize::<bool>().ok())
                                .unwrap_or(false);
                            i.store(inhibited, Ordering::Relaxed);
                        }
                        let out = std::process::Command::new("gsettings").args(["get", "org.gnome.desktop.session", "idle-delay"]).output();
                        if let Some(s) = out.ok().and_then(|o| String::from_utf8_lossy(&o.stdout).split_whitespace().last().and_then(|v| v.parse::<u64>().ok())) {
                            d.store(s * 1_000_000_000, Ordering::Relaxed);
                        }
                    }
                    tick = tick.wrapping_add(1);
                    std::thread::sleep(Duration::from_millis(250));
                }
            })
            .expect("idle thread");
        Idle { last_active_ns: last, session_inhibited: inhibited, delay_ns: delay }
    }

    /// The user did something.
    pub fn active(&self, now_ns: u64) {
        self.last_active_ns.store(now_ns, Ordering::Relaxed);
    }

    pub fn session_inhibited(&self) -> bool {
        self.session_inhibited.load(Ordering::Relaxed)
    }

    pub fn delay_ns(&self) -> u64 {
        self.delay_ns.load(Ordering::Relaxed)
    }
}
