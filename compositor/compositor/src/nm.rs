//! NetworkManager's secret agent: the compositor asks for a Wi-Fi network's
//! password when NetworkManager needs one (a network joined for the first
//! time, from Settings, or one whose password was wrong), as phosh's agent
//! does - in the system's dialog (dialog.rs), typed on the compositor's own
//! keyboard (keys.rs).
//!
//! The agent is registered with NetworkManager's AgentManager; its
//! `GetSecrets` waits for the dialog's answer and gives NetworkManager the
//! key (`psk` for WPA, `wep-key0` for WEP, `password` for 802.1X), which it
//! keeps as it does any network's. Asked again for the same network
//! (`REQUEST_NEW`), the dialog says the last one did not work. A password
//! is never logged.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use smithay::reexports::calloop::ping::Ping;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

const PATH: &str = "/org/freedesktop/NetworkManager/SecretAgent";
const ALLOW_INTERACTION: u32 = 0x1;
const REQUEST_NEW: u32 = 0x2;

type Settings = HashMap<String, HashMap<String, OwnedValue>>;

/// What NetworkManager asks for.
pub struct Ask {
    /// The network's name.
    pub network: String,
    /// The last password did not work.
    pub again: bool,
    setting: String,
    key: String,
    connection: OwnedObjectPath,
    reply: async_channel::Sender<Option<String>>,
}

#[derive(Default)]
struct Shared {
    asks: Vec<Ask>,
    cancelled: bool,
    /// Per network: how many times its own stored key was given back
    /// silently after a failed handshake, and since when.
    retries: HashMap<String, (u32, std::time::Instant)>,
}

/// A failed handshake on a network whose key NetworkManager keeps itself
/// (psk-flags 0) is a weak link far more often than a wrong key: the key
/// is given back as it is, silently, this many times in RETRY_WINDOW
/// before the dialog says "it did not work" (2026-10-22: a 5 GHz AP at 60 %
/// dropped the 4-way handshake twice in a day, reason 15, with the right
/// key).
const SILENT_RETRIES: u32 = 2;
const RETRY_WINDOW: std::time::Duration = std::time::Duration::from_secs(180);

struct Agent {
    shared: Arc<Mutex<Shared>>,
    wake: Ping,
}

/// The agent's errors, by the names NetworkManager knows: a cancel is not
/// asked again.
#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "org.freedesktop.NetworkManager.SecretAgent")]
enum AgentError {
    #[zbus(error)]
    ZBus(zbus::Error),
    UserCanceled(String),
    NoSecrets(String),
}

#[zbus::interface(name = "org.freedesktop.NetworkManager.SecretAgent")]
impl Agent {
    async fn get_secrets(&self, #[zbus(connection)] conn: &zbus::Connection, connection: Settings, connection_path: OwnedObjectPath, setting_name: String, _hints: Vec<String>, flags: u32) -> Result<Settings, AgentError> {
        let ssid = connection
            .get("802-11-wireless")
            .and_then(|w| w.get("ssid"))
            .and_then(|v| Vec::<u8>::try_from(v.try_clone().ok()?).ok())
            .map(|b| String::from_utf8_lossy(&b).into_owned());
        let id = connection.get("connection").and_then(|c| c.get("id")).and_then(|v| String::try_from(v.try_clone().ok()?).ok());
        let network = ssid.or(id).unwrap_or_else(|| "this network".into());
        // Which key, by what the network is.
        let key = match setting_name.as_str() {
            "802-11-wireless-security" => {
                let mgmt = connection.get(&setting_name).and_then(|s| s.get("key-mgmt")).and_then(|v| String::try_from(v.try_clone().ok()?).ok()).unwrap_or_default();
                if mgmt == "none" { "wep-key0" } else { "psk" }
            }
            "802-1x" => "password",
            _ => {
                tracing::info!("nm: asked for {setting_name}: not ours to answer");
                return Err(AgentError::NoSecrets("Not an agent for that".into()));
            }
        };
        if flags & ALLOW_INTERACTION == 0 {
            return Err(AgentError::NoSecrets("Nobody may be asked".into()));
        }
        // Asked again for a network whose key NetworkManager keeps: the key
        // given back as it is, a couple of times, before anyone is asked.
        let kept = connection.get(&setting_name).and_then(|s| s.get("psk-flags")).and_then(|v| u32::try_from(v.try_clone().ok()?).ok()).unwrap_or(0) == 0;
        if flags & REQUEST_NEW != 0 && kept && key == "psk" {
            let tries = {
                let mut shared = self.shared.lock().unwrap();
                let entry = shared.retries.entry(network.clone()).or_insert((0, std::time::Instant::now()));
                if entry.1.elapsed() > RETRY_WINDOW {
                    *entry = (0, std::time::Instant::now());
                }
                entry.0 += 1;
                entry.0
            };
            if tries <= SILENT_RETRIES {
                let own = conn
                    .call_method(Some("org.freedesktop.NetworkManager"), connection_path.as_ref(), Some("org.freedesktop.NetworkManager.Settings.Connection"), "GetSecrets", &(setting_name.as_str(),))
                    .await
                    .and_then(|r| r.body().deserialize::<Settings>());
                match own {
                    Ok(mut secrets) if secrets.get(&setting_name).is_some_and(|s| s.contains_key(key)) => {
                        tracing::info!("nm: the handshake failed on a network with its key kept: the key given back as it is ({tries} of {SILENT_RETRIES})");
                        let inner = secrets.remove(&setting_name).unwrap_or_default();
                        let mut out = Settings::new();
                        out.insert(setting_name, inner);
                        return Ok(out);
                    }
                    Ok(_) => tracing::info!("nm: the kept key could not be read back: asking"),
                    Err(e) => tracing::info!("nm: the kept key could not be read back ({e}): asking"),
                }
            }
        }
        tracing::info!("nm: a password asked for a network{}", if flags & REQUEST_NEW != 0 { " (again)" } else { "" });
        let (tx, rx) = async_channel::bounded(1);
        self.shared.lock().unwrap().asks.push(Ask { network, again: flags & REQUEST_NEW != 0, setting: setting_name.clone(), key: key.to_owned(), connection: connection_path, reply: tx });
        self.wake.ping();
        match rx.recv().await {
            Ok(Some(mut secret)) => {
                let mut inner: HashMap<String, OwnedValue> = HashMap::new();
                inner.insert(key.to_owned(), Value::from(secret.clone()).try_to_owned().map_err(|e| AgentError::ZBus(e.into()))?);
                // SAFETY: zeroing the bytes keeps the String valid UTF-8.
                unsafe { secret.as_bytes_mut().fill(0) };
                let mut out = Settings::new();
                out.insert(setting_name, inner);
                Ok(out)
            }
            _ => Err(AgentError::UserCanceled("Cancelled".into())),
        }
    }

    async fn cancel_get_secrets(&self, connection_path: OwnedObjectPath, setting_name: String) {
        let mut shared = self.shared.lock().unwrap();
        shared.asks.retain(|a| {
            if a.connection == connection_path && a.setting == setting_name {
                let _ = a.reply.try_send(None);
                false
            } else {
                true
            }
        });
        shared.cancelled = true;
        drop(shared);
        self.wake.ping();
    }

    async fn save_secrets(&self, _connection: Settings, _connection_path: OwnedObjectPath) {}

    async fn delete_secrets(&self, _connection: Settings, _connection_path: OwnedObjectPath) {}
}

pub struct Wifi {
    shared: Arc<Mutex<Shared>>,
    _conn: Option<zbus::blocking::Connection>,
}

impl Wifi {
    pub fn new(wake: Ping) -> Wifi {
        let shared = Arc::new(Mutex::new(Shared::default()));
        let agent = Agent { shared: shared.clone(), wake };
        let conn = crate::bus::system().and_then(|c| crate::bus::serve(&c, &[], PATH, agent).map(|_| c));
        let conn = match conn {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("nm: no secret agent: {e}");
                return Wifi { shared, _conn: None };
            }
        };
        match conn.call_method(Some("org.freedesktop.NetworkManager"), "/org/freedesktop/NetworkManager/AgentManager", Some("org.freedesktop.NetworkManager.AgentManager"), "Register", &("org.item.SecretAgent")) {
            Ok(_) => tracing::info!("nm: the secret agent"),
            Err(e) => tracing::warn!("nm: not registered: {e}"),
        }
        Wifi { shared, _conn: Some(conn) }
    }

    /// The first ask waiting: the network, whether again.
    pub fn waiting(&self) -> Option<(String, bool)> {
        self.shared.lock().unwrap().asks.first().map(|a| (a.network.clone(), a.again))
    }

    /// The password for the first ask.
    pub fn answer(&self, secret: String) {
        let mut shared = self.shared.lock().unwrap();
        if !shared.asks.is_empty() {
            let a = shared.asks.remove(0);
            tracing::info!("nm: {} given", a.key);
            let _ = a.reply.try_send(Some(secret));
        }
    }

    /// The first ask dismissed (Cancel).
    pub fn dismiss(&self) {
        let mut shared = self.shared.lock().unwrap();
        if !shared.asks.is_empty() {
            let a = shared.asks.remove(0);
            let _ = a.reply.try_send(None);
            tracing::info!("nm: dismissed");
        }
    }

    /// Whether NetworkManager took an ask back since last asked.
    pub fn take_cancelled(&self) -> bool {
        std::mem::take(&mut self.shared.lock().unwrap().cancelled)
    }
}
