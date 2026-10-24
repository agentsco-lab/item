//! Turn: how the frame turns between the book and the stacked shell. By
//! the dock's turn button (the default: a tap makes that panel the deck,
//! another the book again), or by the sensors - the hinge, gravity and
//! the gyroscope read while the display is lit, the frame turning as the
//! phone is stood up. The compositor reads this at its start.

use adw::prelude::*;

use crate::keyboard::{home_conf, switch};
use crate::ui::{group, write_conf};

pub fn page() -> gtk::Widget {
    let page = adw::PreferencesPage::new();
    let g = group("Turn");
    g.add(&switch(
        "By the sensors",
        "The frame turns as the hinge and the gyroscope say the phone is held: stood up, the upper panel is the screen. Off: the dock's turn button alone, and gravity and the gyroscope are not read. Holding the dock's turn button switches this too, at once (green while the sensors have it); this switch takes effect at item's next start (sudo item-switch item).",
        home_conf("turn") == "sensors",
        |on| write_conf("turn", if on { "sensors\n" } else { "button\n" }),
    ));
    page.add(&g);
    page.upcast()
}
