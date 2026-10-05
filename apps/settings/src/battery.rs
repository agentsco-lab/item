//! The battery through UPower's display device, and its health from the
//! power supply.

use adw::prelude::*;
use gtk::{gio, glib};

use crate::ui::{dim, group, read, row};

struct Shown {
    bar: gtk::LevelBar,
    level: gtk::Label,
    state: gtk::Label,
}

impl Shown {
    fn show(&self, p: &gio::DBusProxy) {
        let get = |name: &str| p.cached_property(name);
        if let Some(pct) = get("Percentage").and_then(|v| v.get::<f64>()) {
            self.bar.set_value(pct);
            self.level.set_label(&format!("{pct:.0} %"));
        }
        let mut words = match get("State").and_then(|v| v.get::<u32>()).unwrap_or(0) {
            1 => "Charging",
            2 | 6 => "On battery",
            3 => "Empty",
            4 => "Full",
            5 => "Not charging",
            _ => "Unknown",
        }
        .to_owned();
        let on_battery = words == "On battery";
        let left = get(if on_battery { "TimeToEmpty" } else { "TimeToFull" }).and_then(|v| v.get::<i64>()).unwrap_or(0);
        if left > 0 {
            let (h, m) = (left / 3600, left / 60 % 60);
            words += &format!(" · {h} h {m} min {}", if on_battery { "left" } else { "to full" });
        }
        self.state.set_label(&words);
    }
}

pub fn page() -> gtk::Widget {
    let page = adw::PreferencesPage::new();
    let g = group("");
    let charge = row("Charge", "");
    let bar = gtk::LevelBar::builder().min_value(0.0).max_value(100.0).valign(gtk::Align::Center).width_request(140).build();
    let level = dim("");
    charge.add_suffix(&level);
    charge.add_suffix(&bar);
    g.add(&charge);
    let state_row = row("State", "");
    let state = dim("");
    state_row.add_suffix(&state);
    g.add(&state_row);
    for (file, title) in [("health", "Health"), ("cycle_count", "Charge cycles")] {
        let v = read(&format!("/sys/class/power_supply/battery/{file}"));
        if !v.is_empty() {
            g.add(&row(title, &v));
        }
    }
    page.add(&g);

    let shown = Shown { bar, level, state };
    gio::DBusProxy::for_bus(
        gio::BusType::System,
        gio::DBusProxyFlags::NONE,
        None,
        "org.freedesktop.UPower",
        "/org/freedesktop/UPower/devices/DisplayDevice",
        "org.freedesktop.UPower.Device",
        gio::Cancellable::NONE,
        move |r| {
            let Ok(proxy) = r else { return };
            shown.show(&proxy);
            proxy.connect_local("g-properties-changed", false, move |args| {
                if let Some(p) = args[0].get::<gio::DBusProxy>().ok() {
                    shown.show(&p);
                }
                None::<glib::Value>
            });
        },
    );
    page.upcast()
}
