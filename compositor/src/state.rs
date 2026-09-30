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
use smithay::wayland::input_method::{InputMethodHandler, InputMethodManagerState, PopupSurface as ImPopup};
use smithay::wayland::shell::wlr_layer::{Layer, LayerSurface as WlrLayerSurface, WlrLayerShellHandler, WlrLayerShellState};
use smithay::wayland::selection::wlr_data_control::{DataControlHandler, DataControlState};
use smithay::wayland::text_input::TextInputManagerState;
use smithay::wayland::virtual_keyboard::VirtualKeyboardManagerState;
use smithay::{
    delegate_compositor, delegate_data_device, delegate_input_method_manager, delegate_layer_shell, delegate_output,
    delegate_seat, delegate_shm, delegate_text_input_manager, delegate_virtual_keyboard_manager, delegate_xdg_shell,
};

use crate::layout;
use crate::boost::GpuBoost;
use crate::back::Back;
use crate::clock::Clock;
use crate::curtain::Curtain;
use crate::dock::Dock;
use crate::gesture::Gestures;
use crate::grid::Grid;
use crate::shade::Shade;

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
    layer_shell_state: WlrLayerShellState,
    _input_method_state: InputMethodManagerState,
    _text_input_state: TextInputManagerState,
    _virtual_keyboard_state: VirtualKeyboardManagerState,
    data_control_state: DataControlState,
    /// wp_presentation: clients told when their frame reached the screen.
    _presentation_state: smithay::wayland::presentation::PresentationState,
    pub layers: crate::layers::Layers,
    /// The keyboard's height taken off the windows of its panel, as applied.
    osk_applied: Option<(usize, i32)>,
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
    pub shade: Shade,
    pub notes: crate::notify::Notes,
    /// Every app's desktop entry, read once (apps.rs).
    pub entries: Vec<crate::apps::Entry>,
    /// A client committed a new buffer since the loop last looked.
    pub client_frame: bool,
    pub dock: Dock,
    pub boost: GpuBoost,
    pub curtain: Curtain,
    pub gestures: Gestures,
    /// Windows closing, by their surface, and since when (output.rs).
    pub closing: Vec<(smithay::reexports::wayland_server::backend::ObjectId, u64)>,
    pub grid: Grid,
    pub clock: Clock,
    pub back: Back,
    pub lock: crate::lock::Lock,
    pub system: crate::sysscreen::SystemScreen,
    /// The pen's tip is on the screen.
    pub pen_down: bool,
    /// The pen's sheet, right of the right panel (pensheet.rs).
    pub pen: crate::pensheet::PenSheet,
    /// The pen draws on the sheet: its stroke began there.
    pub pen_drawing: bool,
    /// Windows put away, and the panel each was on.
    pub put_away: Vec<(Window, usize)>,
    /// The panel the next window goes to, asked for by a launch from the
    /// dock, and when (CLOCK_MONOTONIC ns).
    pub launch_to: Option<(usize, u64)>,
    /// The socket clients are given.
    pub socket_name: std::ffi::OsString,
}

impl State {
    pub fn new(display_handle: DisplayHandle, loop_signal: LoopSignal, wake: smithay::reexports::calloop::ping::Ping) -> State {
        let dh = &display_handle;
        let mut seat_state = SeatState::new();
        let mut seat = seat_state.new_wl_seat(dh, "duo");
        seat.add_keyboard(Default::default(), 200, 25).expect("keyboard");
        seat.add_touch();
        crate::protocols::create_globals(dh);
        State {
            compositor_state: CompositorState::new::<State>(dh),
            xdg_shell_state: XdgShellState::new::<State>(dh),
            shm_state: ShmState::new::<State>(dh, vec![]),
            _output_manager_state: OutputManagerState::new_with_xdg_output::<State>(dh),
            data_device_state: DataDeviceState::new::<State>(dh),
            layer_shell_state: WlrLayerShellState::new::<State>(dh),
            _input_method_state: InputMethodManagerState::new::<State, _>(dh, |_| true),
            _text_input_state: TextInputManagerState::new::<State>(dh),
            _virtual_keyboard_state: VirtualKeyboardManagerState::new::<State, _>(dh, |_| true),
            data_control_state: DataControlState::new::<State, _>(dh, None, |_| true),
            _presentation_state: smithay::wayland::presentation::PresentationState::new::<State>(dh, libc::CLOCK_MONOTONIC as u32),
            layers: Default::default(),
            osk_applied: None,
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
            shade: Shade::new(wake.clone()),
            notes: crate::notify::Notes::new(wake.clone()),
            entries: crate::apps::all(),
            client_frame: false,
            dock: Dock::new(),
            boost: GpuBoost::new(),
            curtain: Curtain::new(),
            gestures: Gestures::default(),
            closing: Vec::new(),
            grid: Grid::new(),
            clock: Clock::new(),
            back: Back::new(),
            lock: crate::lock::Lock::new(wake.clone()),
            system: crate::sysscreen::SystemScreen::new(wake.clone()),
            pen_down: false,
            pen: crate::pensheet::PenSheet::new(),
            pen_drawing: false,
            put_away: Vec::new(),
            launch_to: None,
            socket_name: Default::default(),
        }
    }

    /// Runs a command as a client of ours: libhybris' Wayland EGL platform, not
    /// the compositor's hwcomposer one, and our desktop name. A login shell,
    /// so the client has the port's environment from /etc/profile.d as under
    /// phosh (WebKit without its DMA-BUF renderer, GTK on GLES). CLIENT_ENV
    /// adds "KEY=VALUE ..." for tests.
    pub fn spawn(&self, command: &str) {
        let child = std::process::Command::new("sh")
            .arg(if std::env::var_os("NO_LOGIN_SHELL").is_some() { "-c" } else { "-lc" })
            .arg(command)
            .env("WAYLAND_DISPLAY", &self.socket_name)
            .env("EGL_PLATFORM", "wayland")
            .env("XDG_CURRENT_DESKTOP", "item")
            .envs(std::env::var("CLIENT_ENV").ok().iter().flat_map(|e| {
                e.split_whitespace().filter_map(|kv| kv.split_once('=')).map(|(k, v)| (k.to_owned(), v.to_owned())).collect::<Vec<_>>()
            }))
            .spawn();
        match child {
            Ok(_) => tracing::info!("spawned: {command}"),
            Err(e) => tracing::warn!("could not spawn {command}: {e}"),
        }
    }

    /// The app ids of the apps with a window, shown or put away.
    pub fn running(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.space.elements().chain(self.put_away.iter().map(|(w, _)| w)).map(app_id).collect();
        ids.sort();
        ids.dedup();
        ids
    }

    /// Which panels a window has. A window on its way away no longer has
    /// its panel.
    pub fn panels_taken(&self) -> [bool; 2] {
        let panels = layout::panels();
        let mut taken = [false; 2];
        for window in self.space.elements().filter(|w| !self.gestures.leaving(w)) {
            if let Some(loc) = self.space.element_location(window) {
                if let Some(p) = panels.iter().position(|p| p.contains(loc)) {
                    taken[p] = true;
                }
            }
        }
        taken
    }

    /// The right shade's rows: the notifications, newest first, then the
    /// open windows, shown and put away, topmost first.
    pub fn shade_rows(&self) -> Vec<crate::quick::Row> {
        let notes = self.notes.list().into_iter().map(|n| crate::quick::Row {
            app_id: format!("note:{}", n.icon),
            name: if n.summary.is_empty() { n.app.clone() } else { n.summary.clone() },
            icon: (!n.icon.is_empty()).then_some(n.icon.clone()),
            detail: Some(n.body.clone()),
        });
        notes.chain(self.window_rows().into_iter().map(|(_, r)| r)).collect()
    }

    /// The open windows, shown and put away, topmost first, as rows.
    pub fn window_rows(&self) -> Vec<(Window, crate::quick::Row)> {
        let entries = &self.entries;
        self.space
            .elements()
            .rev()
            .chain(self.put_away.iter().map(|(w, _)| w))
            .map(|w| {
                let id = app_id(w);
                let entry = entries.iter().find(|e| e.ids.contains(&id));
                let name = entry.map(|e| e.name.clone()).unwrap_or_else(|| if id.is_empty() { "Окно".into() } else { id.clone() });
                (w.clone(), crate::quick::Row { app_id: id, name, icon: entry.and_then(|e| e.icon.clone()), detail: None })
            })
            .collect()
    }

    /// A tap on a shade: a window closed, Settings opened.
    pub fn shade_ask(&mut self, ask: crate::shade::ShadeAsk) {
        match ask {
            crate::shade::ShadeAsk::Close(i) | crate::shade::ShadeAsk::Row(i) if i < self.notes.list().len() => {
                let id = self.notes.list()[i].id;
                if matches!(ask, crate::shade::ShadeAsk::Row(_)) {
                    self.notes.invoke(id);
                } else {
                    self.notes.dismiss(id);
                }
            }
            crate::shade::ShadeAsk::Row(_) => {}
            crate::shade::ShadeAsk::Close(i) => {
                let i = i - self.notes.list().len();
                if let Some((window, _)) = self.window_rows().into_iter().nth(i) {
                    tracing::info!("shade: closing {}", app_id(&window));
                    window.toplevel().unwrap().send_close();
                }
            }
            crate::shade::ShadeAsk::Settings(panel) => {
                if let Some(e) = crate::apps::entry("org.sfduo.Settings.desktop").or_else(|| crate::apps::entry("org.gnome.Settings.desktop")) {
                    let icon = e.icon.as_deref().and_then(|n| crate::apps::icon(n, crate::curtain::ICON));
                    self.launch(&e.exec, &e.ids, icon, panel);
                }
            }
        }
        self.needs_redraw = true;
    }

    /// The window on top on a panel.
    pub fn top_window(&self, panel: usize) -> Option<Window> {
        let rect = layout::panels()[panel];
        self.space
            .elements()
            .rev()
            .find(|w| self.space.element_location(w).is_some_and(|loc| rect.contains(loc)))
            .cloned()
    }

    /// Takes a window out of the space into the windows put away.
    pub fn put_window_away(&mut self, window: Window, panel: usize) {
        tracing::info!("put away: {} from the {} panel", app_id(&window), if panel == 0 { "left" } else { "right" });
        self.space.unmap_elem(&window);
        self.put_away.push((window.clone(), panel));
        // The keyboard goes to what is left on top.
        let keyboard = self.seat.get_keyboard().unwrap();
        if keyboard.current_focus().is_some_and(|f| &f == window.toplevel().unwrap().wl_surface()) {
            let next = self.top_window(panel).or_else(|| self.top_window(1 - panel));
            let surface = next.map(|w| w.toplevel().unwrap().wl_surface().clone());
            keyboard.set_focus(self, surface, smithay::utils::SERIAL_COUNTER.next_serial());
        }
        self.needs_redraw = true;
    }

    /// An app onto a panel, from the dock or the grid: a window of it put
    /// away comes back; else it is launched under the curtain.
    pub fn launch(&mut self, exec: &str, ids: &[String], icon: Option<smithay::backend::renderer::element::memory::MemoryRenderBuffer>, panel: usize) {
        if self.bring_back(ids, panel) || self.call_over(ids, panel) {
            return;
        }
        let now = hybris_hwc::now_ns();
        self.launch_to = Some((panel, now));
        self.curtain.raise(panel, icon, now);
        self.spawn(exec);
        self.needs_redraw = true;
    }

    /// An app with a window open, called from the dock or the grid on a
    /// panel: its window comes to that panel, sliding across, and takes the
    /// focus (item's "an open app, called to the other panel"). Returns
    /// whether it had one.
    pub fn call_over(&mut self, ids: &[String], panel: usize) -> bool {
        let Some(window) = self.space.elements().rev().find(|w| ids.iter().any(|id| id == &app_id(w))).cloned() else {
            return false;
        };
        let rect = layout::panels()[panel];
        let from = self.space.element_location(&window).unwrap_or(rect.loc);
        if from != rect.loc {
            window.toplevel().unwrap().with_pending_state(|s| s.size = Some(rect.size));
            window.toplevel().unwrap().send_pending_configure();
            self.gestures.slide(window.clone(), from.x, rect.loc.x, hybris_hwc::now_ns());
        }
        self.space.map_element(window.clone(), rect.loc, true);
        let surface = window.toplevel().unwrap().wl_surface().clone();
        self.seat.get_keyboard().unwrap().set_focus(self, Some(surface), smithay::utils::SERIAL_COUNTER.next_serial());
        tracing::info!("called over: {} onto the {} panel", app_id(&window), if panel == 0 { "left" } else { "right" });
        self.needs_redraw = true;
        true
    }

    /// Back on a panel: its window gets the focus and Alt+Left (back.rs).
    pub fn go_back(&mut self, panel: usize) {
        let Some(window) = self.top_window(panel) else { return };
        let id = app_id(&window);
        if !crate::back::wants_back(&id) {
            tracing::info!("back: not for {id}");
            return;
        }
        let surface = window.toplevel().unwrap().wl_surface().clone();
        let keyboard = self.seat.get_keyboard().unwrap();
        keyboard.set_focus(self, Some(surface), smithay::utils::SERIAL_COUNTER.next_serial());
        // Alt+Left: evdev's KEY_LEFTALT (56) and KEY_LEFT (105), +8 for xkb.
        let time = (hybris_hwc::now_ns() / 1_000_000) as u32;
        use smithay::backend::input::KeyState;
        use smithay::input::keyboard::{FilterResult, Keycode};
        for (key, state) in [(64u32, KeyState::Pressed), (113, KeyState::Pressed), (113, KeyState::Released), (64, KeyState::Released)] {
            let serial = smithay::utils::SERIAL_COUNTER.next_serial();
            keyboard.input::<(), _>(self, Keycode::new(key), state, serial, time, |_, _, _| FilterResult::Forward);
        }
        tracing::info!("back: Alt+Left to {id}");
    }

    /// A window put away of one of these apps, if there is one, back onto
    /// `panel`. Returns whether one came back.
    pub fn bring_back(&mut self, ids: &[String], panel: usize) -> bool {
        let Some(i) = self.put_away.iter().position(|(w, _)| ids.iter().any(|id| id == &app_id(w))) else {
            return false;
        };
        let (window, _) = self.put_away.remove(i);
        let rect = layout::panels()[panel];
        window.toplevel().unwrap().with_pending_state(|s| s.size = Some(rect.size));
        window.toplevel().unwrap().send_pending_configure();
        self.space.map_element(window.clone(), rect.loc, true);
        self.gestures.bring_back(window.clone(), panel, hybris_hwc::now_ns());
        let surface = window.toplevel().unwrap().wl_surface().clone();
        self.seat.get_keyboard().unwrap().set_focus(self, Some(surface), smithay::utils::SERIAL_COUNTER.next_serial());
        tracing::info!("brought back: {} onto the {} panel", app_id(&window), if panel == 0 { "left" } else { "right" });
        self.needs_redraw = true;
        true
    }

    /// The windows' height on the keyboard's panel follows the keyboard:
    /// shorter by its height while it is up, whole again after.
    pub fn follow_keyboard(&mut self) {
        let now = if std::env::var_os("NO_OSK_RESIZE").is_some() { None } else { self.layers.keyboard_height() };
        if now == self.osk_applied {
            return;
        }
        let panels = layout::panels();
        for (p, rect) in panels.iter().enumerate() {
            let cut = match now {
                Some((kp, h)) if kp == p => h,
                _ => 0,
            };
            let was = match self.osk_applied {
                Some((ap, h)) if ap == p => h,
                _ => 0,
            };
            if cut == was {
                continue;
            }
            for window in self.space.elements().filter(|w| self.space.element_location(w).is_some_and(|l| rect.contains(l))) {
                let t = window.toplevel().unwrap();
                t.with_pending_state(|s| s.size = Some((rect.size.w, rect.size.h - cut).into()));
                t.send_pending_configure();
            }
        }
        tracing::info!("keyboard: {}", match now { Some((p, h)) => format!("up on the {} panel, {h} px", if p == 0 { "left" } else { "right" }), None => "down".into() });
        self.osk_applied = now;
        self.needs_redraw = true;
    }

    /// The surface under a point of the layout, and where that surface starts.
    pub fn surface_under(&self, pos: Point<f64, Logical>) -> Option<(WlSurface, Point<f64, Logical>)> {
        if let Some(hit) = self.layers.surface_under(pos) {
            return Some(hit);
        }
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

/// A window's app id, as its client set it ("" before it does).
pub fn app_id(window: &Window) -> String {
    with_states(window.toplevel().unwrap().wl_surface(), |states| {
        states.data_map.get::<XdgToplevelSurfaceData>().and_then(|d| d.lock().unwrap().app_id.clone())
    })
    .unwrap_or_default()
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
        self.layers.commit(surface);
        self.needs_redraw = true;
        self.commits += 1;
        self.client_frame = true;
        // The first commit after a touch is taken for the client's answer to it,
        // if it comes within half a second.
        if let Some(touched) = self.touch_pending.take() {
            if hybris_hwc::now_ns() - touched < 500_000_000 {
                self.touch_answered.get_or_insert(touched);
            }
        }
        let kind = with_renderer_surface_state(surface, |rs| rs.buffer().map(|b| format!("{:?}", buffer_type(b))))
            .flatten();
        // The launched app's window drew: the curtain over it lifts.
        if kind.is_some() && self.curtain.drawn(surface, hybris_hwc::now_ns()) {
            self.needs_redraw = true;
        }
        if let Some(kind) = kind {
            let id = format!("{:?}", surface.id());
            if self.buffer_kinds.insert(id.clone(), kind.clone()).as_ref() != Some(&kind) {
                let app = self.space.elements().find(|w| w.toplevel().unwrap().wl_surface() == surface).map(app_id);
                tracing::info!("surface {id}: buffer {kind}{}", app.map(|a| format!(", app id {a}")).unwrap_or_default());
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
        // The panel a launch from the dock asked for, if it was lately;
        // else the next in turn.
        let asked = self.launch_to.take().filter(|&(_, at)| hybris_hwc::now_ns() - at < 10_000_000_000).map(|(p, _)| p);
        let panel = match asked {
            Some(p) => panels[p],
            None => {
                self.next_panel += 1;
                panels[(self.next_panel - 1) % panels.len()]
            }
        };
        surface.with_pending_state(|state| {
            state.size = Some(panel.size);
            state.states.set(xdg_toplevel::State::Maximized);
            state.states.set(xdg_toplevel::State::Activated);
        });
        if asked.is_some() {
            let p = panels.iter().position(|r| *r == panel).unwrap_or(0);
            self.curtain.adopt(p, surface.wl_surface());
        }
        let window = Window::new_wayland_window(surface);
        self.space.map_element(window.clone(), panel.loc, true);
        self.needs_redraw = true;
        tracing::info!(
            "window mapped on the {} panel, {:.1} s after start",
            if panel.loc.x == 0 { "left" } else { "right" },
            self.started.elapsed().as_secs_f64()
        );
    }

    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        // The space drops the window at its next refresh; the dock may come
        // back onto its panel. Its last frame is seen closing.
        self.closing.push((surface.wl_surface().id(), hybris_hwc::now_ns()));
        self.needs_redraw = true;
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
        // The keyboard goes to the panel of the window that takes the focus.
        if let Some(f) = focused {
            let panels = layout::panels();
            if let Some(p) = self
                .space
                .elements()
                .find(|w| w.toplevel().unwrap().wl_surface() == f)
                .and_then(|w| self.space.element_location(w))
                .and_then(|l| panels.iter().position(|r| r.contains(l)))
            {
                self.layers.panel = p;
            }
        }
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

impl WlrLayerShellHandler for State {
    fn shell_state(&mut self) -> &mut WlrLayerShellState {
        &mut self.layer_shell_state
    }

    fn new_layer_surface(&mut self, surface: WlrLayerSurface, _output: Option<smithay::reexports::wayland_server::protocol::wl_output::WlOutput>, layer: Layer, namespace: String) {
        tracing::info!("layer surface: {namespace} on {layer:?}");
        self.layers.names.push((surface.wl_surface().clone(), namespace));
        self.layers.surfaces.push(surface);
    }

    fn layer_destroyed(&mut self, surface: WlrLayerSurface) {
        self.layers.surfaces.retain(|l| l != &surface);
        self.layers.names.retain(|(s, _)| s != surface.wl_surface());
        self.needs_redraw = true;
    }
}

impl InputMethodHandler for State {
    fn new_popup(&mut self, _surface: ImPopup) {}
    fn dismiss_popup(&mut self, _surface: ImPopup) {}
    fn popup_repositioned(&mut self, _surface: ImPopup) {}
    fn parent_geometry(&self, parent: &WlSurface) -> smithay::utils::Rectangle<i32, Logical> {
        self.space
            .elements()
            .find(|w| w.toplevel().unwrap().wl_surface() == parent)
            .and_then(|w| self.space.element_geometry(w))
            .unwrap_or_default()
    }
}

impl DataControlHandler for State {
    fn data_control_state(&self) -> &DataControlState {
        &self.data_control_state
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
delegate_layer_shell!(State);
delegate_input_method_manager!(State);
delegate_text_input_manager!(State);
delegate_virtual_keyboard_manager!(State);
smithay::delegate_data_control!(State);
smithay::delegate_presentation!(State);
