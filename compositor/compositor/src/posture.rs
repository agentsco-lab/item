//! The hinge's angle, as the port's sfduo-posture tells it on the session
//! bus (org.sfduo.Posture: Angle, smoothed, up to sixty times a second while
//! the display is lit): 180 flat, less folded like a book, more folded back.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use smithay::reexports::calloop::ping::Ping;

const NAME: &str = "org.sfduo.Posture";
const PATH: &str = "/org/sfduo/Posture";

pub struct Posture {
    angle: Arc<AtomicU64>,
    changed: Arc<AtomicBool>,
}

impl Posture {
    pub fn new(wake: Ping) -> Posture {
        let angle = Arc::new(AtomicU64::new(180f64.to_bits()));
        let changed: Arc<AtomicBool> = Default::default();
        let (a, c) = (angle.clone(), changed.clone());
        let _ = std::thread::Builder::new().name("posture".into()).spawn(move || {
            let Ok(bus) = zbus::blocking::Connection::session() else { return };
            let set = |v: f64| {
                if v.is_finite() && v.to_bits() != a.swap(v.to_bits(), Ordering::Relaxed) {
                    c.store(true, Ordering::Relaxed);
                    wake.ping();
                }
            };
            let get = || {
                bus.call_method(Some(NAME), PATH, Some("org.freedesktop.DBus.Properties"), "Get", &(NAME, "Angle"))
                    .ok()
                    .and_then(|r| r.body().deserialize::<zbus::zvariant::OwnedValue>().ok())
                    .and_then(|v| f64::try_from(v).ok())
            };
            if let Some(v) = get() {
                set(v);
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
            }
        });
        Posture { angle, changed }
    }

    pub fn angle(&self) -> f64 {
        f64::from_bits(self.angle.load(Ordering::Relaxed))
    }

    /// Whether the angle changed since last asked.
    pub fn take_changed(&self) -> bool {
        self.changed.swap(false, Ordering::Relaxed)
    }

    /// How far folded like a book, for the dock: 0 flat (and folded back),
    /// 1 at 90 degrees and below, eased between.
    pub fn book(&self) -> f64 {
        let t = ((180.0 - self.angle()) / 90.0).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    }
}
