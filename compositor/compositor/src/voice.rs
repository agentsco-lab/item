//! The voice into a window: hold the keyboard's microphone key, speak,
//! let go, and the words are typed into the window the deck serves. The
//! sound comes from the port's microphone through PulseAudio (`parecord`,
//! 16 kHz mono) and goes, as a WAV, to a transcription service over the
//! web, with the key from the keyring (secrets.rs: `item-secret set
//! openrouter`) and the model from `~/.config/item/voice`; the text comes
//! back on a thread. No key, or no answer: the deck says so. A model on the
//! phone itself, for when there is no network, is the next step.
//!
//! `~/.config/item/voice`, a `key = value` a line, all optional:
//!   model = elevenlabs/scribe-v2        the service's model
//!   url = https://openrouter.ai/api/v1/audio/transcriptions
//!   secret = openrouter                 the key's name in the keyring
//!   source = source.primary_input       PulseAudio's source to record
//!   language = en

use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

use smithay::reexports::calloop::ping::Ping;

const RATE: u32 = 16_000;

#[derive(Clone, Debug)]
pub struct Config {
    pub model: String,
    pub url: String,
    pub secret: String,
    pub source: String,
    pub language: String,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            model: "elevenlabs/scribe-v2".into(),
            url: "https://openrouter.ai/api/v1/audio/transcriptions".into(),
            secret: "openrouter".into(),
            source: "source.primary_input".into(),
            language: "en".into(),
        }
    }
}

/// The settings as the file has them, the rest as they come.
pub fn config() -> Config {
    let mut c = Config::default();
    let path = format!("{}/.config/item/voice", std::env::var("HOME").unwrap_or_default());
    for line in std::fs::read_to_string(path).unwrap_or_default().lines() {
        let Some((k, v)) = line.split_once('=') else { continue };
        let v = v.trim().to_owned();
        if v.is_empty() {
            continue;
        }
        match k.trim() {
            "model" => c.model = v,
            "url" => c.url = v,
            "secret" => c.secret = v,
            "source" => c.source = v,
            "language" => c.language = v,
            _ => {}
        }
    }
    c
}

/// A recording under way: parecord's raw samples gathered on a thread.
pub struct Recording {
    child: Child,
    bytes: Arc<Mutex<Vec<u8>>>,
    started: std::time::Instant,
}

/// The microphone opened: 16 kHz, mono, 16 bit.
pub fn record(config: &Config) -> Result<Recording, String> {
    let mut child = Command::new("parecord")
        .args(["--raw", "--format=s16le", "--channels=1", &format!("--rate={RATE}"), "--latency-msec=40", &format!("--device={}", config.source)])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("parecord: {e}"))?;
    let mut out = child.stdout.take().ok_or("parecord: no output")?;
    let bytes = Arc::new(Mutex::new(Vec::new()));
    let sink = bytes.clone();
    std::thread::Builder::new()
        .name("voice record".into())
        .spawn(move || {
            let mut buf = [0u8; 4096];
            while let Ok(n) = out.read(&mut buf) {
                if n == 0 {
                    break;
                }
                sink.lock().unwrap().extend_from_slice(&buf[..n]);
            }
        })
        .map_err(|e| e.to_string())?;
    Ok(Recording { child, bytes, started: std::time::Instant::now() })
}

impl Recording {
    pub fn seconds(&self) -> f64 {
        self.started.elapsed().as_secs_f64()
    }

    /// The microphone closed; the samples so far.
    pub fn stop(mut self) -> Vec<u8> {
        let _ = self.child.kill();
        let _ = self.child.wait();
        // The reader ends as the pipe closes; what it had is taken.
        std::thread::sleep(std::time::Duration::from_millis(30));
        std::mem::take(&mut *self.bytes.lock().unwrap())
    }
}

/// The samples as a WAV file in memory.
fn wav(pcm: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(pcm.len() + 44);
    let data = pcm.len() as u32;
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&RATE.to_le_bytes());
    out.extend_from_slice(&(RATE * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data.to_le_bytes());
    out.extend_from_slice(pcm);
    out
}

/// The samples to the service, on a thread; the text (or what went wrong)
/// comes back on `done`, and `wake` wakes the loop for it.
pub fn transcribe(pcm: Vec<u8>, config: Config, done: Sender<Result<String, String>>, wake: Ping) {
    let _ = std::thread::Builder::new().name("voice transcribe".into()).spawn(move || {
        let result = send(&pcm, &config);
        match &result {
            Ok(t) => tracing::info!("voice: {:.1} s of sound, {} characters back", pcm.len() as f64 / (RATE * 2) as f64, t.len()),
            Err(e) => tracing::warn!("voice: {e}"),
        }
        let _ = done.send(result);
        wake.ping();
    });
}

fn send(pcm: &[u8], config: &Config) -> Result<String, String> {
    let key = crate::secrets::get(&config.secret)?.ok_or_else(|| format!("no key: item-secret set {}", config.secret))?;
    use base64::Engine;
    let audio = base64::engine::general_purpose::STANDARD.encode(wav(pcm));
    let body = serde_json::json!({ "model": config.model, "input_audio": { "data": audio, "format": "wav" }, "language": config.language }).to_string();
    // The body in a file (the phone has wget, not curl), on the runtime
    // dir's tmpfs, gone once sent.
    if !config.url.starts_with("https://") {
        return Err(format!("{}: the voice goes over https only", config.url));
    }
    let dir = std::env::var("XDG_RUNTIME_DIR").map_err(|_| "no XDG_RUNTIME_DIR: the voice needs the session's private directory".to_owned())?;
    let path = format!("{dir}/item-voice-{}.json", std::process::id());
    std::fs::write(&path, body).map_err(|e| format!("{path}: {e}"))?;
    // The key in wget's own configuration file, readable by the user alone
    // (0600, on the runtime dir's tmpfs, gone once sent): on the command
    // line it would be anyone's to read in /proc while the request runs.
    let rc = format!("{dir}/item-voice-{}.wgetrc", std::process::id());
    let written = {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&rc).and_then(|mut f| f.write_all(format!("header = Authorization: Bearer {key}\n").as_bytes()))
    };
    drop(key);
    if let Err(e) = written {
        let _ = std::fs::remove_file(&path);
        return Err(format!("{rc}: {e}"));
    }
    let out = Command::new("wget")
        .args(["-q", "-T", "60", "-O", "-", &format!("--config={rc}"), "--header", "Content-Type: application/json", &format!("--post-file={path}"), &config.url])
        .stdin(Stdio::null())
        .output();
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(&rc);
    let out = out.map_err(|e| format!("wget: {e}"))?;
    if !out.status.success() {
        return Err(format!("no answer from {} ({})", config.model, out.status));
    }
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).map_err(|e| format!("the answer: {e}"))?;
    match v["text"].as_str() {
        Some(t) => Ok(t.trim().to_owned()),
        None => Err(format!("the answer: {}", v["error"]["message"].as_str().unwrap_or("no text in it"))),
    }
}
