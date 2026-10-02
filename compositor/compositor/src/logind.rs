//! The lock and the session, as the rest of the system sees them.
//!
//! - logind's session (`org.freedesktop.login1.Session`): its `LockedHint`
//!   follows our lock screen, and its `Lock` and `Unlock` signals (`loginctl
//!   lock-session`, `unlock-session`; the fingerprint reader's service
//!   unlocks that way) lock and unlock it. Its `IdleHint` follows the
//!   screen being dark (the modem is told by it, see `report`).
//! - `org.gnome.ScreenSaver` on the session bus, phosh's job when phosh runs:
//!   `Active` is whether the screen is dark, as phosh means it (the port's
//!   sfduo-fingerprint arms the reader while the session is locked and the
//!   saver not active, that is, lit), `Lock` locks.
//!
//! Requests from the buses come to the loop through a shared list and the
//! ping; the state goes out with `report`, after each frame, only when it
//! changed, on a thread of its own (a call to logind takes milliseconds).

use std::sync::{Arc, Mutex};

use smithay::reexports::calloop::ping::Ping;
use zbus::zvariant::OwnedObjectPath;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Ask {
    Lock,
    Unlock,
    /// The screen dark (true) or lit.
    Blank(bool),
}

#[derive(Default)]
struct Shared {
    asks: Vec<Ask>,
    /// What the saver answers: the screen dark, and since when (s).
    blank: bool,
    blank_since: u64,
}

struct Saver {
    shared: Arc<Mutex<Shared>>,
    wake: Ping,
}

#[zbus::interface(name = "org.gnome.ScreenSaver")]
impl Saver {
    fn lock(&self) {
        self.shared.lock().unwrap().asks.push(Ask::Lock);
        self.wake.ping();
    }

    fn get_active(&self) -> bool {
        self.shared.lock().unwrap().blank
    }

    fn set_active(&self, active: bool) {
        self.shared.lock().unwrap().asks.push(Ask::Blank(active));
        self.wake.ping();
    }

    fn get_active_time(&self) -> u32 {
        let shared = self.shared.lock().unwrap();
        if !shared.blank {
            return 0;
        }
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        now.saturating_sub(shared.blank_since) as u32
    }

    #[zbus(signal)]
    async fn active_changed(emitter: &zbus::object_server::SignalEmitter<'_>, active: bool) -> zbus::Result<()>;
}

pub struct Logind {
    shared: Arc<Mutex<Shared>>,
    system: Option<zbus::blocking::Connection>,
    saver: Option<zbus::blocking::Connection>,
    session: Option<OwnedObjectPath>,
    /// What was last told: locked, blank.
    told: Option<(bool, bool)>,
}

const SAVER_PATH: &str = "/org/gnome/ScreenSaver";

impl Logind {
    pub fn new(wake: Ping) -> Logind {
        let shared = Arc::new(Mutex::new(Shared::default()));
        let system = zbus::blocking::Connection::system().map_err(|e| tracing::warn!("logind: no system bus: {e}")).ok();
        // Our session: the one this process belongs to (item.service's PAM
        // session, or session-run.sh's).
        let session: Option<OwnedObjectPath> = system.as_ref().and_then(|c| {
            c.call_method(
                Some("org.freedesktop.login1"),
                "/org/freedesktop/login1",
                Some("org.freedesktop.login1.Manager"),
                "GetSessionByPID",
                &(std::process::id()),
            )
            .and_then(|reply| reply.body().deserialize::<OwnedObjectPath>())
            .map_err(|e| tracing::warn!("logind: no session for us: {e}"))
            .ok()
        });
        if let (Some(conn), Some(path)) = (&system, &session) {
            tracing::info!("logind: our session {}", path.as_str());
            let rule = zbus::MatchRule::builder()
                .msg_type(zbus::message::Type::Signal)
                .sender("org.freedesktop.login1")
                .and_then(|b| b.interface("org.freedesktop.login1.Session"))
                .and_then(|b| b.path(path.as_str().to_owned()))
                .map(|b| b.build());
            match rule.and_then(|r| zbus::blocking::MessageIterator::for_match_rule(r, conn, Some(8))) {
                Ok(signals) => {
                    let (shared, wake) = (shared.clone(), wake.clone());
                    std::thread::spawn(move || {
                        for message in signals.flatten() {
                            let member = message.header().member().map(|m| m.to_string());
                            let ask = match member.as_deref() {
                                Some("Lock") => Ask::Lock,
                                Some("Unlock") => Ask::Unlock,
                                _ => continue,
                            };
                            tracing::info!("logind: {ask:?}");
                            shared.lock().unwrap().asks.push(ask);
                            wake.ping();
                        }
                    });
                }
                Err(e) => tracing::warn!("logind: no signals: {e}"),
            }
        }
        let saver = Saver { shared: shared.clone(), wake };
        let saver = zbus::blocking::connection::Builder::session()
            .and_then(|b| b.name("org.gnome.ScreenSaver"))
            .and_then(|b| b.serve_at(SAVER_PATH, saver))
            .and_then(|b| b.build())
            .map_err(|e| tracing::warn!("logind: not the screen saver: {e}"))
            .ok();
        if saver.is_some() {
            tracing::info!("logind: serving org.gnome.ScreenSaver");
        }
        Logind { shared, system, saver, session, told: None }
    }

    /// What the buses asked since last time.
    pub fn take_asks(&self) -> Vec<Ask> {
        std::mem::take(&mut self.shared.lock().unwrap().asks)
    }

    /// The lock screen's state, told to logind and the saver's listeners if
    /// it changed.
    pub fn report(&mut self, locked: bool, blank: bool) {
        if self.told == Some((locked, blank)) {
            return;
        }
        let before = self.told.replace((locked, blank));
        if before.map(|(l, _)| l) != Some(locked) {
            if let (Some(conn), Some(path)) = (self.system.clone(), self.session.clone()) {
                std::thread::spawn(move || {
                    let result = conn.call_method(
                        Some("org.freedesktop.login1"),
                        path.as_str(),
                        Some("org.freedesktop.login1.Session"),
                        "SetLockedHint",
                        &(locked),
                    );
                    match result {
                        Ok(_) => tracing::info!("logind: LockedHint {locked}"),
                        Err(e) => tracing::warn!("logind: LockedHint {locked}: {e}"),
                    }
                });
            }
        }
        if before.map(|(_, b)| b) != Some(blank) {
            // The session idle while the screen is dark, as GNOME's session
            // says it: Droidian's ofono binder plugin reads seat0's IdleHint
            // as the screen's state and only then tells the modem (device
            // state, indication filter) to keep quiet - left lit, the modem
            // woke the sleeping phone every ~37 s over glink (2026-10-02).
            if let (Some(conn), Some(path)) = (self.system.clone(), self.session.clone()) {
                std::thread::spawn(move || {
                    let result = conn.call_method(Some("org.freedesktop.login1"), path.as_str(), Some("org.freedesktop.login1.Session"), "SetIdleHint", &(blank));
                    match result {
                        Ok(_) => tracing::info!("logind: IdleHint {blank}"),
                        Err(e) => tracing::warn!("logind: IdleHint {blank}: {e}"),
                    }
                });
            }
            {
                let mut shared = self.shared.lock().unwrap();
                shared.blank = blank;
                shared.blank_since = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
            }
            if let Some(conn) = self.saver.clone() {
                std::thread::spawn(move || {
                    let iface = conn.object_server().interface::<_, Saver>(SAVER_PATH);
                    if let Ok(iface) = iface {
                        let _ = zbus::block_on(Saver::active_changed(iface.signal_emitter(), blank));
                    }
                });
            }
        }
    }
}
