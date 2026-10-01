//! What a status bar would say, for the lock screen's left panel: the
//! battery and the network, read on a thread every 30 s (and at once when
//! asked), the loop woken with the ping when it changed.
//!
//! The battery from the kernel (`/sys/class/power_supply/battery`), the
//! network from NetworkManager (`nmcli`, the cached scan only): the Wi-Fi
//! network's name and signal, else mobile data, else offline.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use smithay::reexports::calloop::ping::Ping;

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Facts {
    pub battery: u32,
    pub charging: bool,
    /// Plugged in and full.
    pub charged: bool,
    /// What the network is called, and its icon's name.
    pub net: String,
    pub net_icon: &'static str,
}

impl Facts {
    /// Adwaita's battery icon for it.
    pub fn battery_icon(&self) -> String {
        let level = (self.battery.min(100) / 10) * 10;
        let state = if self.charged && level == 100 {
            "-charged"
        } else if self.charging {
            "-charging"
        } else {
            ""
        };
        format!("battery-level-{level}{state}-symbolic")
    }
}

pub struct Status {
    shared: Arc<Mutex<(Option<Facts>, bool)>>,
    asks: std::sync::mpsc::Sender<()>,
}

fn run(cmd: &str, args: &[&str]) -> String {
    std::process::Command::new(cmd)
        .args(args)
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default()
}

fn read() -> Facts {
    let file = |f: &str| std::fs::read_to_string(format!("/sys/class/power_supply/battery/{f}")).map(|s| s.trim().to_owned()).unwrap_or_default();
    let battery = file("capacity").parse().unwrap_or(0);
    let state = file("status");
    let plugged = std::fs::read_to_string("/sys/class/power_supply/usb/online").map(|s| s.trim() == "1").unwrap_or(false);
    let charging = state == "Charging";
    let charged = state == "Full" || (state == "Not charging" && plugged);
    let devices = run("nmcli", &["-t", "-f", "TYPE,STATE,CONNECTION", "device"]);
    let up = |kind: &str| devices.lines().find(|l| l.starts_with(&format!("{kind}:connected:"))).map(|l| l.splitn(3, ':').nth(2).unwrap_or("").to_owned());
    let (net, net_icon) = if let Some(name) = up("wifi") {
        let signal = run("nmcli", &["-t", "-f", "ACTIVE,SIGNAL", "dev", "wifi", "list", "--rescan", "no"])
            .lines()
            .find_map(|l| l.strip_prefix("yes:").and_then(|s| s.parse::<u32>().ok()))
            .unwrap_or(0);
        let icon = match signal {
            81.. => "network-wireless-signal-excellent-symbolic",
            56..=80 => "network-wireless-signal-good-symbolic",
            31..=55 => "network-wireless-signal-ok-symbolic",
            6..=30 => "network-wireless-signal-weak-symbolic",
            _ => "network-wireless-signal-none-symbolic",
        };
        (name, icon)
    } else if up("gsm").is_some() {
        ("Mobile data".to_owned(), "network-cellular-signal-good-symbolic")
    } else {
        ("Offline".to_owned(), "network-offline-symbolic")
    };
    Facts { battery, charging, charged, net, net_icon }
}

impl Status {
    pub fn new(wake: Ping) -> Status {
        let shared = Arc::new(Mutex::new((None, false)));
        let (asks, rx) = std::sync::mpsc::channel::<()>();
        let worker = shared.clone();
        std::thread::spawn(move || {
            let mut last: Option<Facts> = None;
            loop {
                let facts = read();
                if last.as_ref() != Some(&facts) {
                    *worker.lock().unwrap() = (Some(facts.clone()), true);
                    last = Some(facts);
                    wake.ping();
                }
                match rx.recv_timeout(Duration::from_secs(30)) {
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                    _ => {}
                }
            }
        });
        Status { shared, asks }
    }

    /// Read again now (the lock screen came up).
    pub fn ask(&self) {
        let _ = self.asks.send(());
    }

    /// The facts, if they changed since last taken.
    pub fn take(&self) -> Option<Facts> {
        let mut shared = self.shared.lock().unwrap();
        if !shared.1 {
            return None;
        }
        shared.1 = false;
        shared.0.clone()
    }
}
