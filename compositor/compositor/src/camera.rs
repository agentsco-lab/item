//! A peek through the camera on the lock screen (an experiment, CAMERA_PEEK=1;
//! item-tracker #105, #107, #109): the screen lit while locked - the lid
//! opened, the power key - and a small window on the right panel shows what
//! the camera sees, for a few seconds. How the picture is turned and whether
//! mirrored is read at each start from /tmp/item-camera (words: cw, ccw,
//! none; mirror), to find the right way on the device.
//!
//! With CV ID on (CVID=1) the camera is item-face's (face.rs): no peek.
//!
//! The frames come through Droidian's camera stack as droidian-camera's do:
//! GStreamer's droidcamsrc (droidmedia, Android's Camera1), 640x480 NV21,
//! from tools/camera-warm.c - a helper kept running with the stack loaded,
//! the camera held open while the phone is locked, so a peek only starts
//! the preview (CAMERA_WARM names it; built on the phone for the
//! experiment). A thread here takes each frame to grey RGBA at half the
//! size - black and white, the luma alone. It starts with the lid opening,
//! ahead of the screen lighting, and the window's dark pane shows at once,
//! the picture coming up in it. Measured 2026-10-02: the first frame ~0.29 s
//! after the start this way, against ~1.1 s with a gst-launch started for
//! each peek; then ~16 a second.

use std::io::{Read, Write};
use std::process::{ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex};

use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement};
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::reexports::calloop::ping::Ping;

use crate::layout::{self, SCALE};
use crate::shade::ShellElement;

const W: usize = 640;
const H: usize = 480;
/// Shown this long after the screen lit.
const FOR_NS: u64 = 6_000_000_000;
const FADE_NS: f64 = 250e6;
/// No frame this long after a start: the camera closed and started again
/// (Android's camera service can fall over on an open and come back).
const RETRY_NS: u64 = 2_500_000_000;
/// The pane before the picture: in this fast, breathing this slowly.
const PANE_IN_NS: f64 = 120e6;
const BREATH_NS: f64 = 900e6;
/// The window's corners (px of the half-size frame).
const RADIUS: f64 = 18.0;

/// What the helper writes before each start's frames (camera-warm.c).
const MARK: &[u8; 16] = b"camera-warm:play";

#[derive(Default)]
struct Frame {
    rgba: Vec<u8>,
    seq: u64,
    /// Which start the frame is of: the marks seen so far.
    play: u64,
}

pub struct Peek {
    /// The helper's orders; None without the peek or when it is gone.
    orders: Option<ChildStdin>,
    /// Whether the camera is held open; whether it is playing.
    open: bool,
    playing: bool,
    /// Starts so far.
    play: u64,
    turn: std::sync::Arc<std::sync::atomic::AtomicU8>,
    since: u64,
    /// How long this start runs; when it was last tried.
    for_ns: u64,
    tried: u64,
    /// Whether the window is drawn (CAMERA_PEEK without CVID).
    shown: bool,
    /// When the first frame came.
    first: Option<u64>,
    frame: Arc<Mutex<Frame>>,
    drawn: std::cell::RefCell<(u64, Option<MemoryRenderBuffer>)>,
    /// The dark pane, for the accent's version and the window's size.
    pane: std::cell::RefCell<Option<(u64, (usize, usize), MemoryRenderBuffer)>>,
    wake: Ping,
    /// The picture's size as turned (px, logical at scale 1).
    size: (usize, usize),
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Turn {
    None = 0,
    Cw = 1,
    Ccw = 2,
}

impl Peek {
    pub fn new(wake: Ping) -> Peek {
        let mut peek = Peek { orders: None, open: false, playing: false, play: 0, turn: Default::default(), since: 0, for_ns: FOR_NS, tried: 0, shown: false, first: None, frame: Default::default(), drawn: Default::default(), pane: Default::default(), wake, size: (W / 2, H / 2) };
        // With CV ID on, the camera is item-face's (face.rs).
        if std::env::var_os("CAMERA_PEEK").is_some() && std::env::var_os("CVID").is_none() {
            peek.shown = true;
            peek.spawn();
        }
        peek
    }

    /// The helper started, loading the camera stack while nothing waits.
    fn spawn(&mut self) {
        let tool = std::env::var("CAMERA_WARM").unwrap_or_else(|_| "/var/tmp/gst/camera-warm".into());
        let log = std::fs::File::create("/tmp/camera-warm.log").map(Stdio::from).unwrap_or_else(|_| Stdio::null());
        let child = Command::new(&tool).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(log).spawn();
        let Ok(mut child) = child else {
            tracing::warn!("camera: {tool} did not start");
            return;
        };
        let mut out = child.stdout.take().expect("piped");
        self.orders = child.stdin.take();
        let (frame, wake, turn) = (self.frame.clone(), self.wake.clone(), self.turn.clone());
        let _ = std::thread::Builder::new().name("camera".into()).spawn(move || {
            let n = W * H * 3 / 2;
            let (mut buf, mut chunk) = (Vec::with_capacity(2 * n), vec![0u8; 1 << 16]);
            let mut play = 0u64;
            // Frames are taken only after a mark: a frame a stop cut short
            // ends at the next one.
            let mut synced = false;
            loop {
                match out.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(k) => buf.extend_from_slice(&chunk[..k]),
                }
                loop {
                    let look = buf.len().min(n + MARK.len() - 1);
                    if let Some(at) = buf[..look].windows(MARK.len()).position(|w| w == MARK) {
                        buf.drain(..at + MARK.len());
                        play += 1;
                        synced = true;
                        continue;
                    }
                    if !synced {
                        let keep = buf.len().min(MARK.len() - 1);
                        buf.drain(..buf.len() - keep);
                        break;
                    }
                    // (A frame cut within its last 15 bytes would hide the
                    // mark across its end - not waited for: it would hold
                    // each frame back by the next one.)
                    if buf.len() < n {
                        break;
                    }
                    let t = turn.load(std::sync::atomic::Ordering::Relaxed);
                    let rgba = half_rgba(&buf[..n], [Turn::None, Turn::Cw, Turn::Ccw][(t & 3) as usize % 3], t & 4 != 0);
                    buf.drain(..n);
                    let mut f = frame.lock().unwrap();
                    f.rgba = rgba;
                    f.seq += 1;
                    f.play = play;
                    drop(f);
                    wake.ping();
                }
            }
            tracing::warn!("camera: the helper is gone");
            let _ = child.wait();
        });
    }

    fn order(&mut self, c: u8) {
        if let Some(o) = self.orders.as_mut() {
            if o.write_all(&[c]).is_err() {
                self.orders = None;
            }
        }
    }

    /// The screen lit while locked: the camera on.
    pub fn start(&mut self, now_ns: u64) {
        self.start_for(now_ns, FOR_NS);
    }

    /// The camera on for this long (or until not wanted).
    pub fn start_for(&mut self, now_ns: u64, for_ns: u64) {
        if self.orders.is_none() || self.playing {
            return;
        }
        let how = std::fs::read_to_string("/tmp/item-camera").unwrap_or_else(|_| "ccw mirror".into());
        let turn = if how.contains("ccw") { Turn::Ccw } else if how.contains("cw") { Turn::Cw } else { Turn::None };
        let mirror = how.contains("mirror");
        self.size = if turn == Turn::None { (W / 2, H / 2) } else { (H / 2, W / 2) };
        self.turn.store(turn as u8 | if mirror { 4 } else { 0 }, std::sync::atomic::Ordering::Relaxed);
        self.order(b'p');
        self.open = true;
        self.playing = true;
        self.play += 1;
        self.since = now_ns;
        self.tried = now_ns;
        self.for_ns = for_ns;
        self.first = None;
        tracing::info!("camera: peeking (turned {turn:?}, mirrored {mirror})");
    }

    pub fn playing(&self) -> bool {
        self.playing
    }

    /// The camera off - still open while locked.
    pub fn stop(&mut self) {
        if self.playing {
            self.playing = false;
            self.order(b's');
            tracing::info!("camera: off");
        }
    }

    /// Each turn of the loop: off once its time is up or the lock screen
    /// is gone or dark; whether a new frame came (a frame wanted).
    /// Held open while locked, so the next peek starts quickly; closed for
    /// the apps once unlocked.
    pub fn tick(&mut self, now_ns: u64, showing: bool, locked: bool) -> bool {
        if locked != self.open && !self.playing && self.orders.is_some() {
            self.open = locked;
            self.order(if locked { b'o' } else { b'c' });
            tracing::info!("camera: {}", if locked { "held open" } else { "closed" });
        }
        if !self.playing {
            return false;
        }
        if !showing || now_ns > self.since.saturating_add(self.for_ns) {
            self.stop();
            return true;
        }
        let (seq, play) = {
            let f = self.frame.lock().unwrap();
            (f.seq, f.play)
        };
        // No picture yet: the pane breathes - or, too long, the camera is
        // started again.
        if play != self.play {
            if self.first.is_none() && now_ns > self.tried + RETRY_NS {
                tracing::warn!("camera: no frame in {} ms: starting it again", (now_ns - self.tried) / 1_000_000);
                self.order(b'c');
                self.order(b'p');
                self.play += 1;
                self.tried = now_ns;
            }
            return self.shown;
        }
        if self.first.is_none() {
            self.first = Some(now_ns);
            tracing::info!("camera: first frame {} ms after the start", (now_ns - self.since) / 1_000_000);
        }
        // The picture fading in over the pane.
        self.shown && (seq != self.drawn.borrow().0 || now_ns < self.first.unwrap_or(0) + FADE_NS as u64)
    }

    /// The window on the right panel: its dark pane at once, the picture
    /// fading in over it with the first frame.
    pub fn elements(&self, renderer: &mut GlesRenderer, frame_ns: u64) -> Vec<ShellElement> {
        if !self.playing || !self.shown {
            return Vec::new();
        }
        let right = layout::panels()[1];
        // Half the frame's px are logical px at scale 1.
        let x = right.loc.x as f64 + (right.size.w as f64 - self.size.0 as f64) / 2.0;
        let y = 200.0;
        let at = ((x * SCALE as f64).round(), (y * SCALE as f64).round());
        let mut out = Vec::new();
        if let Some(first) = self.first {
            let f = self.frame.lock().unwrap();
            let mut drawn = self.drawn.borrow_mut();
            if f.play == self.play && drawn.0 != f.seq && !f.rgba.is_empty() {
                *drawn = (f.seq, Some(MemoryRenderBuffer::from_slice(&f.rgba, Fourcc::Abgr8888, (self.size.0 as i32, self.size.1 as i32), 1, smithay::utils::Transform::Normal, None)));
            }
            if let Some(buffer) = drawn.1.clone() {
                let k = ((frame_ns.saturating_sub(first)) as f64 / FADE_NS).clamp(0.0, 1.0) as f32;
                if let Ok(e) = MemoryRenderBufferRenderElement::from_buffer(renderer, at, &buffer, Some(k), None, None, Kind::Unspecified) {
                    out.push(ShellElement::Text(e));
                }
                if k >= 1.0 {
                    return out;
                }
            }
        }
        // The pane under it: in quickly, breathing while the picture comes.
        let mut pane = self.pane.borrow_mut();
        if pane.as_ref().map_or(true, |(v, size, _)| *v != crate::accent::version() || *size != self.size) {
            let c = crate::accent::get();
            let tint = |a: u8, base: f64| (base * 0.82 + a as f64 * 0.18).round() as u8;
            let color = [tint(c[0], 22.0), tint(c[1], 22.0), tint(c[2], 24.0), 255];
            *pane = Some((crate::accent::version(), self.size, crate::grid::rounded(self.size.0 as f64, self.size.1 as f64, RADIUS, color)));
        }
        let t = frame_ns.saturating_sub(self.since) as f64;
        let breath = 0.82 + 0.18 * (t / BREATH_NS * std::f64::consts::TAU).cos();
        let k = ((t / PANE_IN_NS).clamp(0.0, 1.0) * breath) as f32;
        let buffer = pane.as_ref().unwrap().2.clone();
        if let Ok(e) = MemoryRenderBufferRenderElement::from_buffer(renderer, at, &buffer, Some(k), None, None, Kind::Unspecified) {
            out.push(ShellElement::Text(e));
        }
        out
    }
}

/// An NV21 frame to grey RGBA at half its size (a pixel per 2x2, its luma),
/// turned a quarter either way or not, mirrored left to right or not, its
/// corners rounded; premultiplied.
fn half_rgba(nv21: &[u8], turn: Turn, mirror: bool) -> Vec<u8> {
    let (w, h) = (W / 2, H / 2);
    // The picture as it comes out: turned, its own size.
    let (ow, oh) = if turn == Turn::None { (w, h) } else { (h, w) };
    let mut out = vec![0u8; ow * oh * 4];
    for oy in 0..oh {
        for ox in 0..ow {
            let mx = if mirror { ow - 1 - ox } else { ox };
            // Where in the half-size frame this pixel comes from.
            let (x, y) = match turn {
                Turn::None => (mx, oy),
                Turn::Cw => (oy, h - 1 - mx),
                Turn::Ccw => (w - 1 - oy, mx),
            };
            let (sx, sy) = (2 * x, 2 * y);
            let lum = nv21[sy * W + sx] as f32;
            let (fx, fy) = (ox as f64 + 0.5, oy as f64 + 0.5);
            let (cx, cy) = (fx.clamp(RADIUS, ow as f64 - RADIUS), fy.clamp(RADIUS, oh as f64 - RADIUS));
            let a = (RADIUS + 0.5 - ((fx - cx).powi(2) + (fy - cy).powi(2)).sqrt()).clamp(0.0, 1.0) as f32;
            let o = (oy * ow + ox) * 4;
            let grey = (lum * a) as u8;
            out[o] = grey;
            out[o + 1] = grey;
            out[o + 2] = grey;
            out[o + 3] = (255.0 * a) as u8;
        }
    }
    out
}
