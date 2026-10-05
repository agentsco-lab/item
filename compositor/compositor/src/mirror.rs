//! The screen, mirrored to whoever connects to
//! $XDG_RUNTIME_DIR/item-mirror.sock - Gridbay's live view of the phone. Only
//! the owner can (the socket is theirs, 0600), and it costs nothing while no
//! one is connected.
//!
//! After a drawn frame, at most MAX_FPS times a second, the frame is shrunk
//! by the GPU to a quarter (output.rs) and read; a thread compresses it (lz4)
//! and writes it: b"IMF1", width, height, raw length, compressed length (u32
//! LE each), the compressed RGBA, rows bottom-up as GL keeps them. A frame
//! the socket is not ready for is dropped, never queued. Nothing changing,
//! nothing is drawn and nothing sent; a change held back by the rate is sent
//! when its time comes, and one who connects gets a frame at once.

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{sync_channel, SyncSender};
use std::sync::{Arc, Mutex};

/// The mirror's size: a quarter of the output's.
pub const SHRINK: i32 = 4;
const MAX_FPS: u64 = 10;

pub struct Mirror {
    /// Someone is connected.
    watched: Arc<AtomicBool>,
    /// Someone just connected: a frame now, changed or not.
    fresh: Arc<AtomicBool>,
    /// A change was held back by the rate: a frame when its time comes.
    owed: bool,
    last_ns: u64,
    frames: Option<SyncSender<(i32, i32, Vec<u8>)>>,
}

impl Mirror {
    pub fn new() -> Mirror {
        let watched: Arc<AtomicBool> = Default::default();
        let fresh: Arc<AtomicBool> = Default::default();
        let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR") else {
            return Mirror { watched, fresh, owed: false, last_ns: 0, frames: None };
        };
        let path = std::path::Path::new(&dir).join("item-mirror.sock");
        let _ = std::fs::remove_file(&path);
        let listener = match UnixListener::bind(&path) {
            Ok(l) => l,
            Err(e) => {
                tracing::warn!("mirror: {}: {e}", path.display());
                return Mirror { watched, fresh, owed: false, last_ns: 0, frames: None };
            }
        };
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
        let client: Arc<Mutex<Option<UnixStream>>> = Default::default();
        // Who is watching: the latest to connect.
        let (c, w, f) = (client.clone(), watched.clone(), fresh.clone());
        let _ = std::thread::Builder::new().name("mirror-accept".into()).spawn(move || {
            for stream in listener.incoming().flatten() {
                tracing::info!("mirror: watched");
                *c.lock().unwrap() = Some(stream);
                w.store(true, Ordering::Relaxed);
                f.store(true, Ordering::Relaxed);
            }
        });
        // The frames out: compressed and written, one at a time.
        let (tx, rx) = sync_channel::<(i32, i32, Vec<u8>)>(1);
        let w = watched.clone();
        let _ = std::thread::Builder::new().name("mirror-send".into()).spawn(move || {
            for (width, height, rgba) in rx {
                let packed = lz4_flex::block::compress(&rgba);
                let mut head = Vec::with_capacity(20);
                head.extend_from_slice(b"IMF1");
                for v in [width as u32, height as u32, rgba.len() as u32, packed.len() as u32] {
                    head.extend_from_slice(&v.to_le_bytes());
                }
                let mut guard = client.lock().unwrap();
                let Some(stream) = guard.as_mut() else { continue };
                if stream.write_all(&head).and_then(|_| stream.write_all(&packed)).is_err() {
                    tracing::info!("mirror: no longer watched");
                    *guard = None;
                    w.store(false, Ordering::Relaxed);
                }
            }
        });
        tracing::info!("mirror: at {}", path.display());
        Mirror { watched, fresh, owed: false, last_ns: 0, frames: Some(tx) }
    }

    /// Whether a frame is to be mirrored now (one was drawn): watched, and
    /// its time come; held back otherwise, owed.
    pub fn due(&mut self, now_ns: u64) -> bool {
        if !self.watched.load(Ordering::Relaxed) {
            self.owed = false;
            return false;
        }
        self.fresh.store(false, Ordering::Relaxed);
        if now_ns.saturating_sub(self.last_ns) < 1_000_000_000 / MAX_FPS {
            self.owed = true;
            return false;
        }
        self.owed = false;
        self.last_ns = now_ns;
        true
    }

    /// Whether a frame is wanted though nothing changed: someone just
    /// connected, or a change held back is now due.
    pub fn wants_frame(&self, now_ns: u64) -> bool {
        self.watched.load(Ordering::Relaxed) && (self.fresh.load(Ordering::Relaxed) || (self.owed && now_ns.saturating_sub(self.last_ns) >= 1_000_000_000 / MAX_FPS))
    }

    /// A frame read back (`width` x `height`, RGBA bottom-up), to be sent if
    /// the socket is ready.
    pub fn send(&self, width: i32, height: i32, rgba: Vec<u8>) {
        if let Some(tx) = &self.frames {
            let _ = tx.try_send((width, height, rgba));
        }
    }
}
