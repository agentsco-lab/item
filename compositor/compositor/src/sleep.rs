//! The phone's sleep, as logind says it (`PrepareForSleep`): what woke it.
//!
//! After a resume `/sys/power/pm_wakeup_irq` holds the interrupt that woke
//! the SoC, named in `/proc/interrupts`. The power key's (pon_kpdpwr) lights
//! the screen: the press that woke the phone is spent on waking it and
//! never arrives as a key. The modem's is a call or a message - the call
//! screen lights itself. Wi-Fi's (a packet) leaves the screen dark. The
//! port's sleep hook that pressed KEY_WAKEUP after every resume does not
//! under item.

use std::sync::{Arc, Mutex};

use smithay::reexports::calloop::ping::Ping;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Woken {
    PowerKey,
    Modem,
    Wifi,
    Other,
}

/// What woke the phone, from the interrupt's name.
fn woken_by() -> (Woken, String) {
    let irq = std::fs::read_to_string("/sys/power/pm_wakeup_irq").ok().and_then(|s| s.trim().parse::<u32>().ok());
    let Some(irq) = irq else { return (Woken::Other, "unknown".into()) };
    let name = std::fs::read_to_string("/proc/interrupts")
        .unwrap_or_default()
        .lines()
        .find(|l| l.trim_start().split(':').next().and_then(|n| n.trim().parse::<u32>().ok()) == Some(irq))
        .and_then(|l| l.split_whitespace().last().map(str::to_owned))
        .unwrap_or_default();
    let kind = if name.contains("kpdpwr") {
        Woken::PowerKey
    } else if name.contains("modem") || name.contains("smp2p") || name.contains("ipa") {
        Woken::Modem
    } else if name.contains("WLAN") || name.contains("wlan") {
        Woken::Wifi
    } else {
        Woken::Other
    };
    (kind, format!("irq {irq} {name}"))
}

pub struct Sleep {
    woken: Arc<Mutex<Option<Woken>>>,
}

impl Sleep {
    pub fn new(wake: Ping) -> Sleep {
        let woken: Arc<Mutex<Option<Woken>>> = Default::default();
        let w = woken.clone();
        std::thread::Builder::new()
            .name("sleep".into())
            .spawn(move || {
                let Ok(bus) = zbus::blocking::Connection::system() else { return };
                let rule = zbus::MatchRule::builder()
                    .msg_type(zbus::message::Type::Signal)
                    .interface("org.freedesktop.login1.Manager")
                    .and_then(|b| b.member("PrepareForSleep"))
                    .map(|b| b.build());
                let Ok(rule) = rule else { return };
                let Ok(signals) = zbus::blocking::MessageIterator::for_match_rule(rule, &bus, Some(8)) else { return };
                for msg in signals.flatten() {
                    let Ok(going) = msg.body().deserialize::<bool>() else { continue };
                    if going {
                        tracing::info!("sleep: going to sleep");
                        continue;
                    }
                    let (kind, what) = woken_by();
                    tracing::info!("sleep: woken by {what} ({kind:?})");
                    *w.lock().unwrap() = Some(kind);
                    wake.ping();
                }
            })
            .expect("sleep thread");
        Sleep { woken }
    }

    /// What woke the phone, once, after a resume.
    pub fn take_woken(&self) -> Option<Woken> {
        self.woken.lock().unwrap().take()
    }
}
