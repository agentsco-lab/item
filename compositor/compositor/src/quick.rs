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
//! - Wi-Fi, mobile data and airplane mode through nmcli, Bluetooth through
//!   rfkill;
//! - the dark style, do not disturb and night light through gsettings
//!   (org.gnome.desktop.interface color-scheme, .notifications show-banners,
//!   .settings-daemon.plugins.color night-light-enabled); night light is
//!   drawn by the compositor itself (output.rs), as gsd-color's needs
//!   mutter;
//! - locking here, powering off and restarting through logind, the latter
//!   two after a second tap.
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
    pub data: bool,
    /// Do not disturb: no banners.
    pub dnd: bool,
    pub night: bool,
    /// Night light's colour temperature, K.
    pub night_k: u32,
}

enum Job {
    Read,
    /// A volume key: the volume read as it is, then a step up or down.
    VolumeStep(bool),
    Brightness(f64),
    Volume(f64),
    Toggle(Tile, bool),
    /// Power off (false) or restart (true).
    Power(bool),
    Media(MediaButton),
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Tile {
    Wifi,
    Data,
    Bluetooth,
    Mute,
    Dnd,
    Dark,
    Airplane,
    Night,
    Settings,
}

const TILES: [Tile; 9] = [Tile::Wifi, Tile::Data, Tile::Bluetooth, Tile::Mute, Tile::Dnd, Tile::Dark, Tile::Airplane, Tile::Night, Tile::Settings];

impl Tile {
    fn label(self) -> &'static str {
        match self {
            Tile::Wifi => "Wi-Fi",
            Tile::Data => "Mobile Data",
            Tile::Bluetooth => "Bluetooth",
            Tile::Mute => "Silent",
            Tile::Dnd => "Do Not Disturb",
            Tile::Dark => "Dark Style",
            Tile::Airplane => "Airplane Mode",
            Tile::Night => "Night Light",
            Tile::Settings => "Settings",
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Tile::Wifi => "/usr/share/icons/Adwaita/symbolic/devices/network-wireless-symbolic.svg",
            Tile::Data => "/usr/share/icons/Adwaita/symbolic/devices/network-cellular-symbolic.svg",
            Tile::Bluetooth => "/usr/share/icons/Adwaita/symbolic/status/bluetooth-active-symbolic.svg",
            Tile::Mute => "/usr/share/icons/Adwaita/symbolic/status/audio-volume-muted-symbolic.svg",
            Tile::Dnd => "/usr/share/icons/Adwaita/symbolic/status/notifications-disabled-symbolic.svg",
            Tile::Dark => "/usr/share/icons/Adwaita/symbolic/status/weather-clear-night-symbolic.svg",
            Tile::Airplane => "/usr/share/icons/Adwaita/symbolic/status/airplane-mode-symbolic.svg",
            Tile::Night => "/usr/share/icons/Adwaita/symbolic/status/night-light-symbolic.svg",
            Tile::Settings => "/usr/share/icons/hicolor/symbolic/apps/org.gnome.Settings-symbolic.svg",
        }
    }

    fn on(self, s: &Sys) -> bool {
        match self {
            Tile::Wifi => s.wifi,
            Tile::Data => s.data,
            Tile::Bluetooth => s.bluetooth,
            Tile::Mute => s.muted,
            Tile::Dnd => s.dnd,
            Tile::Dark => s.dark,
            Tile::Airplane => s.airplane,
            Tile::Night => s.night,
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
    Lock,
    /// Power off and restart: the first tap asks, the second does it.
    Power,
    PowerOff,
    Restart,
    /// The media player's buttons.
    Media(MediaButton),
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum MediaButton {
    Previous,
    PlayPause,
    Next,
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
const BRIGHTNESS_Y: f64 = -560.0;
const VOLUME_Y: f64 = -500.0;
const TILE_H: f64 = 72.0;
const TILE_GAP: f64 = 12.0;
const TILES_Y: f64 = -428.0;
const POWER_Y: f64 = -164.0;
const BUTTON_H: f64 = 52.0;
const ICON: i32 = 24;
const MEDIA_Y: f64 = -560.0;
const MEDIA_H: f64 = 112.0;
const ROWS_Y: f64 = -560.0;
const ROWS_END: f64 = -100.0;
const ROW_H: f64 = 64.0;
const BAR_W: f64 = 12.0;
const BAR_H: f64 = 220.0;
const VOLUME_SHOW_NS: u64 = 1_500_000_000;
const VOLUME_FADE_NS: u64 = 200_000_000;
/// Power off and restart wait this long for their second tap.
const POWER_ASK_NS: u64 = 4_000_000_000;

const WHITE: [f32; 4] = [0.95, 0.95, 0.95, 1.0];
const DIM: [f32; 4] = [0.62, 0.65, 0.7, 1.0];
const RED: [u8; 4] = [0xe0, 0x1b, 0x24, 255];

/// What a media player (MPRIS) is playing.
#[derive(Clone, Default, PartialEq)]
pub struct Media {
    /// Its name on the session bus, and what it calls itself.
    pub bus: String,
    pub player: String,
    pub title: String,
    pub artist: String,
    pub playing: bool,
}

pub struct Quick {
    pub sys: Arc<Mutex<Sys>>,
    pub media: Arc<Mutex<Option<Media>>>,
    jobs: mpsc::Sender<Job>,
    font: Option<Font>,
    tile_on: MemoryRenderBuffer,
    tile_off: MemoryRenderBuffer,
    button: MemoryRenderBuffer,
    button_red: MemoryRenderBuffer,
    media_bg: MemoryRenderBuffer,
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
    /// When power off or restart was first tapped (0: not asked).
    power_asked: std::cell::Cell<u64>,
    /// When a volume key was last pressed (0: never).
    volume_at: std::cell::Cell<u64>,
    bar_track: MemoryRenderBuffer,
    bar_fill: MemoryRenderBuffer,
    /// The banner's card as last drawn, and its notification, for touches.
    pub banner_at: std::cell::Cell<Option<(Rectangle<f64, Logical>, u32)>>,
}

const MEDIA_ICONS: [&str; 4] = [
    "/usr/share/icons/Adwaita/symbolic/actions/media-skip-backward-symbolic.svg",
    "/usr/share/icons/Adwaita/symbolic/actions/media-playback-start-symbolic.svg",
    "/usr/share/icons/Adwaita/symbolic/actions/media-playback-pause-symbolic.svg",
    "/usr/share/icons/Adwaita/symbolic/actions/media-skip-forward-symbolic.svg",
];
const LOCK_ICON: &str = "/usr/share/icons/Adwaita/symbolic/status/system-lock-screen-symbolic.svg";
const POWER_ICON: &str = "/usr/share/icons/Adwaita/symbolic/actions/system-shutdown-symbolic.svg";
const RESTART_ICON: &str = "/usr/share/icons/Adwaita/symbolic/actions/system-reboot-symbolic.svg";

impl Quick {
    pub fn new(wake: Ping) -> Quick {
        let sys = Arc::new(Mutex::new(Sys::default()));
        let media = Arc::new(Mutex::new(None));
        let (tx, rx) = mpsc::channel::<Job>();
        let (shared, shared_media) = (sys.clone(), media.clone());
        std::thread::Builder::new()
            .name("quick settings".into())
            .spawn(move || worker(rx, shared, shared_media, wake))
            .expect("quick settings thread");
        // Read once at the start: night light is drawn from it.
        let _ = tx.send(Job::Read);
        let panel_w = crate::layout::panels()[0].size.w as f64;
        let tile_w = (panel_w - 2.0 * SIDE - 2.0 * TILE_GAP) / 3.0;
        let track_w = panel_w - 2.0 * SIDE - 48.0;
        let button_w = (panel_w - 2.0 * SIDE - TILE_GAP) / 2.0;
        let mut icons = HashMap::new();
        for path in TILES.iter().map(|t| t.icon()).chain([
            "/usr/share/icons/Adwaita/symbolic/status/display-brightness-symbolic.svg",
            "/usr/share/icons/Adwaita/symbolic/status/audio-volume-high-symbolic.svg",
            "/usr/share/icons/Adwaita/symbolic/ui/window-close-symbolic.svg",
            LOCK_ICON,
            POWER_ICON,
            RESTART_ICON,
        ]).chain(MEDIA_ICONS) {
            if let Some(b) = symbolic(path, ICON) {
                icons.insert(path.to_owned(), b);
            }
        }
        Quick {
            sys,
            media,
            jobs: tx,
            font: Font::load(&["/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf", "/usr/share/fonts/truetype/lato/Lato-Regular.ttf"]),
            tile_on: crate::grid::rounded(tile_w, TILE_H, 20.0, [0x35, 0x84, 0xe4, 255]),
            tile_off: crate::grid::rounded(tile_w, TILE_H, 20.0, [34, 34, 34, 34]),
            button: crate::grid::rounded(button_w, BUTTON_H, BUTTON_H / 2.0, [34, 34, 34, 34]),
            button_red: crate::grid::rounded(button_w, BUTTON_H, BUTTON_H / 2.0, RED),
            media_bg: crate::grid::rounded(panel_w - 2.0 * SIDE, MEDIA_H, 22.0, [34, 34, 34, 34]),
            track: crate::grid::rounded(track_w, 8.0, 4.0, [40, 40, 40, 40]),
            fill: crate::grid::rounded(track_w, 8.0, 4.0, [235, 235, 235, 235]),
            knob: crate::grid::rounded(24.0, 24.0, 12.0, [255, 255, 255, 255]),
            row_bg: crate::grid::rounded(panel_w - 2.0 * SIDE, ROW_H - 8.0, 16.0, [16, 16, 16, 16]),
            banner_bg: crate::grid::rounded(panel_w - 32.0, 72.0, 18.0, [36, 40, 46, 245]),
            icons,
            labels: Default::default(),
            app_icons: Default::default(),
            empty: "No windows or notifications".into(),
            power_asked: std::cell::Cell::new(0),
            banner_at: std::cell::Cell::new(None),
            volume_at: std::cell::Cell::new(0),
            bar_track: crate::grid::rounded(BAR_W, BAR_H, BAR_W / 2.0, [30, 32, 36, 220]),
            bar_fill: crate::grid::rounded(BAR_W, BAR_H, BAR_W / 2.0, [240, 240, 240, 240]),
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

    /// The buttons under the tiles: 0 lock (or restart), 1 power.
    fn button_rect(i: usize) -> Rectangle<f64, Logical> {
        let panel_w = crate::layout::panels()[0].size.w as f64;
        let w = (panel_w - 2.0 * SIDE - TILE_GAP) / 2.0;
        Rectangle::new((SIDE + i as f64 * (w + TILE_GAP), POWER_Y).into(), (w, BUTTON_H).into())
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

    /// Where the right shade's rows start: under the player, if one plays.
    fn rows_y(&self) -> f64 {
        if self.media.lock().unwrap().is_some() { MEDIA_Y + MEDIA_H + 16.0 } else { ROWS_Y }
    }

    /// How many rows fit.
    fn max_rows(&self) -> usize {
        ((ROWS_END - self.rows_y()) / ROW_H) as usize
    }

    fn row_rect(&self, i: usize) -> Rectangle<f64, Logical> {
        let panel_w = crate::layout::panels()[0].size.w as f64;
        Rectangle::new((SIDE, self.rows_y() + i as f64 * ROW_H).into(), (panel_w - 2.0 * SIDE, ROW_H - 8.0).into())
    }

    fn close_rect(&self, i: usize) -> Rectangle<f64, Logical> {
        let r = self.row_rect(i);
        Rectangle::new((r.loc.x + r.size.w - ROW_H, r.loc.y).into(), (ROW_H, r.size.h).into())
    }

    /// The player's buttons, right in its card: previous, play, next.
    fn media_button_rect(i: usize) -> Rectangle<f64, Logical> {
        let panel_w = crate::layout::panels()[0].size.w as f64;
        let x = panel_w - SIDE - 16.0 - 3.0 * 52.0 + i as f64 * 52.0;
        Rectangle::new((x, MEDIA_Y + (MEDIA_H - 52.0) / 2.0).into(), (52.0, 52.0).into())
    }

    /// Whether power off and restart are waiting for their second tap.
    fn power_asking(&self) -> bool {
        let t = self.power_asked.get();
        t != 0 && hybris_hwc::now_ns() < t + POWER_ASK_NS
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
            let asking = self.power_asking();
            if Self::button_rect(0).contains(local) {
                return Some(if asking { Control::Restart } else { Control::Lock });
            }
            if Self::button_rect(1).contains(local) {
                return Some(if asking { Control::PowerOff } else { Control::Power });
            }
            (0..TILES.len()).find(|&i| Self::tile_rect(i).contains(local)).map(|i| Control::Tile(TILES[i]))
        } else {
            if self.media.lock().unwrap().is_some() {
                let buttons = [MediaButton::Previous, MediaButton::PlayPause, MediaButton::Next];
                if let Some(i) = (0..3).find(|&i| Self::media_button_rect(i).contains(local)) {
                    return Some(Control::Media(buttons[i]));
                }
            }
            let n = rows.min(self.max_rows());
            if let Some(i) = (0..n).find(|&i| self.close_rect(i).contains(local)) {
                return Some(Control::Close(i));
            }
            (0..n).find(|&i| self.row_rect(i).contains(local)).map(Control::Row)
        }
    }

    /// Power off or restart tapped once: they ask for a second tap.
    pub fn ask_power(&self) {
        self.power_asked.set(hybris_hwc::now_ns());
    }

    /// Power off (false) or restart (true), now.
    pub fn power(&self, restart: bool) {
        self.power_asked.set(0);
        let _ = self.jobs.send(Job::Power(restart));
    }

    /// The player's button.
    pub fn media_button(&self, button: MediaButton) {
        let _ = self.jobs.send(Job::Media(button));
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

    /// The volume keys: up or down by a step, and the bar shown.
    pub fn volume_step(&self, up: bool) {
        // The step is taken from the volume as it is, read on the thread:
        // what was last read here may be stale, or never read (a step from
        // an unread 0 set the volume to 0 %).
        let _ = self.jobs.send(Job::VolumeStep(up));
        self.volume_at.set(hybris_hwc::now_ns());
    }

    /// Whether the volume bar is up at `now`, and whether it is fading.
    pub fn volume_bar_state(&self, now: u64) -> (bool, bool) {
        let age = now.saturating_sub(self.volume_at.get());
        let up = self.volume_at.get() != 0 && age < VOLUME_SHOW_NS + VOLUME_FADE_NS;
        (up, up && age >= VOLUME_SHOW_NS)
    }

    /// The volume as an upright bar beside the keys (item's): at the right
    /// panel's right edge, near its top, where the keys are on the Duo; up
    /// 1.5 s after the last press, then fading over 200 ms.
    pub fn volume_bar(&self, renderer: &mut GlesRenderer, now: u64) -> Vec<ShellElement> {
        let (up, _) = self.volume_bar_state(now);
        if !up {
            return Vec::new();
        }
        let age = now.saturating_sub(self.volume_at.get());
        let alpha = if age < VOLUME_SHOW_NS { 1.0 } else { 1.0 - (age - VOLUME_SHOW_NS) as f32 / VOLUME_FADE_NS as f32 };
        let v = self.sys.lock().unwrap().volume;
        let (w, h) = (BAR_W, BAR_H);
        let x = crate::layout::LAYOUT.0 as f64 - 22.0 - w;
        let y = 110.0;
        let s = SCALE as f64;
        let mut out = Vec::new();
        let filled = (h * v).round();
        let src = Rectangle::<f64, Logical>::new((0.0, h - filled).into(), (w, filled).into());
        if filled > 0.0 {
            if let Ok(e) = MemoryRenderBufferRenderElement::from_buffer(renderer, ((x * s).round(), ((y + h - filled) * s).round()), &self.bar_fill, Some(alpha), Some(src), Some(src.size.to_i32_round()), Kind::Unspecified) {
                out.push(ShellElement::Text(e));
            }
        }
        if let Ok(e) = MemoryRenderBufferRenderElement::from_buffer(renderer, ((x * s).round(), (y * s).round()), &self.bar_track, Some(alpha), None, None, Kind::Unspecified) {
            out.push(ShellElement::Text(e));
        }
        out
    }

    /// A tile tapped: toggled (Settings is the caller's to open).
    pub fn toggle(&self, tile: Tile) {
        let mut sys = self.sys.lock().unwrap();
        let on = !tile.on(&sys);
        match tile {
            Tile::Wifi => sys.wifi = on,
            Tile::Data => sys.data = on,
            Tile::Bluetooth => sys.bluetooth = on,
            Tile::Mute => sys.muted = on,
            Tile::Dnd => sys.dnd = on,
            Tile::Dark => sys.dark = on,
            Tile::Airplane => sys.airplane = on,
            Tile::Night => sys.night = on,
            Tile::Settings => return,
        }
        let _ = self.jobs.send(Job::Toggle(tile, on));
    }

    pub fn warm_up(&self, renderer: &mut GlesRenderer) -> usize {
        [&self.tile_on, &self.tile_off, &self.button, &self.button_red, &self.media_bg, &self.track, &self.fill, &self.knob, &self.row_bg, &self.banner_bg]
            .into_iter()
            .chain(self.icons.values())
            .filter(|b| MemoryRenderBufferRenderElement::from_buffer(renderer, (0.0, 0.0), b, None, None, None, Kind::Unspecified).is_ok())
            .count()
    }

    fn label(&self, text: &str, size: f32, color: [f32; 4]) -> Option<MemoryRenderBuffer> {
        let font = self.font.as_ref()?;
        let key = format!("{size}:{color:?}:{text}");
        let mut labels = self.labels.borrow_mut();
        let label = labels.entry(key).or_insert_with(|| {
            let mut l = Label::new(size, color);
            l.set(font, text);
            l
        });
        Some(label.buffer.clone())
    }

    /// A label made before by `label`, its width.
    fn label_width(&self, text: &str, size: f32, color: [f32; 4]) -> f64 {
        let key = format!("{size}:{color:?}:{text}");
        self.labels.borrow().get(&key).map(|l| l.extent.w as f64).unwrap_or(0.0)
    }

    /// What the settings (panel 0) or the windows (panel 1) draw, the sheet's
    /// bottom edge at `bottom` (physical px), the panel's left at `left`,
    /// with `alpha` as the sheet comes.
    pub fn elements(&self, renderer: &mut GlesRenderer, panel: usize, left: i32, bottom: i32, rows: &[Row], alpha: f32) -> Vec<ShellElement> {
        let mut out = Vec::new();
        if alpha <= 0.001 {
            return out;
        }
        let s = SCALE as f64;
        let mut push = |b: &MemoryRenderBuffer, x: f64, y: f64, src: Option<Rectangle<f64, Logical>>| {
            let loc = ((left as f64 + x * s).round(), (bottom as f64 + y * s).round());
            let size = src.map(|r| r.size.to_i32_round());
            if loc.1 + 200.0 * s < 0.0 {
                return;
            }
            if let Ok(e) = MemoryRenderBufferRenderElement::from_buffer(renderer, loc, b, Some(alpha), src, size, Kind::Unspecified) {
                out.push(ShellElement::Text(e));
            }
        };
        let panel_w = crate::layout::panels()[0].size.w as f64;
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
            // A tile: its icon, its name, and under it what it is now.
            for (i, tile) in TILES.iter().enumerate() {
                let r = Self::tile_rect(i);
                let on = tile.on(&sys);
                let state = match tile {
                    Tile::Settings => None,
                    _ => Some(if on { "On".to_owned() } else { "Off".to_owned() }),
                };
                let text_x = r.loc.x + 54.0;
                match &state {
                    Some(st) => {
                        if let Some(l) = self.label(tile.label(), 14.0, WHITE) {
                            push(&l, text_x, r.loc.y + 14.0, None);
                        }
                        let color = if on { [1.0, 1.0, 1.0, 0.8] } else { DIM };
                        if let Some(l) = self.label(st, 12.0, color) {
                            push(&l, text_x, r.loc.y + 38.0, None);
                        }
                    }
                    None => {
                        if let Some(l) = self.label(tile.label(), 14.0, WHITE) {
                            push(&l, text_x, r.loc.y + 25.0, None);
                        }
                    }
                }
                if let Some(ic) = self.icons.get(tile.icon()) {
                    push(ic, r.loc.x + 18.0, r.loc.y + (TILE_H - ICON as f64) / 2.0, None);
                }
                push(if on { &self.tile_on } else { &self.tile_off }, r.loc.x, r.loc.y, None);
            }
            // Lock and power; power asked, restart and power off.
            let asking = self.power_asking();
            let buttons: [(&str, &str, bool); 2] = if asking { [(RESTART_ICON, "Restart", false), (POWER_ICON, "Power off", true)] } else { [(LOCK_ICON, "Lock", false), (POWER_ICON, "Power", false)] };
            for (i, (icon, name, red)) in buttons.into_iter().enumerate() {
                let r = Self::button_rect(i);
                let w = self.label(name, 15.0, WHITE).map(|l| {
                    let w = self.label_width(name, 15.0, WHITE);
                    (l, w)
                });
                let total = ICON as f64 + 10.0 + w.as_ref().map(|(_, w)| *w).unwrap_or(0.0);
                let x0 = r.loc.x + (r.size.w - total) / 2.0;
                if let Some((l, _)) = &w {
                    push(l, x0 + ICON as f64 + 10.0, r.loc.y + 15.0, None);
                }
                if let Some(ic) = self.icons.get(icon) {
                    push(ic, x0, r.loc.y + (BUTTON_H - ICON as f64) / 2.0, None);
                }
                push(if red { &self.button_red } else { &self.button }, r.loc.x, r.loc.y, None);
            }
        } else {
            // What plays, if anything: its title, who, and its buttons.
            if let Some(m) = self.media.lock().unwrap().clone() {
                let title: String = if m.title.is_empty() { m.player.clone() } else { m.title.chars().take(30).collect() };
                if let Some(l) = self.label(&title, 16.0, WHITE) {
                    push(&l, SIDE + 22.0, MEDIA_Y + 30.0, None);
                }
                if !m.artist.is_empty() {
                    let artist: String = m.artist.chars().take(34).collect();
                    if let Some(l) = self.label(&artist, 13.0, DIM) {
                        push(&l, SIDE + 22.0, MEDIA_Y + 60.0, None);
                    }
                }
                let play = if m.playing { MEDIA_ICONS[2] } else { MEDIA_ICONS[1] };
                for (i, icon) in [MEDIA_ICONS[0], play, MEDIA_ICONS[3]].into_iter().enumerate() {
                    let r = Self::media_button_rect(i);
                    if let Some(ic) = self.icons.get(icon) {
                        push(ic, r.loc.x + (r.size.w - ICON as f64) / 2.0, r.loc.y + (r.size.h - ICON as f64) / 2.0, None);
                    }
                }
                push(&self.media_bg, SIDE, MEDIA_Y, None);
            }
            let rows_y = self.rows_y();
            if rows.is_empty() {
                if let Some(l) = self.label(&self.empty, 15.0, DIM) {
                    let w = self.label_width(&self.empty, 15.0, DIM);
                    push(&l, (panel_w - w) / 2.0, rows_y + 16.0, None);
                }
            }
            for (i, row) in rows.iter().take(self.max_rows()).enumerate() {
                let y = rows_y + i as f64 * ROW_H;
                if let Some(ic) = self.icons.get("/usr/share/icons/Adwaita/symbolic/ui/window-close-symbolic.svg") {
                    let c = self.close_rect(i);
                    push(ic, c.loc.x + (c.size.w - ICON as f64) / 2.0, c.loc.y + (c.size.h - ICON as f64) / 2.0, None);
                }
                match &row.detail {
                    Some(d) => {
                        if let Some(l) = self.label(&row.name, 15.0, WHITE) {
                            push(&l, SIDE + 60.0, y + 6.0, None);
                        }
                        if let Some(l) = self.label(d, 13.0, DIM) {
                            push(&l, SIDE + 60.0, y + 30.0, None);
                        }
                    }
                    None => {
                        if let Some(l) = self.label(&row.name, 16.0, WHITE) {
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
        if let Some(l) = self.label(&note.summary, 15.0, WHITE) {
            push(&l, x + 64.0, y + 12.0);
        }
        if let Some(l) = self.label(&note.body, 13.0, DIM) {
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
pub fn symbolic(path: &str, size: i32) -> Option<MemoryRenderBuffer> {
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
    let data = states.get(3) == Some(&"enabled");
    let airplane = states.len() >= 4 && states[1] == "disabled" && states[3] == "disabled";
    let bluetooth = run("rfkill", &["list", "bluetooth"]).is_some_and(|s| s.contains("Soft blocked: no"));
    let gs = |schema: &str, key: &str| run("gsettings", &["get", schema, key]).unwrap_or_default().trim().to_owned();
    let dark = gs("org.gnome.desktop.interface", "color-scheme").contains("prefer-dark");
    let dnd = gs("org.gnome.desktop.notifications", "show-banners") == "false";
    let night = gs("org.gnome.settings-daemon.plugins.color", "night-light-enabled") == "true";
    let night_k = gs("org.gnome.settings-daemon.plugins.color", "night-light-temperature").trim_start_matches("uint32 ").parse().unwrap_or(3500);
    Sys { brightness, volume, muted, wifi, bluetooth, dark, airplane, data, dnd, night, night_k }
}

const MPRIS: &str = "/org/mpris/MediaPlayer2";
const PLAYER: &str = "org.mpris.MediaPlayer2.Player";

/// What the media players play: the one playing, else the first paused
/// with something to show.
fn read_media(bus: &zbus::blocking::Connection) -> Option<Media> {
    let names: Vec<String> = match bus.call_method(Some("org.freedesktop.DBus"), "/org/freedesktop/DBus", Some("org.freedesktop.DBus"), "ListNames", &()).and_then(|r| r.body().deserialize()) {
        Ok(n) => n,
        Err(e) => {
            tracing::debug!("quick: media players: {e}");
            return None;
        }
    };
    let get = |name: &str, iface: &str, prop: &str| -> Option<zbus::zvariant::OwnedValue> {
        bus.call_method(Some(name), MPRIS, Some("org.freedesktop.DBus.Properties"), "Get", &(iface, prop)).ok()?.body().deserialize::<zbus::zvariant::OwnedValue>().ok()
    };
    let mut found: Vec<Media> = Vec::new();
    for name in names.iter().filter(|n| n.starts_with("org.mpris.MediaPlayer2.")) {
        let status: String = get(name, PLAYER, "PlaybackStatus").and_then(|v| String::try_from(v).ok()).unwrap_or_default();
        let meta: HashMap<String, zbus::zvariant::OwnedValue> = get(name, PLAYER, "Metadata").and_then(|v| HashMap::try_from(v).ok()).unwrap_or_default();
        let title = meta.get("xesam:title").and_then(|v| String::try_from(v.try_clone().ok()?).ok()).unwrap_or_default();
        let artist = meta.get("xesam:artist").and_then(|v| Vec::<String>::try_from(v.try_clone().ok()?).ok()).map(|a| a.join(", ")).unwrap_or_default();
        let player = get(name, "org.mpris.MediaPlayer2", "Identity").and_then(|v| String::try_from(v).ok()).unwrap_or_else(|| name.trim_start_matches("org.mpris.MediaPlayer2.").to_owned());
        if status == "Stopped" && title.is_empty() {
            continue;
        }
        found.push(Media { bus: name.clone(), player, title, artist, playing: status == "Playing" });
    }
    found.sort_by_key(|m| !m.playing);
    tracing::debug!("quick: {} media player(s)", found.len());
    found.into_iter().next()
}

fn worker(rx: mpsc::Receiver<Job>, sys: Arc<Mutex<Sys>>, media: Arc<Mutex<Option<Media>>>, wake: Ping) {
    let bus = zbus::blocking::Connection::session().map_err(|e| tracing::warn!("quick: no session bus, no media players: {e}")).ok();
    let read_media_now = || {
        let m = bus.as_ref().and_then(read_media);
        *media.lock().unwrap() = m;
    };
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
                    read_media_now();
                    wake.ping();
                }
                Job::Power(restart) => {
                    let method = if restart { "Reboot" } else { "PowerOff" };
                    set("busctl", &["call", "--system", "org.freedesktop.login1", "/org/freedesktop/login1", "org.freedesktop.login1.Manager", method, "b", "true"]);
                }
                Job::Media(button) => {
                    let name = media.lock().unwrap().as_ref().map(|m| m.bus.clone());
                    if let (Some(bus), Some(name)) = (&bus, name) {
                        let method = match button {
                            MediaButton::Previous => "Previous",
                            MediaButton::PlayPause => "PlayPause",
                            MediaButton::Next => "Next",
                        };
                        tracing::info!("quick: {method} on {name}");
                        let _ = bus.call_method(Some(name.as_str()), MPRIS, Some(PLAYER), method, &());
                        std::thread::sleep(std::time::Duration::from_millis(250));
                        read_media_now();
                        wake.ping();
                    }
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
                Job::VolumeStep(up) => {
                    let now = read_sys();
                    let v = (now.volume + if up { 0.05 } else { -0.05 }).clamp(0.0, 1.0);
                    set("pactl", &["set-sink-volume", "@DEFAULT_SINK@", &format!("{}%", (v * 100.0).round() as u32)]);
                    *sys.lock().unwrap() = Sys { volume: v, ..now };
                    wake.ping();
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
                        Tile::Data => {
                            set("nmcli", &["radio", "wwan", onoff]);
                        }
                        Tile::Dnd => {
                            set("gsettings", &["set", "org.gnome.desktop.notifications", "show-banners", if on { "false" } else { "true" }]);
                        }
                        Tile::Night => {
                            set("gsettings", &["set", "org.gnome.settings-daemon.plugins.color", "night-light-enabled", if on { "true" } else { "false" }]);
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
