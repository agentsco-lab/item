//! The pieces every section is made of.

use adw::prelude::*;

/// A row with a value on its right (selectable, to copy it).
pub fn row(title: &str, value: &str) -> adw::ActionRow {
    let r = adw::ActionRow::builder().title(title).use_markup(false).build();
    if !value.is_empty() {
        r.add_suffix(&dim(value));
    }
    r
}

/// The dimmed label rows show their values in: on one line (wrapped, a row
/// gave it its narrowest width, a word a line), its middle dropped if long.
pub fn dim(text: &str) -> gtk::Label {
    gtk::Label::builder().label(text).selectable(true).css_classes(["dim-label"]).xalign(1.0).ellipsize(gtk::pango::EllipsizeMode::Middle).width_chars(4).build()
}

pub fn group(title: &str) -> adw::PreferencesGroup {
    adw::PreferencesGroup::builder().title(title).build()
}

/// A section not made yet: what it will hold, and its ticket.
pub fn coming(what: &str, ticket: u32) -> gtk::Widget {
    let page = adw::PreferencesPage::new();
    let g = group("");
    g.add(&adw::StatusPage::builder().title("Coming").description(format!("{what}\n\nTracker #{ticket}.")).icon_name("preferences-system-symbolic").build());
    page.add(&g);
    page.upcast()
}

/// A command's output, or "" when it is missing or fails.
pub fn run(argv: &[&str]) -> String {
    std::process::Command::new(argv[0])
        .args(&argv[1..])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .unwrap_or_default()
}

/// A file's text, trimmed, or "".
pub fn read(path: &str) -> String {
    std::fs::read_to_string(path).map(|s| s.trim().to_owned()).unwrap_or_default()
}
