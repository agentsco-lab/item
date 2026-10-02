//! A peek through the camera on the lock screen (an experiment, CAMERA_PEEK=1;
//! item-tracker #105, #107, #109): the screen lit while locked - the lid
//! opened, the power key - and a small window on the left panel shows what
//! the camera sees, mirrored as a mirror is, for a few seconds.
//!
//! The frames come through Droidian's camera stack as droidian-camera's do:
//! GStreamer's droidcamsrc (droidmedia, Android's Camera1), 640x480 NV21,
//! out of a gst-launch child's stdout; a thread here takes each frame to
//! RGBA at half the size. Measured 2026-10-02: the first frame ~1.1 s after
//! the start, then ~16 a second. GST_LAUNCH names the tool (not installed on
//! the port: unpacked in /var/tmp/gst for the experiment).

use std::io::Read;
use std::process::{Child, Command, Stdio};
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
/// The window's corners (px of the half-size frame).
const RADIUS: f64 = 18.0;

#[derive(Default)]
struct Frame {
    rgba: Vec<u8>,
    seq: u64,
}

pub struct Peek {
    on: bool,
    child: Option<Child>,
    since: u64,
    /// When the first frame came.
    first: Option<u64>,
    frame: Arc<Mutex<Frame>>,
    drawn: std::cell::RefCell<(u64, Option<MemoryRenderBuffer>)>,
    wake: Ping,
}

impl Peek {
    pub fn new(wake: Ping) -> Peek {
        Peek { on: std::env::var_os("CAMERA_PEEK").is_some(), child: None, since: 0, first: None, frame: Default::default(), drawn: Default::default(), wake }
    }

    /// The screen lit while locked: the camera on.
    pub fn start(&mut self, now_ns: u64) {
        if !self.on || self.child.is_some() {
            return;
        }
        let tool = std::env::var("GST_LAUNCH").unwrap_or_else(|_| "/var/tmp/gst/root/usr/bin/gst-launch-1.0".into());
        let child = Command::new(tool)
            .args(["-q", "droidcamsrc", "!", "video/x-raw,format=NV21,width=640,height=480", "!", "fdsink", "fd=1"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn();
        let Ok(mut child) = child else {
            tracing::warn!("camera: gst-launch did not start");
            return;
        };
        tracing::info!("camera: peeking");
        let mut out = child.stdout.take().expect("piped");
        let (frame, wake) = (self.frame.clone(), self.wake.clone());
        frame.lock().unwrap().seq = 0;
        let _ = std::thread::Builder::new().name("camera".into()).spawn(move || {
            let mut nv21 = vec![0u8; W * H * 3 / 2];
            while out.read_exact(&mut nv21).is_ok() {
                let rgba = half_rgba(&nv21);
                let mut f = frame.lock().unwrap();
                f.rgba = rgba;
                f.seq += 1;
                drop(f);
                wake.ping();
            }
        });
        self.child = Some(child);
        self.since = now_ns;
        self.first = None;
    }

    /// The camera off.
    pub fn stop(&mut self) {
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            std::thread::spawn(move || {
                let _ = c.wait();
            });
            tracing::info!("camera: off");
        }
    }

    /// Each turn of the loop: off once its time is up or the lock screen
    /// is gone or dark; whether a new frame came (a frame wanted).
    pub fn tick(&mut self, now_ns: u64, showing: bool) -> bool {
        if self.child.is_none() {
            return false;
        }
        if !showing || now_ns > self.since + FOR_NS {
            self.stop();
            return true;
        }
        let seq = self.frame.lock().unwrap().seq;
        if seq > 0 && self.first.is_none() {
            self.first = Some(now_ns);
            tracing::info!("camera: first frame {} ms after the start", (now_ns - self.since) / 1_000_000);
        }
        seq != self.drawn.borrow().0
    }

    /// The window on the left panel, fading in with the first frame.
    pub fn elements(&self, renderer: &mut GlesRenderer, frame_ns: u64) -> Vec<ShellElement> {
        let (Some(first), true) = (self.first, self.child.is_some()) else { return Vec::new() };
        let f = self.frame.lock().unwrap();
        let mut drawn = self.drawn.borrow_mut();
        if drawn.0 != f.seq && !f.rgba.is_empty() {
            *drawn = (f.seq, Some(MemoryRenderBuffer::from_slice(&f.rgba, Fourcc::Abgr8888, ((W / 2) as i32, (H / 2) as i32), 1, smithay::utils::Transform::Normal, None)));
        }
        let Some(buffer) = drawn.1.clone() else { return Vec::new() };
        let k = ((frame_ns.saturating_sub(first)) as f64 / FADE_NS).clamp(0.0, 1.0) as f32;
        let left = layout::panels()[0];
        // Half the frame's px are logical px at scale 1: 320x240 logical.
        let x = left.loc.x as f64 + (left.size.w as f64 - (W / 2) as f64) / 2.0;
        let y = 300.0;
        match MemoryRenderBufferRenderElement::from_buffer(renderer, ((x * SCALE as f64).round(), (y * SCALE as f64).round()), &buffer, Some(k), None, None, Kind::Unspecified) {
            Ok(e) => vec![ShellElement::Text(e)],
            Err(_) => Vec::new(),
        }
    }
}

/// An NV21 frame to RGBA at half its size (a pixel per 2x2), mirrored left to
/// right (a mirror), its corners rounded; premultiplied.
fn half_rgba(nv21: &[u8]) -> Vec<u8> {
    let (w, h) = (W / 2, H / 2);
    let mut out = vec![0u8; w * h * 4];
    let uv = &nv21[W * H..];
    for y in 0..h {
        for x in 0..w {
            let (sx, sy) = (2 * x, 2 * y);
            let lum = nv21[sy * W + sx] as f32;
            let i = (sy / 2) * W + (sx / 2) * 2;
            let (v, u) = (uv[i] as f32 - 128.0, uv[i + 1] as f32 - 128.0);
            let r = lum + 1.402 * v;
            let g = lum - 0.344 * u - 0.714 * v;
            let b = lum + 1.772 * u;
            // Rounded corners.
            let (fx, fy) = (x as f64 + 0.5, y as f64 + 0.5);
            let (cx, cy) = (fx.clamp(RADIUS, w as f64 - RADIUS), fy.clamp(RADIUS, h as f64 - RADIUS));
            let a = (RADIUS + 0.5 - ((fx - cx).powi(2) + (fy - cy).powi(2)).sqrt()).clamp(0.0, 1.0) as f32;
            let o = (y * w + (w - 1 - x)) * 4;
            out[o] = (r.clamp(0.0, 255.0) * a) as u8;
            out[o + 1] = (g.clamp(0.0, 255.0) * a) as u8;
            out[o + 2] = (b.clamp(0.0, 255.0) * a) as u8;
            out[o + 3] = (255.0 * a) as u8;
        }
    }
    out
}
