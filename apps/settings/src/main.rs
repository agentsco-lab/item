//! item-settings: item's own Settings (tracker #159, #160).
//!
//! What a phone needs on the spot, nothing more: Wi-Fi, Bluetooth, the mobile
//! network, the display, sound, the lock, the battery, date and time, and
//! about. What is set once or wants a keyboard - the agent's key, wallpapers,
//! the look, updates, backups - is item/grid's, on the computer ("More in
//! item/grid" at the bottom of the list).
//!
//! Across both panels of the Duo: the sections on the left, the one chosen on
//! the right (AdwNavigationSplitView); on one panel, one column, a section
//! over the list. The look is item's: dark, item's accent (~/.config/item/
//! accent; "auto" - the wallpaper's - is not published by item yet, coral
//! then).
//!
//! Each section reads the system's own services (timedated, UPower, and, as
//! they come, NetworkManager, BlueZ, PipeWire, droidian-fpd): nothing here
//! keeps settings of its own.
//!
//! ITEM_SETTINGS_SECTION=name opens a section; ITEM_SETTINGS_SHOT=file.png
//! draws the window into a picture two seconds after the start.

mod about;
mod battery;
mod datetime;
mod ui;

use std::cell::RefCell;
use std::collections::HashMap;

use adw::prelude::*;
use gtk::{gdk, glib};

const APP_ID: &str = "lab.agentsco.item.Settings";
const CORAL: &str = "#f08a7b";

struct Section {
    key: &'static str,
    title: &'static str,
    icon: &'static str,
    make: fn() -> gtk::Widget,
}

const SECTIONS: &[Section] = &[
    Section { key: "wifi", title: "Wi-Fi", icon: "network-wireless-symbolic", make: || ui::coming("Networks, passwords, hidden ones; forget one.", 161) },
    Section { key: "bluetooth", title: "Bluetooth", icon: "bluetooth-symbolic", make: || ui::coming("Pair, connect, forget.", 161) },
    Section { key: "mobile", title: "Mobile network", icon: "network-cellular-symbolic", make: || ui::coming("Mobile data, roaming, the network's type.", 161) },
    Section { key: "display", title: "Display", icon: "video-display-symbolic", make: || ui::coming("Brightness, night light, the screen's timeout.", 162) },
    Section { key: "sound", title: "Sound", icon: "audio-speakers-symbolic", make: || ui::coming("Outputs and volumes.", 162) },
    Section { key: "lock", title: "Lock", icon: "system-lock-screen-symbolic", make: || ui::coming("The PIN, fingers, CV ID.", 163) },
    Section { key: "battery", title: "Battery", icon: "battery-good-symbolic", make: battery::page },
    Section { key: "time", title: "Date & time", icon: "preferences-system-time-symbolic", make: datetime::page },
    Section { key: "about", title: "About", icon: "help-about-symbolic", make: about::page },
];

/// item's accent, as it keeps it: a colour, or the wallpaper's (coral here).
fn accent() -> String {
    let text = std::fs::read_to_string(glib::home_dir().join(".config/item/accent")).unwrap_or_default();
    let text = text.trim();
    if text.len() == 7 && text.starts_with('#') { text.to_owned() } else { CORAL.to_owned() }
}

fn window(app: &adw::Application) -> adw::ApplicationWindow {
    let win = adw::ApplicationWindow::builder()
        .application(app)
        .title("Settings")
        .default_width(1100)
        .default_height(720)
        .width_request(360)
        .height_request(420)
        .build();
    let split = adw::NavigationSplitView::builder().min_sidebar_width(300.0).max_sidebar_width(400.0).sidebar_width_fraction(0.42).build();
    // One column when it is one panel's width.
    let bp = adw::Breakpoint::new(adw::BreakpointCondition::parse("max-width: 760sp").expect("a breakpoint"));
    bp.add_setter(&split, "collapsed", Some(&true.to_value()));
    win.add_breakpoint(bp);

    let listing = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::Single).css_classes(["navigation-sidebar"]).build();
    for s in SECTIONS {
        let r = adw::ActionRow::builder().title(s.title).activatable(true).use_markup(false).name(s.key).build();
        r.add_prefix(&gtk::Image::from_icon_name(s.icon));
        listing.append(&r);
    }
    let more = gtk::Label::builder()
        .label("More in item/grid, on your computer: the agent and its key, wallpapers, the look, updates and backups.")
        .wrap(true)
        .xalign(0.0)
        .css_classes(["dim-label", "caption"])
        .margin_start(18)
        .margin_end(18)
        .margin_top(12)
        .margin_bottom(18)
        .build();
    let side_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
    side_box.append(&gtk::ScrolledWindow::builder().vexpand(true).hscrollbar_policy(gtk::PolicyType::Never).child(&listing).build());
    side_box.append(&more);
    let side_view = adw::ToolbarView::builder().content(&side_box).build();
    side_view.add_top_bar(&adw::HeaderBar::new());
    split.set_sidebar(Some(&adw::NavigationPage::new(&side_view, "Settings")));

    let content_view = adw::ToolbarView::new();
    content_view.add_top_bar(&adw::HeaderBar::new());
    let content = adw::NavigationPage::new(&content_view, "");
    split.set_content(Some(&content));
    win.set_content(Some(&split));

    // Each section made the first time it is shown, then kept.
    let pages: RefCell<HashMap<&'static str, gtk::Widget>> = RefCell::default();
    let show = move |key: &str, reveal: bool| {
        let Some(s) = SECTIONS.iter().find(|s| s.key == key) else { return };
        let page = pages.borrow_mut().entry(s.key).or_insert_with(s.make).clone();
        content.set_title(s.title);
        content_view.set_content(Some(&page));
        if reveal {
            split.set_show_content(true);
        }
    };
    let chosen = std::env::var("ITEM_SETTINGS_SECTION").ok().filter(|k| !k.is_empty());
    let first = chosen.clone().unwrap_or_else(|| "wifi".to_owned());
    let mut i = 0;
    while let Some(r) = listing.row_at_index(i) {
        if r.widget_name() == first {
            listing.select_row(Some(&r));
        }
        i += 1;
    }
    show(&first, chosen.is_some());
    listing.connect_row_activated(move |_, r| show(&r.widget_name(), true));
    win
}

/// The window drawn into a picture (to look at it without a screen grab).
fn snapshot(win: &adw::ApplicationWindow, path: &str) {
    let paintable = gtk::WidgetPaintable::new(Some(win));
    let snap = gtk::Snapshot::new();
    paintable.snapshot(&snap, win.width() as f64, win.height() as f64);
    let (Some(node), Some(renderer)) = (snap.to_node(), win.renderer()) else { return };
    if let Err(e) = renderer.render_texture(node, None).save_to_png(path) {
        eprintln!("item-settings: {path}: {e}");
    }
}

fn main() -> glib::ExitCode {
    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_activate(|app| {
        adw::StyleManager::default().set_color_scheme(adw::ColorScheme::ForceDark);
        let a = accent();
        let css = gtk::CssProvider::new();
        css.load_from_string(&format!("@define-color accent_bg_color {a}; @define-color accent_color {a}; @define-color accent_fg_color #1b1b1d;"));
        gtk::style_context_add_provider_for_display(&gdk::Display::default().expect("a display"), &css, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
        let win = app.active_window().and_downcast::<adw::ApplicationWindow>().unwrap_or_else(|| window(app));
        win.present();
        if let Ok(shot) = std::env::var("ITEM_SETTINGS_SHOT") {
            let app = app.clone();
            glib::timeout_add_local_once(std::time::Duration::from_secs(2), move || {
                snapshot(&win, &shot);
                app.quit();
            });
        }
    });
    app.run()
}
