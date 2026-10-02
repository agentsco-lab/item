//! CV ID on the lock screen (item-tracker #109; an experiment, CVID=1): the
//! camera's peek (camera.rs) hands its frames here, a thread looks for a
//! face in them and compares it with the faces enrolled - crates/cvid,
//! YuNet and SFace on the CPU, ~125 ms a frame on the Duo - and the lock
//! opens on two frames in a row alike enough, as for a known finger.
//!
//! Enrolling: in the first setup, unseen, the face of the one typing the PIN
//! - the owner - is taken (ENROL_FRAMES frames) and kept once the PIN is
//! accepted (setup.rs), in ~/.config/item/cvid, readable by the user alone:
//! the numbers only, never a picture. By hand: with /tmp/item-cvid-enrol
//! there, the next peek keeps the face it sees, and the file goes. The
//! models are read from CVID_MODELS (/var/tmp/cvid by default).

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Instant;

use smithay::reexports::calloop::ping::Ping;

const W: usize = 640;
const H: usize = 480;
/// Alike enough: SFace's cosine, this or above, frames in a row.
const ALIKE: f32 = 0.42;
const IN_A_ROW: u32 = 2;
const ENROL_FRAMES: usize = 5;
/// Enough to keep, the PIN accepted before all are taken.
const ENROL_MIN: usize = 2;
const ENROL_FLAG: &str = "/tmp/item-cvid-enrol";

#[derive(Default)]
struct Shared {
    /// The newest frame not yet looked at (NV21).
    frame: Mutex<Option<Vec<u8>>>,
    ready: Condvar,
    /// Whether frames are wanted now.
    armed: AtomicBool,
    enrolling: AtomicBool,
    /// Kept as soon as taken (the flag file), not waiting for the PIN.
    keep_at_once: AtomicBool,
    /// The PIN accepted: keep what is taken, now or once it is all taken.
    keep: AtomicBool,
    matched: AtomicBool,
    /// The face taken for enrolling, all its frames.
    taken: AtomicBool,
    /// The faces enrolled, and the frames taken for enrolling.
    known: Mutex<Vec<[f32; 128]>>,
    pending: Mutex<Vec<[f32; 128]>>,
}

pub struct Face {
    shared: Arc<Shared>,
    on: bool,
}

/// Where the frames go from the camera's thread.
#[derive(Clone)]
pub struct Sink(Arc<Shared>);

impl Sink {
    /// A camera frame: taken when the thread is free for it.
    pub fn offer(&self, nv21: &[u8]) {
        if !self.0.armed.load(Ordering::Relaxed) {
            return;
        }
        let mut slot = self.0.frame.lock().unwrap();
        if slot.is_none() {
            *slot = Some(nv21.to_vec());
            self.0.ready.notify_one();
        }
    }
}

fn kept() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".config/item/cvid")
}

fn load_kept() -> Vec<[f32; 128]> {
    let data = std::fs::read(kept()).unwrap_or_default();
    data.chunks_exact(128 * 4)
        .map(|c| {
            let mut e = [0.0f32; 128];
            for (i, b) in c.chunks_exact(4).enumerate() {
                e[i] = f32::from_le_bytes([b[0], b[1], b[2], b[3]]);
            }
            e
        })
        .collect()
}

fn save_kept(faces: &[[f32; 128]]) {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let path = kept();
    if let Some(d) = path.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let bytes: Vec<u8> = faces.iter().flat_map(|e| e.iter().flat_map(|v| v.to_le_bytes())).collect();
    if let Ok(mut f) = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&path) {
        let _ = f.write_all(&bytes);
    }
}

/// A camera frame upright, as the Duo is held (a quarter turn
/// counter-clockwise, as camera.rs shows it), not mirrored: RGB.
fn upright(nv21: &[u8]) -> cvid::Image {
    let mut rgb = vec![0u8; W * H * 3];
    let uv = &nv21[W * H..];
    for oy in 0..W {
        for ox in 0..H {
            let (x, y) = (W - 1 - oy, ox);
            let lum = nv21[y * W + x] as f32;
            let i = (y / 2) * W + (x / 2) * 2;
            let (v, u) = (uv[i] as f32 - 128.0, uv[i + 1] as f32 - 128.0);
            let o = (oy * H + ox) * 3;
            rgb[o] = (lum + 1.402 * v).clamp(0.0, 255.0) as u8;
            rgb[o + 1] = (lum - 0.344 * u - 0.714 * v).clamp(0.0, 255.0) as u8;
            rgb[o + 2] = (lum + 1.772 * u).clamp(0.0, 255.0) as u8;
        }
    }
    cvid::Image { w: H, h: W, rgb }
}

impl Face {
    pub fn new(wake: Ping) -> Face {
        let shared: Arc<Shared> = Default::default();
        let on = std::env::var_os("CVID").is_some();
        if on {
            let s = shared.clone();
            let _ = std::thread::Builder::new().name("cvid".into()).spawn(move || run(s, wake));
        }
        Face { shared, on }
    }

    pub fn sink(&self) -> Option<Sink> {
        self.on.then(|| Sink(self.shared.clone()))
    }

    pub fn on(&self) -> bool {
        self.on
    }

    /// Each turn of the loop: frames wanted while the lock screen would
    /// take a face and the camera peeks, or to enrol (the setup's PIN).
    pub fn want(&self, on: bool, enrol: bool) {
        if !self.on {
            return;
        }
        let was = self.shared.armed.swap(on, Ordering::Relaxed);
        if on && !was {
            let by_hand = std::path::Path::new(ENROL_FLAG).exists();
            let enrol = enrol || by_hand;
            if enrol {
                self.shared.pending.lock().unwrap().clear();
                self.shared.keep_at_once.store(by_hand, Ordering::Relaxed);
            }
            self.shared.enrolling.store(enrol, Ordering::Relaxed);
            tracing::info!("cvid: looking{}", if enrol { " (enrolling)" } else { "" });
        }
    }

    /// The PIN accepted: the face taken as it was typed is the owner's.
    pub fn keep(&self) {
        if !self.on {
            return;
        }
        let n = self.shared.pending.lock().unwrap().len();
        if n >= ENROL_MIN {
            keep(&self.shared);
        } else {
            // Kept once enough is taken.
            self.shared.keep.store(true, Ordering::Relaxed);
        }
    }

    /// Whether the face for enrolling was all taken since last asked.
    pub fn take_taken(&self) -> bool {
        self.shared.taken.swap(false, Ordering::Relaxed)
    }

    /// Whether a known face was seen since last asked.
    pub fn take_matched(&self) -> bool {
        self.shared.matched.swap(false, Ordering::Relaxed)
    }
}

/// The frames taken for enrolling kept as the faces enrolled.
fn keep(s: &Shared) {
    let taken = std::mem::take(&mut *s.pending.lock().unwrap());
    if taken.is_empty() {
        return;
    }
    save_kept(&taken);
    tracing::info!("cvid: enrolled, {} frame(s)", taken.len());
    *s.known.lock().unwrap() = taken;
    s.keep.store(false, Ordering::Relaxed);
}

fn run(s: Arc<Shared>, wake: Ping) {
    let dir = PathBuf::from(std::env::var("CVID_MODELS").unwrap_or_else(|_| "/var/tmp/cvid".into()));
    let t = Instant::now();
    let det = cvid::Detector::new(&dir.join("face_detection_yunet_2023mar.onnx"), 256, 320);
    let emb = cvid::Embedder::new(&dir.join("face_recognition_sface_2021dec.onnx"));
    let (Ok(det), Ok(emb)) = (det, emb) else {
        tracing::warn!("cvid: the models did not load from {}", dir.display());
        return;
    };
    // The first run sets things up, slowly: done now, not with a face.
    let _ = det.detect(&cvid::Image { w: 240, h: 320, rgb: vec![0; 240 * 320 * 3] });
    *s.known.lock().unwrap() = load_kept();
    tracing::info!("cvid: ready in {} ms, {} face frame(s) enrolled", t.elapsed().as_millis(), s.known.lock().unwrap().len());
    let mut row = 0u32;
    loop {
        let frame = {
            let mut slot = s.frame.lock().unwrap();
            while slot.is_none() {
                slot = s.ready.wait(slot).unwrap();
            }
            slot.take().unwrap()
        };
        if !s.armed.load(Ordering::Relaxed) {
            row = 0;
            continue;
        }
        let t = Instant::now();
        let img = upright(&frame);
        let faces = match det.detect(&img) {
            Ok(f) => f,
            Err(e) => {
                tracing::warn!("cvid: {e}");
                continue;
            }
        };
        let td = t.elapsed().as_millis();
        let Some(face) = faces.first() else {
            row = 0;
            tracing::info!("cvid: no face ({td} ms)");
            continue;
        };
        let Ok(e) = emb.embed(&img, face) else { continue };
        let ms = t.elapsed().as_millis();
        if s.enrolling.load(Ordering::Relaxed) {
            let n = {
                let mut p = s.pending.lock().unwrap();
                p.push(e);
                p.len()
            };
            tracing::info!("cvid: taking a face, {n} of {ENROL_FRAMES} ({ms} ms)");
            if n >= ENROL_FRAMES {
                s.enrolling.store(false, Ordering::Relaxed);
                if s.keep_at_once.swap(false, Ordering::Relaxed) {
                    let _ = std::fs::remove_file(ENROL_FLAG);
                    keep(&s);
                } else if s.keep.load(Ordering::Relaxed) {
                    keep(&s);
                }
                s.taken.store(true, Ordering::Relaxed);
                wake.ping();
            }
            continue;
        }
        let best = s.known.lock().unwrap().iter().map(|k| cvid::cosine(k, &e)).fold(f32::MIN, f32::max);
        row = if best >= ALIKE { row + 1 } else { 0 };
        tracing::info!("cvid: face {:.2}, alike {best:.3}, {row} in a row (detect {td} ms, all {ms} ms)", face.score);
        if row >= IN_A_ROW {
            row = 0;
            s.matched.store(true, Ordering::Relaxed);
            wake.ping();
        }
    }
}
