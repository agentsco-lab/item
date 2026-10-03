//! The camera, through tools/camera-warm.c: a helper kept running with
//! Droidian's camera stack loaded, taking one-byte orders - 'o' open and
//! warm, 'p' play, 's' stop (still open), 'c' close - and writing a mark
//! before each play's NV21 640x480 frames.

use std::io::{Read, Write};
use std::process::{ChildStdin, Command, Stdio};
use std::sync::mpsc::SyncSender;

pub const W: usize = 640;
pub const H: usize = 480;
const MARK: &[u8; 16] = b"camera-warm:play";

pub struct Camera {
    orders: Option<ChildStdin>,
    /// Plays started so far: frames are tagged with the play they are of.
    pub play: u64,
}

impl Camera {
    /// The helper started; each frame goes to `frames` with its play's
    /// number, made by `wrap` - dropped when the receiver is still busy.
    pub fn new<M: Send + 'static>(frames: SyncSender<M>, wrap: fn(u64, Vec<u8>) -> M) -> Camera {
        let tool = std::env::var("CAMERA_WARM").unwrap_or_else(|_| "/var/tmp/gst/camera-warm".into());
        let child = Command::new(&tool).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::inherit()).spawn();
        let Ok(mut child) = child else {
            tracing::warn!("camera: {tool} did not start");
            return Camera { orders: None, play: 0 };
        };
        let mut out = child.stdout.take().expect("piped");
        let orders = child.stdin.take();
        let _ = std::thread::Builder::new().name("camera".into()).spawn(move || {
            let n = W * H * 3 / 2;
            let (mut buf, mut chunk) = (Vec::with_capacity(2 * n), vec![0u8; 1 << 16]);
            let (mut play, mut synced) = (0u64, false);
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
                    if buf.len() < n {
                        break;
                    }
                    let frame = buf[..n].to_vec();
                    buf.drain(..n);
                    let _ = frames.try_send(wrap(play, frame));
                }
            }
            tracing::warn!("camera: the helper is gone");
            let _ = child.wait();
        });
        Camera { orders, play: 0 }
    }

    fn order(&mut self, c: u8) {
        if let Some(o) = self.orders.as_mut() {
            if o.write_all(&[c]).is_err() {
                self.orders = None;
            }
        }
    }

    pub fn open(&mut self) {
        self.order(b'o');
    }

    pub fn close(&mut self) {
        self.order(b'c');
    }

    /// Frames from now on, of the play returned.
    pub fn play(&mut self) -> u64 {
        self.order(b'p');
        self.play += 1;
        self.play
    }

    pub fn stop(&mut self) {
        self.order(b's');
    }

    /// No frame came: closed and played again.
    pub fn restart(&mut self) -> u64 {
        self.order(b'c');
        self.play()
    }
}

/// A frame upright, as the Duo is held (a quarter turn counter-clockwise),
/// not mirrored: RGB.
pub fn upright(nv21: &[u8]) -> cvid::Image {
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
