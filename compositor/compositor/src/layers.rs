//! Layer surfaces (wlr-layer-shell), for the on-screen keyboard: the port's
//! phosh-osk-stevia, run as a client, with the input-method, text-input and
//! virtual-keyboard protocols (state.rs) doing the rest - a text field that
//! takes the keyboard focus activates the input method, and stevia shows
//! itself and types through its virtual keyboard.
//!
//! item keeps the keyboard off the hinge. A surface anchored to the bottom
//! and both sides - the keyboard - is given one panel's width, not the
//! output's, and stands at the bottom of the panel whose window has the
//! keyboard focus. While it is up, the windows on that panel are made
//! shorter by its height, so a text field is not left under it. Other layer
//! surfaces get what they ask for, at the output's top left.

use smithay::backend::renderer::element::surface::{render_elements_from_surface_tree, WaylandSurfaceRenderElement};
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::renderer::utils::with_renderer_surface_state;
use smithay::desktop::utils::{send_frames_surface_tree, under_from_surface_tree};
use smithay::desktop::WindowSurfaceType;
use smithay::output::Output;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Point, Rectangle};
use smithay::wayland::compositor::with_states;
use smithay::wayland::shell::wlr_layer::{Anchor, LayerSurface, LayerSurfaceCachedState, LayerSurfaceData};

use crate::layout::{self, SCALE};

/// The keyboard asked up by item (the power key, input.rs).
pub static KEYBOARD_ASKED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[derive(Default)]
pub struct Layers {
    pub surfaces: Vec<LayerSurface>,
    /// Each surface's namespace, by its wl_surface.
    pub names: Vec<(WlSurface, String)>,
    /// The panel the keyboard stands on.
    pub panel: usize,
    /// Keyboards seen all the way down once since they came: stevia shows
    /// itself as it starts and slides away, which flashed its keys over the
    /// right panel's bottom; until it is down once it is not drawn.
    pub settled: std::cell::RefCell<Vec<WlSurface>>,
}

/// A layer surface's committed state: its anchor, the size it asks for,
/// and its margins (top, right, bottom, left).
fn cached(s: &LayerSurface) -> (Anchor, smithay::utils::Size<i32, Logical>) {
    let (a, size, _) = cached_all(s);
    (a, size)
}

fn cached_all(s: &LayerSurface) -> (Anchor, smithay::utils::Size<i32, Logical>, [i32; 4]) {
    with_states(s.wl_surface(), |states| {
        let mut c = states.cached_state.get::<LayerSurfaceCachedState>();
        let c = c.current();
        (c.anchor, c.size, [c.margin.top, c.margin.right, c.margin.bottom, c.margin.left])
    })
}

/// Whether a layer surface is a keyboard: bottom, left and right.
fn is_keyboard(s: &LayerSurface) -> bool {
    let (a, _) = cached(s);
    a.contains(Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT)
}

fn has_buffer(surface: &WlSurface) -> bool {
    with_renderer_surface_state(surface, |s| s.buffer().is_some()).unwrap_or(false)
}

impl Layers {
    /// A layer surface's commit: its first gets the configure with its size.
    pub fn commit(&self, surface: &WlSurface) {
        let Some(layer) = self.surfaces.iter().find(|l| l.wl_surface() == surface) else { return };
        if std::env::var_os("OSK_DEBUG").is_some() && self.is_osk(layer) {
            let (_, size, m) = cached_all(layer);
            tracing::info!("osk commit: {}x{} margins {:?} buffer {} settled {}", size.w, size.h, m, has_buffer(surface), !self.unsettled(layer));
        }
        let sent = with_states(surface, |states| states.data_map.get::<LayerSurfaceData>().unwrap().lock().unwrap().initial_configure_sent);
        if sent {
            return;
        }
        let (want, keyboard) = (cached(layer).1, is_keyboard(layer));
        let panel_w = layout::panels()[0].size.w;
        let size = if keyboard { (panel_w, want.h).into() } else { want };
        layer.with_pending_state(|s| s.size = Some(size));
        layer.send_configure();
        tracing::info!("layer surface: {}x{} asked, {}x{} given{}", want.w, want.h, size.w, size.h, if keyboard { " (keyboard)" } else { "" });
    }

    /// Where a layer surface stands, logical px, if it is shown.
    fn place(&self, layer: &LayerSurface) -> Option<Rectangle<i32, Logical>> {
        if !has_buffer(layer.wl_surface()) {
            return None;
        }
        let (anchor, size, [top, right, bottom, left]) = cached_all(layer);
        let geo = with_renderer_surface_state(layer.wl_surface(), |s| s.surface_size()).flatten().unwrap_or(size);
        // A keyboard across the whole width (a stock one): one panel, the
        // focused window's.
        if is_keyboard(layer) {
            let panel = layout::panels()[self.panel];
            return Some(Rectangle::new((panel.loc.x, panel.size.h - geo.h - bottom).into(), geo));
        }
        // Else as the protocol has it, by its anchors and margins - item's
        // keyboard takes its panel by its left margin (717: the right one)
        // and hides below the edge by a negative bottom margin.
        let (w, h) = layout::LAYOUT;
        let x = match (anchor.contains(Anchor::LEFT), anchor.contains(Anchor::RIGHT)) {
            (true, false) => left,
            (false, true) => w - geo.w - right,
            _ => (w - geo.w) / 2 + left - right,
        };
        let y = match (anchor.contains(Anchor::TOP), anchor.contains(Anchor::BOTTOM)) {
            (true, false) => top,
            (false, true) => h - geo.h - bottom,
            _ => (h - geo.h) / 2 + top - bottom,
        };
        Some(Rectangle::new((x, y).into(), geo))
    }

    /// The keyboard's height on its panel, as much of it as is on screen,
    /// if it is up: the surface at the bottom of the screen named "osk", or
    /// a stock one across the width.
    /// Whether a layer surface is a keyboard.
    fn is_osk(&self, l: &LayerSurface) -> bool {
        is_keyboard(l) || self.names.iter().any(|(s, n)| s == l.wl_surface() && n == "osk")
    }

    /// A keyboard not yet down once since it came (see `settled`) - unless
    /// item asked it up (the power key): stevia makes a new surface each
    /// time it shows, and without its slide it is never down.
    fn unsettled(&self, l: &LayerSurface) -> bool {
        self.is_osk(l) && !KEYBOARD_ASKED.load(std::sync::atomic::Ordering::Relaxed) && !self.settled.borrow().contains(l.wl_surface())
    }

    pub fn keyboard_height(&self) -> Option<(usize, i32)> {
        let screen_h = layout::LAYOUT.1;
        self.surfaces.iter().filter(|l| self.is_osk(l)).find_map(|l| {
            let r = self.place(l)?;
            let shown = (screen_h - r.loc.y).min(r.size.h);
            if shown <= 0 {
                let mut settled = self.settled.borrow_mut();
                if !settled.contains(l.wl_surface()) {
                    settled.push(l.wl_surface().clone());
                }
                return None;
            }
            if self.unsettled(l) {
                return None;
            }
            let panel = layout::panel_at(r.loc.to_f64() + Point::from((1.0, 1.0))).unwrap_or(self.panel);
            Some((panel, shown))
        })
    }

    /// The surface under a point, on a layer surface.
    pub fn surface_under(&self, pos: Point<f64, Logical>) -> Option<(WlSurface, Point<f64, Logical>)> {
        self.surfaces.iter().rev().find_map(|l| {
            let at = self.place(l)?;
            if self.unsettled(l) || !at.to_f64().contains(pos) {
                return None;
            }
            under_from_surface_tree(l.wl_surface(), pos, at.loc, WindowSurfaceType::ALL).map(|(s, p)| (s, p.to_f64()))
        })
    }

    pub fn elements(&self, renderer: &mut GlesRenderer) -> Vec<WaylandSurfaceRenderElement<GlesRenderer>> {
        let mut out = Vec::new();
        for layer in self.surfaces.iter().rev() {
            let Some(at) = self.place(layer) else { continue };
            if self.unsettled(layer) {
                continue;
            }
            out.extend(render_elements_from_surface_tree(
                renderer,
                layer.wl_surface(),
                at.loc.to_physical(SCALE),
                smithay::utils::Scale::from(SCALE as f64),
                1.0,
                Kind::Unspecified,
            ));
        }
        out
    }

    pub fn send_frames(&self, output: &Output, time: std::time::Duration, take: &mut impl FnMut(&smithay::reexports::wayland_server::protocol::wl_surface::WlSurface) -> bool) {
        for layer in &self.surfaces {
            if !take(layer.wl_surface()) {
                continue;
            }
            send_frames_surface_tree(layer.wl_surface(), output, time, Some(std::time::Duration::ZERO), |_, _| Some(output.clone()));
        }
    }
}
