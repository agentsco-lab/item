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
use smithay::backend::renderer::{Bind, Blit, ExportMem, ImportEgl, Offscreen, Renderer};
use smithay::reexports::wayland_server::Resource;
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
    /// A window that closed: its last frame, shrinking and fading.
    Snapshot=smithay::backend::renderer::element::texture::TextureRenderElement<smithay::backend::renderer::gles::GlesTexture>,
    /// A window being put away or brought back: smaller, moved, fading.
    Moving=RelocateRenderElement<RescaleRenderElement<WaylandSurfaceRenderElement<GlesRenderer>>>,
    /// A texture through a shader of ours: the lock screen's wave (wave.frag).
    Shaded=smithay::backend::renderer::gles::element::TextureShaderElement,
    /// Drawn by a shader of ours alone: the first setup's circle (orb.frag).
    Pixel=smithay::backend::renderer::gles::element::PixelShaderElement,
    /// The shell's own, carried along the ribbon with its page.
    Carried=RelocateRenderElement<ShellElement>,
}

/// The rows of the screen's bottom a run of frames keeps: the dock and above.
const FRAME_ROWS: i32 = 600;

/// A window's last frame: its main surface's texture and where it stood,
/// kept so that a window can be seen closing after its surface is gone.
pub struct Snapshot {
    texture: smithay::backend::renderer::gles::GlesTexture,
    loc: smithay::utils::Point<i32, smithay::utils::Logical>,
    size: smithay::utils::Size<i32, smithay::utils::Logical>,
    scale: i32,
    transform: Transform,
    id: smithay::backend::renderer::element::Id,
}

/// A window's close (item's, phoc's): 200 ms, easing out, to 0.92 of its
/// size, fading.
pub const CLOSE_NS: u64 = 200_000_000;

/// The hwcomposer window's buffers.
const BUFFERS: usize = 3;

/// This thread's CPU time (ns), for LOG_TIMES: wall time less it is waiting.
fn cpu_ns() -> u64 {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut ts) };
    ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64
}

/// Which clients hear their frame callbacks now (pace.rs).
#[derive(Clone, Copy, PartialEq)]
pub enum Group {
    All,
    Quick,
    Slow,
}

/// Our own buffer, with its own damage tracker: it keeps every pixel between
/// frames, so only what changed is drawn into it.
struct Canvas {
    /// A texture, so that night light can draw it to the screen warmed.
    buffer: smithay::backend::renderer::gles::GlesTexture,
    tracker: OutputDamageTracker,
}

pub struct Screen {
    pub output: Output,
    /// Frames swapped so far (the buffer age follows from it).
    frames_drawn: u64,
    /// A screenshot asked for: the next frame is drawn whole and saved here.
    pub shot: Option<String>,
    /// Frames to draw whole and present whatever changed: after the display
    /// is lit again, as at start (hwcomposer does not show the first).
    pub reprime: u32,
    /// Each window's last frame, by its surface.
    snapshots: std::collections::HashMap<smithay::reexports::wayland_server::backend::ObjectId, Snapshot>,
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
    /// Our own buffer the frame is drawn into (CANVAS=0: none).
    canvas: Option<Canvas>,
    canvas_ready: bool,
    /// The doors' picture of the lock screen, and since when (door.rs).
    door: Option<(u64, smithay::backend::renderer::gles::GlesTexture)>,
    door_ids: Vec<smithay::backend::renderer::element::Id>,
    wave_program: Option<smithay::backend::renderer::gles::GlesTexProgram>,
    orb_program: Option<smithay::backend::renderer::gles::GlesPixelProgram>,
    wave_id: smithay::backend::renderer::element::Id,
    /// The screen under the shade, blurred (glass.rs), and which opening of
    /// the shade it was taken for.
    glass: Option<crate::glass::Glass>,
    glass_for: u64,
    /// Night light's shader (night.frag), and the colour it last warmed to.
    night_program: Option<smithay::backend::renderer::gles::GlesTexProgram>,
    night_was: Option<[f32; 3]>,
    /// The dimming before the screen goes dark.
    dim_id: smithay::backend::renderer::element::Id,
    /// The ribbon's dot.
    dot: Option<smithay::backend::renderer::element::memory::MemoryRenderBuffer>,
    /// The wallpaper (walls.rs): a texture the output's scale, wider than
    /// the screen by its parallax; the one before, fading out under it, and
    /// since when; the id it is drawn under, new as it moves, and at which
    /// shift.
    wall: Option<smithay::backend::renderer::gles::GlesTexture>,
    wall_old: Option<(smithay::backend::renderer::gles::GlesTexture, u64, smithay::backend::renderer::element::Id)>,
    wall_at: (smithay::backend::renderer::element::Id, f32),
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
        let canvas = if std::env::var_os("CANVAS").is_some_and(|v| v == "0") {
            None
        } else {
            let buffer: smithay::backend::renderer::gles::GlesTexture = renderer.create_buffer(Fourcc::Abgr8888, (width, height).into()).expect("the canvas");
            Some(Canvas { buffer, tracker: OutputDamageTracker::new((width, height), SCALE as f64, Transform::Flipped180) })
        };
        tracing::info!("frames: {}", if canvas.is_some() { "drawn where changed into a buffer of our own, copied whole" } else { "drawn whole (CANVAS=0)" });
        let vsync_period_ns = hwc.vsync_period_ns as u64;
        Screen { output, frames_drawn: 0, reprime: 0, snapshots: Default::default(), shot: None, frames_left: 0, vsync_period_ns, pixels: width as i64 * height as i64, clock_origin_ns: hybris_hwc::now_ns(), surface, renderer, damage_tracker, canvas, canvas_ready: false, door: None, door_ids: Vec::new(), wave_program: None, orb_program: None, wave_id: smithay::backend::renderer::element::Id::new(), glass: None, glass_for: 0, night_program: None, night_was: None, dim_id: smithay::backend::renderer::element::Id::new(), dot: None, wall: None, wall_old: None, wall_at: (smithay::backend::renderer::element::Id::new(), 0.0), _hwc: hwc }
    }

    /// Draws what changed in the space and hands the frame to hwcomposer.
    ///
    /// Nothing changed since the last frame: no frame, no swap. Else the frame
    /// is drawn whole. `frame_ns` is when the frame
    /// will be on screen: the shade's runs are drawn where they will be then.
    pub fn render(&mut self, state: &State, frame_ns: u64) -> FrameCost {
        let t0 = hybris_hwc::now_ns();
        let c0 = cpu_ns();
        // Night light, as its setting says; NIGHT=K forces it, for tests
        // that leave the user's settings alone.
        let night = match std::env::var("NIGHT").ok().and_then(|k| k.parse().ok()) {
            Some(k) => Some(warm_white(k)),
            None => {
                let sys = state.shade.quick.sys.lock().unwrap();
                sys.night.then(|| warm_white(sys.night_k))
            }
        };
        // The shade coming out of a closed screen: the glass under it from
        // the frame on screen now, which it is not in yet.
        if state.shade.opened != self.glass_for {
            self.glass_for = state.shade.opened;
            if let (Some(glass), Some(canvas), true) = (self.glass.as_mut(), self.canvas.as_mut(), self.canvas_ready) {
                let size = self.output.current_mode().unwrap().size;
                if let Err(e) = glass.take(&mut self.renderer, &mut canvas.buffer, size) {
                    tracing::warn!("glass: {e}");
                }
            }
        }
        // The shade over the launch curtain over the dock over the windows.
        // The lock screen over everything.
        // The volume bar over everything, the lock screen next.
        let mut elements: Vec<FrameElement> = Vec::new();
        // About to go dark for idleness: dimmed over everything.
        if state.lock.dimming(frame_ns) {
            let (w, h) = crate::layout::LAYOUT;
            let rect = smithay::utils::Rectangle::<i32, smithay::utils::Physical>::from_size((w * SCALE, h * SCALE).into());
            elements.push(FrameElement::Shell(ShellElement::Solid(smithay::backend::renderer::element::solid::SolidColorRenderElement::new(self.dim_id.clone(), rect, smithay::backend::renderer::utils::CommitCounter::default(), [0.0, 0.0, 0.0, 0.55], smithay::backend::renderer::element::Kind::Unspecified))));
        }
        elements.extend(state.shade.quick.volume_bar(&mut self.renderer, frame_ns).into_iter().map(FrameElement::from));
        // The first setup: its circle over its words (setup.rs, orb.frag).
        if let Some(orb) = state.setup.orb() {
            elements.extend(self.orb_element(&orb));
        }
        elements.extend(state.setup.elements(&mut self.renderer, frame_ns).into_iter().map(FrameElement::from));
        elements.extend(state.lock.elements(&mut self.renderer, frame_ns).into_iter().map(FrameElement::from));
        // The system's dialog, under the lock screen, over everything else.
        elements.extend(state.dialog.elements(&mut self.renderer, frame_ns).into_iter().map(FrameElement::from));
        // The lock screen, or the setup, going: a picture of it taken as it
        // began, in a wave from where it was touched, or as doors (door.rs).
        let turn = state
            .setup
            .turning(frame_ns)
            .map(|(style, k, start, from)| (style, k, start, from, true))
            .or_else(|| state.lock.turning(frame_ns).map(|(style, k, start)| (style, k, start, state.lock.touched_at(), false)));
        if let Some((style, k, start, from, setup)) = turn {
            if self.door.as_ref().map(|d| d.0) != Some(start) {
                self.door = self.cover_picture(state, start, setup).map(|t| (start, t));
            }
            if let Some((_, texture)) = &self.door {
                let texture = texture.clone();
                if style.mode == crate::door::Mode::Wave {
                    elements.extend(self.wave(from, frame_ns, start, &style, texture));
                } else {
                    elements.extend(self.door_strips(&style, k, texture));
                }
            }
        } else if self.door.is_some() && !state.lock.locked && !state.setup.active {
            self.door = None;
        }
        elements.extend(state.back.elements(&mut self.renderer).into_iter().map(FrameElement::from));
        let rows: Vec<crate::quick::Row> = if state.shade.visible() { state.shade_rows() } else { Vec::new() };
        // A new notification's banner, on top of everything; none while
        // do not disturb is on.
        state.shade.quick.banner_at.set(None);
        let dnd = state.shade.quick.sys.lock().unwrap().dnd;
        if let Some(note) = state.notes.banner(frame_ns).filter(|_| !dnd) {
            let (_, banner) = state.shade.quick.banner(&mut self.renderer, &note, frame_ns.saturating_sub(note.at_ns));
            elements.extend(banner.into_iter().map(FrameElement::from));
        }
        let glass = self.glass.is_some() && self.canvas.is_some();
        elements.extend(state.shade.elements(&mut self.renderer, frame_ns, &rows, glass).into_iter().map(FrameElement::from));
        // The wallpaper's choosing, over the desktop.
        let current = state.walls.names.iter().position(|n| *n == state.walls.current).unwrap_or(0);
        elements.extend(state.picker.elements(&mut self.renderer, frame_ns, &state.walls.thumbs, current).into_iter().map(FrameElement::from));
        if let (Some(g), true) = (&self.glass, state.shade.visible()) {
            // Under each sheet, the glass cut to it: half the output's size,
            // so at scale 1 its logical px are the output's; bottom-up, as
            // the canvas it came from.
            for (panel, h) in crate::layout::panels().into_iter().zip(state.shade.heights(frame_ns)) {
                if h <= 0 {
                    continue;
                }
                let src = smithay::utils::Rectangle::<f64, smithay::utils::Logical>::new((panel.loc.x as f64, 0.0).into(), (panel.size.w as f64, h as f64).into());
                elements.push(FrameElement::Snapshot(smithay::backend::renderer::element::texture::TextureRenderElement::from_static_texture(
                    g.id.clone(),
                    self.renderer.context_id(),
                    ((panel.loc.x * SCALE) as f64, 0.0),
                    g.texture().clone(),
                    1,
                    Transform::Flipped180,
                    None,
                    Some(src),
                    Some((panel.size.w, h).into()),
                    None,
                    smithay::backend::renderer::element::Kind::Unspecified,
                )));
            }
        }
        elements.extend(state.curtain.elements(&mut self.renderer, frame_ns).into_iter().map(FrameElement::from));
        // The keyboard and other layer surfaces, over the windows and the dock.
        elements.extend(state.layers.elements(&mut self.renderer).into_iter().map(FrameElement::Window));
        let running = state.running();
        // The ribbon moving (ribbon.rs): each page where it is on its way,
        // what belongs to a panel's page carried with it.
        let ribbon = state.ribbon.moving();
        let xs = if ribbon { state.ribbon.xs(frame_ns) } else { Vec::new() };
        let width = crate::layout::LAYOUT.0 as f64;
        let seen = |x: f64| x > -crate::ribbon::PAGE && x < width;
        let carry = |parts: Vec<ShellElement>, dx: f64| -> Vec<FrameElement> {
            let at = smithay::utils::Point::<i32, smithay::utils::Physical>::from(((dx * SCALE as f64).round() as i32, 0));
            parts.into_iter().map(|e| FrameElement::Carried(RelocateRenderElement::from_element(e, at, Relocate::Relative))).collect()
        };
        // Its dots, at the bottom of the left panel, above the dock.
        if let Some((n, pos, alpha)) = state.ribbon.dots(frame_ns) {
            let dot = self.dot.get_or_insert_with(|| crate::grid::rounded(7.0, 7.0, 3.5, [255, 255, 255, 255])).clone();
            let left = crate::layout::panels()[0];
            let step = 16.0;
            let x0 = left.loc.x as f64 + (left.size.w as f64 - (n as f64 - 1.0) * step - 7.0) / 2.0;
            for i in 0..n {
                // The two pages shown are lit, the rest dim.
                let near = (1.0 - (i as f64 - pos).abs()).max(0.0).max((1.0 - (i as f64 - pos - 1.0).abs()).max(0.0));
                let a = alpha * (0.3 + 0.7 * near as f32);
                let loc = (((x0 + i as f64 * step) * SCALE as f64).round(), ((left.size.h as f64 - 108.0) * SCALE as f64).round());
                if let Ok(e) = smithay::backend::renderer::element::memory::MemoryRenderBufferRenderElement::from_buffer(&mut self.renderer, loc, &dot, Some(a), None, None, smithay::backend::renderer::element::Kind::Unspecified) {
                    elements.push(FrameElement::Shell(ShellElement::Text(e)));
                }
            }
        }
        if ribbon {
            for (page, x) in &xs {
                match page {
                    crate::ribbon::Page::System if seen(*x) => {
                        let parts = state.system.picture(&mut self.renderer, frame_ns);
                        elements.extend(carry(parts, *x));
                    }
                    crate::ribbon::Page::Pen if seen(*x) => {
                        let parts = state.pen.picture(&mut self.renderer);
                        elements.extend(carry(parts, *x - crate::ribbon::PAGE));
                    }
                    _ => {}
                }
            }
        } else {
            elements.extend(state.system.elements(&mut self.renderer, frame_ns).into_iter().map(FrameElement::from));
            elements.extend(state.pen.elements(&mut self.renderer, frame_ns).into_iter().map(FrameElement::from));
        }
        elements.extend(state.grid.elements(&mut self.renderer, frame_ns, &running).into_iter().map(FrameElement::from));
        // The dock is the screen's: where it stands (it follows the ribbon
        // itself, dock.rs). The clock is its desk's: carried with it.
        let shift_now = crate::state::wallpaper_shift(state.ribbon.position(frame_ns));
        let wall_for_drops = self.wall.as_ref().map(|t| (t, (wall_width() as f64, crate::layout::LAYOUT.1 as f64), crate::state::WALL_LEFT + shift_now));
        elements.extend(state.dock.elements(&mut self.renderer, frame_ns, &running, wall_for_drops).into_iter().map(FrameElement::from));
        let clock_panel = state.ribbon.clock_panel().or(state.dock.home_panel());
        // Choosing the wallpaper: no clock over it.
        let clock_panel = if state.picker.visible(frame_ns) { None } else { clock_panel };
        let carried = state.ribbon.clock_panel().is_some();
        let clock = state.clock.elements(&mut self.renderer, clock_panel, frame_ns, carried);
        match clock_panel.and_then(|k| state.ribbon.carried(k, frame_ns)) {
            Some(dx) => elements.extend(carry(clock, dx)),
            None => elements.extend(clock.into_iter().map(FrameElement::from)),
        }
        // The windows, topmost first; one put away or brought back as the
        // gesture has it (gesture.rs).
        let scale = smithay::utils::Scale::from(SCALE as f64);
        // Windows closing: their last frame, shrinking about its centre and
        // fading, over the windows left.
        for (sid, since) in &state.closing {
            let Some(snap) = self.snapshots.get(sid) else { continue };
            let k = (frame_ns.saturating_sub(*since) as f64 / CLOSE_NS as f64).clamp(0.0, 1.0);
            let e = 1.0 - (1.0 - k).powi(3);
            let s = 1.0 - 0.08 * e;
            let size = snap.size.to_f64().upscale(s).to_i32_round();
            let centre = snap.loc.to_f64() + snap.size.to_f64().downscale(2.0).to_point();
            let at = (centre - size.to_f64().downscale(2.0).to_point()).to_physical(SCALE as f64);
            elements.push(FrameElement::Snapshot(smithay::backend::renderer::element::texture::TextureRenderElement::from_static_texture(
                snap.id.clone(),
                self.renderer.context_id(),
                at,
                snap.texture.clone(),
                snap.scale,
                snap.transform,
                Some((1.0 - e) as f32),
                None,
                Some(size),
                None,
                smithay::backend::renderer::element::Kind::Unspecified,
            )));
        }
        let context = self.renderer.context_id();
        // The ribbon moving: the windows on its pages where they are on
        // their way, not where they stand.
        if ribbon {
            for (page, x) in &xs {
                if let crate::ribbon::Page::App(window) = page {
                    if !seen(*x) {
                        continue;
                    }
                    let loc = smithay::utils::Point::<i32, smithay::utils::Physical>::from(((x * SCALE as f64).round() as i32, 0));
                    let parts: Vec<WaylandSurfaceRenderElement<GlesRenderer>> = window.render_elements(&mut self.renderer, loc, scale, 1.0);
                    elements.extend(parts.into_iter().map(FrameElement::Window));
                }
            }
        }
        for window in state.space.elements().rev().filter(|_| !ribbon) {
            let Some(mut loc) = state.space.element_location(window) else { continue };
            // Its last frame, in case it closes.
            let surface = window.toplevel().unwrap().wl_surface();
            let snap = smithay::backend::renderer::utils::with_renderer_surface_state(surface, |rs| {
                Some((rs.texture::<smithay::backend::renderer::gles::GlesTexture>(context.clone())?.clone(), rs.surface_size()?, rs.buffer_scale(), rs.buffer_transform()))
            })
            .flatten();
            if let Some((texture, size, bscale, transform)) = snap {
                let id = self.snapshots.get(&surface.id()).map(|s| s.id.clone()).unwrap_or_else(smithay::backend::renderer::element::Id::new);
                self.snapshots.insert(surface.id(), Snapshot { texture, loc, size, scale: bscale, transform, id });
            }
            if let Some(dx) = state.gestures.slide_offset(window, frame_ns) {
                loc.x += dx;
            }
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
        // The wallpaper, under everything; it moves a little with the ribbon.
        let shift = crate::state::wallpaper_shift(state.ribbon.position(frame_ns)) as f32;
        let wall = self.wallpaper(shift, frame_ns);
        elements.extend(wall.into_iter().map(FrameElement::Snapshot));
        let elements_ns = hybris_hwc::now_ns() - t0;
        let primed = self.frames_drawn >= BUFFERS as u64 && self.reprime == 0;
        self.reprime = self.reprime.saturating_sub(1);
        let (damaged_px, age, shot) = if let Some(canvas) = self.canvas.as_mut() {
            // The frame is drawn into a buffer of our own, only where it
            // changed, and copied whole into the window's buffer: the window's
            // buffers keep nothing reliable between frames on Adreno through
            // libhybris (a partial redraw into them left garbage, step 6), ours
            // keeps everything. Drawing the whole scene cost about 4.5 ms of
            // GPU from rest; the copy costs a fraction of it.
            let age = if self.canvas_ready { 1 } else { 0 };
            // Night light changed: the frame goes out again, warmed or not.
            let night_now = night;
            let tb = hybris_hwc::now_ns();
            let cb = cpu_ns();
            let mut target = self.renderer.bind(&mut canvas.buffer).expect("bind the canvas");
            let tbound = hybris_hwc::now_ns();
            let cbound = cpu_ns();
            let result = canvas
                .tracker
                .render_output(&mut self.renderer, &mut target, age, &elements, [0.08, 0.1, 0.14, 1.0])
                .expect("render_output");
            self.canvas_ready = true;
            let changed_night = night_now != self.night_was;
            self.night_was = night_now;
            let damaged: i64 = result.damage.map(|rects| rects.iter().map(|r| r.size.w as i64 * r.size.h as i64).sum()).unwrap_or(0);
            // hwcomposer's buffers still need a frame each after a start or a
            // power-on, changed or not.
            if damaged == 0 && primed && self.shot.is_none() && !changed_night {
                return FrameCost { draw_ns: hybris_hwc::now_ns() - t0, elements_ns, swap_ns: 0, age, damaged_px: 0, swapped: false };
            }
            // Screenshots come from our buffer, copied before the swap (a read
            // makes the context current without the window's surface, and
            // the swap would then fail).
            let size = self.output.current_mode().unwrap().size;
            let tr = hybris_hwc::now_ns();
            let cr = cpu_ns();
            let shot = Self::take_shot(&mut self.renderer, &mut self.shot, &mut self.frames_left, self.frames_drawn, size, &target, damaged);
            drop(target);
            let whole = smithay::utils::Rectangle::from_size((size.w, size.h).into());
            match (night, &self.night_program) {
                // Night light: the canvas drawn to the screen warmed.
                (Some(warm), Some(program)) => {
                    use smithay::backend::renderer::Frame;
                    let mut to = self.renderer.bind(&mut self.surface).expect("bind the window");
                    let mut frame = self.renderer.render(&mut to, size, Transform::Normal).expect("night light");
                    let src = smithay::utils::Rectangle::<f64, smithay::utils::Buffer>::from_size((size.w as f64, size.h as f64).into());
                    let uniforms = [smithay::backend::renderer::gles::Uniform::new("warm", (warm[0], warm[1], warm[2]))];
                    frame.render_texture_from_to(&canvas.buffer, src, whole, &[whole], &[whole], Transform::Normal, 1.0, Some(program), &uniforms).expect("night light");
                    let _ = frame.finish().expect("night light");
                }
                _ => {
                    let from = self.renderer.bind(&mut canvas.buffer).expect("bind the canvas");
                    let mut to = self.renderer.bind(&mut self.surface).expect("bind the window");
                    self.renderer
                        .blit(&from, &mut to, whole, whole, smithay::backend::renderer::TextureFilter::Nearest)
                        .expect("blit");
                }
            }
            if std::env::var_os("LOG_TIMES").is_some() {
                let ms = |a: u64, b: u64| b.saturating_sub(a) as f64 / 1e6;
                let now = hybris_hwc::now_ns();
                let cnow = cpu_ns();
                tracing::info!(
                    "times: at {:.6} n {} elements {:.2} (cpu {:.2}) bind {:.2} ({:.2}) render {:.2} ({:.2}) blit {:.2} ({:.2}) ms, {} px",
                    t0 as f64 / 1e9, elements.len(), ms(t0, tb), ms(c0, cb), ms(tb, tbound), ms(cb, cbound), ms(tbound, tr), ms(cbound, cr), ms(tr, now), ms(cr, cnow), damaged
                );
            }
            (damaged.max(1), age, shot)
        } else {
            // CANVAS=0: every frame drawn whole into the window's buffer, as
            // before step 22. The damage tracker only says whether anything
            // changed.
            if primed && self.shot.is_none() {
                let changed = self.damage_tracker.damage_output(1, &elements).map(|(d, _)| d.is_some()).unwrap_or(true);
                if !changed {
                    return FrameCost { draw_ns: hybris_hwc::now_ns() - t0, elements_ns, swap_ns: 0, age: 0, damaged_px: 0, swapped: false };
                }
            }
            let size = self.output.current_mode().unwrap().size;
            let mut target = self.renderer.bind(&mut self.surface).expect("bind");
            let result = self
                .damage_tracker
                .render_output(&mut self.renderer, &mut target, 0, &elements, [0.08, 0.1, 0.14, 1.0])
                .expect("render_output");
            let damaged: i64 = result.damage.map(|rects| rects.iter().map(|r| r.size.w as i64 * r.size.h as i64).sum()).unwrap_or(0);
            let shot = Self::take_shot(&mut self.renderer, &mut self.shot, &mut self.frames_left, self.frames_drawn, size, &target, damaged);
            (damaged, 0, shot)
        };
        let t1 = hybris_hwc::now_ns();
        let swapped = damaged_px > 0;
        if swapped {
            let cs = cpu_ns();
            self.surface.swap_buffers(None).expect("swap_buffers");
            if std::env::var_os("LOG_TIMES").is_some() {
                tracing::info!("times: swap {:.2} (cpu {:.2}) ms", (hybris_hwc::now_ns() - t1) as f64 / 1e6, (cpu_ns() - cs) as f64 / 1e6);
            }
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

    /// The screenshot or frame run asked for, copied out of `target`; read
    /// after the swap.
    #[allow(clippy::too_many_arguments)]
    fn take_shot(renderer: &mut GlesRenderer, shot: &mut Option<String>, frames_left: &mut u32, frames_drawn: u64, size: smithay::utils::Size<i32, smithay::utils::Physical>, target: &smithay::backend::renderer::gles::GlesTarget<'_>, damaged: i64) -> Option<(String, smithay::backend::renderer::gles::GlesMapping, smithay::utils::Size<i32, smithay::utils::Physical>)> {
        // FRAMES_EVERY=n: a run keeps every n-th frame, to span a slow move.
        let every = std::env::var("FRAMES_EVERY").ok().and_then(|n| n.parse::<u64>().ok()).unwrap_or(1).max(1);
        let run = damaged > 0 && *frames_left > 0 && shot.is_none() && frames_drawn % every == 0;
        if run {
            *frames_left -= 1;
            *shot = Some(format!("/tmp/item-frame-{:03}-{}.rgba", frames_drawn, hybris_hwc::now_ns() / 1_000_000 % 100_000));
        }
        let path = shot.take()?;
        // GL's rows are bottom-up: the first rows are the screen's bottom.
        let size = if run { (size.w, FRAME_ROWS).into() } else { size };
        let region = smithay::utils::Rectangle::from_size((size.w, size.h).into());
        match renderer.copy_framebuffer(target, region, Fourcc::Abgr8888) {
            Ok(mapping) => Some((path, mapping, size)),
            Err(e) => {
                tracing::warn!("screenshot: {e}");
                None
            }
        }
    }

    /// A wallpaper decoded (walls.rs) to the GPU, fading in over the one
    /// before; Aurora (no rows) drawn by wall.frag.
    pub fn set_wallpaper(&mut self, picture: crate::walls::Picture) {
        use smithay::backend::renderer::ImportMem;
        let (name, rgba, w, h) = picture;
        let t = hybris_hwc::now_ns();
        let texture = if rgba.is_empty() {
            self.aurora()
        } else {
            self.renderer.import_memory(&rgba, Fourcc::Abgr8888, (w as i32, h as i32).into(), false).map_err(|e| tracing::warn!("wallpaper: {e}")).ok()
        };
        let Some(texture) = texture else { return };
        tracing::info!("wallpaper: {name} to the GPU in {:.1} ms", (hybris_hwc::now_ns() - t) as f64 / 1e6);
        if let Some(old) = self.wall.replace(texture) {
            self.wall_old = Some((old, hybris_hwc::now_ns(), smithay::backend::renderer::element::Id::new()));
        }
        self.wall_at = (smithay::backend::renderer::element::Id::new(), self.wall_at.1);
    }

    /// Whether a wallpaper is still fading in.
    pub fn wall_fading(&self, now_ns: u64) -> bool {
        self.wall_old.as_ref().is_some_and(|(_, at, _)| now_ns < at + WALL_FADE_NS + 20_000_000)
    }

    /// Aurora: wall.frag (wallpaper.glsl) drawn once into a texture.
    fn aurora(&mut self) -> Option<smithay::backend::renderer::gles::GlesTexture> {
        use smithay::backend::renderer::gles::{Uniform, UniformName, UniformType};
        let h = crate::layout::LAYOUT.1;
        let full_w = wall_width();
        let source = include_str!("wall.frag").replace("//_WALLPAPER_", include_str!("wallpaper.glsl"));
        let program = match self.renderer.compile_custom_pixel_shader(&source, &[UniformName::new("shift", UniformType::_1f)]) {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!("the wallpaper's shader: {e}");
                return None;
            }
        };
        let area = smithay::utils::Rectangle::<i32, smithay::utils::Logical>::from_size((full_w, h).into());
        // The texture's left edge is WALL_LEFT left of the screen's.
        let e = smithay::backend::renderer::gles::element::PixelShaderElement::new(program, area, Some(vec![area]), 1.0, vec![Uniform::new("shift", -(crate::state::WALL_LEFT as f32))], smithay::backend::renderer::element::Kind::Unspecified);
        let size = smithay::utils::Size::<i32, smithay::utils::Buffer>::from((full_w * SCALE, h * SCALE));
        let mut texture: smithay::backend::renderer::gles::GlesTexture = self.renderer.create_buffer(Fourcc::Abgr8888, size).ok()?;
        let mut tracker = OutputDamageTracker::new((size.w, size.h), SCALE as f64, Transform::Normal);
        {
            let mut target = self.renderer.bind(&mut texture).ok()?;
            tracker.render_output(&mut self.renderer, &mut target, 0, &[FrameElement::Pixel(e)], [0.0, 0.0, 0.0, 1.0]).ok()?;
        }
        Some(texture)
    }

    /// The wallpaper at `shift`: its texture seen through a window the
    /// screen's size, moved by the shift; the one before under it while the
    /// new one fades in.
    fn wallpaper(&mut self, shift: f32, frame_ns: u64) -> Vec<smithay::backend::renderer::element::texture::TextureRenderElement<smithay::backend::renderer::gles::GlesTexture>> {
        use smithay::backend::renderer::element::texture::TextureRenderElement;
        let (w, h) = crate::layout::LAYOUT;
        // Moved: a new id, so the frame takes it all again.
        if self.wall_at.1 != shift {
            self.wall_at = (smithay::backend::renderer::element::Id::new(), shift);
        }
        let src = smithay::utils::Rectangle::<f64, smithay::utils::Logical>::new(((crate::state::WALL_LEFT as f32 + shift) as f64, 0.0).into(), (w as f64, h as f64).into());
        let opaque = smithay::utils::Rectangle::<i32, smithay::utils::Buffer>::from_size((wall_width() * SCALE, h * SCALE).into());
        let context = self.renderer.context_id();
        let element = |id: smithay::backend::renderer::element::Id, texture: smithay::backend::renderer::gles::GlesTexture, alpha: Option<f32>| {
            TextureRenderElement::from_static_texture(id, context.clone(), (0.0, 0.0), texture, SCALE, Transform::Normal, alpha, Some(src), Some((w, h).into()), alpha.is_none().then(|| vec![opaque]), smithay::backend::renderer::element::Kind::Unspecified)
        };
        let mut out = Vec::new();
        let Some(texture) = self.wall.clone() else { return out };
        match self.wall_old.clone() {
            Some((old, at, old_id)) if frame_ns < at + WALL_FADE_NS => {
                let k = (frame_ns.saturating_sub(at) as f32 / WALL_FADE_NS as f32).clamp(0.0, 1.0);
                let k = k * k * (3.0 - 2.0 * k);
                // The new one, a new id each frame of the fade.
                out.push(element(smithay::backend::renderer::element::Id::new(), texture, Some(k.max(0.001))));
                out.push(element(old_id, old, None));
            }
            Some(_) => {
                self.wall_old = None;
                self.wall_at.0 = smithay::backend::renderer::element::Id::new();
                out.push(element(self.wall_at.0.clone(), texture, None));
            }
            None => out.push(element(self.wall_at.0.clone(), texture, None)),
        }
        out
    }

    /// The first setup's circle (orb.frag).
    fn orb_element(&mut self, orb: &crate::setup::Orb) -> Option<FrameElement> {
        use smithay::backend::renderer::gles::{Uniform, UniformName, UniformType};
        if self.orb_program.is_none() {
            let names = [
                UniformName::new("centre", UniformType::_2f),
                UniformName::new("radius", UniformType::_1f),
                UniformName::new("thickness", UniformType::_1f),
                UniformName::new("progress", UniformType::_1f),
                UniformName::new("segments", UniformType::_1f),
                UniformName::new("base", UniformType::_4f),
                UniformName::new("accent", UniformType::_4f),
                UniformName::new("print", UniformType::_1f),
                UniformName::new("lit", UniformType::_1f),
                UniformName::new("flash", UniformType::_1f),
            ];
            match self.renderer.compile_custom_pixel_shader(include_str!("orb.frag"), &names) {
                Ok(p) => self.orb_program = Some(p),
                Err(e) => {
                    tracing::warn!("setup: the circle's shader: {e}");
                    return None;
                }
            }
        }
        // Drawn over a square round it alone, wherever it goes (across the
        // hinge too); smithay gives the shader the area's size in logical px.
        let reach = orb.r * 1.08 + 4.0;
        let (x0, y0) = ((orb.x - reach).floor(), (orb.y - reach).floor());
        let side = (2.0 * reach).ceil() as i32 + 2;
        let area = smithay::utils::Rectangle::<i32, smithay::utils::Logical>::new((x0 as i32, y0 as i32).into(), (side, side).into());
        let uniforms = vec![
            Uniform::new("centre", ((orb.x - x0) as f32, (orb.y - y0) as f32)),
            Uniform::new("radius", orb.r as f32),
            Uniform::new("thickness", orb.thickness as f32),
            Uniform::new("progress", orb.progress as f32),
            Uniform::new("segments", orb.segments as f32),
            Uniform::new("base", (orb.base[0], orb.base[1], orb.base[2], orb.base[3])),
            Uniform::new("accent", (orb.accent[0], orb.accent[1], orb.accent[2], orb.accent[3])),
            Uniform::new("print", orb.print as f32),
            Uniform::new("lit", orb.lit as f32),
            Uniform::new("flash", orb.flash as f32),
        ];
        let program = self.orb_program.clone()?;
        Some(FrameElement::Pixel(smithay::backend::renderer::gles::element::PixelShaderElement::new(program, area, None, 1.0, uniforms, smithay::backend::renderer::element::Kind::Unspecified)))
    }

    /// The lock screen, or the setup with its circle, as it stood at
    /// `frame_ns`, in a texture the size of the output.
    fn cover_picture(&mut self, state: &State, frame_ns: u64, setup: bool) -> Option<smithay::backend::renderer::gles::GlesTexture> {
        use smithay::backend::renderer::Offscreen;
        let size = self.output.current_mode()?.size;
        let mut elements: Vec<FrameElement> = Vec::new();
        if setup {
            if let Some(orb) = state.setup.orb_at_rest() {
                elements.extend(self.orb_element(&orb));
            }
            elements.extend(state.setup.picture(&mut self.renderer, frame_ns).into_iter().map(FrameElement::from));
        } else {
            elements.extend(state.lock.picture(&mut self.renderer, frame_ns).into_iter().map(FrameElement::from));
        }
        let mut texture: smithay::backend::renderer::gles::GlesTexture = self.renderer.create_buffer(Fourcc::Abgr8888, (size.w, size.h).into()).ok()?;
        let mut tracker = OutputDamageTracker::new((size.w, size.h), SCALE as f64, Transform::Normal);
        {
            let mut target = self.renderer.bind(&mut texture).ok()?;
            tracker.render_output(&mut self.renderer, &mut target, 0, &elements, [0.0, 0.0, 0.0, 1.0]).ok()?;
        }
        tracing::info!("lock: a picture for the doors");
        Some(texture)
    }

    /// The wave's shader, compiled once (at the warm-up, not on the first
    /// unlock: that cost a 100 ms frame).
    fn wave_program(&mut self) -> Option<smithay::backend::renderer::gles::GlesTexProgram> {
        use smithay::backend::renderer::gles::{UniformName, UniformType};
        if self.wave_program.is_none() {
            let names = [
                UniformName::new("size", UniformType::_2f),
                UniformName::new("centre", UniformType::_2f),
                UniformName::new("radius", UniformType::_1f),
                UniformName::new("edge", UniformType::_1f),
                UniformName::new("glow", UniformType::_1f),
            ];
            match self.renderer.compile_custom_texture_shader(include_str!("wave.frag"), &names) {
                Ok(p) => self.wave_program = Some(p),
                Err(e) => tracing::warn!("lock: the wave's shader: {e}"),
            }
        }
        self.wave_program.clone()
    }

    /// The lock screen's picture going in a wave from where it was touched.
    fn wave(&mut self, from: (f64, f64), frame_ns: u64, start: u64, style: &crate::door::Style, texture: smithay::backend::renderer::gles::GlesTexture) -> Vec<FrameElement> {
        use smithay::backend::renderer::element::texture::TextureRenderElement;
        use smithay::backend::renderer::gles::Uniform;
        let Some(program) = self.wave_program() else { return Vec::new() };
        let (w, h) = crate::layout::LAYOUT;
        let (cx, cy) = from;
        // To the farthest corner, and its edge past it.
        let far = [(0.0, 0.0), (w as f64, 0.0), (0.0, h as f64), (w as f64, h as f64)]
            .iter()
            .map(|(x, y)| ((x - cx).powi(2) + (y - cy).powi(2)).sqrt())
            .fold(0.0, f64::max);
        let edge = 90.0;
        // Quick from the touch, slowing as it spreads (ease out, cubic).
        let t = (frame_ns.saturating_sub(start) as f64 / style.ns as f64).clamp(0.0, 1.0);
        let k = 1.0 - (1.0 - t).powi(3);
        let radius = k * (far + 2.0 * edge);
        let glow = 0.35 * (1.0 - t * 0.6);
        if std::env::var_os("LOG_WAVE").is_some() {
            tracing::info!("wave: t {t:.3} radius {radius:.0} of {far:.0}, from {cx:.0},{cy:.0}, frame {} ms after start", frame_ns.saturating_sub(start) / 1_000_000);
        }
        let inner = TextureRenderElement::from_static_texture(
            self.wave_id.clone(),
            self.renderer.context_id(),
            (0.0, 0.0),
            texture,
            SCALE,
            Transform::Normal,
            None,
            None,
            Some((w, h).into()),
            None,
            smithay::backend::renderer::element::Kind::Unspecified,
        );
        let uniforms = vec![
            Uniform::new("size", (w as f32, h as f32)),
            Uniform::new("centre", (cx as f32, cy as f32)),
            Uniform::new("radius", radius as f32),
            Uniform::new("edge", edge as f32),
            Uniform::new("glow", glow as f32),
        ];
        vec![FrameElement::Shaded(smithay::backend::renderer::gles::element::TextureShaderElement::new(inner, program, uniforms))]
    }

    /// The turning halves as strips of `texture`, each under its shade.
    fn door_strips(&mut self, style: &crate::door::Style, k: f64, texture: smithay::backend::renderer::gles::GlesTexture) -> Vec<FrameElement> {
        use smithay::backend::renderer::element::solid::SolidColorRenderElement;
        use smithay::backend::renderer::element::texture::TextureRenderElement;
        use smithay::backend::renderer::utils::CommitCounter;
        let (w, h) = crate::layout::LAYOUT;
        let panels = crate::layout::panels();
        let middle = (panels[0].loc.x + panels[0].size.w + panels[1].loc.x) as f64 / 2.0;
        let strips = crate::door::strips(style, k, w as f64, middle, h as f64);
        while self.door_ids.len() < 2 * strips.len() {
            self.door_ids.push(smithay::backend::renderer::element::Id::new());
        }
        let context = self.renderer.context_id();
        let commit = CommitCounter::from((k * 10000.0) as usize);
        let mut out = Vec::with_capacity(2 * strips.len());
        for (i, st) in strips.iter().enumerate() {
            // Shared edges round alike: no seams between strips. Logical px,
            // the output's scale.
            let (x0, x1) = (st.x0.round() as i32, st.x1.round() as i32);
            let hh = st.height.round() as i32;
            if x1 <= x0 || hh <= 0 {
                continue;
            }
            let y0 = (h - hh) / 2;
            let rect = smithay::utils::Rectangle::<i32, smithay::utils::Physical>::new((x0 * SCALE, y0 * SCALE).into(), ((x1 - x0) * SCALE, hh * SCALE).into());
            if st.dim > 0.005 {
                out.push(FrameElement::Shell(ShellElement::Solid(SolidColorRenderElement::new(self.door_ids[2 * i + 1].clone(), rect, commit, [0.0, 0.0, 0.0, st.dim as f32], smithay::backend::renderer::element::Kind::Unspecified))));
            }
            let src = smithay::utils::Rectangle::<f64, smithay::utils::Logical>::new((st.src_x0, 0.0).into(), (st.src_x1 - st.src_x0, h as f64).into());
            out.push(FrameElement::Snapshot(TextureRenderElement::from_static_texture(
                self.door_ids[2 * i].clone(),
                context.clone(),
                ((x0 * SCALE) as f64, (y0 * SCALE) as f64),
                texture.clone(),
                SCALE,
                Transform::Normal,
                None,
                Some(src),
                Some((x1 - x0, hh).into()),
                None,
                smithay::backend::renderer::element::Kind::Unspecified,
            )));
        }
        out
    }

    /// Whether every buffer of the window has had a frame.
    pub fn primed(&self) -> bool {
        self.frames_drawn >= BUFFERS as u64
    }

    /// Forgets the last frames of windows gone and done closing.
    pub fn forget(&mut self, keep: &[smithay::reexports::wayland_server::backend::ObjectId]) {
        self.snapshots.retain(|id, _| keep.contains(id));
    }

    /// The system screen's page and the pen's sheet, sent to the GPU now,
    /// at rest: each a panel's size, sent in the frame that first shows it
    /// it took that frame some 8 ms, and the ribbon a vsync.
    pub fn warm_pages(&mut self, state: &State) {
        use smithay::backend::renderer::element::memory::MemoryRenderBufferRenderElement;
        for b in state.system.page_buffer().into_iter().chain(state.pen.canvas_buffer()) {
            let _ = MemoryRenderBufferRenderElement::from_buffer(&mut self.renderer, (0.0, 0.0), b, None, None, None, smithay::backend::renderer::element::Kind::Unspecified);
        }
    }

    /// Uploads the shell's textures before they are first needed.
    pub fn warm_up(&mut self, state: &State) {
        let t = hybris_hwc::now_ns();
        let _ = self.wave_program();

        if self.night_program.is_none() {
            match self.renderer.compile_custom_texture_shader(include_str!("night.frag"), &[smithay::backend::renderer::gles::UniformName::new("warm", smithay::backend::renderer::gles::UniformType::_3f)]) {
                Ok(p) => self.night_program = Some(p),
                Err(e) => tracing::warn!("night light's shader: {e}"),
            }
        }
        if self.glass.is_none() && self.canvas.is_some() {
            let size = self.output.current_mode().unwrap().size;
            self.glass = crate::glass::Glass::new(&mut self.renderer, size);
        }
        let _ = self.orb_element(&crate::setup::Orb { x: 0.0, y: 0.0, r: 1.0, thickness: 1.0, progress: 0.0, segments: 0.0, base: [0.0; 4], accent: [0.0; 4], print: 0.0, lit: 0.0, flash: 0.0 });
        let n = state.shade.warm_up(&mut self.renderer) + state.dock.warm_up(&mut self.renderer) + state.grid.warm_up(&mut self.renderer) + state.clock.warm_up(&mut self.renderer) + state.back.warm_up(&mut self.renderer) + state.lock.warm_up(&mut self.renderer) + state.pen.warm_up(&mut self.renderer) + state.setup.warm_up(&mut self.renderer);
        tracing::info!("warm-up: {n} textures in {:.1} ms", (hybris_hwc::now_ns() - t) as f64 / 1e6);
    }

    /// The presentation feedback the windows and layer surfaces asked for
    /// with the frame just drawn, to be answered when it is on screen.
    pub fn take_feedback(&self, state: &State) -> smithay::desktop::utils::OutputPresentationFeedback {
        use smithay::reexports::wayland_protocols::wp::presentation_time::server::wp_presentation_feedback::Kind;
        let mut feedback = smithay::desktop::utils::OutputPresentationFeedback::new(&self.output);
        let output = self.output.clone();
        for window in state.space.elements() {
            window.take_presentation_feedback(&mut feedback, |_, _| Some(output.clone()), |_, _| Kind::Vsync);
        }
        for layer in &state.layers.surfaces {
            smithay::desktop::utils::take_presentation_feedback_surface_tree(layer.wl_surface(), &mut feedback, |_, _| Some(output.clone()), |_, _| Kind::Vsync);
        }
        feedback
    }

    /// Frame callbacks to every window, after a frame went out, with the time
    /// it will be on screen.
    /// Returns whether any surface was waiting for its callback.
    pub fn send_frames(&self, state: &State, time: Duration, group: Group) -> bool {
        let now = hybris_hwc::now_ns();
        let mut any = false;
        let mut take = |surface: &smithay::reexports::wayland_server::protocol::wl_surface::WlSurface| {
            let quick = state.paces.quick(surface);
            let ours = match group {
                Group::All => true,
                Group::Quick => quick,
                Group::Slow => !quick,
            };
            if std::env::var_os("PACE_DEBUG").is_some() {
                tracing::info!("pace: {} quick {quick} ours {ours} waiting {}", surface.id(), crate::pace::Paces::waiting(surface));
            }
            if ours && crate::pace::Paces::waiting(surface) {
                state.paces.sent(surface, now);
                any = true;
            }
            ours
        };
        state.layers.send_frames(&self.output, time, &mut take);
        for window in state.space.elements() {
            if take(window.toplevel().unwrap().wl_surface()) {
                window.send_frame(&self.output, time, Some(Duration::ZERO), |_, _| Some(self.output.clone()));
            }
        }
        any
    }
}

/// The white of a colour temperature (K), as factors for red, green and blue
/// (Tanner Helland's fit to the black body's colours), for night light.
fn warm_white(kelvin: u32) -> [f32; 3] {
    let t = (kelvin.clamp(1700, 6500) as f64) / 100.0;
    let red = 1.0;
    let green = ((99.470_802_586_1 * t.ln() - 161.119_568_166_1) / 255.0).clamp(0.0, 1.0);
    let blue = if t <= 19.0 { 0.0 } else { ((138.517_731_223_1 * (t - 10.0).ln() - 305.044_792_730_7) / 255.0).clamp(0.0, 1.0) };
    [red, green as f32, blue as f32]
}

/// A wallpaper fading in over the one before.
const WALL_FADE_NS: u64 = 450_000_000;

/// The wallpaper's width, logical px: the screen's and the parallax's room.
fn wall_width() -> i32 {
    crate::layout::LAYOUT.0 + (crate::state::WALL_LEFT + crate::state::WALL_RIGHT) as i32
}
