//! The tick under a finger: the Duo's haptic (a Dialog da7280 on i2c,
//! `da7280-haptic` to the input layer, event1) played through the kernel's
//! force feedback. Of the effects its driver takes only FF_CONSTANT turns
//! the motor (rumble answers EFAULT, periodic EINVAL - 2026-10-21): a
//! level, which the driver plays until told to stop - it ignores the
//! effect's length (the motor ran on under a finger typing, 2026-10-21),
//! so a thread of its own starts it, waits the tick's milliseconds and
//! stops it. The motor needs a couple dozen ms to be felt. feedbackd,
//! which turns it under phosh on other phones through the vendor's
//! vibrator HAL, finds no vibrator here: its events play to nothing. Two
//! effects are uploaded once, a light one for a key and a firm one for
//! Enter and the layers. Off with `~/.config/item/haptic` holding `off`;
//! the device missing (another phone) is quietly no tick.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::fd::AsRawFd;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

const EV_FF: u16 = 0x15;
const FF_CONSTANT: u16 = 0x52;
const FF_GAIN: u16 = 0x60;
/// How long each tick runs (ms).
const LIGHT_MS: u64 = 22;
const FIRM_MS: u64 = 38;
/// `_IOW('E', 0x80, struct ff_effect)`: the struct is 48 bytes on arm64.
const EVIOCSFF: libc::c_ulong = 0x4030_4580;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Tick {
    /// A key: short and soft.
    Light,
    /// Enter, a layer, the shift: a touch longer and full.
    Firm,
}

struct Haptic {
    device: File,
    light: i16,
    firm: i16,
}

static TICKS: OnceLock<Option<Mutex<Sender<Tick>>>> = OnceLock::new();
/// On unless the preferences say off (prefs.rs).
static ON: AtomicBool = AtomicBool::new(true);

pub fn on() -> bool {
    ON.load(Ordering::Relaxed)
}

pub fn set_on(on: bool) {
    if ON.swap(on, Ordering::Relaxed) != on {
        tracing::info!("haptic: {}", if on { "on" } else { "off" });
    }
}

/// The event device named `da7280-haptic`, by the names in /sys.
fn device_path() -> Option<String> {
    for entry in std::fs::read_dir("/sys/class/input").ok()?.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with("event") {
            continue;
        }
        let device_name = std::fs::read_to_string(entry.path().join("device/name")).unwrap_or_default();
        if device_name.trim() == "da7280-haptic" {
            return Some(format!("/dev/input/{name}"));
        }
    }
    None
}

/// `struct ff_effect` for a constant level over `ms`; the id the kernel
/// gives comes back in the buffer's bytes 2..4.
fn constant_effect(level: i16, ms: u16) -> [u8; 48] {
    let mut e = [0u8; 48];
    e[0..2].copy_from_slice(&FF_CONSTANT.to_ne_bytes());
    e[2..4].copy_from_slice(&(-1i16).to_ne_bytes());
    e[4..6].copy_from_slice(&0x4000u16.to_ne_bytes()); // direction: up
    e[10..12].copy_from_slice(&ms.to_ne_bytes()); // replay.length
    e[16..18].copy_from_slice(&level.to_ne_bytes()); // constant.level
    e
}

fn upload(device: &File, level: i16, ms: u16) -> Result<i16, String> {
    let mut e = constant_effect(level, ms);
    // SAFETY: EVIOCSFF reads and writes a struct ff_effect of 48 bytes,
    // which `e` is, on an open event device.
    let r = unsafe { libc::ioctl(device.as_raw_fd(), EVIOCSFF, e.as_mut_ptr()) };
    if r < 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    Ok(i16::from_ne_bytes([e[2], e[3]]))
}

fn open() -> Option<Haptic> {
    let Some(path) = device_path() else {
        tracing::info!("haptic: no da7280-haptic device");
        return None;
    };
    let device = match OpenOptions::new().read(true).write(true).open(&path) {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!("haptic: {path}: {e}");
            return None;
        }
    };
    // The gain back up: it is turned down to stop a runaway motor.
    let _ = (&device).write_all(&event(FF_GAIN, 0xffff));
    let (light, firm) = match (upload(&device, 0x6000, LIGHT_MS as u16), upload(&device, 0x7fff, FIRM_MS as u16)) {
        (Ok(l), Ok(f)) => (l, f),
        (Err(e), _) | (_, Err(e)) => {
            tracing::warn!("haptic: {path}: the effects: {e}");
            return None;
        }
    };
    tracing::info!("haptic: {path}, effects {light} and {firm}");
    Some(Haptic { device, light, firm })
}

/// `struct input_event` on arm64: timeval (16), type, code, value.
fn event(code: u16, value: i32) -> [u8; 24] {
    let mut ev = [0u8; 24];
    ev[16..18].copy_from_slice(&EV_FF.to_ne_bytes());
    ev[18..20].copy_from_slice(&code.to_ne_bytes());
    ev[20..24].copy_from_slice(&value.to_ne_bytes());
    ev
}

/// The thread: each tick started, waited out, stopped; ticks that came
/// while one ran are dropped.
fn start() -> Option<Mutex<Sender<Tick>>> {
    let mut h = open()?;
    let (tx, rx) = channel::<Tick>();
    std::thread::Builder::new()
        .name("haptic".into())
        .spawn(move || {
            while let Ok(kind) = rx.recv() {
                let (id, ms) = match kind {
                    Tick::Light => (h.light, LIGHT_MS),
                    Tick::Firm => (h.firm, FIRM_MS),
                };
                let started = h.device.write_all(&event(id as u16, 1));
                std::thread::sleep(Duration::from_millis(ms));
                let stopped = h.device.write_all(&event(id as u16, 0));
                if let Err(e) = started.and(stopped) {
                    tracing::warn!("haptic: {e}");
                }
                while rx.try_recv().is_ok() {}
            }
        })
        .ok()?;
    Some(Mutex::new(tx))
}

/// A tick now. The first call opens the device and starts the thread.
pub fn tick(kind: Tick) {
    if !ON.load(Ordering::Relaxed) {
        return;
    }
    if let Some(tx) = TICKS.get_or_init(start) {
        let _ = tx.lock().unwrap().send(kind);
    }
}
