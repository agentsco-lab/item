//! polkit's authentication agent: the compositor asks for the PIN when
//! something needs the user to say it is them (a setting only root may
//! change, a system service), as phosh's agent does - the PIN is the
//! user's password on the port.
//!
//! The agent is registered with polkit for the session
//! (`RegisterAuthenticationAgent`); polkit calls `BeginAuthentication` and
//! waits for its reply. The ask goes in a queue the loop shows as a dialog
//! (dialog.rs); the PIN entered is checked by polkit's setuid helper
//! (`polkit-agent-helper-1`, given the cookie and then the PIN as PAM asks
//! for it), which tells polkit itself; the reply follows. A wrong PIN may be
//! tried again; Cancel, or polkit's `CancelAuthentication`, ends it. The PIN
//! is never logged and is zeroed after use.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::sync::{Arc, Mutex};

use smithay::reexports::calloop::ping::Ping;
use zbus::zvariant::OwnedValue;

const PATH: &str = "/org/item/PolicyKit1/AuthenticationAgent";

/// What polkit asks: why, and how to answer it.
pub struct Ask {
    pub message: String,
    pub action: String,
    cookie: String,
    user: String,
    reply: async_channel::Sender<Result<(), String>>,
}

#[derive(Default)]
struct Shared {
    asks: Vec<Ask>,
    /// Cookies polkit took back.
    cancelled: Vec<String>,
}

struct Agent {
    shared: Arc<Mutex<Shared>>,
    wake: Ping,
}

/// The user polkit is to authenticate: ours if it is among those it
/// offers, else the first unix user offered.
fn user_of(identities: &[(String, HashMap<String, OwnedValue>)]) -> String {
    let me = unsafe { libc::getuid() };
    let uids: Vec<u32> = identities
        .iter()
        .filter(|(kind, _)| kind == "unix-user")
        .filter_map(|(_, d)| d.get("uid").and_then(|v| u32::try_from(v.try_clone().ok()?).ok()))
        .collect();
    let uid = if uids.contains(&me) { me } else { uids.first().copied().unwrap_or(me) };
    unsafe {
        let pw = libc::getpwuid(uid);
        if pw.is_null() {
            return std::env::var("USER").unwrap_or_default();
        }
        std::ffi::CStr::from_ptr((*pw).pw_name).to_string_lossy().into_owned()
    }
}

#[zbus::interface(name = "org.freedesktop.PolicyKit1.AuthenticationAgent")]
impl Agent {
    async fn begin_authentication(
        &self,
        action_id: String,
        message: String,
        _icon_name: String,
        _details: HashMap<String, String>,
        cookie: String,
        identities: Vec<(String, HashMap<String, OwnedValue>)>,
    ) -> zbus::fdo::Result<()> {
        let (tx, rx) = async_channel::bounded(1);
        tracing::info!("polkit: asked for {action_id}");
        let user = user_of(&identities);
        self.shared.lock().unwrap().asks.push(Ask { message, action: action_id, cookie, user, reply: tx });
        self.wake.ping();
        match rx.recv().await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(e)) => Err(zbus::fdo::Error::Failed(e)),
            Err(_) => Err(zbus::fdo::Error::Failed("Dismissed".into())),
        }
    }

    async fn cancel_authentication(&self, cookie: String) {
        tracing::info!("polkit: taken back");
        let mut shared = self.shared.lock().unwrap();
        shared.asks.retain(|a| {
            if a.cookie == cookie {
                let _ = a.reply.try_send(Err("Cancelled".into()));
                false
            } else {
                true
            }
        });
        shared.cancelled.push(cookie);
        drop(shared);
        self.wake.ping();
    }
}

pub struct Polkit {
    shared: Arc<Mutex<Shared>>,
    /// The answer to a PIN being checked, from its thread: the cookie, and
    /// whether it was right.
    checked: Arc<Mutex<Option<(String, bool)>>>,
    wake: Ping,
    _conn: Option<zbus::blocking::Connection>,
}

/// polkit's setuid helper, where the distribution put it.
fn helper() -> Option<&'static str> {
    ["/usr/lib/polkit-1/polkit-agent-helper-1", "/usr/libexec/polkit-agent-helper-1", "/usr/lib/policykit-1/polkit-agent-helper-1"].into_iter().find(|p| std::path::Path::new(p).exists())
}

impl Polkit {
    pub fn new(wake: Ping) -> Polkit {
        let shared = Arc::new(Mutex::new(Shared::default()));
        let agent = Agent { shared: shared.clone(), wake: wake.clone() };
        let conn = zbus::blocking::connection::Builder::system().map(|b| b.method_timeout(std::time::Duration::from_secs(5))).and_then(|b| b.serve_at(PATH, agent)).and_then(|b| b.build());
        let conn = match conn {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("polkit: no agent: {e}");
                return Polkit { shared, checked: Default::default(), wake, _conn: None };
            }
        };
        // Registered for the session it runs in.
        let session = std::env::var("XDG_SESSION_ID").ok().or_else(|| {
            let path: zbus::zvariant::OwnedObjectPath = conn
                .call_method(Some("org.freedesktop.login1"), "/org/freedesktop/login1", Some("org.freedesktop.login1.Manager"), "GetSessionByPID", &(std::process::id()))
                .ok()?
                .body()
                .deserialize()
                .ok()?;
            let id: OwnedValue = conn.call_method(Some("org.freedesktop.login1"), path.as_str(), Some("org.freedesktop.DBus.Properties"), "Get", &("org.freedesktop.login1.Session", "Id")).ok()?.body().deserialize().ok()?;
            String::try_from(id).ok()
        });
        match session {
            Some(id) => {
                let mut subject: HashMap<&str, zbus::zvariant::Value> = HashMap::new();
                subject.insert("session-id", zbus::zvariant::Value::from(id.clone()));
                let locale = std::env::var("LANG").unwrap_or_else(|_| "en_US.UTF-8".into());
                match conn.call_method(
                    Some("org.freedesktop.PolicyKit1"),
                    "/org/freedesktop/PolicyKit1/Authority",
                    Some("org.freedesktop.PolicyKit1.Authority"),
                    "RegisterAuthenticationAgent",
                    &(("unix-session", subject), locale, PATH),
                ) {
                    Ok(_) => tracing::info!("polkit: the agent for session {id}{}", if helper().is_none() { " (no helper found!)" } else { "" }),
                    Err(e) => tracing::warn!("polkit: not registered: {e}"),
                }
            }
            None => tracing::warn!("polkit: no session to register for"),
        }
        Polkit { shared, checked: Default::default(), wake, _conn: Some(conn) }
    }

    /// The first ask waiting: its message and action, if any.
    pub fn waiting(&self) -> Option<(String, String)> {
        self.shared.lock().unwrap().asks.first().map(|a| (a.message.clone(), a.action.clone()))
    }

    /// The PIN for the first ask: checked on a thread by polkit's helper.
    pub fn answer(&self, mut pin: String) {
        let Some((cookie, user)) = self.shared.lock().unwrap().asks.first().map(|a| (a.cookie.clone(), a.user.clone())) else {
            unsafe { pin.as_bytes_mut().fill(0) };
            return;
        };
        let (slot, wake) = (self.checked.clone(), self.wake.clone());
        std::thread::spawn(move || {
            let ok = check(&user, &cookie, &pin);
            unsafe { pin.as_bytes_mut().fill(0) };
            *slot.lock().unwrap() = Some((cookie, ok));
            wake.ping();
        });
    }

    /// The first ask dismissed (Cancel).
    pub fn dismiss(&self) {
        let mut shared = self.shared.lock().unwrap();
        if !shared.asks.is_empty() {
            let a = shared.asks.remove(0);
            let _ = a.reply.try_send(Err("Dismissed".into()));
            tracing::info!("polkit: dismissed");
        }
    }

    /// A PIN's answer: Some(true) the ask is done, Some(false) wrong, try
    /// again.
    pub fn take_checked(&self) -> Option<bool> {
        let (cookie, ok) = self.checked.lock().unwrap().take()?;
        let mut shared = self.shared.lock().unwrap();
        if ok {
            if let Some(i) = shared.asks.iter().position(|a| a.cookie == cookie) {
                let a = shared.asks.remove(i);
                let _ = a.reply.try_send(Ok(()));
            }
            tracing::info!("polkit: authenticated");
        } else {
            tracing::info!("polkit: a wrong PIN");
        }
        Some(ok)
    }

    /// Whether polkit took an ask back since last asked.
    pub fn take_cancelled(&self) -> bool {
        !std::mem::take(&mut self.shared.lock().unwrap().cancelled).is_empty()
    }
}

/// polkit's helper, for `user`, with the cookie, answering PAM's prompts
/// with the PIN: it tells polkit itself; true if it says SUCCESS.
fn check(user: &str, cookie: &str, pin: &str) -> bool {
    let Some(helper) = helper() else {
        tracing::warn!("polkit: no polkit-agent-helper-1");
        return false;
    };
    let mut child = match std::process::Command::new(helper).arg(user).stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null()).spawn() {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("polkit: the helper: {e}");
            return false;
        }
    };
    let (Some(mut stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else { return false };
    let _ = writeln!(stdin, "{cookie}");
    let mut ok = false;
    for line in BufReader::new(stdout).lines().map_while(Result::ok) {
        if line.starts_with("PAM_PROMPT_ECHO_OFF") || line.starts_with("PAM_PROMPT_ECHO_ON") {
            let _ = writeln!(stdin, "{pin}");
        } else if line == "SUCCESS" {
            ok = true;
            break;
        } else if line == "FAILURE" {
            break;
        }
    }
    drop(stdin);
    let _ = child.wait();
    ok
}
