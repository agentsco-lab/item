//! The keyring's prompts: the compositor is gnome-keyring's system prompter
//! (`org.gnome.keyring.SystemPrompter`, phosh's job when phosh runs), so
//! that gnome-keyring never opens gcr-prompter's window of its own
//! (docs/LOCK-AND-SETUP.md: only the shell asks for a secret).
//!
//! A prompt that comes while the phone is locked waits. The first PIN after
//! a boot opens the login keyring through PAM, and is kept until the prompts
//! waiting are answered: a password prompt then gets it (the PIN is the
//! keyring's password on the port), sent as gcr's secret exchange does
//! (`sx-aes-1`: Diffie-Hellman in the 1536 bit IKE group, the key from
//! HKDF-SHA256, AES-128-CBC). A prompt that comes while unlocked, or one the
//! PIN did not answer, is declined for now: the compositor's own dialog for
//! them is the next step. The PIN is never logged, and dropped once used.
//!
//! The protocol, gcr's (gcr-system-prompter.c): the caller asks
//! `BeginPrompting(callback)`; the prompter calls back `PromptReady("", {},
//! its exchange)`; the caller asks `PerformPrompt(callback, type,
//! properties, its exchange)`; the prompter answers `PromptReady(reply,
//! {}, exchange with the secret)`; `StopPrompting` ends it with `PromptDone`.

use std::collections::HashMap;
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};

use base64::Engine;
use num_bigint::BigUint;
use smithay::reexports::calloop::ping::Ping;
use zbus::zvariant::{OwnedObjectPath, OwnedValue};

const NAME: &str = "org.gnome.keyring.SystemPrompter";
const PATH: &str = "/org/gnome/keyring/Prompter";
const CALLBACK: &str = "org.gnome.keyring.internal.Prompter.Callback";
const PROTOCOL: &str = "sx-aes-1";

/// RFC 3526's 1536 bit MODP group (gcr's "ietf-ike-grp-modp-1536"), g = 2.
const PRIME: &str = "FFFFFFFFFFFFFFFFC90FDAA22168C234C4C6628B80DC1CD129024E088A67CC74020BBEA63B139B22514A08798E3404DDEF9519B3CD3A431B302B0A6DF25F14374FE1356D6D51C245E485B576625E7EC6F44C42E9A637ED6B0BFF5CB6F406B7EDEE386BFB5A899FA5AE9F24117C4B1FE649286651ECE45B3DC2007CB8A163BF0598DA48361C55D39A69163FA8FD24CF5F83655D23DCA3AD961C62F356208552BB9ED529077096966D670C354E4ABC9804F1746C08CA237327FFFFFFFFFFFFFFFF";

fn random(n: usize) -> Vec<u8> {
    let mut buf = vec![0u8; n];
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        let _ = std::io::Read::read_exact(&mut f, &mut buf);
    }
    buf
}

/// One side of gcr's secret exchange.
struct Exchange {
    private: BigUint,
    public: BigUint,
    key: Option<[u8; 16]>,
}

impl Exchange {
    fn new() -> Exchange {
        let p = BigUint::parse_bytes(PRIME.as_bytes(), 16).unwrap();
        let private = BigUint::from_bytes_be(&random(1536 / 8)) % (&p - 2u32) + 1u32;
        let public = BigUint::from(2u32).modpow(&private, &p);
        Exchange { private, public, key: None }
    }

    fn b64(bytes: &[u8]) -> String {
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    /// The exchange as gcr writes it (a key file), our public key in it, and
    /// the secret if there is one.
    fn send(&self, secret: Option<&[u8]>) -> String {
        let mut out = format!("[{PROTOCOL}]\npublic={}\n", Self::b64(&self.public.to_bytes_be()));
        if let (Some(secret), Some(key)) = (secret, &self.key) {
            use aes::cipher::{BlockEncryptMut, KeyIvInit};
            let iv = random(16);
            let cipher = cbc::Encryptor::<aes::Aes128>::new(key.into(), iv.as_slice().into());
            let sealed = cipher.encrypt_padded_vec_mut::<aes::cipher::block_padding::Pkcs7>(secret);
            out.push_str(&format!("secret={}\niv={}\n", Self::b64(&sealed), Self::b64(&iv)));
        }
        out
    }

    /// The caller's exchange: its public key gives the transport key.
    fn receive(&mut self, text: &str) -> bool {
        let Some(public) = text.lines().find_map(|l| l.trim().strip_prefix("public=")) else { return false };
        let Ok(peer) = base64::engine::general_purpose::STANDARD.decode(public.trim()) else { return false };
        let p = BigUint::parse_bytes(PRIME.as_bytes(), 16).unwrap();
        let shared = BigUint::from_bytes_be(&peer).modpow(&self.private, &p).to_bytes_be();
        // As long as the prime, zeros in front (egg-dh-libgcrypt.c).
        let mut ikm = vec![0u8; 1536 / 8 - shared.len().min(1536 / 8)];
        ikm.extend_from_slice(&shared);
        let mut key = [0u8; 16];
        if hkdf::Hkdf::<sha2::Sha256>::new(None, &ikm).expand(&[], &mut key).is_err() {
            return false;
        }
        self.key = Some(key);
        true
    }
}

/// A prompt between `BeginPrompting` and `StopPrompting`.
struct Prompt {
    sender: String,
    callback: OwnedObjectPath,
    exchange: Exchange,
    /// What it asks, once performed ("password" or "confirm"), and whether
    /// the PIN was given to it already.
    kind: Option<String>,
    tried_pin: bool,
}

#[derive(Default)]
struct Shared {
    prompts: Vec<Prompt>,
    /// The PIN that unlocked, and when: kept 15 s for the prompts waiting
    /// and any coming just after.
    pin: Option<(String, std::time::Instant)>,
}

/// Calls back to a prompt's caller, made on a thread of their own.
enum Call {
    Ready { sender: String, callback: OwnedObjectPath, reply: &'static str, exchange: String },
    Done { sender: String, callback: OwnedObjectPath },
}

struct Prompter {
    shared: Arc<Mutex<Shared>>,
    calls: Sender<Call>,
    wake: Ping,
}

#[zbus::interface(name = "org.gnome.keyring.internal.Prompter")]
impl Prompter {
    fn begin_prompting(&self, #[zbus(header)] header: zbus::message::Header<'_>, callback: OwnedObjectPath) {
        let Some(sender) = header.sender().map(|s| s.to_string()) else { return };
        let exchange = Exchange::new();
        let first = exchange.send(None);
        self.shared.lock().unwrap().prompts.push(Prompt { sender: sender.clone(), callback: callback.clone(), exchange, kind: None, tried_pin: false });
        let _ = self.calls.send(Call::Ready { sender, callback, reply: "", exchange: first });
    }

    fn perform_prompt(
        &self,
        #[zbus(header)] header: zbus::message::Header<'_>,
        callback: OwnedObjectPath,
        kind: String,
        properties: HashMap<String, OwnedValue>,
        exchange: String,
    ) -> zbus::fdo::Result<()> {
        let sender = header.sender().map(|s| s.to_string()).unwrap_or_default();
        let mut shared = self.shared.lock().unwrap();
        let Some(prompt) = shared.prompts.iter_mut().find(|p| p.sender == sender && p.callback == callback) else {
            return Err(zbus::fdo::Error::Failed("Not begun prompting for this prompt callback".into()));
        };
        if !prompt.exchange.receive(&exchange) {
            return Err(zbus::fdo::Error::InvalidArgs("Invalid secret exchange received".into()));
        }
        let title = properties.get("title").and_then(|v| String::try_from(v.try_clone().ok()?).ok()).unwrap_or_default();
        tracing::info!("keyring: a {kind} prompt from {sender}: {title}");
        prompt.kind = Some(kind);
        drop(shared);
        self.wake.ping();
        Ok(())
    }

    fn stop_prompting(&self, #[zbus(header)] header: zbus::message::Header<'_>, callback: OwnedObjectPath) {
        let sender = header.sender().map(|s| s.to_string()).unwrap_or_default();
        self.shared.lock().unwrap().prompts.retain(|p| !(p.sender == sender && p.callback == callback));
        let _ = self.calls.send(Call::Done { sender, callback });
    }
}

pub struct Keyring {
    shared: Arc<Mutex<Shared>>,
    calls: Option<Sender<Call>>,
}

impl Keyring {
    pub fn new(wake: Ping) -> Keyring {
        let shared = Arc::new(Mutex::new(Shared::default()));
        let (tx, rx) = channel::<Call>();
        let prompter = Prompter { shared: shared.clone(), calls: tx.clone(), wake };
        let conn = zbus::blocking::connection::Builder::session()
            .and_then(|b| b.name(NAME))
            .and_then(|b| b.serve_at(PATH, prompter))
            .and_then(|b| b.build());
        let conn = match conn {
            Ok(c) => {
                tracing::info!("keyring: the system prompter");
                c
            }
            Err(e) => {
                tracing::warn!("keyring: not the system prompter: {e}");
                return Keyring { shared, calls: None };
            }
        };
        std::thread::spawn(move || {
            for call in rx {
                let empty: HashMap<&str, zbus::zvariant::Value> = HashMap::new();
                let result = match &call {
                    Call::Ready { sender, callback, reply, exchange } => {
                        conn.call_method(Some(sender.as_str()), callback.as_str(), Some(CALLBACK), "PromptReady", &(*reply, empty, exchange.as_str())).map(|_| ())
                    }
                    Call::Done { sender, callback } => conn.call_method(Some(sender.as_str()), callback.as_str(), Some(CALLBACK), "PromptDone", &()).map(|_| ()),
                };
                // The caller may have gone already after StopPrompting.
                if let Err(e) = result {
                    tracing::debug!("keyring: calling back: {e}");
                }
            }
        });
        Keyring { shared, calls: Some(tx) }
    }

    /// The PIN that has just unlocked the phone, for the prompts waiting.
    pub fn unlocked_with(&self, pin: String) {
        self.shared.lock().unwrap().pin = Some((pin, std::time::Instant::now()));
    }

    /// Answers what can be answered: nothing while locked; after an unlock a
    /// password prompt gets the PIN once, and what is left is declined.
    pub fn service(&self, locked: bool) {
        let Some(calls) = &self.calls else { return };
        if locked {
            return;
        }
        let mut shared = self.shared.lock().unwrap();
        let pin = shared.pin.as_ref().map(|(p, _)| p.clone());
        for prompt in shared.prompts.iter_mut() {
            let Some(kind) = prompt.kind.take() else { continue };
            let (reply, exchange) = match (kind.as_str(), &pin) {
                ("password", Some(pin)) if !prompt.tried_pin => {
                    prompt.tried_pin = true;
                    tracing::info!("keyring: answered with the PIN that unlocked");
                    ("yes", prompt.exchange.send(Some(pin.as_bytes())))
                }
                _ => {
                    tracing::info!("keyring: a {kind} prompt declined");
                    ("no", prompt.exchange.send(None))
                }
            };
            let _ = calls.send(Call::Ready { sender: prompt.sender.clone(), callback: prompt.callback.clone(), reply, exchange });
        }
        // The PIN's time is up: dropped, zeroed.
        if shared.pin.as_ref().is_some_and(|(_, at)| at.elapsed() > std::time::Duration::from_secs(15)) {
            if let Some((mut pin, _)) = shared.pin.take() {
                unsafe { pin.as_bytes_mut().fill(0) };
            }
        }
        if let Some(mut pin) = pin {
            unsafe { pin.as_bytes_mut().fill(0) };
        }
    }
}
