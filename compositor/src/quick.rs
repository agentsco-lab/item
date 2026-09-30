//! The shades' contents, item's split (patches/phosh/0003: "each shade has
//! its own job"): the left shade keeps the settings - brightness, volume,
//! the quick settings - and the right one keeps what is going on - here the
//! open windows, each with a close button. Both keep the time, the date and
//! the battery in their head (shade.rs).
//!
//! The system is read and set on a thread of its own, never in a frame:
//! - the brightness through logind's Session.SetBrightness, as gsd-power
//!   does, both panels' backlights; read from sysfs;
//! - the volume and the mute through pactl (PulseAudio on the port);
//! - Wi-Fi and airplane mode through nmcli, Bluetooth through rfkill;
//! - the dark style through gsettings (org.gnome.desktop.interface
//!   color-scheme).
//!
//! A slider's value goes to the thread as the finger moves; the thread takes
//! only the latest, so a drag does not queue commands.

use std::collections::HashMap;
use std::process::Command;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use resvg::tiny_skia;
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement};
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::reexports::calloop::ping::Ping;
use smithay::utils::{Logical, Point, Rectangle, Transform};

use crate::layout::SCALE;
use crate::shade::ShellElement;
use crate::text::{Font, Label};

/// What the system is, as last read.
#[derive(Clone, Default)]
pub struct Sys {
    pub brightness: f64,
    pub volume: f64,
    pub muted: bool,
    pub wifi: bool,
    pub bluetooth: bool,
    pub dark: bool,
    pub airplane: bool,
}

enum Job {
    Read,
    Brightness(f64),
    Volume(f64),
    Toggle(Tile, bool),
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Tile {
    Wifi,
    Bluetooth,
    Mute,
    Dark,
    Airplane,
    Settings,
}

const TILES: [Tile; 6] = [Tile::Wifi, Tile::Bluetooth, Tile::Mute, Tile::Dark, Tile::Airplane, Tile::Settings];

impl Tile {
    fn label(self) -> &'static str {
        match self {
            Tile::Wifi => "Wi-Fi",
            Tile::Bluetooth => "Bluetooth",
            Tile::Mute => "Без звука",
            Tile::Dark => "Тёмная тема",
            Tile::Airplane => "Авиарежим",
            Tile::Settings => "Настройки",
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Tile::Wifi => "/usr/share/icons/Adwaita/symbolic/devices/network-wireless-symbolic.svg",
            Tile::Bluetooth => "/usr/share/icons/Adwaita/symbolic/status/bluetooth-active-symbolic.svg",
            Tile::Mute => "/usr/share/icons/Adwaita/symbolic/status/audio-volume-muted-symbolic.svg",
            Tile::Dark => "/usr/share/icons/Adwaita/symbolic/status/weather-clear-night-symbolic.svg",
            Tile::Airplane => "/usr/share/icons/Adwaita/symbolic/status/airplane-mode-symbolic.svg",
            Tile::Settings => "/usr/share/icons/hicolor/symbolic/apps/org.gnome.Settings-symbolic.svg",
        }
    }

    fn on(self, s: &Sys) -> bool {
        match self {
            Tile::Wifi => s.wifi,
            Tile::Bluetooth => s.bluetooth,
            Tile::Mute => s.muted,
            Tile::Dark => s.dark,
            Tile::Airplane => s.airplane,
            Tile::Settings => false,
        }
    }
}

/// A control of a shade, by where it is.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Control {
    Brightness,
    Volume,
    Tile(Tile),
    /// The close button of the right shade's row `i`.
    Close(usize),
    /// The rest of row `i`: a notification's action.
    Row(usize),
}

/// A row of the right shade: an open window.
pub struct Row {
    pub app_id: String,
    pub name: String,
    pub icon: Option<String>,
    /// A notification's line of body.
    pub detail: Option<String>,
}

// The layout, logical px from the panel's left and from the sheet's bottom
// edge (the content hangs from it, as the head does).
const SIDE: f64 = 30.0;
const SLIDER_H: f64 = 48.0;
const BRIGHTNESS_Y: f64 = -520.0;
const VOLUME_Y: f64 = -456.0;
const TILE_H: f64 = 84.0;
const TILE_GAP: f64 = 12.0;
const TILES_Y: f64 = -372.0;
const ICON: i32 = 24;
const ROWS_Y: f64 = -520.0;
const ROW_H: f64 = 64.0;
const MAX_ROWS: usize = 7;

pub struct Quick {
    pub sys: Arc<Mutex<Sys>>,
    jobs: mpsc::Sender<Job>,
    font: Option<Font>,
    tile_on: MemoryRenderBuffer,
    tile_off: MemoryRenderBuffer,
    track: MemoryRenderBuffer,
    fill: MemoryRenderBuffer,
    knob: MemoryRenderBuffer,
    row_bg: MemoryRenderBuffer,
    banner_bg: MemoryRenderBuffer,
    icons: HashMap<String, MemoryRenderBuffer>,
    labels: std::cell::RefCell<HashMap<String, Label>>,
    app_icons: std::cell::RefCell<HashMap<String, Option<MemoryRenderBuffer>>>,
    /// The windows' row caption when there are none.
    empty: String,
    /// The banner's card as last drawn, and its notification, for touches.
    pub banner_at: std::cell::Cell<Option<(Rectangle<f64, Logical>, u32)>>,
}

impl Quick {
    pub fn new(wake: Ping) -> Quick {
        let sys = Arc::new(Mutex::new(Sys::default()));
        let (tx, rx) = mpsc::channel::<Job>();
        let shared = sys.clone();
        std::thread::Builder::new()
            .name("quick settings".into())
            .spawn(move || worker(rx, shared, wake))
            .expect("quick settings thread");
        let panel_w = crate::layout::panels()[0].size.w as f64;
        let tile_w = (panel_w - 2.0 * SIDE - 2.0 * TILE_GAP) / 3.0;
        let track_w = panel_w - 2.0 * SIDE - 48.0;
        let mut icons = HashMap::new();
        for path in TILES.iter().map(|t| t.icon()).chain([
            "/usr/share/icons/Adwaita/symbolic/status/display-brightness-symbolic.svg",
            "/usr/share/icons/Adwaita/symbolic/status/audio-volume-high-symbolic.svg",
            "/usr/share/icons/Adwaita/symbolic/ui/window-close-symbolic.svg",
        ]) {
            if let Some(b) = symbolic(path, ICON) {
                icons.insert(path.to_owned(), b);
            }
        }
        Quick {
            sys,
            jobs: tx,
            font: Font::load(&["/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf", "/usr/share/fonts/truetype/lato/Lato-Regular.ttf"]),
            tile_on: crate::grid::rounded(tile_w, TILE_H, 18.0, [0x35, 0x84, 0xe4, 255]),
            tile_off: crate::grid::rounded(tile_w, TILE_H, 18.0, [30, 30, 30, 30]),
            track: crate::grid::rounded(track_w, 8.0, 4.0, [40, 40, 40, 40]),
            fill: crate::grid::rounded(track_w, 8.0, 4.0, [235, 235, 235, 235]),
            knob: crate::grid::rounded(24.0, 24.0, 12.0, [255, 255, 255, 255]),
            row_bg: crate::grid::rounded(panel_w - 2.0 * SIDE, ROW_H - 8.0, 14.0, [20, 20, 20, 20]),
            banner_bg: crate::grid::rounded(panel_w - 32.0, 72.0, 18.0, [36, 40, 46, 245]),
            icons,
            labels: Default::default(),
            app_icons: Default::default(),
            empty: "Нет окон и уведомлений".into(),
            banner_at: std::cell::Cell::new(None),
        }
    }

    /// The system read again (the shade is being pulled).
    pub fn read(&self) {
        let _ = self.jobs.send(Job::Read);
    }

    fn tile_rect(i: usize) -> Rectangle<f64, Logical> {
        let panel_w = crate::layout::panels()[0].size.w as f64;
        let w = (panel_w - 2.0 * SIDE - 2.0 * TILE_GAP) / 3.0;
        let (col, row) = ((i % 3) as f64, (i / 3) as f64);
        Rectangle::new((SIDE + col * (w + TILE_GAP), TILES_Y + row * (TILE_H + TILE_GAP)).into(), (w, TILE_H).into())
    }

    fn slider_rect(y: f64) -> Rectangle<f64, Logical> {
        let panel_w = crate::layout::panels()[0].size.w as f64;
        Rectangle::new((SIDE, y).into(), (panel_w - 2.0 * SIDE, SLIDER_H).into())
    }

    /// The track of a slider, from its rect: left of it the icon.
    fn track_span(y: f64) -> (f64, f64) {
        let r = Self::slider_rect(y);
        (r.loc.x + 48.0, r.loc.x + r.size.w)
    }

    fn close_rect(i: usize) -> Rectangle<f64, Logical> {
        let panel_w = crate::layout::panels()[0].size.w as f64;
        Rectangle::new((panel_w - SIDE - ROW_H, ROWS_Y + i as f64 * ROW_H).into(), (ROW_H, ROW_H - 8.0).into())
    }

    /// The control at `local`: the point from the panel's left and the
    /// sheet's bottom edge. `panel` 0 is the settings, 1 the windows.
    pub fn hit(&self, panel: usize, local: Point<f64, Logical>, rows: usize) -> Option<Control> {
        if panel == 0 {
            if Self::slider_rect(BRIGHTNESS_Y).contains(local) {
                return Some(Control::Brightness);
            }
            if Self::slider_rect(VOLUME_Y).contains(local) {
                return Some(Control::Volume);
            }
            (0..TILES.len()).find(|&i| Self::tile_rect(i).contains(local)).map(|i| Control::Tile(TILES[i]))
        } else {
            let n = rows.min(MAX_ROWS);
            if let Some(i) = (0..n).find(|&i| Self::close_rect(i).contains(local)) {
                return Some(Control::Close(i));
            }
            let panel_w = crate::layout::panels()[0].size.w as f64;
            (0..n)
                .find(|&i| Rectangle::<f64, Logical>::new((SIDE, ROWS_Y + i as f64 * ROW_H).into(), (panel_w - 2.0 * SIDE, ROW_H - 8.0).into()).contains(local))
                .map(Control::Row)
        }
    }

    /// A slider moved to x (from the panel's left): its value set.
    pub fn slide(&self, control: Control, x: f64) {
        let y = if control == Control::Brightness { BRIGHTNESS_Y } else { VOLUME_Y };
        let (a, b) = Self::track_span(y);
        let v = ((x - a) / (b - a)).clamp(0.0, 1.0);
        let mut sys = self.sys.lock().unwrap();
        match control {
            Control::Brightness => {
                sys.brightness = v;
                let _ = self.jobs.send(Job::Brightness(v));
            }
            Control::Volume => {
                sys.volume = v;
                let _ = self.jobs.send(Job::Volume(v));
            }
            _ => {}
        }
    }

    /// The volume keys: up or down by a step.
    pub fn volume_step(&self, up: bool) {
        let mut sys = self.sys.lock().unwrap();
        sys.volume = (sys.volume + if up { 0.05 } else { -0.05 }).clamp(0.0, 1.0);
        let _ = self.jobs.send(Job::Volume(sys.volume));
    }

    /// A tile tapped: toggled (Settings is the caller's to open).
    pub fn toggle(&self, tile: Tile) {
        let mut sys = self.sys.lock().unwrap();
        let on = !tile.on(&sys);
        match tile {
            Tile::Wifi => sys.wifi = on,
            Tile::Bluetooth => sys.bluetooth = on,
            Tile::Mute => sys.muted = on,
            Tile::Dark => sys.dark = on,
            Tile::Airplane => sys.airplane = on,
            Tile::Settings => return,
        }
        let _ = self.jobs.send(Job::Toggle(tile, on));
    }

    pub fn warm_up(&self, renderer: &mut GlesRenderer) -> usize {
        [&self.tile_on, &self.tile_off, &self.track, &self.fill, &self.knob, &self.row_bg, &self.banner_bg]
            .into_iter()
            .chain(self.icons.values())
            .filter(|b| MemoryRenderBufferRenderElement::from_buffer(renderer, (0.0, 0.0), b, None, None, None, Kind::Unspecified).is_ok())
            .count()
    }

    fn label(&self, text: &str, size: f32, color: [f32; 4]) -> Option<MemoryRenderBuffer> {
        let font = self.font.as_ref()?;
        let key = format!("{size}:{text}");
        let mut labels = self.labels.borrow_mut();
        let label = labels.entry(key).or_insert_with(|| {
            let mut l = Label::new(size, color);
            l.set(font, text);
            l
        });
        Some(label.buffer.clone())
    }

    fn label_width(&self, text: &str, size: f32) -> f64 {
        let key = format!("{size}:{text}");
        self.labels.borrow().get(&key).map(|l| l.extent.w as f64).unwrap_or(0.0)
    }

    /// What the settings (panel 0) or the windows (panel 1) draw, the sheet's
    /// bottom edge at `bottom` (physical px), the panel's left at `left`.
    pub fn elements(&self, renderer: &mut GlesRenderer, panel: usize, left: i32, bottom: i32, rows: &[Row]) -> Vec<ShellElement> {
        let mut out = Vec::new();
        let s = SCALE as f64;
        let mut push = |b: &MemoryRenderBuffer, x: f64, y: f64, src: Option<Rectangle<f64, Logical>>| {
            let loc = ((left as f64 + x * s).round(), (bottom as f64 + y * s).round());
            let size = src.map(|r| r.size.to_i32_round());
            if loc.1 + 200.0 * s < 0.0 {
                return;
            }
            if let Ok(e) = MemoryRenderBufferRenderElement::from_buffer(renderer, loc, b, None, src, size, Kind::Unspecified) {
                out.push(ShellElement::Text(e));
            }
        };
        let white = [0.95, 0.95, 0.95, 1.0];
        if panel == 0 {
            let sys = self.sys.lock().unwrap().clone();
            for (y, value, icon) in [
                (BRIGHTNESS_Y, sys.brightness, "/usr/share/icons/Adwaita/symbolic/status/display-brightness-symbolic.svg"),
                (VOLUME_Y, if sys.muted { 0.0 } else { sys.volume }, "/usr/share/icons/Adwaita/symbolic/status/audio-volume-high-symbolic.svg"),
            ] {
                let (a, b) = Self::track_span(y);
                let cy = y + SLIDER_H / 2.0;
                let knob_x = a + (b - a) * value;
                push(&self.knob, knob_x - 12.0, cy - 12.0, None);
                push(&self.fill, a, cy - 4.0, Some(Rectangle::new((0.0, 0.0).into(), ((b - a) * value, 8.0).into())));
                push(&self.track, a, cy - 4.0, None);
                if let Some(i) = self.icons.get(icon) {
                    push(i, SIDE + 4.0, cy - ICON as f64 / 2.0, None);
                }
            }
            for (i, tile) in TILES.iter().enumerate() {
                let r = Self::tile_rect(i);
                if let Some(l) = self.label(tile.label(), 13.0, white) {
                    let w = self.label_width(tile.label(), 13.0);
                    push(&l, r.loc.x + (r.size.w - w) / 2.0, r.loc.y + 50.0, None);
                }
                if let Some(ic) = self.icons.get(tile.icon()) {
                    push(ic, r.loc.x + (r.size.w - ICON as f64) / 2.0, r.loc.y + 16.0, None);
                }
                push(if tile.on(&sys) { &self.tile_on } else { &self.tile_off }, r.loc.x, r.loc.y, None);
            }
        } else {
            if rows.is_empty() {
                if let Some(l) = self.label(&self.empty, 15.0, [0.6, 0.62, 0.66, 1.0]) {
                    let w = self.label_width(&self.empty, 15.0);
                    let panel_w = crate::layout::panels()[0].size.w as f64;
                    push(&l, (panel_w - w) / 2.0, ROWS_Y + 16.0, None);
                }
            }
            for (i, row) in rows.iter().take(MAX_ROWS).enumerate() {
                let y = ROWS_Y + i as f64 * ROW_H;
                if let Some(ic) = self.icons.get("/usr/share/icons/Adwaita/symbolic/ui/window-close-symbolic.svg") {
                    let c = Self::close_rect(i);
                    push(ic, c.loc.x + (c.size.w - ICON as f64) / 2.0, c.loc.y + (c.size.h - ICON as f64) / 2.0, None);
                }
                match &row.detail {
                    Some(d) => {
                        if let Some(l) = self.label(&row.name, 15.0, white) {
                            push(&l, SIDE + 60.0, y + 6.0, None);
                        }
                        if let Some(l) = self.label(d, 13.0, [0.62, 0.65, 0.7, 1.0]) {
                            push(&l, SIDE + 60.0, y + 30.0, None);
                        }
                    }
                    None => {
                        if let Some(l) = self.label(&row.name, 16.0, white) {
                            push(&l, SIDE + 60.0, y + 16.0, None);
                        }
                    }
                }
                let icon = self
                    .app_icons
                    .borrow_mut()
                    .entry(row.app_id.clone())
                    .or_insert_with(|| row.icon.as_deref().and_then(|n| crate::apps::icon(n, 32)))
                    .clone();
                if let Some(ic) = icon {
                    push(&ic, SIDE + 14.0, y + 12.0, None);
                }
                push(&self.row_bg, SIDE, y, None);
            }
        }
        out
    }
}

impl Quick {
    /// The banner of a new notification at the top of the right panel,
    /// `age` into its time, sliding in and out over 200 ms. The card's
    /// rect (logical px) for touches, and what it draws.
    pub fn banner(&self, renderer: &mut GlesRenderer, note: &crate::notify::Note, age_ns: u64) -> (Rectangle<f64, Logical>, Vec<ShellElement>) {
        let panel = crate::layout::panels()[1];
        let (w, h) = (panel.size.w as f64 - 32.0, 72.0);
        let slide = 200e6;
        let left_ns = crate::notify::BANNER_NS.saturating_sub(age_ns) as f64;
        let k = (age_ns as f64 / slide).min(left_ns / slide).clamp(0.0, 1.0);
        let e = 1.0 - (1.0 - k).powi(3);
        let y = -h + (16.0 + h) * e;
        let x = panel.loc.x as f64 + 16.0;
        let rect = Rectangle::new((x, y).into(), (w, h).into());
        let s = SCALE as f64;
        let mut out = Vec::new();
        let mut push = |b: &MemoryRenderBuffer, bx: f64, by: f64| {
            if let Ok(el) = MemoryRenderBufferRenderElement::from_buffer(renderer, ((bx * s).round(), (by * s).round()), b, None, None, None, Kind::Unspecified) {
                out.push(ShellElement::Text(el));
            }
        };
        let white = [0.95, 0.95, 0.95, 1.0];
        if let Some(l) = self.label(&note.summary, 15.0, white) {
            push(&l, x + 64.0, y + 12.0);
        }
        if let Some(l) = self.label(&note.body, 13.0, [0.62, 0.65, 0.7, 1.0]) {
            push(&l, x + 64.0, y + 38.0);
        }
        let icon = self
            .app_icons
            .borrow_mut()
            .entry(format!("note:{}", note.icon))
            .or_insert_with(|| (!note.icon.is_empty()).then(|| crate::apps::icon(&note.icon, 36)).flatten())
            .clone();
        if let Some(ic) = icon {
            push(&ic, x + 16.0, y + 18.0);
        }
        push(&self.banner_bg, x, y);
        self.banner_at.set(Some((rect, note.id)));
        (rect, out)
    }
}

/// A symbolic icon, recoloured white.
fn symbolic(path: &str, size: i32) -> Option<MemoryRenderBuffer> {
    let mut pixmap = crate::apps::icon_pixmap(path, size)?;
    for px in pixmap.pixels_mut() {
        let a = px.alpha();
        *px = tiny_skia::PremultipliedColorU8::from_rgba(a, a, a, a).unwrap_or(*px);
    }
    let n = pixmap.width() as i32;
    Some(MemoryRenderBuffer::from_slice(pixmap.data(), Fourcc::Abgr8888, (n, n), SCALE, Transform::Normal, None))
}

fn run(cmd: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(cmd).args(args).output().ok()?;
    if !out.status.success() {
        tracing::warn!("quick: {cmd} {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim());
    }
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// A command that changes something: logged.
fn set(cmd: &str, args: &[&str]) {
    tracing::info!("quick: {cmd} {}", args.join(" "));
    run(cmd, args);
}

fn read_sys() -> Sys {
    let num = |p: &str| std::fs::read_to_string(p).ok().and_then(|s| s.trim().parse::<f64>().ok());
    let bl = "/sys/class/backlight/panel0-backlight";
    let brightness = match (num(&format!("{bl}/brightness")), num(&format!("{bl}/max_brightness"))) {
        (Some(b), Some(m)) if m > 0.0 => b / m,
        _ => 0.5,
    };
    // The default sink's block in `pactl list sinks`.
    let default = run("pactl", &["info"]).and_then(|i| i.lines().find_map(|l| l.strip_prefix("Default Sink: ").map(str::to_owned)));
    let sinks = run("pactl", &["list", "sinks"]).unwrap_or_default();
    let block = sinks
        .split("Sink #")
        .find(|b| default.as_ref().is_some_and(|d| b.contains(&format!("Name: {d}"))))
        .unwrap_or("");
    let volume = block
        .lines()
        .find(|l| l.trim_start().starts_with("Volume:"))
        .and_then(|l| l.split('/').nth(1))
        .and_then(|p| p.trim().trim_end_matches('%').trim().parse::<f64>().ok())
        .map(|p| p / 100.0)
        .unwrap_or(0.5);
    let muted = block.lines().any(|l| l.trim() == "Mute: yes");
    let radio = run("nmcli", &["radio", "all"]).unwrap_or_default();
    let states: Vec<&str> = radio.lines().nth(1).map(|l| l.split_whitespace().collect()).unwrap_or_default();
    let wifi = states.get(1) == Some(&"enabled");
    let airplane = states.len() >= 4 && states[1] == "disabled" && states[3] == "disabled";
    let bluetooth = run("rfkill", &["list", "bluetooth"]).is_some_and(|s| s.contains("Soft blocked: no"));
    let dark = run("gsettings", &["get", "org.gnome.desktop.interface", "color-scheme"]).is_some_and(|s| s.contains("prefer-dark"));
    Sys { brightness, volume, muted, wifi, bluetooth, dark, airplane }
}

fn worker(rx: mpsc::Receiver<Job>, sys: Arc<Mutex<Sys>>, wake: Ping) {
    while let Ok(first) = rx.recv() {
        // Only the latest value of a slider counts.
        let mut jobs = vec![first];
        jobs.extend(rx.try_iter());
        let last_b = jobs.iter().rposition(|j| matches!(j, Job::Brightness(_)));
        let last_v = jobs.iter().rposition(|j| matches!(j, Job::Volume(_)));
        for (i, job) in jobs.into_iter().enumerate() {
            match job {
                Job::Read => {
                    let s = read_sys();
                    *sys.lock().unwrap() = s;
                    wake.ping();
                }
                Job::Brightness(v) if Some(i) == last_b => {
                    for bl in ["panel0-backlight", "panel1-backlight"] {
                        let max = std::fs::read_to_string(format!("/sys/class/backlight/{bl}/max_brightness"))
                            .ok()
                            .and_then(|s| s.trim().parse::<f64>().ok())
                            .unwrap_or(255.0);
                        let n = ((v * max).round() as u32).max(1).to_string();
                        set(
                            "busctl",
                            &["call", "--system", "org.freedesktop.login1", "/org/freedesktop/login1/session/auto", "org.freedesktop.login1.Session", "SetBrightness", "ssu", "backlight", bl, &n],
                        );
                    }
                }
                Job::Volume(v) if Some(i) == last_v => {
                    set("pactl", &["set-sink-volume", "@DEFAULT_SINK@", &format!("{}%", (v * 100.0).round() as u32)]);
                }
                Job::Toggle(tile, on) => {
                    let onoff = if on { "on" } else { "off" };
                    match tile {
                        Tile::Wifi => {
                            set("nmcli", &["radio", "wifi", onoff]);
                        }
                        Tile::Airplane => {
                            set("nmcli", &["radio", "all", if on { "off" } else { "on" }]);
                        }
                        Tile::Bluetooth => {
                            set("rfkill", &[if on { "unblock" } else { "block" }, "bluetooth"]);
                        }
                        Tile::Mute => {
                            set("pactl", &["set-sink-mute", "@DEFAULT_SINK@", if on { "1" } else { "0" }]);
                        }
                        Tile::Dark => {
                            set("gsettings", &["set", "org.gnome.desktop.interface", "color-scheme", if on { "prefer-dark" } else { "default" }]);
                        }
                        Tile::Settings => {}
                    }
                    // What the system says now.
                    *sys.lock().unwrap() = read_sys();
                    wake.ping();
                }
                _ => {}
            }
        }
    }
}
