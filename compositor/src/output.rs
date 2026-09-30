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
use smithay::backend::renderer::{Bind, ImportEgl};
use smithay::desktop::space::{space_render_elements, SpaceRenderElements};
use smithay::desktop::Window;
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
    Space=SpaceRenderElements<GlesRenderer, WaylandSurfaceRenderElement<GlesRenderer>>,
}

/// The hwcomposer window's buffers.
const BUFFERS: usize = 3;

pub struct Screen {
    pub output: Output,
    /// Frames swapped so far (the buffer age follows from it).
    frames_drawn: u64,
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
        let mut renderer = unsafe { GlesRenderer::new(context) }.expect("GlesRenderer");

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
        Screen { output, frames_drawn: 0, vsync_period_ns, pixels: width as i64 * height as i64, clock_origin_ns: hybris_hwc::now_ns(), surface, renderer, damage_tracker, _hwc: hwc }
    }

    /// Draws what changed in the space and hands the frame to hwcomposer.
    ///
    /// The buffer's age says how many frames old its contents are; the damage
    /// tracker redraws only what changed since then (everything when the age
    /// is 0, unknown). Nothing changed: no swap. `frame_ns` is when the frame
    /// will be on screen: the shade's runs are drawn where they will be then.
    pub fn render(&mut self, state: &State, frame_ns: u64) -> FrameCost {
        let t0 = hybris_hwc::now_ns();
        // EGL says 2 here, but libhybris' hwcomposer window hands out its
        // BUFFERS in strict turn: a buffer last held the frame BUFFERS frames
        // ago, and nothing before it had been drawn at all. Trusting EGL
        // left the undamaged parts of a buffer three frames old, not two: an
        // empty panel flickered.
        let age = if self.frames_drawn < BUFFERS as u64 { 0 } else { BUFFERS };
        let mut elements: Vec<FrameElement> = state.shade.elements(&mut self.renderer, frame_ns).into_iter().map(FrameElement::from).collect();
        elements.extend(
            space_render_elements::<_, Window, _>(&mut self.renderer, [&state.space], &self.output, 1.0)
                .expect("render elements")
                .into_iter()
                .map(FrameElement::from),
        );
        let damaged_px: i64 = {
            let mut target = self.renderer.bind(&mut self.surface).expect("bind");
            let result = self
                .damage_tracker
                .render_output(&mut self.renderer, &mut target, age, &elements, [0.08, 0.1, 0.14, 1.0])
                .expect("render_output");
            result
                .damage
                .map(|rects| rects.iter().map(|r| r.size.w as i64 * r.size.h as i64).sum())
                .unwrap_or(0)
        };
        let t1 = hybris_hwc::now_ns();
        let swapped = damaged_px > 0;
        if swapped {
            self.surface.swap_buffers(None).expect("swap_buffers");
            // Only a swapped frame uses up a buffer.
            self.frames_drawn += 1;
        }
        FrameCost {
            draw_ns: t1 - t0,
            swap_ns: hybris_hwc::now_ns() - t1,
            age,
            damaged_px,
            swapped,
        }
    }

    /// Frame callbacks to every window, after a frame went out, with the time
    /// it will be on screen.
    pub fn send_frames(&self, state: &State, time: Duration) {
        for window in state.space.elements() {
            window.send_frame(&self.output, time, Some(Duration::ZERO), |_, _| Some(self.output.clone()));
        }
    }
}
