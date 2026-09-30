//! The dock: item's two halves, a rounded slab at each panel's outer bottom
//! corner, drawn by the compositor. A tap on an icon launches the app onto
//! that panel. A half stands on an empty panel and slides down out of sight
//! while a window has the panel; it comes back when the panel is empty again.
//!
//! The apps are item's: `~/.config/sfduo/dock.json` (`{"left": [...],
//! "right": [...]}`, desktop file names), else a default. Icons come from the
//! hicolor theme, SVG through resvg or PNG, rasterized once at start at the
//! output's scale.

use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement};
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::input::TouchSlot;
use smithay::utils::{Logical, Point, Rectangle, Transform};

use resvg::tiny_skia;

use crate::layout::{self, SCALE};
use crate::shade::ShellElement;

/// An icon's side, logical px.
const ICON: i32 = 48;
/// Between icons, and around them inside the slab.
const GAP: i32 = 14;
const PAD: i32 = 12;
/// The slab's distance from the panel's outer and bottom edges.
const MARGIN: i32 = 10;
const RADIUS: f32 = 20.0;
const SLAB: [u8; 4] = [38, 48, 43, 235];
/// How long a half takes to slide in or out.
const SLIDE_NS: u64 = 260_000_000;
/// A touch that travels this far is not a tap.
const TAP: f64 = 12.0;

const DEFAULT: [&[&str]; 2] = [
    &["org.gnome.Calls.desktop", "sm.puri.Chatty.desktop"],
    &["org.gnome.Epiphany.desktop", "org.gnome.Settings.desktop", "org.gnome.Weather.desktop"],
];

struct App {
    name: String,
    exec: String,
    icon: Option<MemoryRenderBuffer>,
}

struct Slide {
    from: f64,
    to: f64,
    start_ns: u64,
}

struct Half {
    apps: Vec<App>,
    slab: MemoryRenderBuffer,
    /// Where the slab stands when shown, logical px.
    rect: Rectangle<i32, Logical>,
    /// 0 shown, 1 out of sight.
    hidden: f64,
    slide: Option<Slide>,
    /// The icon a finger is on, and the finger.
    pressed: Option<(usize, TouchSlot, Point<f64, Logical>)>,
}

impl Half {
    fn hidden_at(&self, frame_ns: u64) -> f64 {
        match &self.slide {
            Some(s) => {
                let p = (frame_ns.saturating_sub(s.start_ns) as f64 / SLIDE_NS as f64).clamp(0.0, 1.0);
                // Eased in and out: it leaves from rest and arrives at rest.
                let e = p * p * (3.0 - 2.0 * p);
                s.from + (s.to - s.from) * e
            }
            None => self.hidden,
        }
    }

    /// How far down the slab is, logical px, for `hidden`.
    fn drop(&self, hidden: f64) -> i32 {
        ((self.rect.size.h + MARGIN + 4) as f64 * hidden).round() as i32
    }

    /// The icon cell `i`, logical px, with the slab shown.
    fn cell(&self, i: usize) -> Rectangle<i32, Logical> {
        Rectangle::new(
            (self.rect.loc.x + PAD + i as i32 * (ICON + GAP), self.rect.loc.y + PAD).into(),
            (ICON, ICON).into(),
        )
    }
}

pub struct Dock {
    halves: Vec<Half>,
}

/// What a touch on the dock asks for.
pub enum Tap {
    /// Launch this command on this panel.
    Launch(String, usize),
}

impl Dock {
    pub fn new() -> Dock {
        let lists = config();
        let panels = layout::panels();
        let halves = lists
            .iter()
            .enumerate()
            .map(|(p, names)| {
                let apps: Vec<App> = names.iter().filter_map(|n| app(n)).collect();
                let n = apps.len() as i32;
                let w = 2 * PAD + n * ICON + (n - 1).max(0) * GAP;
                let h = 2 * PAD + ICON;
                let panel = panels[p];
                // The outer edge: left of the left panel, right of the right.
                let x = if p == 0 { panel.loc.x + MARGIN } else { panel.loc.x + panel.size.w - MARGIN - w };
                let y = panel.size.h - MARGIN - h;
                tracing::info!(
                    "dock {}: {}",
                    if p == 0 { "left" } else { "right" },
                    apps.iter().map(|a| format!("{}{}", a.name, if a.icon.is_some() { "" } else { " (no icon)" })).collect::<Vec<_>>().join(", ")
                );
                Half { apps, slab: slab(w, h), rect: Rectangle::new((x, y).into(), (w, h).into()), hidden: 0.0, slide: None, pressed: None }
            })
            .collect();
        Dock { halves }
    }

    /// Shows the half on each panel that is free, hides it on each that is
    /// taken. Returns whether a half set off.
    pub fn follow(&mut self, taken: [bool; 2], now_ns: u64) -> bool {
        let mut moved = false;
        for (half, &taken) in self.halves.iter_mut().zip(taken.iter()) {
            let to = if taken { 1.0 } else { 0.0 };
            if half.hidden != to {
                let from = half.hidden_at(now_ns);
                half.slide = Some(Slide { from, to, start_ns: now_ns });
                half.hidden = to;
                half.pressed = None;
                moved = true;
            }
        }
        moved
    }

    /// After a frame for `frame_ns`: whether a half is still sliding.
    pub fn settle(&mut self, frame_ns: u64) -> bool {
        let mut sliding = false;
        for half in &mut self.halves {
            if let Some(s) = &half.slide {
                if frame_ns >= s.start_ns + SLIDE_NS {
                    half.slide = None;
                } else {
                    sliding = true;
                }
            }
        }
        sliding
    }

    /// A touch down: taken if it lands on a shown half. Returns whether it was.
    pub fn down(&mut self, slot: TouchSlot, pos: Point<f64, Logical>) -> bool {
        for half in &mut self.halves {
            if half.hidden > 0.0 || half.slide.is_some() || !half.rect.to_f64().contains(pos) {
                continue;
            }
            let icon = (0..half.apps.len()).find(|&i| half.cell(i).to_f64().contains(pos));
            half.pressed = icon.map(|i| (i, slot, pos));
            return true;
        }
        false
    }

    pub fn holds(&self, slot: TouchSlot) -> bool {
        self.halves.iter().any(|h| h.pressed.is_some_and(|(_, s, _)| s == slot))
    }

    /// A finger on an icon that moves away is no longer a tap on it.
    pub fn motion(&mut self, slot: TouchSlot, pos: Point<f64, Logical>) {
        for half in &mut self.halves {
            if let Some((_, s, at)) = half.pressed {
                if s == slot && ((pos.x - at.x).powi(2) + (pos.y - at.y).powi(2)).sqrt() > TAP {
                    half.pressed = None;
                }
            }
        }
    }

    pub fn up(&mut self, slot: TouchSlot) -> Option<Tap> {
        for (p, half) in self.halves.iter_mut().enumerate() {
            if let Some((i, s, _)) = half.pressed {
                if s == slot {
                    half.pressed = None;
                    return Some(Tap::Launch(half.apps[i].exec.clone(), p));
                }
            }
        }
        None
    }

    pub fn cancel(&mut self) {
        for half in &mut self.halves {
            half.pressed = None;
        }
    }

    /// Uploads the slabs and icons; returns how many.
    pub fn warm_up(&self, renderer: &mut GlesRenderer) -> usize {
        self.halves
            .iter()
            .flat_map(|h| std::iter::once(&h.slab).chain(h.apps.iter().filter_map(|a| a.icon.as_ref())))
            .filter(|b| MemoryRenderBufferRenderElement::from_buffer(renderer, (0.0, 0.0), b, None, None, None, Kind::Unspecified).is_ok())
            .count()
    }

    /// What the dock draws for a frame shown at `frame_ns`, topmost first.
    pub fn elements(&self, renderer: &mut GlesRenderer, frame_ns: u64) -> Vec<ShellElement> {
        let mut out = Vec::new();
        for half in &self.halves {
            let hidden = half.hidden_at(frame_ns);
            if hidden >= 1.0 || half.apps.is_empty() {
                continue;
            }
            let drop = half.drop(hidden);
            let mut push = |buffer: &MemoryRenderBuffer, at: Point<i32, Logical>, alpha: f32| {
                let loc = ((at.x * SCALE) as f64, ((at.y + drop) * SCALE) as f64);
                match MemoryRenderBufferRenderElement::from_buffer(renderer, loc, buffer, Some(alpha), None, None, Kind::Unspecified) {
                    Ok(e) => out.push(ShellElement::Text(e)),
                    Err(e) => tracing::warn!("dock: {e}"),
                }
            };
            for (i, app) in half.apps.iter().enumerate() {
                if let Some(icon) = &app.icon {
                    // A pressed icon dims under the finger.
                    let alpha = if half.pressed.is_some_and(|(p, _, _)| p == i) { 0.55 } else { 1.0 };
                    push(icon, half.cell(i).loc, alpha);
                }
            }
            push(&half.slab, half.rect.loc, 1.0);
        }
        out
    }
}

/// The dock's apps: item's configuration, else the default.
fn config() -> [Vec<String>; 2] {
    let path = std::env::var("HOME").map(|h| format!("{h}/.config/sfduo/dock.json")).unwrap_or_default();
    let parsed = std::fs::read_to_string(&path).ok().and_then(|text| {
        // {"left": [...], "right": [...]}: two lists of quoted names, without
        // a JSON parser.
        let list = |key: &str| -> Option<Vec<String>> {
            let start = text.find(&format!("\"{key}\""))?;
            let open = start + text[start..].find('[')?;
            let close = open + text[open..].find(']')?;
            Some(text[open + 1..close].split(',').map(|s| s.trim().trim_matches('"').to_owned()).filter(|s| !s.is_empty()).collect())
        };
        Some([list("left")?, list("right")?])
    });
    match parsed {
        Some(lists) => {
            tracing::info!("dock: apps from {path}");
            lists
        }
        None => DEFAULT.map(|l| l.iter().map(|s| s.to_string()).collect()),
    }
}

/// An app from its desktop file: its name, its command without field codes,
/// and its icon.
fn app(desktop: &str) -> Option<App> {
    let home = std::env::var("HOME").unwrap_or_default();
    let dirs = [format!("{home}/.local/share/applications"), "/usr/local/share/applications".into(), "/usr/share/applications".into()];
    let text = dirs.iter().find_map(|d| std::fs::read_to_string(format!("{d}/{desktop}")).ok());
    let Some(text) = text else {
        tracing::warn!("dock: no {desktop}");
        return None;
    };
    // The [Desktop Entry] group only: actions come after it.
    let entry = text.split("\n[").next().unwrap_or(&text);
    let key = |k: &str| entry.lines().find_map(|l| l.strip_prefix(&format!("{k}="))).map(str::to_owned);
    let exec = key("Exec")?
        .split_whitespace()
        .filter(|w| !(w.starts_with('%') && w.len() == 2))
        .collect::<Vec<_>>()
        .join(" ");
    let icon = key("Icon").and_then(|name| icon(&name));
    Some(App { name: key("Name").unwrap_or_else(|| desktop.to_owned()), exec, icon })
}

/// An icon from the hicolor theme, `ICON` logical px at the output's scale.
fn icon(name: &str) -> Option<MemoryRenderBuffer> {
    let px = (ICON * SCALE) as u32;
    let base = "/usr/share/icons/hicolor";
    let pixmap = if let Ok(data) = std::fs::read(format!("{base}/scalable/apps/{name}.svg")) {
        let tree = resvg::usvg::Tree::from_data(&data, &resvg::usvg::Options::default()).ok()?;
        let size = tree.size();
        let scale = px as f32 / size.width().max(size.height());
        let mut pixmap = tiny_skia::Pixmap::new(px, px)?;
        resvg::render(&tree, tiny_skia::Transform::from_scale(scale, scale), &mut pixmap.as_mut());
        pixmap
    } else {
        // The PNG closest above the size, scaled down.
        let png = ["256x256", "192x192", "128x128", "96x96", "64x64", "48x48"]
            .iter()
            .find_map(|s| tiny_skia::Pixmap::load_png(format!("{base}/{s}/apps/{name}.png")).ok())?;
        let mut pixmap = tiny_skia::Pixmap::new(px, px)?;
        let scale = px as f32 / png.width().max(png.height()) as f32;
        pixmap.draw_pixmap(
            0,
            0,
            png.as_ref(),
            &tiny_skia::PixmapPaint { quality: tiny_skia::FilterQuality::Bicubic, ..Default::default() },
            tiny_skia::Transform::from_scale(scale, scale),
            None,
        );
        pixmap
    };
    // tiny-skia's pixels are premultiplied RGBA: R, G, B, A in memory, as
    // Abgr8888 is.
    Some(MemoryRenderBuffer::from_slice(pixmap.data(), Fourcc::Abgr8888, (px as i32, px as i32), SCALE, Transform::Normal, None))
}

/// The slab: a rounded rectangle `w` by `h` logical px.
fn slab(w: i32, h: i32) -> MemoryRenderBuffer {
    let (pw, ph) = ((w * SCALE) as u32, (h * SCALE) as u32);
    let mut pixmap = tiny_skia::Pixmap::new(pw.max(1), ph.max(1)).expect("slab pixmap");
    let r = RADIUS * SCALE as f32;
    let (fw, fh) = (pw as f32, ph as f32);
    let mut pb = tiny_skia::PathBuilder::new();
    // Quarter circles as cubics.
    let k = r * 0.5523;
    pb.move_to(r, 0.0);
    pb.line_to(fw - r, 0.0);
    pb.cubic_to(fw - r + k, 0.0, fw, r - k, fw, r);
    pb.line_to(fw, fh - r);
    pb.cubic_to(fw, fh - r + k, fw - r + k, fh, fw - r, fh);
    pb.line_to(r, fh);
    pb.cubic_to(r - k, fh, 0.0, fh - r + k, 0.0, fh - r);
    pb.line_to(0.0, r);
    pb.cubic_to(0.0, r - k, r - k, 0.0, r, 0.0);
    pb.close();
    if let Some(path) = pb.finish() {
        let mut paint = tiny_skia::Paint::default();
        paint.set_color_rgba8(SLAB[0], SLAB[1], SLAB[2], SLAB[3]);
        paint.anti_alias = true;
        pixmap.fill_path(&path, &paint, tiny_skia::FillRule::Winding, tiny_skia::Transform::identity(), None);
    }
    MemoryRenderBuffer::from_slice(pixmap.data(), Fourcc::Abgr8888, (pw as i32, ph as i32), SCALE, Transform::Normal, None)
}
