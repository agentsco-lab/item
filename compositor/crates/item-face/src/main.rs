//! item-face: CV ID's service (item-tracker #112), as droidian-fpd is the
//! fingerprint's - its own process, as root, on the system bus as
//! org.sfduo.Face. It holds the camera (camera.rs), the models (crates/cvid)
//! and the faces enrolled, in /var/lib/item-face/<uid>: 128 numbers a frame,
//! never a picture, readable by root alone. Nothing of the camera leaves it;
//! its callers hear only whether a face was known.
//!
//! Methods (each for the calling user):
//! - Open, Close: the camera held open and warm (while the phone is locked),
//!   or let go for the apps.
//! - Identify: the camera on for up to IDENTIFY_FOR; Identified(uid, alike)
//!   on a known face, live, two frames in a row; else NotIdentified(uid).
//! - Enrol: frames of the face taken (Taken(uid, n) once ENROL_FRAMES are),
//!   not kept until Keep(pin) - the PIN checked by PAM for the caller's
//!   user, so no app can put its own face in the owner's place. Kept(uid,
//!   n) when kept.
//! - Cancel; Remove; Enrolled -> how many frames are kept.

mod camera;
#[path = "../../../compositor/src/pam.rs"]
mod pam;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{sync_channel, Receiver, RecvTimeoutError, SyncSender};
use std::time::{Duration, Instant};

use zbus::blocking::Connection;

const NAME: &str = "org.sfduo.Face";
const PATH: &str = "/org/sfduo/Face";
const STORE: &str = "/var/lib/item-face";

/// Alike enough: SFace's cosine, this or above, frames in a row, live.
const ALIKE: f32 = 0.42;
const IN_A_ROW: u32 = 2;
const LIVE_MIN: f32 = 0.5;
const IDENTIFY_FOR: Duration = Duration::from_secs(6);
const ENROL_FOR: Duration = Duration::from_secs(60);
const ENROL_FRAMES: usize = 5;
/// Enough to keep when the PIN comes before all are taken.
const ENROL_MIN: usize = 2;
/// No frame this long after a start: the camera closed and started again.
const RETRY: Duration = Duration::from_millis(2500);

enum Msg {
    Open,
    Close,
    Identify(u32),
    Enrol(u32),
    Cancel,
    /// The PIN accepted for this user: keep what is taken.
    Keep(u32),
    Remove(u32),
    Frame(u64, Vec<u8>),
}

fn frame(play: u64, data: Vec<u8>) -> Msg {
    Msg::Frame(play, data)
}

fn store(uid: u32) -> PathBuf {
    PathBuf::from(STORE).join(uid.to_string())
}

fn load(uid: u32) -> Vec<[f32; 128]> {
    let data = std::fs::read(store(uid)).unwrap_or_default();
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

fn save(uid: u32, faces: &[[f32; 128]]) {
    use std::io::Write;
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    let _ = std::fs::DirBuilder::new().recursive(true).mode(0o700).create(STORE);
    let bytes: Vec<u8> = faces.iter().flat_map(|e| e.iter().flat_map(|v| v.to_le_bytes())).collect();
    let tmp = store(uid).with_extension("new");
    if let Ok(mut f) = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp) {
        if f.write_all(&bytes).is_ok() {
            let _ = std::fs::rename(&tmp, store(uid));
        }
    }
}

enum Mode {
    Idle,
    Identify { uid: u32, known: Vec<[f32; 128]>, row: u32 },
    Enrol { uid: u32 },
}

struct Worker {
    camera: camera::Camera,
    bus: Connection,
    det: cvid::Detector,
    emb: cvid::Embedder,
    live: Option<cvid::Liveness>,
    mode: Mode,
    /// The play looked at, since when, when it was last (re)started, and
    /// whether a frame of it came.
    play: u64,
    until: Instant,
    tried: Instant,
    seen: bool,
    /// Frames looked at in this look, faces found in them, when it began.
    frames: u32,
    faces: u32,
    began: Instant,
    held: bool,
    pending: HashMap<u32, Vec<[f32; 128]>>,
    keep_when_taken: HashMap<u32, bool>,
}

impl Worker {
    fn signal<B>(&self, name: &str, body: &B)
    where
        B: zbus::export::serde::Serialize + zbus::zvariant::DynamicType,
    {
        if let Err(e) = self.bus.emit_signal(None::<&str>, PATH, NAME, name, body) {
            tracing::warn!("face: {name}: {e}");
        }
    }

    fn start(&mut self, mode: Mode, for_: Duration) {
        self.play = self.camera.play();
        self.mode = mode;
        self.until = Instant::now() + for_;
        self.tried = Instant::now();
        self.seen = false;
        (self.frames, self.faces, self.began) = (0, 0, Instant::now());
    }

    fn stop(&mut self) {
        if !matches!(self.mode, Mode::Idle) {
            self.mode = Mode::Idle;
            if self.held {
                self.camera.stop();
            } else {
                self.camera.close();
            }
        }
    }

    fn keep(&mut self, uid: u32) -> bool {
        let taken = self.pending.remove(&uid).unwrap_or_default();
        if taken.len() < ENROL_MIN {
            self.pending.insert(uid, taken);
            return false;
        }
        save(uid, &taken);
        tracing::info!("face: {uid}: enrolled, {} frame(s)", taken.len());
        self.signal("Kept", &(uid, taken.len() as u32));
        true
    }

    fn msg(&mut self, m: Msg) {
        match m {
            Msg::Open => {
                self.held = true;
                if matches!(self.mode, Mode::Idle) {
                    self.camera.open();
                }
            }
            Msg::Close => {
                self.held = false;
                if matches!(self.mode, Mode::Idle) {
                    self.camera.close();
                }
            }
            Msg::Identify(uid) => {
                let known = load(uid);
                if known.is_empty() {
                    self.signal("NotIdentified", &(uid,));
                    return;
                }
                tracing::info!("face: {uid}: identifying");
                self.start(Mode::Identify { uid, known, row: 0 }, IDENTIFY_FOR);
            }
            Msg::Enrol(uid) => {
                tracing::info!("face: {uid}: enrolling");
                self.pending.insert(uid, Vec::new());
                self.keep_when_taken.remove(&uid);
                self.start(Mode::Enrol { uid }, ENROL_FOR);
            }
            Msg::Cancel => self.stop(),
            Msg::Keep(uid) => {
                if !self.keep(uid) {
                    // Kept once enough is taken.
                    self.keep_when_taken.insert(uid, true);
                }
            }
            Msg::Remove(uid) => {
                let _ = std::fs::remove_file(store(uid));
                tracing::info!("face: {uid}: removed");
            }
            Msg::Frame(play, data) => {
                if play == self.play && !matches!(self.mode, Mode::Idle) {
                    if !self.seen {
                        tracing::info!("face: the first frame {} ms after the ask", self.began.elapsed().as_millis());
                    }
                    self.seen = true;
                    self.frames += 1;
                    self.look(&data);
                }
            }
        }
    }

    fn look(&mut self, nv21: &[u8]) {
        let t = Instant::now();
        let img = camera::upright(nv21);
        let Ok(faces) = self.det.detect(&img) else { return };
        let Some(face) = faces.first() else {
            if let Mode::Identify { row, .. } = &mut self.mode {
                *row = 0;
            }
            return;
        };
        self.faces += 1;
        let Ok(e) = self.emb.embed(&img, face) else { return };
        match &mut self.mode {
            Mode::Idle => {}
            Mode::Enrol { uid } => {
                let uid = *uid;
                let taken = self.pending.entry(uid).or_default();
                taken.push(e);
                let n = taken.len();
                tracing::info!("face: {uid}: taken {n} of {ENROL_FRAMES} ({} ms)", t.elapsed().as_millis());
                if n >= ENROL_FRAMES {
                    self.stop();
                    self.signal("Taken", &(uid, n as u32));
                    if self.keep_when_taken.remove(&uid).is_some() {
                        self.keep(uid);
                    }
                }
            }
            Mode::Identify { uid, known, row } => {
                let uid = *uid;
                let best = known.iter().map(|k| cvid::cosine(k, &e)).fold(f32::MIN, f32::max);
                let alive = self.live.as_ref().map_or(Ok(1.0), |l| l.live(&img, face)).unwrap_or(0.0);
                *row = if best >= ALIKE && alive >= LIVE_MIN { *row + 1 } else { 0 };
                tracing::info!("face: {uid}: alike {best:.3}, live {alive:.3}, {row} in a row ({} ms)", t.elapsed().as_millis());
                if *row >= IN_A_ROW {
                    tracing::info!("face: {uid}: identified {} ms after the ask", self.began.elapsed().as_millis());
                    self.stop();
                    self.signal("Identified", &(uid, best as f64));
                }
            }
        }
    }

    /// Time up, or no frame: the camera off, or started again.
    fn tick(&mut self) {
        if matches!(self.mode, Mode::Idle) {
            return;
        }
        let now = Instant::now();
        if now >= self.until {
            let uid = match &self.mode {
                Mode::Identify { uid, .. } => Some(*uid),
                _ => None,
            };
            self.stop();
            if let Some(uid) = uid {
                tracing::info!("face: {uid}: not identified ({} frames, a face in {})", self.frames, self.faces);
                self.signal("NotIdentified", &(uid,));
            }
            return;
        }
        if !self.seen && now >= self.tried + RETRY {
            tracing::warn!("face: no frame in {} ms: the camera started again", (now - self.tried).as_millis());
            self.play = self.camera.restart();
            self.tried = now;
        }
    }
}

fn run(rx: Receiver<Msg>, tx: SyncSender<Msg>, bus: Connection) {
    let dir = PathBuf::from(std::env::var("CVID_MODELS").unwrap_or_else(|_| "/var/tmp/cvid".into()));
    let t = Instant::now();
    let det = cvid::Detector::new(&dir.join("face_detection_yunet_2023mar.onnx"), 256, 320);
    let emb = cvid::Embedder::new(&dir.join("face_recognition_sface_2021dec.onnx"));
    let (Ok(det), Ok(emb)) = (det, emb) else {
        tracing::error!("face: the models did not load from {}", dir.display());
        std::process::exit(1);
    };
    let live = cvid::Liveness::new(&dir.join("MiniFASNetV2.onnx")).ok();
    if live.is_none() {
        tracing::warn!("face: no liveness check: faces are not told from pictures");
    }
    // The first run sets things up, slowly: done now, not with a face.
    let _ = det.detect(&cvid::Image { w: 240, h: 320, rgb: vec![0; 240 * 320 * 3] });
    tracing::info!("face: models ready in {} ms", t.elapsed().as_millis());
    let camera = camera::Camera::new(tx, frame);
    let now = Instant::now();
    let mut w = Worker { camera, bus, det, emb, live, mode: Mode::Idle, play: 0, until: now, tried: now, seen: false, held: false, frames: 0, faces: 0, began: now, pending: HashMap::new(), keep_when_taken: HashMap::new() };
    loop {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(m) => w.msg(m),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        w.tick();
    }
}

struct Face {
    tx: SyncSender<Msg>,
}

/// The calling user's uid.
async fn caller(hdr: &zbus::message::Header<'_>, conn: &zbus::Connection) -> zbus::fdo::Result<u32> {
    let sender = hdr.sender().ok_or_else(|| zbus::fdo::Error::AccessDenied("no sender".into()))?;
    let dbus = zbus::fdo::DBusProxy::new(conn).await?;
    Ok(dbus.get_connection_unix_user(sender.clone().into()).await?)
}

#[zbus::interface(name = "org.sfduo.Face")]
impl Face {
    fn open(&self) {
        let _ = self.tx.send(Msg::Open);
    }

    fn close(&self) {
        let _ = self.tx.send(Msg::Close);
    }

    async fn identify(&self, #[zbus(header)] hdr: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection) -> zbus::fdo::Result<()> {
        let uid = caller(&hdr, conn).await?;
        let _ = self.tx.send(Msg::Identify(uid));
        Ok(())
    }

    async fn enrol(&self, #[zbus(header)] hdr: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection) -> zbus::fdo::Result<()> {
        let uid = caller(&hdr, conn).await?;
        let _ = self.tx.send(Msg::Enrol(uid));
        Ok(())
    }

    fn cancel(&self) {
        let _ = self.tx.send(Msg::Cancel);
    }

    /// The PIN, checked for the caller's user: what is taken is kept.
    async fn keep(&self, pin: String, #[zbus(header)] hdr: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection) -> zbus::fdo::Result<bool> {
        let uid = caller(&hdr, conn).await?;
        let user = unsafe {
            let pw = libc::getpwuid(uid);
            if pw.is_null() {
                return Ok(false);
            }
            std::ffi::CStr::from_ptr((*pw).pw_name).to_string_lossy().into_owned()
        };
        // PAM waits ~2 s after a wrong PIN: on a thread of its own.
        let (done, answer) = async_channel::bounded(1);
        std::thread::spawn(move || {
            let mut pin = pin;
            let ok = pam::check_for(&user, &pin);
            // SAFETY: zeroing the bytes keeps the String valid UTF-8.
            unsafe { std::ptr::write_bytes(pin.as_mut_ptr(), 0, pin.len()) };
            let _ = done.send_blocking(ok);
        });
        let ok = answer.recv().await.unwrap_or(false);
        if ok {
            let _ = self.tx.send(Msg::Keep(uid));
        }
        Ok(ok)
    }

    async fn remove(&self, #[zbus(header)] hdr: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection) -> zbus::fdo::Result<()> {
        let uid = caller(&hdr, conn).await?;
        let _ = self.tx.send(Msg::Remove(uid));
        Ok(())
    }

    async fn enrolled(&self, #[zbus(header)] hdr: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection) -> zbus::fdo::Result<u32> {
        let uid = caller(&hdr, conn).await?;
        Ok(load(uid).len() as u32)
    }
}

fn main() {
    tracing_subscriber::fmt().with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into())).init();
    let (tx, rx) = sync_channel::<Msg>(2);
    let bus = zbus::blocking::connection::Builder::system()
        .and_then(|b| b.name(NAME))
        .and_then(|b| b.serve_at(PATH, Face { tx: tx.clone() }))
        .and_then(|b| b.build());
    let bus = match bus {
        Ok(b) => b,
        Err(e) => {
            tracing::error!("face: the system bus: {e}");
            std::process::exit(1);
        }
    };
    tracing::info!("face: on the system bus as {NAME}");
    run(rx, tx, bus);
}
