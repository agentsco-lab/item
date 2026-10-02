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
    _idle_inhibit_state: smithay::wayland::idle_inhibit::IdleInhibitManagerState,
    /// Surfaces asking the screen to stay on (idle-inhibit), and the
    /// idleness they and gnome-session's inhibitors hold off (idle.rs).
    pub inhibitors: Vec<WlSurface>,
    pub idle: crate::idle::Idle,
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
    /// The lock as logind and the screen saver's listeners see it (logind.rs).
    pub logind: crate::logind::Logind,
    /// The fingerprint reader (fingerprint.rs).
    pub fingerprint: crate::fingerprint::Fingerprint,
    /// The battery and the network, for the lock screen (status.rs).
    pub status: crate::status::Status,
    /// gnome-keyring's prompts (keyring.rs).
    pub keyring: crate::keyring::Keyring,
    /// The first setup (setup.rs), and the finger on it: slot, where down, where now.
    pub setup: crate::setup::Setup,
    pub setup_touch: Option<(smithay::backend::input::TouchSlot, f64, f64, f64, f64)>,
    pub system: crate::sysscreen::SystemScreen,
    /// The pen's tip is on the screen.
    pub pen_down: bool,
    /// The pen's sheet, right of the right panel (pensheet.rs).
    pub pen: crate::pensheet::PenSheet,
    /// The pen draws on the sheet: its stroke began there.
    pub pen_drawing: bool,
    /// How quickly each client answers its frame callbacks (pace.rs).
    pub paces: crate::pace::Paces,
    /// Windows put away, and the panel each was on.
    pub put_away: Vec<(Window, usize)>,
    /// A touch in a panel's bottom band, until it shows which way it goes:
    /// up puts the window away, sideways moves the ribbon.
    pub bottom_start: Option<(smithay::backend::input::TouchSlot, Point<f64, Logical>)>,
    /// polkit's agent, and the system's own dialog it asks through.
    pub polkit: crate::polkit::Polkit,
    pub dialog: crate::dialog::Dialog,
    /// NetworkManager's secret agent; and who the dialog is up for.
    pub wifi: crate::nm::Wifi,
    /// Calls, from gnome-calls, shown over the lock screen.
    pub calls: crate::calls::Calls,
    /// An urgent notification to answer (an alarm), over everything.
    pub alert: crate::alert::Alert,
    /// The Duo folded shut: nothing lights the screen until it opens.
    pub lid_shut: bool,
    /// The privacy dot: camera, microphone, location in use.
    pub privacy: crate::privacy::Privacy,
    /// Who opens pages over whom (org.sfduo.Dock.Follow) and the windows
    /// asked to be shown again (org.sfduo.Phoc.Present).
    pub follow: crate::follow::Follow,
    /// The tour after the first setup.
    pub tour: crate::tour::Tour,
    /// org.gnome.Mutter.DisplayConfig's PowerSaveMode.
    pub display_config: crate::display::Display,
    /// What woke the phone from sleep.
    pub sleep: crate::sleep::Sleep,
    pub asker: Asker,
    /// When the power key went down, unlocked and lit (input.rs).
    pub power_down_at: Option<u64>,
    /// The wallpapers, and which one is on (walls.rs).
    pub walls: crate::walls::Walls,
    /// The desktop's editing: choosing the wallpaper (picker.rs).
    pub picker: crate::picker::Picker,
    /// The panel each window was last opened onto: the dock's half its
    /// app's icon goes to while it runs. (Its app id comes after it opens.)
    pub opened_on: Vec<(Window, usize)>,
    /// A window a finger carries from its bottom band: the finger, the
    /// window, the panel it was on, where the finger is (x).
    pub carrying: Option<(smithay::backend::input::TouchSlot, Window, usize, f64)>,
    /// Every page in one row, the panels a window onto it (ribbon.rs).
    pub ribbon: crate::ribbon::Ribbon,
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
            _idle_inhibit_state: smithay::wayland::idle_inhibit::IdleInhibitManagerState::new::<State>(dh),
            inhibitors: Vec::new(),
            idle: crate::idle::Idle::new(),
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
            logind: crate::logind::Logind::new(wake.clone()),
            fingerprint: crate::fingerprint::Fingerprint::new(wake.clone()),
            status: crate::status::Status::new(wake.clone()),
            keyring: crate::keyring::Keyring::new(wake.clone()),
            setup: crate::setup::Setup::new(wake.clone()),
            setup_touch: None,
            system: crate::sysscreen::SystemScreen::new(wake.clone()),
            pen_down: false,
            pen: crate::pensheet::PenSheet::new(),
            pen_drawing: false,
            paces: Default::default(),
            put_away: Vec::new(),
            ribbon: crate::ribbon::Ribbon::new(),
            power_down_at: None,
            polkit: crate::polkit::Polkit::new(wake.clone()),
            dialog: crate::dialog::Dialog::new(),
            wifi: crate::nm::Wifi::new(wake.clone()),
            calls: crate::calls::Calls::new(wake.clone()),
            alert: crate::alert::Alert::new(),
            lid_shut: false,
            privacy: crate::privacy::Privacy::new(wake.clone()),
            follow: crate::follow::Follow::new(wake.clone()),
            tour: crate::tour::Tour::new(),
            display_config: crate::display::Display::new(wake.clone()),
            sleep: crate::sleep::Sleep::new(wake.clone()),
            asker: Asker::Polkit,
            walls: crate::walls::Walls::new(wake.clone()),
            picker: crate::picker::Picker::new(),
            opened_on: Default::default(),
            carrying: None,
            bottom_start: None,
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
        let _ = self.spawn_child(command);
    }

    /// As `spawn`, keeping the child to wait for.
    pub fn spawn_child(&self, command: &str) -> Option<std::process::Child> {
        // A login shell for Droidian's profile (WebKit, GLES), which also sets
        // XDG_CURRENT_DESKTOP=Phosh:GNOME for everyone (zz-gnome-vars.sh):
        // the desktop is ours again after it. "item:GNOME" as phosh's
        // "Phosh:GNOME": GNOME's apps still see GNOME, and NotShowIn=item and
        // item's portal configuration apply.
        let command = format!("export XDG_CURRENT_DESKTOP=item:GNOME XDG_SESSION_DESKTOP=item; {command}");
        use std::os::unix::process::CommandExt;
        let mut command_line = std::process::Command::new("sh");
        // An app gets every core, not the loop's own (sched.rs).
        unsafe {
            command_line.pre_exec(|| {
                crate::sched::unpin();
                Ok(())
            });
        }
        let child = command_line
            .arg(if std::env::var_os("NO_LOGIN_SHELL").is_some() { "-c" } else { "-lc" })
            .arg(&command)
            .env("WAYLAND_DISPLAY", &self.socket_name)
            .env("EGL_PLATFORM", "wayland")
            .env("XDG_CURRENT_DESKTOP", "item:GNOME")
            .env("XDG_SESSION_DESKTOP", "item")
            .envs(std::env::var("CLIENT_ENV").ok().iter().flat_map(|e| {
                e.split_whitespace().filter_map(|kv| kv.split_once('=')).map(|(k, v)| (k.to_owned(), v.to_owned())).collect::<Vec<_>>()
            }))
            .spawn();
        match child {
            Ok(child) => {
                tracing::info!("spawned: {command}");
                Some(child)
            }
            Err(e) => {
                tracing::warn!("could not spawn {command}: {e}");
                None
            }
        }
    }

    /// The app ids of the apps with a window, shown or put away.
    pub fn running(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.windows().iter().map(app_id).collect();
        ids.sort();
        ids.dedup();
        ids
    }

    /// Which panels the dock may not stand on, for the ribbon at `view`:
    /// those whose page is not a desk, but for a window on its way away;
    /// and those under the launch curtain or the grid.
    pub fn dock_taken(&self, view: usize) -> [bool; 2] {
        // Choosing the wallpaper: the dock away, the wallpaper whole.
        if self.picker.is_open() {
            return [true, true];
        }
        let mut taken = self.pages_taken(view, None);
        if let Some(p) = self.grid.panel() {
            taken[p] = true;
        }
        taken
    }

    /// The panels whose pages are not desks at `view`, and those under the
    /// launch curtain; a window on its way away frees its panel, unless it
    /// is `but` (whose motion the dock follows itself).
    fn pages_taken(&self, view: usize, but: Option<&Window>) -> [bool; 2] {
        use crate::ribbon::Page;
        let pages = self.ribbon.at(view);
        let mut taken = [0, 1].map(|k| match pages[k] {
            Some(Page::Desk(_)) | None => false,
            Some(Page::App(w)) | Some(Page::Wide(w)) => Some(w) == but || !self.gestures.leaving(w),
            Some(_) => true,
        });
        if let Some(p) = self.curtain.panel() {
            taken[p] = true;
        }
        taken
    }

    /// Where the dock goes, and how: under a finger (the ribbon moving
    /// along, the grid coming up, a window going away) it follows the
    /// motion between the two ends; else it moves on its own to where it
    /// stands for the panels now.
    /// Where a carried window would go for a finger at `x`: across both
    /// panels (None) over the hinge, else onto the panel under it.
    pub fn carry_target(x: f64) -> Option<usize> {
        let panels = layout::panels();
        let hinge = (panels[0].loc.x + panels[0].size.w + panels[1].loc.x) as f64 / 2.0;
        if (x - hinge).abs() < 150.0 {
            None
        } else if x < hinge {
            Some(0)
        } else {
            Some(1)
        }
    }

    /// The carried window let go at `x`.
    pub fn carry_window_up(&mut self) {
        let Some((_, window, from, x)) = self.carrying.take() else { return };
        let spanned = self.ribbon.spanned(&window);
        match Self::carry_target(x) {
            None if !spanned => self.ribbon.span(&window),
            Some(p) if spanned => self.ribbon.unspan(&window, p),
            Some(p) if p != from => {
                self.ribbon.open(window.clone(), p);
                self.opened_onto(&window, p);
            }
            _ => {}
        }
        self.needs_redraw = true;
    }

    /// Pages over their leaders (follow.rs): a window asked to be shown
    /// again comes back, on its leader's panel; a follower's window not yet
    /// over its leader's page goes over it. Returns whether anything did.
    pub fn follow_pages(&mut self) -> bool {
        let mut changed = false;
        for id in self.follow.take_present() {
            let leader = self.follow.leader(&id);
            let panel = (0..2)
                .find(|&p| matches!(self.ribbon.on(p), Some(crate::ribbon::Page::App(w)) if leader.as_deref() == Some(app_id(w).as_str())))
                .unwrap_or(1);
            if self.bring_back(&[id.clone()], panel) {
                tracing::info!("follow: {id} presented");
                changed = true;
            }
        }
        let followers: Vec<(Window, Window)> = self
            .ribbon
            .pages
            .iter()
            .filter_map(|p| match p {
                crate::ribbon::Page::App(w) if !self.ribbon.covered.iter().any(|(f, _)| f == w) => {
                    let leader = self.follow.leader(&app_id(w))?;
                    let l = self.ribbon.pages.iter().find_map(|q| match q {
                        crate::ribbon::Page::App(lw) if lw != w && app_id(lw) == leader => Some(lw.clone()),
                        _ => None,
                    })?;
                    Some((w.clone(), l))
                }
                _ => None,
            })
            .collect();
        for (follower, leader) in followers {
            self.ribbon.cover(&leader, &follower);
            changed = true;
        }
        changed
    }

    /// A window opened onto a panel, for its app's icon in the dock.
    fn opened_onto(&mut self, window: &Window, panel: usize) {
        self.opened_on.retain(|(w, _)| w != window);
        self.opened_on.push((window.clone(), panel));
    }

    pub fn place_dock(&mut self, frame_ns: u64) {
        // The apps running, on the half of the panel each was opened onto
        // (the right one if not known).
        let windows = self.windows();
        self.opened_on.retain(|(w, _)| windows.contains(w));
        let mut running: Vec<(String, usize)> = windows
            .iter()
            .map(|w| {
                // A page of another app's (follow.rs) is that app running.
                let id = app_id(w);
                (self.follow.leader(&id).unwrap_or(id), self.opened_on.iter().find(|(o, _)| o == w).map(|(_, p)| *p).unwrap_or(1))
            })
            .collect();
        running.sort();
        running.dedup_by(|a, b| a.0 == b.0);
        self.dock.set_running(&running);
        let view = self.ribbon.view;
        if let Some((v, k)) = self.ribbon.scrolling(frame_ns) {
            let (from, to) = (self.dock_taken(v), self.dock_taken(v + 1));
            self.dock.scrub(from, to, k);
        } else if let Some((panel, k)) = self.grid.moving(frame_ns) {
            let from = self.pages_taken(view, None);
            let mut to = from;
            to[panel] = true;
            self.dock.scrub(from, to, k);
        } else if let Some((window, k)) = self.gestures.moving_window(frame_ns).filter(|(w, _)| self.ribbon.find(w).is_some()) {
            let from = self.pages_taken(view, Some(&window));
            let mut to = from;
            if let Some(panel) = (0..2).find(|&p| self.ribbon.on(p) == Some(&crate::ribbon::Page::App(window.clone()))) {
                to[panel] = false;
            }
            if let Some(p) = self.grid.panel() {
                to[p] = true;
            }
            self.dock.scrub(from, to, k);
        } else {
            let taken = self.dock_taken(view);
            // A move begins: the clocks up now, before its first frame.
            if self.dock.follow(taken, frame_ns) {
                self.boost.kick(hybris_hwc::now_ns());
            }
        }
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
        let windows = self.windows();
        let running: Vec<String> = windows.iter().map(app_id).collect();
        windows
            .iter()
            // A page of another app's (follow.rs) is that app's row.
            .filter(|w| self.follow.leader(&app_id(w)).is_none_or(|l| !running.contains(&l)))
            .map(|w| {
                let id = app_id(w);
                let entry = entries.iter().find(|e| e.ids.contains(&id));
                let name = entry.map(|e| e.name.clone()).unwrap_or_else(|| if id.is_empty() { "Window".into() } else { id.clone() });
                (w.clone(), crate::quick::Row { app_id: id, name, icon: entry.and_then(|e| e.icon.clone()), detail: None })
            })
            .collect()
    }

    /// Every window: those in the ribbon (shown first), then those put away.
    pub fn windows(&self) -> Vec<Window> {
        let mut out: Vec<Window> = self.space.elements().rev().cloned().collect();
        for page in &self.ribbon.pages {
            if let crate::ribbon::Page::App(w) = page {
                if !out.contains(w) {
                    out.push(w.clone());
                }
            }
        }
        for (_, w) in &self.ribbon.covered {
            if !out.contains(w) {
                out.push(w.clone());
            }
        }
        for (w, _) in &self.put_away {
            if !out.contains(w) {
                out.push(w.clone());
            }
        }
        out
    }

    /// Where the ribbon stopped, applied: the windows on the pages shown
    /// mapped onto their panels, the others out of the space; the system
    /// screen and the pen's sheet out if their pages are shown.
    pub fn apply_ribbon(&mut self) {
        use crate::ribbon::Page;
        self.ribbon.unsettled = false;
        let panels = layout::panels();
        let shown = [self.ribbon.on(0).cloned(), self.ribbon.on(1).cloned()];
        let windows: Vec<Window> = self.ribbon.pages.iter().filter_map(|p| if let Page::App(w) = p { Some(w.clone()) } else { None }).collect();
        for w in windows {
            // Across both panels: as wide as the two, standing where its
            // own page is (its Wide page shown alone, half of it shows).
            if self.ribbon.spanned(&w) {
                let own = shown.iter().position(|s| s.as_ref() == Some(&Page::App(w.clone())));
                let wide = shown.iter().position(|s| s.as_ref() == Some(&Page::Wide(w.clone())));
                let x = match (own, wide) {
                    (Some(k), _) => Some(panels[k].loc.x),
                    (None, Some(k)) => Some(panels[k].loc.x - crate::ribbon::PAGE as i32),
                    (None, None) => None,
                };
                match x {
                    Some(x) => {
                        let size = (layout::LAYOUT.0, layout::LAYOUT.1);
                        let at: smithay::utils::Point<i32, Logical> = (x, 0).into();
                        if self.space.element_location(&w) != Some(at) || w.geometry().size.w != size.0 {
                            w.toplevel().unwrap().with_pending_state(|s| s.size = Some(size.into()));
                            w.toplevel().unwrap().send_pending_configure();
                            self.space.map_element(w.clone(), at, false);
                        }
                    }
                    None => {
                        if self.space.elements().any(|e| e == &w) {
                            self.space.unmap_elem(&w);
                        }
                    }
                }
                continue;
            }
            match shown.iter().position(|s| s.as_ref() == Some(&Page::App(w.clone()))) {
                Some(k) => {
                    let rect = panels[k];
                    if self.space.element_location(&w) != Some(rect.loc) || w.geometry().size.w > rect.size.w {
                        let cut = match self.osk_applied {
                            Some((kp, h)) if kp == k => h,
                            _ => 0,
                        };
                        w.toplevel().unwrap().with_pending_state(|s| s.size = Some((rect.size.w, rect.size.h - cut).into()));
                        w.toplevel().unwrap().send_pending_configure();
                        self.space.map_element(w.clone(), rect.loc, false);
                    }
                }
                None => {
                    if self.space.elements().any(|e| e == &w) {
                        self.space.unmap_elem(&w);
                    }
                }
            }
        }
        self.system.show(shown[0] == Some(Page::System));
        self.pen.show(shown[1] == Some(Page::Pen));
        // The keyboard to a window shown: the one it had, else the left's.
        let keyboard = self.seat.get_keyboard().unwrap();
        let shown_surfaces: Vec<_> = self.space.elements().map(|w| w.toplevel().unwrap().wl_surface().clone()).collect();
        if !keyboard.current_focus().is_some_and(|f| shown_surfaces.contains(&f)) {
            let next = self.top_window(0).or_else(|| self.top_window(1)).map(|w| w.toplevel().unwrap().wl_surface().clone());
            keyboard.set_focus(self, next, smithay::utils::SERIAL_COUNTER.next_serial());
        }
        tracing::info!("ribbon: applied, {:?} | {:?}", shown[0], shown[1]);
        self.needs_redraw = true;
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
            crate::shade::ShadeAsk::Lock => {
                tracing::info!("shade: lock");
                self.shade.fold();
                self.lock.lock_now();
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
        // Its own, or one across both panels over it.
        self.space
            .elements()
            .rev()
            .find(|w| {
                self.space.element_location(w).is_some_and(|loc| {
                    let width = w.geometry().size.w.max(1);
                    loc.x < rect.loc.x + rect.size.w && loc.x + width > rect.loc.x
                })
            })
            .cloned()
    }

    /// Takes a window out of the space into the windows put away.
    pub fn put_window_away(&mut self, window: Window, panel: usize) {
        tracing::info!("put away: {} from the {} panel", app_id(&window), if panel == 0 { "left" } else { "right" });
        self.space.unmap_elem(&window);
        self.ribbon.remove(&window);
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
        self.spawn_app(exec, ids.first().map(String::as_str).unwrap_or("app"));
        self.needs_redraw = true;
    }

    /// An app started in a systemd scope of its own, as GNOME's shell does
    /// (`app-gnome-ID-RANDOM.scope`; here `app-item-…`): xdg-desktop-portal
    /// knows an app by its scope, and an app in the compositor's own unit
    /// was refused the portals (GTK's settings among them). If the scope
    /// cannot be made (no user manager: found once, at the first launch's
    /// start, SCOPES), the app starts as before.
    pub fn spawn_app(&self, exec: &str, id: &str) {
        if !SCOPES.load(std::sync::atomic::Ordering::Relaxed) {
            self.spawn(exec);
            return;
        }
        // systemd's escaping for a unit name: '-' separates the parts.
        let id: String = id.chars().map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '_' { c.to_string() } else { format!("\\x{:02x}", c as u32) }).collect();
        let unit = format!("app-item-{id}-{}.scope", hybris_hwc::now_ns() % 1_000_000_000);
        let quoted = exec.replace('\'', "'\\''");
        self.spawn(&format!("exec systemd-run --user --scope --collect --quiet --slice=app.slice --unit='{unit}' -- sh -c '{quoted}'"));
    }

    /// An app with a window open, called from the dock or the grid on a
    /// panel: its window comes to that panel, sliding across, and takes the
    /// focus (item's "an open app, called to the other panel"). Returns
    /// whether it had one.
    pub fn call_over(&mut self, ids: &[String], panel: usize) -> bool {
        let in_row = self.ribbon.pages.iter().rev().find_map(|p| match p {
            crate::ribbon::Page::App(w) if ids.iter().any(|id| id == &app_id(w)) => Some(w.clone()),
            _ => None,
        });
        // An app under a page of its own (follow.rs): the page is called.
        let in_row = in_row.or_else(|| self.ribbon.covered.iter().find(|(_, l)| ids.iter().any(|id| id == &app_id(l))).map(|(f, _)| f.clone()));
        let Some(window) = in_row else {
            return false;
        };
        // Its page to the panel asked for, the row running there.
        if self.ribbon.on(panel) != Some(&crate::ribbon::Page::App(window.clone())) {
            self.ribbon.open(window.clone(), panel);
            self.opened_onto(&window, panel);
        }
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
        self.ribbon.open(window.clone(), panel);
        self.opened_onto(&window, panel);
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
/// The wallpaper's shift for the ribbon at `position` (pages): it moves a
/// little with the row, behind it (wallpaper.glsl).
pub fn wallpaper_shift(position: f64) -> f64 {
    ((position - 1.0) * crate::ribbon::PAGE * 0.12).clamp(-WALL_LEFT, WALL_RIGHT)
}

/// How far the wallpaper may move each way (its texture is that much wider
/// than the screen, output.rs).
pub const WALL_LEFT: f64 = 60.0;
pub const WALL_RIGHT: f64 = 140.0;

/// Who the system's dialog is up for.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Asker {
    Polkit,
    Wifi,
}

/// Whether apps can be started in scopes of their own (State::spawn_app):
/// the user's systemd answers; found at the start, on a thread.
pub static SCOPES: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

/// Finds out whether the user's systemd makes scopes.
pub fn check_scopes() {
    std::thread::spawn(|| {
        let ok = std::process::Command::new("systemd-run").args(["--user", "--scope", "--collect", "--quiet", "true"]).status().map(|s| s.success()).unwrap_or(false);
        SCOPES.store(ok, std::sync::atomic::Ordering::Relaxed);
        tracing::info!("apps: {}", if ok { "each in a scope of its own" } else { "no scopes (no user manager): started directly" });
    });
}

impl State {
    /// Whether the screen may go dark when idle: no living window holds it
    /// on, nor an inhibitor of gnome-session's.
    pub fn idle_held(&mut self) -> bool {
        use smithay::reexports::wayland_server::Resource;
        self.inhibitors.retain(|s| s.is_alive());
        !self.inhibitors.is_empty() || self.idle.session_inhibited()
    }
}

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
        self.paces.committed(surface, hybris_hwc::now_ns());
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
        // The pages move for it: the clocks up before they do.
        self.boost.kick(hybris_hwc::now_ns());
        let p = panels.iter().position(|r| *r == panel).unwrap_or(0);
        self.ribbon.open(window.clone(), p);
        self.opened_onto(&window, p);
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
        self.boost.kick(hybris_hwc::now_ns());
        // Its page leaves the row.
        let page = self.ribbon.pages.iter().find_map(|p| match p {
            crate::ribbon::Page::App(w) if w.toplevel().unwrap().wl_surface() == surface.wl_surface() => Some(w.clone()),
            _ => None,
        });
        if let Some(w) = page {
            self.ribbon.remove(&w);
        }
        self.put_away.retain(|(w, _)| w.toplevel().unwrap().wl_surface() != surface.wl_surface());
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

impl smithay::wayland::idle_inhibit::IdleInhibitHandler for State {
    fn inhibit(&mut self, surface: WlSurface) {
        tracing::info!("idle: held off by a window");
        if !self.inhibitors.contains(&surface) {
            self.inhibitors.push(surface);
        }
    }

    fn uninhibit(&mut self, surface: WlSurface) {
        tracing::info!("idle: let go by a window");
        self.inhibitors.retain(|s| s != &surface);
    }
}
smithay::delegate_idle_inhibit!(State);
smithay::delegate_presentation!(State);
