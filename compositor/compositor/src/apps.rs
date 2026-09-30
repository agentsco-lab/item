//! The apps: desktop files and their icons, for the dock and the app grid.
//!
//! A desktop file gives an app's name, its command (without field codes),
//! its icon, and the app ids its windows may give (the file's name and
//! StartupWMClass). Icons come from the hicolor theme, SVG through resvg or
//! PNG, else a file of that name under /usr/share/icons or pixmaps.

use resvg::tiny_skia;
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
use smithay::utils::Transform;

use crate::layout::SCALE;

pub struct Entry {
    pub name: String,
    pub exec: String,
    pub ids: Vec<String>,
    pub icon: Option<String>,
}

fn dirs() -> Vec<String> {
    let home = std::env::var("HOME").unwrap_or_default();
    vec![
        format!("{home}/.local/share/applications"),
        "/usr/local/share/applications".into(),
        "/usr/share/applications".into(),
        "/var/lib/flatpak/exports/share/applications".into(),
    ]
}

/// The `[Desktop Entry]` group's keys (actions come after it).
fn keys(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut in_entry = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_entry = line == "[Desktop Entry]";
            continue;
        }
        if in_entry {
            if let Some((k, v)) = line.split_once('=') {
                out.push((k.trim().to_owned(), v.trim().to_owned()));
            }
        }
    }
    out
}

fn parse(desktop: &str, text: &str) -> Option<Entry> {
    let keys = keys(text);
    let key = |k: &str| keys.iter().find(|(kk, _)| kk == k).map(|(_, v)| v.clone());
    let exec = key("Exec")?
        .split_whitespace()
        .filter(|w| !(w.starts_with('%') && w.len() == 2))
        .collect::<Vec<_>>()
        .join(" ");
    let mut ids = vec![desktop.trim_end_matches(".desktop").to_owned()];
    ids.extend(key("StartupWMClass"));
    Some(Entry { name: key("Name").unwrap_or_else(|| desktop.to_owned()), exec, ids, icon: key("Icon") })
}

/// One app by its desktop file's name.
pub fn entry(desktop: &str) -> Option<Entry> {
    let text = dirs().iter().find_map(|d| std::fs::read_to_string(format!("{d}/{desktop}")).ok());
    match text {
        Some(t) => parse(desktop, &t),
        None => {
            tracing::warn!("apps: no {desktop}");
            None
        }
    }
}

/// Every app to show, by name: applications not hidden, not NoDisplay,
/// and shown on a GNOME or Phosh desktop, the first file of a name winning.
pub fn all() -> Vec<Entry> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for dir in dirs() {
        let Ok(files) = std::fs::read_dir(&dir) else { continue };
        let mut files: Vec<_> = files.flatten().map(|f| f.file_name().to_string_lossy().into_owned()).collect();
        files.sort();
        for name in files {
            if !name.ends_with(".desktop") || !seen.insert(name.clone()) {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(format!("{dir}/{name}")) else { continue };
            let keys = keys(&text);
            let key = |k: &str| keys.iter().find(|(kk, _)| kk == k).map(|(_, v)| v.as_str());
            let yes = |k: &str| key(k).is_some_and(|v| v.eq_ignore_ascii_case("true"));
            let desktops = ["GNOME", "Phosh"];
            let shown_here = key("OnlyShowIn").is_none_or(|v| desktops.iter().any(|d| v.contains(d)))
                && !key("NotShowIn").is_some_and(|v| desktops.iter().any(|d| v.contains(d)));
            if key("Type") != Some("Application") || yes("NoDisplay") || yes("Hidden") || !shown_here {
                continue;
            }
            out.extend(parse(&name, &text));
        }
    }
    out.sort_by_key(|e| e.name.to_lowercase());
    out
}

/// An icon, `size` logical px at the output's scale, as tiny-skia's
/// premultiplied RGBA.
pub fn icon_pixmap(name: &str, size: i32) -> Option<tiny_skia::Pixmap> {
    let px = (size * SCALE) as u32;
    let svg = |path: &str| -> Option<tiny_skia::Pixmap> {
        let data = std::fs::read(path).ok()?;
        let tree = resvg::usvg::Tree::from_data(&data, &resvg::usvg::Options::default()).ok()?;
        let s = tree.size();
        let scale = px as f32 / s.width().max(s.height());
        let mut pixmap = tiny_skia::Pixmap::new(px, px)?;
        resvg::render(&tree, tiny_skia::Transform::from_scale(scale, scale), &mut pixmap.as_mut());
        Some(pixmap)
    };
    let png = |path: &str| -> Option<tiny_skia::Pixmap> {
        let png = tiny_skia::Pixmap::load_png(path).ok()?;
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
        Some(pixmap)
    };
    if name.starts_with('/') {
        return if name.ends_with(".svg") { svg(name) } else { png(name) };
    }
    let base = "/usr/share/icons/hicolor";
    svg(&format!("{base}/scalable/apps/{name}.svg"))
        .or_else(|| ["256x256", "192x192", "128x128", "96x96", "64x64", "48x48"].iter().find_map(|s| png(&format!("{base}/{s}/apps/{name}.png"))))
        .or_else(|| svg(&format!("/usr/share/icons/{name}.svg")))
        .or_else(|| svg(&format!("/usr/share/pixmaps/{name}.svg")))
        .or_else(|| png(&format!("/usr/share/pixmaps/{name}.png")))
        .or_else(|| svg(&format!("/var/lib/flatpak/exports/share/icons/hicolor/scalable/apps/{name}.svg")))
}

/// An icon as a texture, `size` logical px at the output's scale.
pub fn icon(name: &str, size: i32) -> Option<MemoryRenderBuffer> {
    let pixmap = icon_pixmap(name, size)?;
    let px = pixmap.width() as i32;
    // tiny-skia's pixels are premultiplied RGBA: R, G, B, A in memory, as
    // Abgr8888 is.
    Some(MemoryRenderBuffer::from_slice(pixmap.data(), Fourcc::Abgr8888, (px, px), SCALE, Transform::Normal, None))
}
