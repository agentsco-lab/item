//! Folded back to back, the Duo is a camera (item-tracker #105, a first cut;
//! from the lock screen only; off unless FOLDCAM=1 while what folding does
//! is worked out): fold it on past ARM_DEG and the camera
//! starts, past ON_DEG the left panel is its viewfinder and the right one
//! goes dark; unfold below OFF_DEG and the lock screen is back. A tap on the
//! viewfinder or a volume key takes a shot: the frame stills at once, the
//! panel flashes, the picture (the preview's 1440x1920) is written to
//! ~/Pictures/Camera in the background.
//!
//! The near panel is taken to be the left - the camera, on the right
//! panel's inner face, looks away from the user then - until it can be told
//! (#116). The camera is item-face's while the phone is locked (face.rs);
//! it lets it go while this has it.
//!
//! The frames come from tools/camera-warm.c as the peek's did (camera.rs),
//! 1920x1440 NV21 at 30 a second; a thread turns each upright, a pixel per
//! 2x2, for the viewfinder, and keeps the last whole one for a shot.

use std::io::{Read, Write};
use std::process::{ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex};

use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement};
use smithay::backend::renderer::element::solid::SolidColorRenderElement;
use smithay::backend::renderer::element::{Id, Kind};
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::renderer::utils::CommitCounter;
use smithay::reexports::calloop::ping::Ping;
use smithay::utils::{Logical, Physical, Point, Rectangle, Transform};

use crate::layout::{self, SCALE};
use crate::shade::ShellElement;

const W: usize = 1920;
const H: usize = 1440;
/// The hinge's angle: the camera started on the way; the viewfinder; back
/// to the lock screen.
const ARM_DEG: f64 = 290.0;
const ON_DEG: f64 = 330.0;
const OFF_DEG: f64 = 270.0;
/// Left alone this long, it closes - and waits for an unfold to open again.
const IDLE_NS: u64 = 60_000_000_000;
/// No frame this long after a start: the camera started again (it may still
/// have been item-face's).
const RETRY_NS: u64 = 1_500_000_000;
/// A shot: the frame stilled, the flash.
const STILL_NS: u64 = 700_000_000;
const FLASH_NS: f64 = 180e6;
const MARK: &[u8; 16] = b"camera-warm:play";
const SHUTTER: f64 = 76.0;

#[derive(Default)]
struct Frame {
    /// The viewfinder's picture: upright, half size, RGBA.
    rgba: Vec<u8>,
    /// The last whole frame (NV21), for a shot.
    nv21: Vec<u8>,
    seq: u64,
    play: u64,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Stage {
    Off,
    /// The camera starting, the lock screen still shown.
    Arming,
    On,
}

pub struct FoldCam {
    on: bool,
    orders: Option<ChildStdin>,
    frame: Arc<Mutex<Frame>>,
    play: u64,
    stage: Stage,
    since: u64,
    tried: u64,
    last_used: u64,
    /// Closed for idleness: open again only after an unfold.
    latched: bool,
    drawn: std::cell::RefCell<(u64, Option<MemoryRenderBuffer>)>,
    still: Option<(u64, MemoryRenderBuffer)>,
    flash_at: Option<u64>,
    shutter: std::cell::OnceCell<Option<MemoryRenderBuffer>>,
    /// The right panel's backlight before it went dark.
    far_level: Option<String>,
    ids: [Id; 3],
    wake: Ping,
}

impl FoldCam {
    pub fn new(wake: Ping) -> FoldCam {
        FoldCam { on: std::env::var_os("FOLDCAM").is_some(), orders: None, frame: Default::default(), play: 0, stage: Stage::Off, since: 0, tried: 0, last_used: 0, latched: false, drawn: Default::default(), still: None, flash_at: None, shutter: Default::default(), far_level: None, ids: [Id::new(), Id::new(), Id::new()], wake }
    }

    /// The helper, started on first use and kept.
    fn spawn(&mut self) {
        if self.orders.is_some() {
            return;
        }
        let tool = std::env::var("CAMERA_WARM").unwrap_or_else(|_| "/usr/libexec/item/camera-warm".into());
        let child = Command::new(&tool).args([W.to_string(), H.to_string()]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn();
        let Ok(mut child) = child else {
            tracing::warn!("foldcam: {tool} did not start");
            return;
        };
        let mut out = child.stdout.take().expect("piped");
        self.orders = child.stdin.take();
        let (frame, wake) = (self.frame.clone(), self.wake.clone());
        let _ = std::thread::Builder::new().name("foldcam".into()).spawn(move || {
            let n = W * H * 3 / 2;
            let (mut buf, mut chunk) = (Vec::with_capacity(2 * n), vec![0u8; 1 << 18]);
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
                    let rgba = upright(&buf[..n], 2);
                    let mut f = frame.lock().unwrap();
                    f.rgba = rgba;
                    f.nv21.clear();
                    f.nv21.extend_from_slice(&buf[..n]);
                    f.seq += 1;
                    f.play = play;
                    drop(f);
                    buf.drain(..n);
                    wake.ping();
                }
            }
            tracing::warn!("foldcam: the helper is gone");
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

    /// Whether the camera is this one's (item-face lets it go).
    pub fn wants_camera(&self) -> bool {
        self.stage != Stage::Off
    }

    /// The viewfinder is up: the lock screen under it, its touches here.
    pub fn active(&self) -> bool {
        self.stage == Stage::On
    }

    fn start(&mut self, now: u64) {
        self.spawn();
        self.order(b'p');
        self.play += 1;
        self.stage = Stage::Arming;
        (self.since, self.tried, self.last_used) = (now, now, now);
        tracing::info!("foldcam: the camera starting");
    }

    fn stop(&mut self) {
        if self.stage == Stage::Off {
            return;
        }
        self.order(b'c');
        self.stage = Stage::Off;
        self.still = None;
        self.flash_at = None;
        self.far(true);
        tracing::info!("foldcam: off");
    }

    /// The right panel lit or dark (its backlight; its level kept).
    fn far(&mut self, lit: bool) {
        let path = "/sys/class/backlight/panel1-backlight/brightness";
        if lit {
            if let Some(level) = self.far_level.take() {
                let _ = std::fs::write(path, level);
            }
        } else if self.far_level.is_none() {
            self.far_level = std::fs::read_to_string(path).ok().map(|s| s.trim().to_owned());
            let _ = std::fs::write(path, "0");
        }
    }

    /// Each turn of the loop, with the hinge's angle; whether to draw.
    pub fn tick(&mut self, now: u64, angle: f64, locked: bool, lit: bool) -> bool {
        if !self.on {
            return false;
        }
        if !locked || !lit {
            self.latched = false;
            let was = self.stage != Stage::Off;
            self.stop();
            return was;
        }
        if angle < OFF_DEG {
            self.latched = false;
            if self.stage != Stage::Off {
                self.stop();
                return true;
            }
            return false;
        }
        match self.stage {
            Stage::Off if angle >= ARM_DEG && !self.latched => {
                self.start(now);
                false
            }
            Stage::Arming if angle >= ON_DEG => {
                self.stage = Stage::On;
                self.last_used = now;
                self.far(false);
                tracing::info!("foldcam: on, {} ms after the start", (now - self.since) / 1_000_000);
                true
            }
            Stage::On if now > self.last_used + IDLE_NS => {
                tracing::info!("foldcam: left alone: closed");
                self.stop();
                self.latched = true;
                true
            }
            Stage::Off => false,
            _ => {
                let (seq, play) = {
                    let f = self.frame.lock().unwrap();
                    (f.seq, f.play)
                };
                if play != self.play && now > self.tried + RETRY_NS {
                    tracing::warn!("foldcam: no frame yet: the camera started again");
                    self.order(b'c');
                    self.order(b'p');
                    self.play += 1;
                    self.tried = now;
                }
                self.stage == Stage::On && (seq != self.drawn.borrow().0 || self.flash_at.is_some_and(|t| (now - t) as f64 <= FLASH_NS) || self.still.is_some())
            }
        }
    }

    /// A touch while the viewfinder is up: on it, a shot. Whether it was
    /// this one's.
    pub fn touch(&mut self, pos: Point<f64, Logical>, now: u64) -> bool {
        if !self.active() {
            return false;
        }
        if layout::panel_at(pos) == Some(0) {
            self.shoot(now);
        }
        true
    }

    /// The shutter: the frame stilled, the flash, the file written behind.
    pub fn shoot(&mut self, now: u64) {
        if !self.active() {
            return;
        }
        self.last_used = now;
        let (nv21, rgba) = {
            let f = self.frame.lock().unwrap();
            if f.play != self.play || f.nv21.is_empty() {
                return;
            }
            (f.nv21.clone(), f.rgba.clone())
        };
        self.still = Some((now, MemoryRenderBuffer::from_slice(&rgba, Fourcc::Abgr8888, ((H / 2) as i32, (W / 2) as i32), 1, Transform::Normal, None)));
        self.flash_at = Some(now);
        crate::fingerprint::buzz("camera-shutter");
        std::thread::spawn(move || save(&nv21));
    }

    pub fn elements(&self, renderer: &mut GlesRenderer, frame_ns: u64) -> Vec<ShellElement> {
        let mut out = Vec::new();
        if !self.active() {
            return out;
        }
        let [left, right] = layout::panels();
        // The shutter, at the bottom of the viewfinder.
        if let Some(ring) = self.shutter.get_or_init(shutter_ring) {
            let at = ((left.loc.x as f64 + (left.size.w as f64 - SHUTTER) / 2.0) * SCALE as f64, (left.size.h as f64 - SHUTTER - 120.0) * SCALE as f64);
            if let Ok(e) = MemoryRenderBufferRenderElement::from_buffer(renderer, at, ring, None, None, None, Kind::Unspecified) {
                out.push(ShellElement::Text(e));
            }
        }
        // The flash: white, gone quickly.
        if let Some(t) = self.flash_at {
            let k = 1.0 - (frame_ns.saturating_sub(t) as f64 / FLASH_NS).clamp(0.0, 1.0);
            if k > 0.0 {
                let rect = Rectangle::<i32, Physical>::new(left.loc.to_physical(SCALE), left.size.to_physical(SCALE));
                out.push(ShellElement::Solid(SolidColorRenderElement::new(self.ids[1].clone(), rect, CommitCounter::from((k * 100.0) as usize), [k as f32, k as f32, k as f32, k as f32], Kind::Unspecified)));
            }
        }
        // The picture: stilled after a shot, else the newest frame - the
        // panel's 3:4 from the camera's 3:4, whole.
        let still = self.still.as_ref().filter(|(t, _)| frame_ns < t + STILL_NS).map(|(_, b)| b.clone());
        let buffer = still.or_else(|| {
            let f = self.frame.lock().unwrap();
            let mut drawn = self.drawn.borrow_mut();
            if f.play == self.play && drawn.0 != f.seq && !f.rgba.is_empty() {
                *drawn = (f.seq, Some(MemoryRenderBuffer::from_slice(&f.rgba, Fourcc::Abgr8888, ((H / 2) as i32, (W / 2) as i32), 1, Transform::Normal, None)));
            }
            drawn.1.clone()
        });
        if let Some(b) = buffer {
            if let Ok(e) = MemoryRenderBufferRenderElement::from_buffer(renderer, (left.loc.x as f64 * SCALE as f64, 0.0), &b, None, None, Some(left.size), Kind::Unspecified) {
                out.push(ShellElement::Text(e));
            }
        } else {
            let rect = Rectangle::<i32, Physical>::new(left.loc.to_physical(SCALE), left.size.to_physical(SCALE));
            out.push(ShellElement::Solid(SolidColorRenderElement::new(self.ids[0].clone(), rect, CommitCounter::default(), [0.0, 0.0, 0.0, 1.0], Kind::Unspecified)));
        }
        // The far panel: dark (its backlight off too).
        let rect = Rectangle::<i32, Physical>::new(right.loc.to_physical(SCALE), right.size.to_physical(SCALE));
        out.push(ShellElement::Solid(SolidColorRenderElement::new(self.ids[2].clone(), rect, CommitCounter::default(), [0.0, 0.0, 0.0, 1.0], Kind::Unspecified)));
        out
    }

    /// After a frame: the still's time up.
    pub fn settle(&mut self, frame_ns: u64) {
        if self.still.as_ref().is_some_and(|(t, _)| frame_ns >= t + STILL_NS) {
            self.still = None;
        }
        if self.flash_at.is_some_and(|t| (frame_ns.saturating_sub(t)) as f64 > FLASH_NS) {
            self.flash_at = None;
        }
    }
}

/// A frame (NV21, landscape as the sensor gives it) upright as the Duo is
/// held folded, the camera facing away - a quarter turn counter-clockwise,
/// not mirrored - a pixel per `step` x `step`; RGBA.
fn upright(nv21: &[u8], step: usize) -> Vec<u8> {
    let (w, h) = (W / step, H / step);
    // Turned: h wide, w tall.
    let (ow, oh) = (h, w);
    let mut out = vec![0u8; ow * oh * 4];
    let uv = &nv21[W * H..];
    for oy in 0..oh {
        for ox in 0..ow {
            let (x, y) = (w - 1 - oy, ox);
            let (sx, sy) = (x * step, y * step);
            let lum = nv21[sy * W + sx] as f32;
            let i = (sy / 2) * W + (sx / 2) * 2;
            let (v, u) = (uv[i] as f32 - 128.0, uv[i + 1] as f32 - 128.0);
            let o = (oy * ow + ox) * 4;
            out[o] = (lum + 1.402 * v).clamp(0.0, 255.0) as u8;
            out[o + 1] = (lum - 0.344 * u - 0.714 * v).clamp(0.0, 255.0) as u8;
            out[o + 2] = (lum + 1.772 * u).clamp(0.0, 255.0) as u8;
            out[o + 3] = 255;
        }
    }
    out
}

/// A shot written: the whole frame upright, JPEG, ~/Pictures/Camera.
fn save(nv21: &[u8]) {
    let t = std::time::Instant::now();
    let rgba = upright(nv21, 1);
    let rgb: Vec<u8> = rgba.chunks_exact(4).flat_map(|p| [p[0], p[1], p[2]]).collect();
    let dir = std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default()).join("Pictures/Camera");
    let _ = std::fs::create_dir_all(&dir);
    let tm = crate::shade::local_time();
    let path = dir.join(format!("IMG_{:04}{:02}{:02}_{:02}{:02}{:02}.jpg", tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday, tm.tm_hour, tm.tm_min, tm.tm_sec));
    let encoder = match jpeg_encoder::Encoder::new_file(&path, 90) {
        Ok(e) => e,
        Err(e) => {
            tracing::warn!("foldcam: {}: {e}", path.display());
            return;
        }
    };
    match encoder.encode(&rgb, H as u16, W as u16, jpeg_encoder::ColorType::Rgb) {
        Ok(()) => tracing::info!("foldcam: {} written in {} ms", path.display(), t.elapsed().as_millis()),
        Err(e) => tracing::warn!("foldcam: {}: {e}", path.display()),
    }
}

/// The shutter: a white ring around a white disc.
fn shutter_ring() -> Option<MemoryRenderBuffer> {
    use resvg::tiny_skia;
    let px = (SHUTTER * SCALE as f64) as u32;
    let mut pixmap = tiny_skia::Pixmap::new(px, px)?;
    let c = px as f32 / 2.0;
    let mut paint = tiny_skia::Paint::default();
    paint.anti_alias = true;
    paint.set_color_rgba8(255, 255, 255, 235);
    let ring = tiny_skia::PathBuilder::from_circle(c, c, c - 3.0 * SCALE as f32)?;
    pixmap.stroke_path(&ring, &paint, &tiny_skia::Stroke { width: 4.0 * SCALE as f32, ..Default::default() }, tiny_skia::Transform::identity(), None);
    let disc = tiny_skia::PathBuilder::from_circle(c, c, c - 11.0 * SCALE as f32)?;
    pixmap.fill_path(&disc, &paint, tiny_skia::FillRule::Winding, tiny_skia::Transform::identity(), None);
    Some(MemoryRenderBuffer::from_slice(pixmap.data(), Fourcc::Abgr8888, (px as i32, px as i32), SCALE, Transform::Normal, None))
}
