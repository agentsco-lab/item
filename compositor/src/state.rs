//! The compositor's state and its Wayland protocol handlers: compositor,
//! xdg-shell, shm, seat, output and data-device (smithay's smallvil, less
//! pointer grabs).

use std::collections::HashMap;

use smithay::backend::renderer::buffer_type;
use smithay::backend::renderer::utils::{on_commit_buffer_handler, with_renderer_surface_state};
use smithay::desktop::{PopupKind, PopupManager, Space, Window, WindowSurfaceType};
use smithay::input::{Seat, SeatHandler, SeatState};
use smithay::reexports::calloop::LoopSignal;
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
use smithay::reexports::wayland_server::backend::{ClientData, ClientId, DisconnectReason};
use smithay::reexports::wayland_server::protocol::{wl_buffer, wl_seat, wl_surface::WlSurface};
use smithay::reexports::wayland_server::{Client, DisplayHandle, Resource};
use smithay::utils::{Logical, Point, Serial};
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
use smithay::{
    delegate_compositor, delegate_data_device, delegate_output, delegate_seat, delegate_shm, delegate_xdg_shell,
};

use crate::layout;

pub struct State {
    pub display_handle: DisplayHandle,
    pub loop_signal: LoopSignal,
    pub space: Space<Window>,
    pub popups: PopupManager,
    pub seat: Seat<State>,
    compositor_state: CompositorState,
    xdg_shell_state: XdgShellState,
    shm_state: ShmState,
    _output_manager_state: OutputManagerState,
    seat_state: SeatState<State>,
    data_device_state: DataDeviceState,
    /// The panel the next window opens on.
    next_panel: usize,
    /// Each surface's kind of buffer (shm or EGL), logged when it changes.
    buffer_kinds: HashMap<String, String>,
    pub touches: u32,
    pub commits: u32,
    started: std::time::Instant,
    /// Something changed since the last frame: draw at the next vsync.
    pub needs_redraw: bool,
    /// A touch no client has answered yet (CLOCK_MONOTONIC ns).
    pub touch_pending: Option<u64>,
    /// A touch a client has answered with a commit: the next frame shows it.
    pub touch_answered: Option<u64>,
    /// Stop after this many seconds (for tests).
    pub stop_after: Option<u64>,
}

impl State {
    pub fn new(display_handle: DisplayHandle, loop_signal: LoopSignal) -> State {
        let dh = &display_handle;
        let mut seat_state = SeatState::new();
        let mut seat = seat_state.new_wl_seat(dh, "duo");
        seat.add_keyboard(Default::default(), 200, 25).expect("keyboard");
        seat.add_touch();
        State {
            compositor_state: CompositorState::new::<State>(dh),
            xdg_shell_state: XdgShellState::new::<State>(dh),
            shm_state: ShmState::new::<State>(dh, vec![]),
            _output_manager_state: OutputManagerState::new_with_xdg_output::<State>(dh),
            data_device_state: DataDeviceState::new::<State>(dh),
            seat_state,
            seat,
            display_handle,
            loop_signal,
            space: Space::default(),
            popups: PopupManager::default(),
            next_panel: 0,
            buffer_kinds: HashMap::new(),
            touches: 0,
            commits: 0,
            started: std::time::Instant::now(),
            needs_redraw: true,
            touch_pending: None,
            touch_answered: None,
            stop_after: None,
        }
    }

    /// The surface under a point of the layout, and where that surface starts.
    pub fn surface_under(&self, pos: Point<f64, Logical>) -> Option<(WlSurface, Point<f64, Logical>)> {
        self.space.element_under(pos).and_then(|(window, location)| {
            window
                .surface_under(pos - location.to_f64(), WindowSurfaceType::ALL)
                .map(|(s, p)| (s, (p + location).to_f64()))
        })
    }
}

#[derive(Default)]
pub struct ClientState {
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
        self.needs_redraw = true;
        self.commits += 1;
        // The first commit after a touch is taken for the client's answer to it,
        // if it comes within half a second.
        if let Some(touched) = self.touch_pending.take() {
            if hybris_hwc::now_ns() - touched < 500_000_000 {
                self.touch_answered.get_or_insert(touched);
            }
        }
        let kind = with_renderer_surface_state(surface, |rs| rs.buffer().map(|b| format!("{:?}", buffer_type(b))))
            .flatten();
        if let Some(kind) = kind {
            let id = format!("{:?}", surface.id());
            if self.buffer_kinds.insert(id.clone(), kind.clone()).as_ref() != Some(&kind) {
                tracing::info!("surface {id}: buffer {kind}");
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
        // Each new window takes the next panel, maximized to it, as item tiles
        // a window to the panel it was launched from.
        let panels = layout::panels();
        let panel = panels[self.next_panel % panels.len()];
        self.next_panel += 1;
        surface.with_pending_state(|state| {
            state.size = Some(panel.size);
            state.states.set(xdg_toplevel::State::Maximized);
            state.states.set(xdg_toplevel::State::Activated);
        });
        let window = Window::new_wayland_window(surface);
        self.space.map_element(window, panel.loc, true);
        self.needs_redraw = true;
        tracing::info!(
            "window mapped on the {} panel, {:.1} s after start",
            if panel.loc.x == 0 { "left" } else { "right" },
            self.started.elapsed().as_secs_f64()
        );
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
