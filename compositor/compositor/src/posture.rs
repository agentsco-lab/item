//! The hinge's angle, as the port's sfduo-posture tells it on the session
//! bus (org.sfduo.Posture: Angle, smoothed, up to sixty times a second while
//! the display is lit): 180 flat, less folded like a book, more folded back.
//! And, folded back to back, which panel faces the user (Facing: Microsoft's
//! own posture sensor, #116).

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};

use smithay::reexports::calloop::ping::Ping;

const NAME: &str = "org.sfduo.Posture";
const PATH: &str = "/org/sfduo/Posture";
/// Folded back: the left panel's glass from here (degrees) to here.
const FOLD_FROM: f64 = 190.0;
const FOLD_TO: f64 = 330.0;

pub struct Posture {
    angle: Arc<AtomicU64>,
    changed: Arc<AtomicBool>,
    /// Back to back, the panel facing the user: 0 not known, 1 the left, 2
    /// the right.
    facing: Arc<AtomicU8>,
    /// The far panel's dark (#117), faded in and out.
    far: Mutex<Far>,
}

/// The far panel's backlight fades out over this long, and back in.
const FAR_OUT_NS: f64 = 400e6;
const FAR_IN_NS: f64 = 300e6;
/// Folded back this far (degrees), the far panel goes dark; it lights again
/// only below this, so it does not blink at the threshold.
const DARK_FROM: f64 = 330.0;
const LIT_BELOW: f64 = 300.0;

/// The panels' dark: how dark each is (0 lit, 1 dark), its backlight's level
/// before, what was last written, when last stepped; which is wanted dark;
/// whether the display was blank.
#[derive(Default)]
struct Far {
    dim: [f64; 2],
    level: [Option<u32>; 2],
    written: [Option<u32>; 2],
    at: u64,
    want: Option<usize>,
    blank: bool,
}

fn backlight(panel: usize) -> String {
    format!("/sys/class/backlight/panel{panel}-backlight/brightness")
}

fn facing_of(v: &str) -> u8 {
    match v {
        "left" => 1,
        "right" => 2,
        _ => 0,
    }
}

impl Posture {
    pub fn new(wake: Ping) -> Posture {
        let angle = Arc::new(AtomicU64::new(180f64.to_bits()));
        let changed: Arc<AtomicBool> = Default::default();
        let facing: Arc<AtomicU8> = Default::default();
        let (a, c, f) = (angle.clone(), changed.clone(), facing.clone());
        let _ = std::thread::Builder::new().name("posture".into()).spawn(move || {
            let Ok(bus) = zbus::blocking::Connection::session() else { return };
            let set = |v: f64| {
                if v.is_finite() && v.to_bits() != a.swap(v.to_bits(), Ordering::Relaxed) {
                    c.store(true, Ordering::Relaxed);
                    wake.ping();
                }
            };
            let get = |name: &str| {
                bus.call_method(Some(NAME), PATH, Some("org.freedesktop.DBus.Properties"), "Get", &(NAME, name))
                    .ok()
                    .and_then(|r| r.body().deserialize::<zbus::zvariant::OwnedValue>().ok())
            };
            if let Some(v) = get("Angle").and_then(|v| f64::try_from(v).ok()) {
                set(v);
            }
            // Not known (open, or turning) keeps the last one known: the fold
            // starts on the side the phone was last held, rather than on the
            // left and jumping across near the end.
            let set_facing = |v: &str| {
                if facing_of(v) != 0 && f.swap(facing_of(v), Ordering::Relaxed) != facing_of(v) {
                    tracing::info!("posture: facing the user: {v}");
                    c.store(true, Ordering::Relaxed);
                    wake.ping();
                }
            };
            if let Some(v) = get("Facing").and_then(|v| String::try_from(v).ok()) {
                set_facing(&v);
            }
            let rule = zbus::MatchRule::builder()
                .msg_type(zbus::message::Type::Signal)
                .interface("org.freedesktop.DBus.Properties")
                .and_then(|b| b.member("PropertiesChanged"))
                .and_then(|b| b.path(PATH))
                .map(|b| b.build());
            let Ok(rule) = rule else { return };
            let Ok(signals) = zbus::blocking::MessageIterator::for_match_rule(rule, &bus, Some(64)) else { return };
            tracing::info!("posture: following {NAME}'s Angle");
            type Changed = (String, std::collections::HashMap<String, zbus::zvariant::OwnedValue>, Vec<String>);
            for msg in signals.flatten() {
                let Ok((iface, props, _)) = msg.body().deserialize::<Changed>() else { continue };
                if iface != NAME {
                    continue;
                }
                if let Some(v) = props.get("Angle").and_then(|v| f64::try_from(v.try_clone().ok()?).ok()) {
                    set(v);
                }
                if let Some(v) = props.get("Facing").and_then(|v| String::try_from(v.try_clone().ok()?).ok()) {
                    set_facing(&v);
                }
            }
        });
        Posture { angle, changed, facing, far: Mutex::new(Far::default()) }
    }

    pub fn angle(&self) -> f64 {
        f64::from_bits(self.angle.load(Ordering::Relaxed))
    }

    /// Whether the angle changed since last asked.
    pub fn take_changed(&self) -> bool {
        self.changed.swap(false, Ordering::Relaxed)
    }

    /// How far folded back past flat (#117): 0 up to FOLD_FROM degrees, 1
    /// at FOLD_TO and beyond, eased between.
    pub fn fold_back(&self) -> f64 {
        let t = ((self.angle() - FOLD_FROM) / (FOLD_TO - FOLD_FROM)).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    }

    /// The panel facing the user, folded back: the right one when the
    /// posture sensor says so, else the left (as before it was known).
    pub fn near(&self) -> usize {
        if self.facing.load(Ordering::Relaxed) == 2 { 1 } else { 0 }
    }

    /// Each frame, folded back and locked or not (`folded`): the far panel
    /// fades dark past DARK_FROM, and back below LIT_BELOW or when no longer
    /// folded - its backlight stepped down and up, its level kept and put
    /// back. Left alone while the display is blank (the lock's dark has the
    /// backlights), and written again after it.
    pub fn far_dark(&self, folded: bool, blank: bool, now_ns: u64) {
        let Ok(mut far) = self.far.lock() else { return };
        let angle = self.angle();
        let deep = angle >= DARK_FROM || (far.want.is_some() && angle >= LIT_BELOW);
        far.want = (folded && deep).then(|| 1 - self.near());
        if blank {
            far.blank = true;
            far.at = now_ns;
            return;
        }
        if far.blank {
            far.blank = false;
            far.written = [None; 2];
        }
        let dt = now_ns.saturating_sub(far.at).min(100_000_000) as f64;
        far.at = now_ns;
        for p in 0..2 {
            let target = if far.want == Some(p) { 1.0 } else { 0.0 };
            if far.dim[p] == 0.0 && target == 0.0 {
                continue;
            }
            if far.dim[p] == 0.0 {
                let Some(level) = std::fs::read_to_string(backlight(p)).ok().and_then(|s| s.trim().parse::<u32>().ok()) else { continue };
                far.level[p] = Some(level);
                tracing::info!("posture: panel {p} fading dark, the far one (was {level})");
            }
            far.dim[p] = if target > far.dim[p] { (far.dim[p] + dt / FAR_OUT_NS).min(1.0) } else { (far.dim[p] - dt / FAR_IN_NS).max(0.0) };
            let Some(level) = far.level[p] else { continue };
            let d = far.dim[p];
            let value = (level as f64 * (1.0 - d * d * (3.0 - 2.0 * d))).round() as u32;
            if far.written[p] != Some(value) {
                let _ = std::fs::write(backlight(p), value.to_string());
                far.written[p] = Some(value);
            }
            if far.dim[p] == 0.0 {
                far.level[p] = None;
                far.written[p] = None;
                tracing::info!("posture: panel {p} lit again ({level})");
            }
        }
    }

    /// The far panel's backlight still fading: frames wanted.
    pub fn far_moving(&self) -> bool {
        let Ok(far) = self.far.lock() else { return false };
        !far.blank && (0..2).any(|p| far.dim[p] != if far.want == Some(p) { 1.0 } else { 0.0 })
    }

    /// How far folded like a book, for the dock: 0 flat (and folded back),
    /// 1 at 90 degrees and below, eased between.
    pub fn book(&self) -> f64 {
        let t = ((180.0 - self.angle()) / 90.0).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    }
}
