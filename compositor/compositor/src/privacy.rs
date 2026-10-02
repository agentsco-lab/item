//! The privacy dot (item's #99): a small dot in the right panel's top corner
//! while something can see or hear - Android's colours: green for the
//! camera, orange for the microphone, blue for the location. Camera over
//! microphone over location.
//!
//! - The microphone is PulseAudio's to say: a source output is something
//!   recording. A thread follows `pactl subscribe` and asks again for the
//!   source outputs when one comes or goes.
//! - The camera goes through Android's camera service, which tells no one:
//!   it is the camera app's window being open (the state's, by app id).
//! - The location is GeoClue's `InUse`, asked every few seconds while
//!   GeoClue runs (asking would start it).

use std::io::{BufRead, BufReader};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
use smithay::reexports::calloop::ping::Ping;

/// The dot's size and its distance from the panel's corner, logical px.
pub const DOT: f64 = 10.0;
pub const INSET: f64 = 10.0;

pub struct Privacy {
    mic: Arc<AtomicBool>,
    location: Arc<AtomicBool>,
    green: MemoryRenderBuffer,
    orange: MemoryRenderBuffer,
    blue: MemoryRenderBuffer,
}

/// Whether PulseAudio has a source output: something recording.
fn recording() -> bool {
    std::process::Command::new("pactl")
        .args(["list", "short", "source-outputs"])
        .output()
        .map(|o| !String::from_utf8_lossy(&o.stdout).trim().is_empty())
        .unwrap_or(false)
}

impl Privacy {
    pub fn new(wake: Ping) -> Privacy {
        let (mic, location) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)));
        let (m, w) = (mic.clone(), wake.clone());
        std::thread::Builder::new()
            .name("privacy mic".into())
            .spawn(move || loop {
                m.store(recording(), Ordering::Relaxed);
                w.ping();
                let child = std::process::Command::new("pactl").arg("subscribe").stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null()).spawn();
                let Ok(mut child) = child else {
                    std::thread::sleep(Duration::from_secs(30));
                    continue;
                };
                if let Some(out) = child.stdout.take() {
                    for line in BufReader::new(out).lines().map_while(Result::ok) {
                        if line.contains("source-output") {
                            let now = recording();
                            if now != m.swap(now, Ordering::Relaxed) {
                                tracing::info!("privacy: the microphone {}", if now { "in use" } else { "free" });
                                w.ping();
                            }
                        }
                    }
                }
                let _ = child.wait();
                // PulseAudio went away: try again in a while.
                std::thread::sleep(Duration::from_secs(5));
            })
            .expect("privacy mic thread");
        let l = location.clone();
        std::thread::Builder::new()
            .name("privacy location".into())
            .spawn(move || {
                let Ok(bus) = zbus::blocking::Connection::system() else { return };
                loop {
                    // Asked only while it runs: asking would start it (D-Bus
                    // activation), and not running nothing uses it.
                    let running = bus
                        .call_method(Some("org.freedesktop.DBus"), "/org/freedesktop/DBus", Some("org.freedesktop.DBus"), "NameHasOwner", &("org.freedesktop.GeoClue2"))
                        .ok()
                        .and_then(|r| r.body().deserialize::<bool>().ok())
                        .unwrap_or(false);
                    let in_use = running && bus
                        .call_method(Some("org.freedesktop.GeoClue2"), "/org/freedesktop/GeoClue2/Manager", Some("org.freedesktop.DBus.Properties"), "Get", &("org.freedesktop.GeoClue2.Manager", "InUse"))
                        .ok()
                        .and_then(|r| r.body().deserialize::<zbus::zvariant::OwnedValue>().ok())
                        .and_then(|v| bool::try_from(v).ok())
                        .unwrap_or(false);
                    if in_use != l.swap(in_use, Ordering::Relaxed) {
                        tracing::info!("privacy: the location {}", if in_use { "in use" } else { "free" });
                        wake.ping();
                    }
                    std::thread::sleep(Duration::from_secs(4));
                }
            })
            .expect("privacy location thread");
        let dot = |rgb: [u8; 3]| crate::grid::rounded(DOT, DOT, DOT / 2.0, [rgb[0], rgb[1], rgb[2], 255]);
        Privacy { mic, location, green: dot([0x30, 0xd1, 0x58]), orange: dot([0xff, 0x9f, 0x0a]), blue: dot([0x0a, 0x84, 0xff]) }
    }

    /// The dot to show, if any: `camera` is whether the camera app is open.
    pub fn dot(&self, camera: bool) -> Option<&MemoryRenderBuffer> {
        if camera {
            Some(&self.green)
        } else if self.mic.load(Ordering::Relaxed) {
            Some(&self.orange)
        } else if self.location.load(Ordering::Relaxed) {
            Some(&self.blue)
        } else {
            None
        }
    }
}
