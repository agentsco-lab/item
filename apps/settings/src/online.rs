//! Online: whether the shell may reach the network by itself. item sends
//! nothing anywhere unless the owner turns this on; with it on, the system
//! screen asks met.no for the weather of GNOME Weather's city (its
//! coordinates, not the phone's) every half hour and GitHub for the port's
//! latest release once a day. Apps, the agent and the keyboard's voice are
//! their own: each with its own key or sign-in, none of this switch.

use adw::prelude::*;

use crate::keyboard::{home_conf, switch};
use crate::ui::{group, write_conf};

pub fn page() -> gtk::Widget {
    let page = adw::PreferencesPage::new();
    let g = group("Online");
    g.add(&switch("Weather and updates", "The system screen asks met.no for the weather of Weather's city and GitHub for the port's latest release. Off: item reaches nothing by itself.", home_conf("online") == "on", |on| {
        write_conf("online", if on { "on\n" } else { "off\n" });
    }));
    page.add(&g);
    page.upcast()
}
