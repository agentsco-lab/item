//! About: the versions this phone runs, its serial number, its space.

use adw::prelude::*;
use gtk::gio;

use crate::ui::{group, read, row, run};

fn size_words(mut n: f64) -> String {
    for unit in ["B", "KB", "MB", "GB", "TB"] {
        if n < 1000.0 || unit == "TB" {
            return if matches!(unit, "B" | "KB") { format!("{n:.0} {unit}") } else { format!("{n:.1} {unit}") };
        }
        n /= 1000.0;
    }
    unreachable!()
}

/// Whether something is mounted right at this path.
fn mounted(path: &str) -> bool {
    read("/proc/self/mounts").lines().any(|l| l.split_whitespace().nth(1) == Some(path))
}

pub fn page() -> gtk::Widget {
    let page = adw::PreferencesPage::new();
    let phone = group("This phone");
    let os_name = read("/etc/os-release")
        .lines()
        .find_map(|l| l.strip_prefix("PRETTY_NAME=").map(|v| v.trim_matches('"').to_owned()))
        .unwrap_or_else(|| "Linux".to_owned());
    let item = run(&["dpkg-query", "-W", "-f", "${Version}", "item"]);
    let port = run(&["dpkg-query", "-W", "-f", "${Version}", "adaptation-droidian-surfaceduo"]);
    let serial = read("/proc/cmdline").split_whitespace().find_map(|w| w.strip_prefix("androidboot.serialno=")).unwrap_or_default().to_owned();
    let shown = match item.split_once('~') {
        _ if item.is_empty() => "not installed".to_owned(),
        Some((v, rest)) if rest.starts_with("git") => format!("{v} (development build)"),
        Some((v, _)) => v.to_owned(),
        None => item.clone(),
    };
    phone.add(&row("item", &shown));
    phone.add(&row("System", &os_name));
    if !port.is_empty() {
        phone.add(&row("Port", &port));
    }
    phone.add(&row("Kernel", &read("/proc/sys/kernel/osrelease")));
    if !serial.is_empty() {
        phone.add(&row("Serial number", &serial));
    }
    page.add(&phone);

    let storage = group("Storage");
    for (mount, name) in [("/", "System and your files"), ("/userdata", "Data partition")] {
        if mount != "/" && !mounted(mount) {
            continue;
        }
        let Ok(info) = gio::File::for_path(mount).query_filesystem_info("filesystem::size,filesystem::free", gio::Cancellable::NONE) else { continue };
        let total = info.attribute_uint64("filesystem::size") as f64;
        let free = info.attribute_uint64("filesystem::free") as f64;
        let r = row(name, &format!("{} free of {}", size_words(free), size_words(total)));
        let bar = gtk::LevelBar::builder().min_value(0.0).max_value(1.0).value(1.0 - free / total.max(1.0)).valign(gtk::Align::Center).width_request(120).build();
        r.add_suffix(&bar);
        storage.add(&r);
    }
    page.add(&storage);
    page.upcast()
}
