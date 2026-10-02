//! `org.gnome.Mutter.DisplayConfig`'s `PowerSaveMode`, as mutter and phosh
//! serve it: 0 while the screen is lit, 3 (off) while it is dark, and
//! PropertiesChanged at each change.
//!
//! The port's daemons go by it: sfduo-posture stops reading the hinge while
//! the screen is dark (it polled it 25 times a second all night under item,
//! some 15 % of a core), sfduo-brightness puts the backlight back as it
//! lights. Set from outside (gsd-power), the screen goes dark or lights as
//! asked. Only this property: the rest of the interface is mutter's.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use smithay::reexports::calloop::ping::Ping;
use zbus::zvariant::Value;

const NAME: &str = "org.gnome.Mutter.DisplayConfig";
const PATH: &str = "/org/gnome/Mutter/DisplayConfig";

#[derive(Default)]
struct Shared {
    mode: i32,
    /// A mode asked for from outside, not yet done.
    asked: Option<i32>,
}

struct Config {
    shared: Arc<Mutex<Shared>>,
    wake: Ping,
}

#[zbus::interface(name = "org.gnome.Mutter.DisplayConfig")]
impl Config {
    #[zbus(property)]
    fn power_save_mode(&self) -> i32 {
        self.shared.lock().unwrap().mode
    }

    #[zbus(property)]
    fn set_power_save_mode(&mut self, mode: i32) {
        tracing::info!("display: power save mode {mode} asked");
        self.shared.lock().unwrap().asked = Some(mode);
        self.wake.ping();
    }
}

pub struct Display {
    shared: Arc<Mutex<Shared>>,
    conn: Option<zbus::blocking::Connection>,
}

impl Display {
    pub fn new(wake: Ping) -> Display {
        let shared: Arc<Mutex<Shared>> = Default::default();
        let conn = zbus::blocking::connection::Builder::session()
            .and_then(|b| b.name(NAME))
            .and_then(|b| b.serve_at(PATH, Config { shared: shared.clone(), wake }))
            .and_then(|b| b.build());
        match &conn {
            Ok(_) => tracing::info!("display: {NAME}'s PowerSaveMode"),
            Err(e) => tracing::warn!("display: not on the bus: {e}"),
        }
        Display { shared, conn: conn.ok() }
    }

    /// The screen lit or dark: told if it changed.
    pub fn set_dark(&self, dark: bool) {
        let mode = if dark { 3 } else { 0 };
        {
            let mut s = self.shared.lock().unwrap();
            if s.mode == mode {
                return;
            }
            s.mode = mode;
        }
        if let Some(conn) = &self.conn {
            let mut changed: HashMap<&str, Value> = HashMap::new();
            changed.insert("PowerSaveMode", Value::I32(mode));
            let _ = conn.emit_signal(None::<&str>, PATH, "org.freedesktop.DBus.Properties", "PropertiesChanged", &(NAME, changed, Vec::<String>::new()));
        }
    }

    /// A mode asked for from outside, once: whether dark.
    pub fn take_asked(&self) -> Option<bool> {
        self.shared.lock().unwrap().asked.take().map(|m| m != 0)
    }
}
