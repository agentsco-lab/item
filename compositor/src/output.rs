//! The screen: hwcomposer's display 0, EGL on its window, smithay's GLES
//! renderer, and the Wayland output clients see.
//!
//! GL draws the EGL surface's default framebuffer bottom-up, so frames are
//! rendered with smithay's `Flipped180` (a vertical flip, as smallvil does
//! for winit), while clients see an untransformed output.

use std::time::Duration;

use hybris_hwc::Output as HwcOutput;
use smithay::backend::egl::context::{GlAttributes, PixelFormatRequirements};
use smithay::backend::egl::{EGLContext, EGLDisplay, EGLSurface};
use smithay::backend::renderer::damage::OutputDamageTracker;
use smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement;
use smithay::backend::renderer::element::render_elements;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::{Bind, ExportMem, ImportEgl};
use smithay::backend::renderer::element::utils::{Relocate, RelocateRenderElement, RescaleRenderElement};
use smithay::backend::renderer::element::AsRenderElements;
use smithay::output::{Mode, Output, PhysicalProperties, Scale, Subpixel};
use smithay::reexports::wayland_server::DisplayHandle;
use smithay::utils::Transform;
use smithay_hybris::{HwcWindow, HybrisDisplay};

use crate::layout::SCALE;
use crate::shade::ShellElement;
use crate::state::State;

/// What drawing a frame took.
pub struct FrameCost {
    pub draw_ns: u64,
    /// Of the draw: gathering the elements, clients' buffers made textures.
    pub elements_ns: u64,
    pub swap_ns: u64,
    /// The buffer's age (0: unknown, all redrawn).
    pub age: usize,
    /// Physical px redrawn.
    pub damaged_px: i64,
    /// Whether a frame went to hwcomposer (nothing damaged: no).
    pub swapped: bool,
}

render_elements! {
    /// What a frame is drawn from: the shell's own rectangles over the windows.
    FrameElement<=GlesRenderer>;
    Shell=ShellElement,
    Window=WaylandSurfaceRenderElement<GlesRenderer>,
    /// A window being put away or brought back: smaller, moved, fading.
    Moving=RelocateRenderElement<RescaleRenderElement<WaylandSurfaceRenderElement<GlesRenderer>>>,
}

/// The rows of the screen's bottom a run of frames keeps: the dock and above.
const FRAME_ROWS: i32 = 600;

/// The hwcomposer window's buffers.
const BUFFERS: usize = 3;

pub struct Screen {
    pub output: Output,
    /// Frames swapped so far (the buffer age follows from it).
    frames_drawn: u64,
    /// A screenshot asked for: the next frame is drawn whole and saved here.
    pub shot: Option<String>,
    /// A run of frames asked for: this many more swapped frames are saved as
    /// they are, not drawn whole, the bottom `FRAME_ROWS` rows only.
    pub frames_left: u32,
    pub vsync_period_ns: u64,
    /// The output's physical pixels.
    pub pixels: i64,
    /// CLOCK_MONOTONIC at start: frame callbacks' times count from it.
    pub clock_origin_ns: u64,
    surface: EGLSurface,
    renderer: GlesRenderer,
    damage_tracker: OutputDamageTracker,
    _hwc: HwcOutput,
}

impl Screen {
    pub fn new(dh: &DisplayHandle) -> Screen {
        let hwc = HwcOutput::open(BUFFERS as i32);
        let (width, height) = (hwc.width, hwc.height);
        tracing::info!("hwcomposer display 0: {width}x{height}, vsync {:.3} ms", hwc.vsync_period_ns as f64 / 1e6);

        let egl_display = unsafe { EGLDisplay::new(HybrisDisplay) }.expect("EGLDisplay");
        // No swap interval: the swap hands the frame to hwcomposer and returns;
        // frames are paced by hwcomposer's vsync events instead.
        let attributes = GlAttributes { version: (3, 0), profile: None, debug: false, vsync: false };
        let context = EGLContext::new_with_config(&egl_display, attributes, PixelFormatRequirements::_8_bit())
            .expect("EGLContext");
        let pixel_format = context.pixel_format().expect("pixel format");
        let surface = unsafe { EGLSurface::new(&egl_display, pixel_format, context.config_id(), HwcWindow(hwc.window)) }
            .expect("EGLSurface");
        // INSTANCING=0 leaves GL instancing out: each damaged rectangle is
        // drawn on its own, not as an instance of one draw.
        let mut capabilities = unsafe { GlesRenderer::supported_capabilities(&context) }.expect("capabilities");
        if std::env::var_os("INSTANCING").is_some_and(|v| v == "0") {
            capabilities.retain(|c| !matches!(c, smithay::backend::renderer::gles::Capability::Instancing));
        }
        tracing::info!("renderer: {capabilities:?}");
        tracing::info!("EGL: {}", egl_display.extensions().iter().filter(|e| e.contains("age") || e.contains("damage") || e.contains("partial")).cloned().collect::<Vec<_>>().join(" "));
        let mut renderer = unsafe { GlesRenderer::with_capabilities(context, capabilities) }.expect("GlesRenderer");

        // GL clients' buffers (libhybris' android_wlegl) import through EGL.
        match renderer.bind_wl_display(dh) {
            Ok(()) => tracing::info!("EGL bound to the Wayland display"),
            Err(e) => tracing::warn!("bind_wl_display failed, GL clients will not show: {e}"),
        }

        let output = Output::new(
            "duo".into(),
            PhysicalProperties {
                size: (0, 0).into(),
                subpixel: Subpixel::Unknown,
                make: "Microsoft".into(),
                model: "Surface Duo".into(),
            },
        );
        let mode = Mode { size: (width, height).into(), refresh: 60_000 };
        output.change_current_state(Some(mode), Some(Transform::Normal), Some(Scale::Integer(SCALE)), Some((0, 0).into()));
        output.set_preferred(mode);

        let damage_tracker = OutputDamageTracker::new((width, height), SCALE as f64, Transform::Flipped180);
        let vsync_period_ns = hwc.vsync_period_ns as u64;
        Screen { output, frames_drawn: 0, shot: None, frames_left: 0, vsync_period_ns, pixels: width as i64 * height as i64, clock_origin_ns: hybris_hwc::now_ns(), surface, renderer, damage_tracker, _hwc: hwc }
    }

    /// Draws what changed in the space and hands the frame to hwcomposer.
    ///
    /// Nothing changed since the last frame: no frame, no swap. Else the frame
    /// is drawn whole. `frame_ns` is when the frame
    /// will be on screen: the shade's runs are drawn where they will be then.
    pub fn render(&mut self, state: &State, frame_ns: u64) -> FrameCost {
        let t0 = hybris_hwc::now_ns();
        // The shade over the launch curtain over the dock over the windows.
        let mut elements: Vec<FrameElement> = state.shade.elements(&mut self.renderer, frame_ns).into_iter().map(FrameElement::from).collect();
        elements.extend(state.curtain.elements(&mut self.renderer, frame_ns).into_iter().map(FrameElement::from));
        elements.extend(state.grid.elements(&mut self.renderer, frame_ns).into_iter().map(FrameElement::from));
        elements.extend(state.dock.elements(&mut self.renderer, frame_ns).into_iter().map(FrameElement::from));
        // The windows, topmost first; one put away or brought back as the
        // gesture has it (gesture.rs).
        let scale = smithay::utils::Scale::from(SCALE as f64);
        for window in state.space.elements().rev() {
            let Some(loc) = state.space.element_location(window) else { continue };
            let loc = loc.to_physical(SCALE);
            match state.gestures.progress(window, frame_ns) {
                Some(p) => {
                    let geo = smithay::utils::Rectangle::new(loc, window.geometry().size.to_physical(SCALE));
                    let (s, offset) = crate::gesture::placement(geo, p);
                    let parts: Vec<WaylandSurfaceRenderElement<GlesRenderer>> =
                        window.render_elements(&mut self.renderer, loc, scale, crate::gesture::alpha(p));
                    elements.extend(parts.into_iter().map(|e| {
                        FrameElement::Moving(RelocateRenderElement::from_element(
                            RescaleRenderElement::from_element(e, loc, s),
                            offset,
                            Relocate::Relative,
                        ))
                    }));
                }
                None => {
                    let parts: Vec<WaylandSurfaceRenderElement<GlesRenderer>> = window.render_elements(&mut self.renderer, loc, scale, 1.0);
                    elements.extend(parts.into_iter().map(FrameElement::Window));
                }
            }
        }
        let elements_ns = hybris_hwc::now_ns() - t0;
        // Every frame is drawn whole (age 0). Drawing only the damage on top
        // of the buffer's old contents (its age: BUFFERS, the window's strict
        // turn) left garbage on Adreno through libhybris: the driver does not
        // load a buffer's contents into its tiles, so around a partial redraw
        // each tile drawn in kept whatever the GPU last had there - pieces of
        // other icons, a neighbour icon gone (log/2026-09-30-step-6-dock.md).
        // Asking EGL for the age, which makes some drivers load them, made it
        // worse. The damage tracker still says whether anything changed:
        // nothing, and there is no frame. PARTIAL=1 draws the damage only, for
        // tests.
        let partial = std::env::var_os("PARTIAL").is_some_and(|v| v == "1");
        let primed = self.frames_drawn >= BUFFERS as u64;
        if !partial && primed && self.shot.is_none() {
            let changed = self.damage_tracker.damage_output(1, &elements).map(|(d, _)| d.is_some()).unwrap_or(true);
            if !changed {
                return FrameCost { draw_ns: hybris_hwc::now_ns() - t0, elements_ns, swap_ns: 0, age: 0, damaged_px: 0, swapped: false };
            }
        }
        let age = if partial && primed && self.shot.is_none() { BUFFERS } else { 0 };
        let mut shot = None;
        let damaged_px: i64 = {
            let mut target = self.renderer.bind(&mut self.surface).expect("bind");
            let result = self
                .damage_tracker
                .render_output(&mut self.renderer, &mut target, age, &elements, [0.08, 0.1, 0.14, 1.0])
                .expect("render_output");
            if std::env::var_os("LOG_DAMAGE").is_some() {
                use smithay::backend::renderer::element::Element;
                let geos: Vec<String> = elements
                    .iter()
                    .take(12)
                    .map(|e| {
                        let g = e.geometry(smithay::utils::Scale::from(SCALE as f64));
                        format!("{},{} {}x{}", g.loc.x, g.loc.y, g.size.w, g.size.h)
                    })
                    .collect();
                let rects: Vec<String> = result
                    .damage
                    .map(|r| r.iter().map(|r| format!("{},{} {}x{}", r.loc.x, r.loc.y, r.size.w, r.size.h)).collect())
                    .unwrap_or_default();
                tracing::info!("damage: frame {} age {age}: {} | elements: {}", self.frames_drawn, rects.join("; "), geos.join("; "));
            }
            let damaged = result
                .damage
                .map(|rects| rects.iter().map(|r| r.size.w as i64 * r.size.h as i64).sum())
                .unwrap_or(0);
            // The frame as it goes to the screen, copied before the swap and
            // read after it: reading makes the context current without the
            // surface, and a swap then fails (BadSurface).
            let run = damaged > 0 && self.frames_left > 0 && self.shot.is_none();
            if run {
                self.frames_left -= 1;
                self.shot = Some(format!("/tmp/item-frame-{:03}-{}.rgba", self.frames_drawn, hybris_hwc::now_ns() / 1_000_000 % 100_000));
            }
            if let Some(path) = self.shot.take() {
                let size = self.output.current_mode().unwrap().size;
                // GL's rows are bottom-up: the first rows are the screen's bottom.
                let size = if run { (size.w, FRAME_ROWS).into() } else { size };
                let region = smithay::utils::Rectangle::from_size((size.w, size.h).into());
                match self.renderer.copy_framebuffer(&target, region, Fourcc::Abgr8888) {
                    Ok(mapping) => shot = Some((path, mapping, size)),
                    Err(e) => tracing::warn!("screenshot: {e}"),
                }
            }
            damaged
        };
        let t1 = hybris_hwc::now_ns();
        let swapped = damaged_px > 0;
        if swapped {
            self.surface.swap_buffers(None).expect("swap_buffers");
            // Only a swapped frame uses up a buffer.
            self.frames_drawn += 1;
        }
        if std::env::var_os("LOG_BUFFERS").is_some() && swapped {
            let order = hybris_hwc::presented_buffers(12);
            let mut ids: Vec<usize> = order.clone();
            ids.sort();
            ids.dedup();
            let names: String = order.iter().map(|b| (b'A' + ids.iter().position(|x| x == b).unwrap() as u8) as char).collect();
            tracing::info!("buffers: frame {} age {age}: {names} ({} seen)", self.frames_drawn, ids.len());
        }
        if let Some((path, mapping, size)) = shot {
            let saved = self
                .renderer
                .map_texture(&mapping)
                .map_err(|e| e.to_string())
                .and_then(|bytes| std::fs::write(&path, bytes).map_err(|e| e.to_string()));
            match saved {
                // Rows bottom-up, as GL has them.
                Ok(()) => tracing::info!("screenshot: {path} ({}x{}, RGBA, bottom-up)", size.w, size.h),
                Err(e) => tracing::warn!("screenshot: {e}"),
            }
        }
        FrameCost {
            draw_ns: t1 - t0,
            elements_ns,
            swap_ns: hybris_hwc::now_ns() - t1,
            age,
            damaged_px,
            swapped,
        }
    }

    /// Whether every buffer of the window has had a frame.
    pub fn primed(&self) -> bool {
        self.frames_drawn >= BUFFERS as u64
    }

    /// Uploads the shell's textures before they are first needed.
    pub fn warm_up(&mut self, state: &State) {
        let t = hybris_hwc::now_ns();
        let n = state.shade.warm_up(&mut self.renderer) + state.dock.warm_up(&mut self.renderer) + state.grid.warm_up(&mut self.renderer);
        tracing::info!("warm-up: {n} textures in {:.1} ms", (hybris_hwc::now_ns() - t) as f64 / 1e6);
    }

    /// Frame callbacks to every window, after a frame went out, with the time
    /// it will be on screen.
    pub fn send_frames(&self, state: &State, time: Duration) {
        for window in state.space.elements() {
            window.send_frame(&self.output, time, Some(Duration::ZERO), |_, _| Some(self.output.clone()));
        }
    }
}
