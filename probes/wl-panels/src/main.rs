//! Probe C, stage 2: Wayland clients drawn through hwcomposer.
//!
//! A minimal smithay compositor (the protocol handlers of smithay's smallvil,
//! less pointer grabs) whose output is hwcomposer's display 0 through
//! `hybris_hwc` and `smithay_hybris`. One output of 2784x1800 at scale 2: the
//! left panel is logical x 0-675, the hinge 675-717, the right panel 717-1392.
//! Toplevels go to the panels in turn, each maximized to its panel, as item
//! tiles a window to the panel it was launched from.
//!
//! The renderer is bound to the Wayland display (`bind_wl_display`), so GL
//! clients' buffers - libhybris' `android_wlegl` - import as EGL images; shm
//! clients' buffers upload as textures.
//!
//! `wl-panels SECONDS [CLIENT...]`: run for SECONDS, spawning each CLIENT
//! with WAYLAND_DISPLAY set. Once a second: frames, time between presents,
//! and windows.

use std::ffi::OsString;
use std::sync::Arc;
use std::time::Duration;

use hybris_hwc::{now_ns, take_stats, Output as HwcOutput};
use smithay::backend::egl::context::{GlAttributes, PixelFormatRequirements};
use smithay::backend::egl::{EGLContext, EGLDisplay, EGLSurface};
use smithay::backend::renderer::damage::OutputDamageTracker;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::input::{AbsolutePositionEvent, Event, InputEvent, TouchEvent};
use smithay::backend::libinput::LibinputInputBackend;
use smithay::backend::renderer::buffer_type;
use smithay::backend::renderer::utils::{on_commit_buffer_handler, with_renderer_surface_state};
use smithay::backend::renderer::{Bind, ImportEgl};
use smithay::desktop::{PopupKind, PopupManager, Space, Window, WindowSurfaceType};
use smithay::input::{Seat, SeatHandler, SeatState};
use smithay::output::{Mode, Output, PhysicalProperties, Scale, Subpixel};
use smithay::reexports::calloop::generic::Generic;
use smithay::reexports::calloop::{EventLoop, Interest, LoopSignal, PostAction};
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
use smithay::reexports::wayland_server::backend::{ClientData, ClientId, DisconnectReason};
use smithay::reexports::wayland_server::protocol::{wl_buffer, wl_seat, wl_surface::WlSurface};
use smithay::reexports::wayland_server::{Client, Display, DisplayHandle, Resource};
use smithay::input::touch::{DownEvent, MotionEvent, UpEvent};
use smithay::reexports::input::{Libinput, LibinputInterface};
use smithay::utils::{Logical, Point, Serial, Transform, SERIAL_COUNTER};
use smithay::wayland::buffer::BufferHandler;
use smithay::wayland::compositor::{
    get_parent, is_sync_subsurface, with_states, CompositorClientState, CompositorHandler, CompositorState,
};
use smithay::wayland::output::{OutputHandler, OutputManagerState};
use smithay::wayland::selection::data_device::{
    set_data_device_focus, ClientDndGrabHandler, DataDeviceHandler, DataDeviceState, ServerDndGrabHandler,
};
use smithay::wayland::selection::SelectionHandler;
use smithay::wayland::shell::xdg::{
    PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState, XdgToplevelSurfaceData,
};
use smithay::wayland::shm::{ShmHandler, ShmState};
use smithay::wayland::socket::ListeningSocketSource;
use smithay::{
    delegate_compositor, delegate_data_device, delegate_output, delegate_seat, delegate_shm, delegate_xdg_shell,
};
use smithay_hybris::{HwcWindow, HybrisDisplay};

/// The panels in logical coordinates at scale 2: left, then right past the hinge.
const PANELS: [(i32, i32); 2] = [(0, 675), (717, 675)];
const PANEL_HEIGHT: i32 = 900;

struct State {
    display_handle: DisplayHandle,
    socket_name: OsString,
    loop_signal: LoopSignal,
    space: Space<Window>,
    popups: PopupManager,
    compositor_state: CompositorState,
    xdg_shell_state: XdgShellState,
    shm_state: ShmState,
    _output_manager_state: OutputManagerState,
    seat_state: SeatState<State>,
    data_device_state: DataDeviceState,
    next_panel: usize,
    buffer_kinds: std::collections::HashMap<String, String>,
    seat: Seat<State>,
    touches: u32,
    input_events: u32,
}

#[derive(Default)]
struct ClientState {
    compositor_state: CompositorClientState,
}

impl ClientData for ClientState {
    fn initialized(&self, _: ClientId) {}
    fn disconnected(&self, _: ClientId, _: DisconnectReason) {}
}

impl CompositorHandler for State {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor_state
    }

    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        &client.get_data::<ClientState>().unwrap().compositor_state
    }

    fn commit(&mut self, surface: &WlSurface) {
        on_commit_buffer_handler::<Self>(surface);
        // Which kind of buffer each surface brings: shm (drawn by the CPU) or
        // EGL (a GL client's, over libhybris' android_wlegl); logged on change.
        let kind = with_renderer_surface_state(surface, |rs| rs.buffer().map(|b| format!("{:?}", buffer_type(b))))
            .flatten();
        if let Some(kind) = kind {
            // Protocol ids repeat across clients; the object id does not.
            let id = format!("{:?}", surface.id());
            if self.buffer_kinds.insert(id.clone(), kind.clone()).as_ref() != Some(&kind) {
                println!("surface {id}: buffer {kind}");
            }
        }
        if !is_sync_subsurface(surface) {
            let mut root = surface.clone();
            while let Some(parent) = get_parent(&root) {
                root = parent;
            }
            if let Some(window) = self.space.elements().find(|w| w.toplevel().unwrap().wl_surface() == &root) {
                window.on_commit();
            }
        }

        // A toplevel's first commit gets its first configure.
        if let Some(window) = self.space.elements().find(|w| w.toplevel().unwrap().wl_surface() == surface).cloned() {
            let sent = with_states(surface, |states| {
                states.data_map.get::<XdgToplevelSurfaceData>().unwrap().lock().unwrap().initial_configure_sent
            });
            if !sent {
                window.toplevel().unwrap().send_configure();
            }
        }
        self.popups.commit(surface);
        if let Some(PopupKind::Xdg(ref xdg)) = self.popups.find_popup(surface) {
            if !xdg.is_initial_configure_sent() {
                xdg.send_configure().expect("initial configure failed");
            }
        }
    }
}

impl BufferHandler for State {
    fn buffer_destroyed(&mut self, _: &wl_buffer::WlBuffer) {}
}

impl ShmHandler for State {
    fn shm_state(&self) -> &ShmState {
        &self.shm_state
    }
}

impl XdgShellHandler for State {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        // Each new window takes the next panel, maximized to it.
        let (x, w) = PANELS[self.next_panel % PANELS.len()];
        self.next_panel += 1;
        surface.with_pending_state(|state| {
            state.size = Some((w, PANEL_HEIGHT).into());
            state.states.set(xdg_toplevel::State::Maximized);
            state.states.set(xdg_toplevel::State::Activated);
        });
        let window = Window::new_wayland_window(surface);
        self.space.map_element(window, (x, 0), true);
        println!("window mapped on the {} panel", if x == 0 { "left" } else { "right" });
    }

    fn new_popup(&mut self, surface: PopupSurface, _: PositionerState) {
        let _ = self.popups.track_popup(PopupKind::Xdg(surface));
    }

    fn reposition_request(&mut self, surface: PopupSurface, positioner: PositionerState, token: u32) {
        surface.with_pending_state(|state| {
            state.geometry = positioner.get_geometry();
            state.positioner = positioner;
        });
        surface.send_repositioned(token);
    }

    fn grab(&mut self, _: PopupSurface, _: wl_seat::WlSeat, _: Serial) {}
}

impl SeatHandler for State {
    type KeyboardFocus = WlSurface;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<State> {
        &mut self.seat_state
    }

    fn focus_changed(&mut self, seat: &Seat<Self>, focused: Option<&WlSurface>) {
        let client = focused.and_then(|s| self.display_handle.get_client(s.id()).ok());
        set_data_device_focus(&self.display_handle, seat, client);
    }
}

impl SelectionHandler for State {
    type SelectionUserData = ();
}

impl DataDeviceHandler for State {
    fn data_device_state(&self) -> &DataDeviceState {
        &self.data_device_state
    }
}

impl ClientDndGrabHandler for State {}
impl ServerDndGrabHandler for State {}
impl OutputHandler for State {}

delegate_compositor!(State);
delegate_shm!(State);
delegate_xdg_shell!(State);
delegate_seat!(State);
delegate_data_device!(State);
delegate_output!(State);

/// libinput's path interface: the probe runs as root and opens the device itself.
struct OpenDirect;

impl LibinputInterface for OpenDirect {
    fn open_restricted(&mut self, path: &std::path::Path, flags: i32) -> Result<std::os::fd::OwnedFd, i32> {
        use std::os::unix::fs::OpenOptionsExt;
        std::fs::OpenOptions::new()
            .read(true)
            .write(flags & 3 != 0)
            .custom_flags(flags)
            .open(path)
            .map(std::os::fd::OwnedFd::from)
            .map_err(|e| e.raw_os_error().unwrap_or(-1))
    }

    fn close_restricted(&mut self, fd: std::os::fd::OwnedFd) {
        drop(fd);
    }
}

/// The layout the touchscreen covers: the whole output with the hinge, in
/// logical px (2784x1800 at scale 2). The touchscreen spans both panels and
/// the hinge, as the output does, so it maps one to one.
const LAYOUT: (i32, i32) = (1392, 900);

impl State {
    fn surface_under(&self, pos: Point<f64, Logical>) -> Option<(WlSurface, Point<f64, Logical>)> {
        self.space.element_under(pos).and_then(|(window, location)| {
            window
                .surface_under(pos - location.to_f64(), WindowSurfaceType::ALL)
                .map(|(s, p)| (s, (p + location).to_f64()))
        })
    }

    fn on_input(&mut self, event: InputEvent<LibinputInputBackend>) {
        if self.input_events < 20 {
            let name = format!("{event:?}");
            println!("input event: {}", name.split(|c| c == ' ' || c == '{').next().unwrap_or(&name));
        }
        self.input_events += 1;
        let Some(touch) = self.seat.get_touch() else { return };
        match event {
            InputEvent::TouchDown { event } => {
                let pos = event.position_transformed(LAYOUT.into());
                let serial = SERIAL_COUNTER.next_serial();
                // The window under the finger comes up and takes the keyboard.
                if let Some(window) = self.space.element_under(pos).map(|(w, _)| w.clone()) {
                    self.space.raise_element(&window, true);
                    let surface = window.toplevel().unwrap().wl_surface().clone();
                    self.seat.get_keyboard().unwrap().set_focus(self, Some(surface), serial);
                }
                let under = self.surface_under(pos);
                self.touches += 1;
                println!("touch down {:?} at {:.0},{:.0} -> {}", event.slot(), pos.x, pos.y,
                         if under.is_some() { "a window" } else { "nothing" });
                touch.down(self, under, &DownEvent { slot: event.slot(), location: pos, serial, time: event.time_msec() });
            }
            InputEvent::TouchMotion { event } => {
                let pos = event.position_transformed(LAYOUT.into());
                let under = self.surface_under(pos);
                touch.motion(self, under, &MotionEvent { slot: event.slot(), location: pos, time: event.time_msec() });
            }
            InputEvent::TouchUp { event } => {
                touch.up(self, &UpEvent { slot: event.slot(), serial: SERIAL_COUNTER.next_serial(), time: event.time_msec() });
            }
            InputEvent::TouchFrame { .. } => touch.frame(self),
            InputEvent::TouchCancel { .. } => touch.cancel(self),
            _ => {}
        }
    }
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .init();
    let mut args = std::env::args().skip(1);
    let seconds: u64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(30);
    let clients: Vec<String> = args.collect();

    // hwcomposer, EGL and the renderer, as in stage 1.
    let hwc = HwcOutput::open(3);
    let (width, height) = (hwc.width, hwc.height);
    let egl_display = unsafe { EGLDisplay::new(HybrisDisplay) }.expect("EGLDisplay");
    let attributes = GlAttributes { version: (3, 0), profile: None, debug: false, vsync: true };
    let context = EGLContext::new_with_config(&egl_display, attributes, PixelFormatRequirements::_8_bit())
        .expect("EGLContext");
    let pixel_format = context.pixel_format().expect("pixel format");
    let mut surface = unsafe { EGLSurface::new(&egl_display, pixel_format, context.config_id(), HwcWindow(hwc.window)) }
        .expect("EGLSurface");
    let mut renderer = unsafe { GlesRenderer::new(context) }.expect("GlesRenderer");

    // The Wayland side.
    let mut event_loop: EventLoop<State> = EventLoop::try_new().expect("event loop");
    let display: Display<State> = Display::new().expect("display");
    let dh = display.handle();
    match renderer.bind_wl_display(&dh) {
        Ok(()) => println!("EGL bound to the Wayland display: GL clients' buffers can import"),
        Err(e) => println!("bind_wl_display failed: {e}"),
    }

    let mut seat_state = SeatState::new();
    let mut seat = seat_state.new_wl_seat(&dh, "duo");
    seat.add_keyboard(Default::default(), 200, 25).expect("keyboard");
    seat.add_touch();

    let output = Output::new(
        "duo".into(),
        PhysicalProperties { size: (0, 0).into(), subpixel: Subpixel::Unknown, make: "Microsoft".into(), model: "Surface Duo".into() },
    );
    let _global = output.create_global::<State>(&dh);
    let mode = Mode { size: (width, height).into(), refresh: 60_000 };
    output.change_current_state(Some(mode), Some(Transform::Normal), Some(Scale::Integer(2)), Some((0, 0).into()));
    output.set_preferred(mode);

    let listening = ListeningSocketSource::new_auto().expect("socket");
    let socket_name = listening.socket_name().to_os_string();
    let handle = event_loop.handle();
    handle
        .insert_source(listening, |stream, _, state: &mut State| {
            state.display_handle.insert_client(stream, Arc::new(ClientState::default())).unwrap();
        })
        .expect("socket source");
    handle
        .insert_source(Generic::new(display, Interest::READ, smithay::reexports::calloop::Mode::Level), |_, display, state| {
            unsafe { display.get_mut().dispatch_clients(state).unwrap() };
            Ok(PostAction::Continue)
        })
        .expect("display source");

    // Touch: the Duo's one touchscreen, through libinput's path interface.
    let mut libinput = Libinput::new_from_path(OpenDirect);
    // The port's sfduo-pen-split grabs the raw digitizer (surface_touchscreen)
    // and gives the fingers their own device; take that when it is there.
    let touchscreen = std::env::var("TOUCHSCREEN").ok().or_else(|| {
        ["sfduo touchscreen", "surface_touchscreen"].iter().find_map(|want| {
            std::fs::read_dir("/sys/class/input").ok()?.flatten().find_map(|d| {
                let name = std::fs::read_to_string(d.path().join("device/name")).ok()?;
                let node = d.file_name().to_string_lossy().into_owned();
                (name.trim() == *want && node.starts_with("event")).then(|| format!("/dev/input/{node}"))
            })
        })
    }).unwrap_or_else(|| "/dev/input/event5".into());
    match libinput.path_add_device(&touchscreen) {
        Some(d) => println!("touch: {} ({touchscreen})", d.name()),
        None => println!("touch: could not add {touchscreen}"),
    }
    handle
        .insert_source(LibinputInputBackend::new(libinput), |event, _, state: &mut State| state.on_input(event))
        .expect("libinput source");

    let mut state = State {
        display_handle: dh.clone(),
        socket_name: socket_name.clone(),
        loop_signal: event_loop.get_signal(),
        space: Space::default(),
        popups: PopupManager::default(),
        compositor_state: CompositorState::new::<State>(&dh),
        xdg_shell_state: XdgShellState::new::<State>(&dh),
        shm_state: ShmState::new::<State>(&dh, vec![]),
        _output_manager_state: OutputManagerState::new_with_xdg_output::<State>(&dh),
        seat_state,
        data_device_state: DataDeviceState::new::<State>(&dh),
        seat,
        next_panel: 0,
        buffer_kinds: Default::default(),
        touches: 0,
        input_events: 0,
    };
    state.space.map_output(&output, (0, 0));
    println!("listening on {:?}", state.socket_name);

    for c in &clients {
        match std::process::Command::new("sh").arg("-c").arg(c).env("WAYLAND_DISPLAY", &socket_name).spawn() {
            Ok(_) => println!("spawned: {c}"),
            Err(e) => println!("could not spawn {c}: {e}"),
        }
    }

    // GL draws the EGL surface's default framebuffer bottom-up: render with a
    // vertical flip (smithay's Flipped180, as smallvil does for winit), while
    // clients see an untransformed output.
    let mut damage_tracker = OutputDamageTracker::new((width, height), 2.0, Transform::Flipped180);
    let start = now_ns();
    let mut next_report = start + 1_000_000_000;
    let begin = std::time::Instant::now();
    while (now_ns() - start) / 1_000_000_000 < seconds {
        event_loop.dispatch(Some(Duration::ZERO), &mut state).expect("dispatch");

        {
            let elements = smithay::desktop::space::space_render_elements::<_, Window, _>(
                &mut renderer, [&state.space], &output, 1.0,
            )
            .expect("render elements");
            let mut target = renderer.bind(&mut surface).expect("bind");
            damage_tracker
                .render_output(&mut renderer, &mut target, 0, &elements, [0.08, 0.1, 0.14, 1.0])
                .expect("render_output");
        }
        surface.swap_buffers(None).expect("swap_buffers");

        for window in state.space.elements() {
            window.send_frame(&output, begin.elapsed(), Some(Duration::ZERO), |_, _| Some(output.clone()));
        }
        state.space.refresh();
        state.popups.cleanup();
        let _ = state.display_handle.flush_clients();

        let now = now_ns();
        if now >= next_report {
            let st = take_stats();
            println!("{:5.1} s: {:3} fps  mean {:5.2} ms  max {:6.2} ms  over 20 ms {:2}  windows {}  touches {}  errors {}",
                     (now - start) as f64 / 1e9, st.frames, st.mean_ms, st.max_ms, st.over_20ms,
                     state.space.elements().count(), state.touches, st.errors);
            next_report += 1_000_000_000;
        }
    }
    state.loop_signal.stop();
    println!("done");
}
