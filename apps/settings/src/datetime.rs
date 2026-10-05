//! Date and time through timedated: the zone, and the time from the network
//! or not (polkit asks when it must).

use std::cell::Cell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gio, glib};

use crate::ui::{dim, group, row, run};

const NAME: &str = "org.freedesktop.timedate1";
const PATH: &str = "/org/freedesktop/timedate1";

struct DateTime {
    bus: Option<gio::DBusConnection>,
    auto: adw::SwitchRow,
    zone: adw::ComboRow,
    zones: Vec<String>,
    /// Set while the rows are filled from timedated, so it is not written back.
    loading: Cell<bool>,
}

impl DateTime {
    fn get(&self, name: &str) -> Option<glib::Variant> {
        let v = self.bus.as_ref()?.call_sync(Some(NAME), PATH, "org.freedesktop.DBus.Properties", "Get", Some(&(NAME, name).to_variant()), None, gio::DBusCallFlags::NONE, 2000, gio::Cancellable::NONE).ok()?;
        v.child_value(0).as_variant()
    }

    fn load(&self) {
        self.loading.set(true);
        self.auto.set_active(self.get("NTP").and_then(|v| v.get::<bool>()).unwrap_or(false));
        self.auto.set_sensitive(self.get("CanNTP").and_then(|v| v.get::<bool>()).unwrap_or(false));
        let tz = self.get("Timezone").and_then(|v| v.get::<String>()).unwrap_or_else(|| "UTC".to_owned());
        if let Some(i) = self.zones.iter().position(|z| *z == tz) {
            self.zone.set_selected(i as u32);
        }
        self.loading.set(false);
    }

    fn call(self: &Rc<Self>, method: &str, args: glib::Variant) {
        let Some(bus) = &self.bus else { return };
        let me = self.clone();
        bus.call(Some(NAME), PATH, NAME, method, Some(&args), None, gio::DBusCallFlags::ALLOW_INTERACTIVE_AUTHORIZATION, 60000, gio::Cancellable::NONE, move |r| {
            if let Err(e) = r {
                eprintln!("item-settings: date and time: {}", e.message());
            }
            me.load();
        });
    }
}

pub fn page() -> gtk::Widget {
    let page = adw::PreferencesPage::new();
    let g = group("");
    let now = row("Now", "");
    let now_label = dim("");
    now.add_suffix(&now_label);
    g.add(&now);
    let auto = adw::SwitchRow::builder().title("Set automatically").subtitle("From the network").use_markup(false).build();
    g.add(&auto);
    let mut zones: Vec<String> = run(&["timedatectl", "list-timezones"]).lines().map(str::to_owned).collect();
    if zones.is_empty() {
        zones.push("UTC".to_owned());
    }
    let names: Vec<&str> = zones.iter().map(String::as_str).collect();
    let zone = adw::ComboRow::builder().title("Time zone").model(&gtk::StringList::new(&names)).enable_search(true).build();
    g.add(&zone);
    page.add(&g);

    let bus = gio::bus_get_sync(gio::BusType::System, gio::Cancellable::NONE).ok();
    let dt = Rc::new(DateTime { bus, auto, zone, zones, loading: Cell::new(false) });
    dt.load();
    let d = dt.clone();
    dt.auto.connect_active_notify(move |s| {
        if !d.loading.get() {
            d.call("SetNTP", (s.is_active(), true).to_variant());
        }
    });
    let d = dt.clone();
    dt.zone.connect_selected_notify(move |c| {
        if !d.loading.get() {
            if let Some(z) = d.zones.get(c.selected() as usize) {
                d.call("SetTimezone", (z.as_str(), true).to_variant());
            }
        }
    });

    let tick = move || {
        if let Ok(t) = glib::DateTime::now_local().and_then(|t| t.format("%A %-d %B, %H:%M")) {
            now_label.set_label(&t);
        }
    };
    tick();
    // Weakly: the label's page goes when the window does.
    let page_weak = page.downgrade();
    glib::timeout_add_seconds_local(1, move || {
        if page_weak.upgrade().is_none() {
            return glib::ControlFlow::Break;
        }
        tick();
        glib::ControlFlow::Continue
    });
    page.upcast()
}
