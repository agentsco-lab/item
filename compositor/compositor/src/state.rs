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
    delegate_seat, delegate_shm, delegate_virtual_keyboard_manager, delegate_xdg_activation, delegate_xdg_shell,
};
use smithay::wayland::xdg_activation::{XdgActivationHandler, XdgActivationState, XdgActivationToken, XdgActivationTokenData};

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
    /// xdg-activation: an app presenting a window of its own that is put
    /// away (warm since boot, or put away by a swipe) brings it back.
    xdg_activation_state: XdgActivationState,
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
    /// The frame turned: every window shown is sized to its panel again.
    pub relayout: bool,
    /// The touches in flight cancelled once for the turn's animation (input.rs).
    pub turn_cancelled: bool,
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
    /// The frame's turn as the dock's turn button asked (layout.rs's
    /// values: 0 the book, 1 the right panel up, 2 the left), applied by
    /// main.rs; with the turn by the sensors (prefs.rs) unused.
    pub turn_asked: u8,
    pub turn_by_sensors: bool,
    /// Whether gravity and the gyroscope stream now: the turn the
    /// sensors', or the first setup up (its drop leans with gravity).
    pub sensors_streaming: bool,
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
    /// The keyboard of item's own (kbd.rs), and a launch of the Agent entry
    /// waiting for its window to put it under (ns).
    pub kbd: crate::kbd::Keyboard,
    pub kbd_wanted: Option<u64>,
    /// The keyboard came up by itself for a text field (text_field), not
    /// by a hand: it goes by itself when the field is left.
    kbd_auto: bool,
    /// A text-input enable (true) or disable waiting for its commit
    /// (protocols.rs).
    pub field_pending: Option<bool>,
    /// When the text field was left: the keyboard goes half a second
    /// later unless another field takes the cursor.
    field_left_at: Option<u64>,
    /// The last touch that landed on a window (input.rs): a field raises
    /// the keyboard only soon after one, not when a page focuses a field
    /// by itself on loading.
    pub window_touched_at: u64,
    /// The surface whose text inputs were told `enter` (focus_changed):
    /// smithay tells them only with an input method bound, and item has
    /// none in item mode.
    text_entered: Option<WlSurface>,
    /// The keyboard's preferences from Settings (prefs.rs).
    pub prefs: crate::prefs::Prefs,
    /// The window the keyboard came up under: it goes along when that one
    /// is moved to the other panel.
    pub kbd_window: Option<Window>,
    /// A key the keyboard sent (the finger's lift, ns), and one a client
    /// answered with a commit: the next frame shows it (main.rs's report).
    pub key_sent: Option<u64>,
    pub key_answered: Option<u64>,
    /// The pen draws on the sheet: its stroke began there.
    pub pen_drawing: bool,
    /// How quickly each client answers its frame callbacks (pace.rs).
    pub paces: crate::pace::Paces,
    /// Windows put away, and the panel each was on.
    pub put_away: Vec<(Window, usize)>,
    /// The dock's apps kept warm (draft 49): launched after boot, their
    /// windows put away before a frame shows them. By app id: since when,
    /// and when its first window came (its others for a while after are
    /// warm too: a first-run dialog, a second toplevel).
    warming: HashMap<String, Warming>,
    /// Windows warmed and put away that were never opened: not rows of the
    /// shade (the owner did not open them); one brought back is the shade's.
    warm_unopened: Vec<Window>,
    /// Every app id warmed this boot: a late window of one of them (its
    /// launch forgotten, a dialog after its first window) is still put away.
    warm_known: std::collections::HashSet<String>,
    /// Apps to spawn from the tick: the resident Chromium's thread asks for
    /// a plain launch when the resident could not be reached (exec, id).
    spawn_later: std::sync::Arc<std::sync::Mutex<Vec<(String, String)>>>,
    /// The loop woken from a thread.
    wake: smithay::reexports::calloop::ping::Ping,
    /// Windows that came while a warm launch was pending, not yet shown nor
    /// put away: told apart by app id at their first buffer.
    warm_pending: Vec<(Window, usize)>,
    /// The desktop files still to warm, and when the next may start.
    warm_queue: Vec<String>,
    warm_next_ns: u64,
    warm_begun: bool,
    /// How many windows have been put away or closed (the tour's hint for
    /// apps goes when one is).
    pub windows_gone: u64,
    /// A touch in a panel's bottom band, until it shows which way it goes:
    /// up puts the window away, sideways moves the ribbon.
    pub bottom_start: Option<(smithay::backend::input::TouchSlot, Point<f64, Logical>)>,
    /// The frame turned: what the deck (the lower panel) shows - the system
    /// screen or the pen's sheet, swiped in from a side; None is black.
    pub deck: Option<crate::ribbon::Page>,
    /// A finger down on the deck: its slot, where it started (x, y), where it is.
    pub deck_swipe: Option<(smithay::backend::input::TouchSlot, f64, f64, f64, f64)>,
    /// The deck's page on its way in or out: which side it lives at (1 the
    /// right, -1 the left), since when, whether it is leaving.
    pub deck_run: Option<(f64, u64, bool)>,
    /// The open windows as covers on the deck (flow.rs).
    pub flow: crate::flow::Flow,
    /// When the screen last had a window or the curtain (the dock stays on the deck a moment after).
    pub deck_wanted_ns: u64,
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
    /// The lid opened on the way into sleep: the screen lit once awake.
    pub lid_light_pending: bool,
    /// A peek through the camera on the lock screen (camera.rs, an
    /// experiment).
    pub camera_peek: crate::camera::Peek,
    /// CV ID (face.rs).
    pub face: crate::face::Face,
    /// Folded back to back from the lock screen, a camera (foldcam.rs).
    pub foldcam: crate::foldcam::FoldCam,
    /// The pen's first move on the sheet not yet shown: (libinput's time, us;
    /// when it reached the compositor, ns).
    pub pen_moved: Option<(u64, u64)>,
    /// The stroke's pen->screen times so far (ms), for its percentiles.
    pub pen_latencies: Vec<f64>,
    /// The hinge's angle (posture.rs).
    pub posture: crate::posture::Posture,
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
    /// The wallpapers, and which one is on (walls.rs).
    pub walls: crate::walls::Walls,
    /// Wallpapers from Unsplash, with a key (unsplash.rs): after the set's
    /// own in the picker's strip.
    pub gallery: crate::unsplash::Gallery,
    /// The batch's picture being fetched (its place in the batch): ringed
    /// there once on, the sheet not moved.
    pub picked_from_batch: Option<usize>,
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
        // US and Russian: the keyboard of item's own types Cyrillic in the
        // second group (typing.rs).
        seat.add_keyboard(smithay::input::keyboard::XkbConfig { layout: "us,ru", ..Default::default() }, 200, 25).expect("keyboard");
        seat.add_touch();
        crate::protocols::create_globals(dh);
        let face = crate::face::Face::new(wake.clone());
        State {
            compositor_state: CompositorState::new::<State>(dh),
            xdg_shell_state: XdgShellState::new::<State>(dh),
            xdg_activation_state: XdgActivationState::new::<State>(dh),
            shm_state: ShmState::new::<State>(dh, vec![]),
            _output_manager_state: OutputManagerState::new_with_xdg_output::<State>(dh),
            data_device_state: DataDeviceState::new::<State>(dh),
            layer_shell_state: WlrLayerShellState::new::<State>(dh),
            // item's own keyboard (kbd.rs): no client gets the input method,
            // so stevia, which the session starts (a required component of
            // it), never comes up over a text field; with stevia chosen it
            // works as phosh's.
            _input_method_state: InputMethodManagerState::new::<State, _>(dh, |_| crate::kbd::choice() == crate::kbd::Choice::Stevia),
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
            relayout: false,
            deck: None,
            deck_swipe: None,
            deck_run: None,
            flow: Default::default(),
            deck_wanted_ns: 0,
            turn_cancelled: false,
            touch_pending: None,
            touch_answered: None,
            stop_after: None,
            shade: Shade::new(wake.clone()),
            notes: crate::notify::Notes::new(wake.clone()),
            entries: crate::apps::all(),
            client_frame: false,
            dock: Dock::new(),
            turn_asked: 0,
            turn_by_sensors: crate::prefs::turn_by_sensors(),
            sensors_streaming: crate::prefs::turn_by_sensors(),
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
            kbd: crate::kbd::Keyboard::new(wake.clone()),
            kbd_wanted: None,
            kbd_auto: false,
            field_pending: None,
            field_left_at: None,
            window_touched_at: 0,
            text_entered: None,
            prefs: crate::prefs::Prefs::load(),
            kbd_window: None,
            key_sent: None,
            key_answered: None,
            pen_drawing: false,
            paces: Default::default(),
            put_away: Vec::new(),
            warming: HashMap::new(),
            warm_unopened: Vec::new(),
            warm_known: Default::default(),
            spawn_later: Default::default(),
            wake: wake.clone(),
            warm_pending: Vec::new(),
            warm_queue: Vec::new(),
            warm_next_ns: 0,
            warm_begun: false,
            windows_gone: 0,
            ribbon: crate::ribbon::Ribbon::new(),
            polkit: crate::polkit::Polkit::new(wake.clone()),
            dialog: crate::dialog::Dialog::new(),
            wifi: crate::nm::Wifi::new(wake.clone()),
            calls: crate::calls::Calls::new(wake.clone()),
            alert: crate::alert::Alert::new(),
            lid_shut: false,
            lid_light_pending: false,
            posture: crate::posture::Posture::new(wake.clone()),
            camera_peek: crate::camera::Peek::new(wake.clone()),
            face,
            foldcam: crate::foldcam::FoldCam::new(wake.clone()),
            pen_moved: None,
            pen_latencies: Vec::new(),
            privacy: crate::privacy::Privacy::new(wake.clone()),
            follow: crate::follow::Follow::new(wake.clone()),
            tour: crate::tour::Tour::new(),
            display_config: crate::display::Display::new(wake.clone()),
            sleep: crate::sleep::Sleep::new(wake.clone()),
            asker: Asker::Polkit,
            walls: crate::walls::Walls::new(wake.clone()),
            gallery: crate::unsplash::Gallery::new(wake.clone()),
            picked_from_batch: None,
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
        // SAFETY: the hook runs in the forked child before exec and only makes sched syscalls (unpin):
        // no allocation, no lock, nothing async-signal-unsafe.
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
                // The command's first words only: a line may carry a URL or a path of the owner's.
                tracing::info!("spawned: {}", command.split_whitespace().take(3).collect::<Vec<_>>().join(" "));
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
        // The frame turned (layout.rs): the lower panel is the deck, the
        // dock's halves at its corners, the window above untouched.
        if crate::layout::turned() {
            // The dock is the screen's taskbar, or on the deck with a window
            // up (dock.rs's targets); a deck page under a window leaves it
            // nowhere: hidden.
            let hidden = self.window_up() && self.deck.is_some();
            return [hidden, hidden];
        }
        let mut taken = self.pages_taken(view, None);
        if let Some(p) = self.grid.bound_panel() {
            taken[p] = true;
        }
        // The keyboard's panel: the dock leaves it as the keyboard comes up
        // (stevia's, or item's own tile: kbd.rs).
        if let Some(p) = self.layers.keyboard_panel().or_else(|| self.kbd.panel()) {
            taken[p] = true;
        }
        // The pen's sheet spread over the left panel too.
        if self.pen.spread() {
            taken[0] = true;
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
                .unwrap_or(if layout::turned() { 0 } else { 1 });
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
        self.dock.set_book(self.posture.book());
        // The deck calls the dock as soon as the launch curtain covers the
        // screen; it lets go only after the screen has been bare for a
        // moment (the curtain lifting before the window's page is applied
        // must not send it up and back).
        let now = hybris_hwc::now_ns();
        if layout::turned() && (self.window_up() || self.curtain.panel() == Some(0)) {
            self.deck_wanted_ns = now;
        }
        let on_deck = layout::turned() && now.saturating_sub(self.deck_wanted_ns) < 400_000_000;
        self.dock.set_on_deck(on_deck, now);
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
            self.dock.scrub(from, to, k, false);
        } else if let Some((panel, k)) = self.grid.held(frame_ns) {
            // The app grid under a finger: the dock goes with the finger
            // (let go, it goes on at its own pace from there, dock.rs).
            let from = self.pages_taken(view, None);
            let mut to = from;
            to[panel] = true;
            self.dock.scrub(from, to, k, true);
        } else if let Some((window, k)) = self.gestures.moving_window(frame_ns).filter(|(w, _)| self.ribbon.find(w).is_some()) {
            let from = self.pages_taken(view, Some(&window));
            let mut to = from;
            if let Some(panel) = (0..2).find(|&p| self.ribbon.on(p) == Some(&crate::ribbon::Page::App(window.clone()))) {
                to[panel] = false;
            }
            if let Some(p) = self.grid.panel() {
                to[p] = true;
            }
            // At the dock's own pace, not the flick's; let go part of the way,
            // it goes on from there (the clock with it), not snapped.
            self.dock.scrub_paced(from, to, k, frame_ns);
        } else {
            let taken = self.dock_taken(view);
            // A move begins: the clocks up now, before its first frame.
            if self.dock.follow(taken, frame_ns) {
                self.boost.kick(hybris_hwc::now_ns());
            }
        }
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

    /// The open windows, shown and put away, topmost first, as rows; not
    /// those warmed and never opened.
    pub fn window_rows(&self) -> Vec<(Window, crate::quick::Row)> {
        let entries = &self.entries;
        let windows: Vec<Window> = self.windows().into_iter().filter(|w| !self.warm_unopened.contains(w)).collect();
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
        let relayout = std::mem::take(&mut self.relayout);
        let panels = layout::panels();
        // Turned, the lower panel is the deck: black, or the page swiped onto it.
        let shown = [self.ribbon.on(0).cloned(), if layout::turned() { self.deck.clone() } else { self.ribbon.on(1).cloned() }];
        // The keyboard along with its window, if the row moved that one
        // to the other panel (kbd.rs).
        if let (Some(w), Some(kp)) = (self.kbd_window.clone(), self.kbd.panel()) {
            let wp = (0..2).find(|&p| matches!(&shown[p], Some(Page::App(x)) | Some(Page::Wide(x)) if x == &w));
            if let Some(wp) = wp.filter(|&wp| wp != kp) {
                self.kbd.show(Some(wp), None);
            }
        }
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
                        let size = (layout::size().0, layout::size().1);
                        let at: smithay::utils::Point<i32, Logical> = (x, 0).into();
                        if relayout || self.space.element_location(&w) != Some(at) || w.geometry().size.w != size.0 {
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
                    if relayout || self.space.element_location(&w) != Some(rect.loc) || w.geometry().size.w > rect.size.w {
                        let cut = match self.kbd.cut().or(self.osk_applied) {
                            Some((kp, h)) if kp == k => h,
                            _ => 0,
                        };
                        w.toplevel().unwrap().with_pending_state(|s| s.size = Some((rect.size.w, rect.size.h - cut - self.dock_band(k)).into()));
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
        self.system.show(shown.contains(&Some(Page::System)));
        self.pen.show(shown.contains(&Some(Page::Pen)));
        // The keyboard to a window shown: the one it had, else the left's.
        let keyboard = self.seat.get_keyboard().unwrap();
        let shown_surfaces: Vec<_> = self.space.elements().map(|w| w.toplevel().unwrap().wl_surface().clone()).collect();
        if !keyboard.current_focus().is_some_and(|f| shown_surfaces.contains(&f)) {
            let next = self.top_window(0).or_else(|| self.top_window(1)).map(|w| w.toplevel().unwrap().wl_surface().clone());
            keyboard.set_focus(self, next, smithay::utils::SERIAL_COUNTER.next_serial());
        }
        tracing::info!("ribbon: applied, {:?} | {:?}", shown[0], shown[1]);
        // The flow's covers: the ribbon's windows, the screen's one in the middle.
        let flow_windows: Vec<Window> = self.ribbon.pages.iter().filter_map(|p| if let Page::App(w) = p { Some(w.clone()) } else { None }).collect();
        let flow_shown = match &shown[0] { Some(Page::App(w)) => flow_windows.iter().position(|x| x == w), _ => None };
        self.flow.sync(flow_windows, flow_shown);
        // What the keyboard (and the deck's strip) would type into now.
        let (web, terminal) = self.keyboard_window().map(|w| (crate::kbd::is_browser(&app_id(&w)), crate::kbd::is_terminal(&app_id(&w)))).unwrap_or((false, false));
        self.kbd.set_window(web, terminal);
        self.needs_redraw = true;
    }

    /// The band the dock takes from a panel's windows: the turned frame's
    /// upper panel carries the dock along its bottom as a taskbar.
    pub fn dock_band(&self, _panel: usize) -> i32 {
        // The dock leaves the screen to its window (it goes to the deck):
        // no band taken from windows.
        0
    }

    /// The frame turned: whether a window stands on the screen.
    pub fn window_up(&self) -> bool {
        matches!(self.ribbon.on(0), Some(crate::ribbon::Page::App(_) | crate::ribbon::Page::Wide(_)))
    }

    /// A swipe across the deck (the turned frame's lower panel) let go:
    /// left brings the pen's sheet, right the system screen - the ribbon's
    /// two ends - and the other way takes the one shown away.
    pub fn deck_swiped(&mut self, dx: f64, dy: f64) {
        use crate::ribbon::Page;
        const SWIPE: f64 = 80.0;
        // Up and down are the keyboard's; sideways, with the keyboard up,
        // is the keyboard's own (the space bar's cursor, the alternatives).
        if dy.abs() > dx.abs() {
            if dy <= -SWIPE {
                self.deck_keyboard(true);
            } else if dy >= SWIPE {
                self.deck_keyboard(false);
            }
            return;
        }
        if self.kbd.out() {
            return;
        }
        let next = match (self.deck.clone(), dx) {
            (None, d) if d <= -SWIPE => Some(Page::Pen),
            (None, d) if d >= SWIPE => Some(Page::System),
            (Some(Page::Pen), d) if d >= SWIPE => None,
            (Some(Page::System), d) if d <= -SWIPE => None,
            (deck, _) => deck,
        };
        if next == self.deck || self.deck_run.is_some() {
            return;
        }
        tracing::info!("deck: {}", match &next { Some(Page::Pen) => "the pen's sheet", Some(Page::System) => "the system screen", _ => "black" });
        let side = |p: &Page| if *p == Page::Pen { 1.0 } else { -1.0 };
        let now = hybris_hwc::now_ns();
        match (&self.deck, &next) {
            // A page comes in from its side.
            (None, Some(p)) => {
                self.deck_run = Some((side(p), now, false));
                self.deck = next;
                self.apply_ribbon();
            }
            // The page shown goes back out its side; the deck is black after.
            (Some(p), None) => self.deck_run = Some((side(p), now, true)),
            _ => {
                self.deck = next;
                self.apply_ribbon();
            }
        }
        self.needs_redraw = true;
    }

    /// The flow settled on a cover (flow.rs): the screen pages to its window.
    pub fn flow_chose(&mut self, i: usize) {
        let Some(window) = self.flow.windows.get(i).cloned() else { return };
        if let Some(page) = self.ribbon.find(&window) {
            if self.ribbon.view != page {
                tracing::info!("flow: {} chosen, page {page}", app_id(&window));
                // The screen pages as the row runs, in the row's own time.
                self.ribbon.go_to_in(page, crate::flow::RUN_NS as u64);
                self.needs_redraw = true;
            }
        }
    }

    /// A vertical swipe on the deck: up pulls the keyboard, down puts it
    /// away (the owner's word, 2026-10-22). Not while the system screen is
    /// up there (it scrolls).
    pub fn deck_keyboard(&mut self, up: bool) {
        if up && matches!(self.deck, Some(crate::ribbon::Page::System)) {
            return;
        }
        if self.kbd.out() != up {
            tracing::info!("deck: the keyboard {}", if up { "pulled up" } else { "put away" });
            self.toggle_keyboard();
        }
    }

    /// How long a deck page takes to come or go (ns), and its way at `now`:
    /// the page's offset from its place, logical px, while it runs.
    pub const DECK_NS: u64 = 350_000_000;
    pub fn deck_offset(&self, now: u64) -> Option<f64> {
        let (side, start, leaving) = self.deck_run?;
        let k = (now.saturating_sub(start) as f64 / Self::DECK_NS as f64).clamp(0.0, 1.0);
        let e = if k < 0.5 { 4.0 * k * k * k } else { 1.0 - (-2.0 * k + 2.0).powi(3) / 2.0 };
        let w = layout::panels()[1].size.w as f64;
        Some(side * w * if leaving { e } else { 1.0 - e })
    }

    /// The deck's run settled: whether a frame is still wanted.
    pub fn settle_deck(&mut self, now: u64) -> bool {
        let Some((_, start, leaving)) = self.deck_run else { return false };
        if now < start + Self::DECK_NS {
            return true;
        }
        self.deck_run = None;
        if leaving {
            self.deck = None;
            self.apply_ribbon();
        }
        true
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
            crate::shade::ShadeAsk::Face => {
                if crate::face::enrolled() {
                    tracing::info!("shade: face unlock off");
                    self.face.remove();
                } else if self.face.on() {
                    tracing::info!("shade: face unlock on: the face taken with the PIN");
                    self.shade.fold();
                    self.lock.lock_now();
                    self.lock.enrol_face();
                }
            }
            crate::shade::ShadeAsk::Settings(panel) => {
                if let Some(e) = crate::apps::entry("lab.agentsco.item.Settings.desktop").or_else(|| crate::apps::entry("org.gnome.Settings.desktop")) {
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
                    let height = w.geometry().size.h.max(1);
                    // Turned, the panels share their x: the deck's rows
                    // must not find the screen's window (the put-away on a
                    // deck swipe, 2026-10-23).
                    loc.x < rect.loc.x + rect.size.w && loc.x + width > rect.loc.x && loc.y < rect.loc.y + rect.size.h && loc.y + height > rect.loc.y
                })
            })
            .cloned()
    }

    /// Takes a window out of the space into the windows put away.
    pub fn put_window_away(&mut self, window: Window, panel: usize) {
        tracing::info!("put away: {} from the {} panel", app_id(&window), if panel == 0 { "left" } else { "right" });
        self.space.unmap_elem(&window);
        self.ribbon.remove(&window);
        self.keyboard_gone_with(&window);
        self.put_away.push((window.clone(), panel));
        self.windows_gone += 1;
        // The keyboard goes to what is left on top.
        let keyboard = self.seat.get_keyboard().unwrap();
        if keyboard.current_focus().is_some_and(|f| &f == window.toplevel().unwrap().wl_surface()) {
            let next = self.top_window(panel).or_else(|| self.top_window(1 - panel));
            let surface = next.map(|w| w.toplevel().unwrap().wl_surface().clone());
            keyboard.set_focus(self, surface, smithay::utils::SERIAL_COUNTER.next_serial());
        }
        self.needs_redraw = true;
    }

    /// A window that waited for its app id (new_toplevel, draft 49): a
    /// warming app's is put away as it is, never shown; another's is
    /// shown now, as new_toplevel would have.
    fn warm_or_show(&mut self, window: Window, pi: usize) {
        let id = app_id(&window);
        let now = hybris_hwc::now_ns();
        let asked = self.launch_to.filter(|&(_, at)| now.saturating_sub(at) < 10_000_000_000).map(|(p, _)| p);
        let warm = self.warming.get(&id).copied();
        // A warming app's first window, or one of its later ones (a dialog,
        // a second toplevel), or a window of an app warmed earlier whose
        // launch was forgotten: put away unshown - unless the dock asked for
        // it meanwhile (claimed), or a launch is waiting and this is not a
        // warming app's window.
        let claimed = warm.is_some_and(|w| w.claimed);
        if !claimed && (warm.is_some() || (asked.is_none() && self.warm_known.contains(&id))) {
            match warm {
                Some(w) if w.arrived.is_none() => tracing::info!("warm: {id} ready in {:.2} s, put away unshown", now.saturating_sub(w.since) as f64 / 1e9),
                _ => tracing::info!("warm: {id}: another window, put away unshown"),
            }
            if let Some(w) = warm {
                for v in self.warming.values_mut().filter(|v| v.since == w.since) {
                    v.arrived.get_or_insert(now);
                }
            }
            self.warm_unopened.push(window.clone());
            self.put_away.push((window, pi));
            return;
        }
        // Shown: on the panel asked for, under the curtain raised for it.
        let pi = match asked {
            Some(p) => {
                self.launch_to = None;
                p
            }
            None if layout::turned() => 0,
            None => pi,
        };
        if let Some(w) = warm {
            self.warming.retain(|_, v| v.since != w.since);
        }
        let panel = layout::panels()[pi];
        let band = self.dock_band(pi);
        window.toplevel().unwrap().with_pending_state(|s| s.size = Some((panel.size.w, panel.size.h - band).into()));
        window.toplevel().unwrap().send_pending_configure();
        self.curtain.adopt(pi, window.toplevel().unwrap().wl_surface());
        self.curtain.drawn(window.toplevel().unwrap().wl_surface(), now);
        self.ribbon.open(window.clone(), pi);
        self.opened_onto(&window, pi);
        self.space.map_element(window.clone(), panel.loc, true);
        let surface = window.toplevel().unwrap().wl_surface().clone();
        self.seat.get_keyboard().unwrap().set_focus(self, Some(surface), smithay::utils::SERIAL_COUNTER.next_serial());
        self.needs_redraw = true;
        tracing::info!("window mapped on the {} panel after a warm wait", if pi == 0 { "left" } else { "right" });
    }

    /// Whether a window of one of `ids` is about: shown, put away or waiting.
    fn app_about(&self, ids: &[String]) -> bool {
        let is = |w: &Window| ids.iter().any(|id| *id == app_id(w));
        self.windows().iter().any(is) || self.put_away.iter().any(|(w, _)| is(w)) || self.warm_pending.iter().any(|(w, _)| is(w))
    }

    /// The dock's apps warmed after boot (draft 49): from 30 s after the
    /// start (in the first setup, once past its welcome), one every 3 s, each launched without a curtain and put away
    /// at its first buffer; not Chromium (resident on its own), not an app
    /// about already, none when the memory left is under 1.5 GB. Called
    /// from the tick.
    pub fn warm_tick(&mut self, now: u64) {
        const START_S: f64 = 30.0;
        const GAP_NS: u64 = 3_000_000_000;
        const FORGET_NS: u64 = 60_000_000_000;
        /// An arrived app's later windows are warm for this long.
        const MORE_NS: u64 = 15_000_000_000;
        const MEM_FLOOR_KB: u64 = 1_500_000;
        // Launches asked from a thread (the resident Chromium's, when the
        // resident could not be reached): plain ones, now.
        let later: Vec<(String, String)> = std::mem::take(&mut *self.spawn_later.lock().unwrap());
        for (exec, id) in later {
            tracing::info!("chromium: no resident instance answered, launched as a process");
            self.spawn_app_plain(&exec, &id);
        }
        // Not at the setup's welcome nor in the tour: the screen is theirs.
        // Past the welcome, at once (an app's windows, dialogs too, are put
        // away unshown): the apps warm when the setup ends.
        if self.setup.welcoming() || self.tour.active() {
            return;
        }
        if !self.warm_begun {
            if !self.setup.active && self.started.elapsed().as_secs_f64() < START_S {
                return;
            }
            self.warm_begun = true;
            // Chromium is resident on its own (item-chromium.service), known by its ids.
            self.warm_queue = crate::dock::configured_desktops().into_iter().filter(|d| !crate::apps::entry(d).is_some_and(|e| e.ids.iter().any(|i| crate::kbd::is_browser(i)))).collect();
            self.warm_next_ns = now;
        }
        // A warming app that never came is forgotten; an arrived one is
        // remembered a while, for its later windows.
        self.warming.retain(|id, w| match w.arrived {
            None => {
                let kept = now.saturating_sub(w.since) < FORGET_NS;
                if !kept {
                    tracing::info!("warm: {id} never came, forgotten");
                }
                kept
            }
            Some(at) => now.saturating_sub(at) < MORE_NS,
        });
        if self.warm_queue.is_empty() || now < self.warm_next_ns || self.warming.values().any(|w| w.arrived.is_none()) {
            return;
        }
        let desktop = self.warm_queue.remove(0);
        self.warm_next_ns = now + GAP_NS;
        let Some(entry) = crate::apps::entry(&desktop) else { return };
        if self.app_about(&entry.ids) {
            return;
        }
        let available_kb = crate::sysfacts::mem_available_kb().unwrap_or(u64::MAX);
        if available_kb < MEM_FLOOR_KB {
            tracing::info!("warm: {} MB left, {desktop} not warmed", available_kb >> 10);
            self.warm_queue.clear();
            return;
        }
        for id in &entry.ids {
            self.warming.insert(id.clone(), Warming { since: now, arrived: None, claimed: false });
            self.warm_known.insert(id.clone());
        }
        tracing::info!("warm: launching {desktop}");
        self.spawn_app(&entry.exec, entry.ids.first().map(String::as_str).unwrap_or("app"));
    }

    /// The dock's turn button tapped: the panel it was on becomes the deck
    /// (the other the screen); tapped while turned, the book again. The
    /// turn itself is main.rs's, each frame, with the blur.
    pub fn ask_turn(&mut self, deck: Option<usize>) {
        self.turn_asked = match deck {
            None => 0,
            Some(0) => 1,
            Some(_) => 2,
        };
        tracing::info!("turn: asked {} by the dock's button", match self.turn_asked { 0 => "the book", 1 => "the right panel up", _ => "the left panel up" });
        crate::fingerprint::buzz("button-pressed");
        self.needs_redraw = true;
    }

    /// The turn button held: the turn handed to the sensors (gravity and
    /// the gyroscope start streaming, the posture decides; the button
    /// green) or back to the button (the streams stop; the frame stays as
    /// it is until the button is tapped). Kept in ~/.config/item/turn, as
    /// Settings' Turn page writes it.
    pub fn toggle_turn_mode(&mut self) {
        let on = !self.turn_by_sensors;
        self.turn_by_sensors = on;
        self.turn_asked = crate::layout::turn();
        self.dock.set_auto(on);
        if on != self.sensors_streaming && !self.setup.active {
            self.posture.by_sensors(on);
            self.sensors_streaming = on;
        }
        crate::prefs::write("turn", if on { "sensors" } else { "button" });
        tracing::info!("turn: {} (the dock's button held)", if on { "by the sensors" } else { "by the button" });
        self.needs_redraw = true;
    }

    /// An app onto a panel, from the dock or the grid: a window of it put
    /// away comes back; else it is launched under the curtain.
    /// The picker's strip as it is now: the set's pictures, then - with
    /// Unsplash's key - its group (Shuffle, its batch); the group's state
    /// and the kept ones told to the picker. Returns the strip's count.
    pub fn picker_strip(&mut self) -> usize {
        let own = self.walls.shown.len();
        let on = self.gallery.available();
        self.picker.group = on.then_some(own);
        self.picker.group_busy = self.gallery.busy();
        self.picker.group_note = self.gallery.note();
        self.picker.kept = self.walls.shown.iter().enumerate().filter(|(_, &n)| self.walls.names[n].starts_with("unsplash-")).map(|(i, _)| i).collect();
        own + if on { 1 + self.gallery.len() } else { 0 }
    }

    /// The strip's small pictures, in its order (picker_strip's).
    pub fn picker_thumbs(&self) -> Vec<Option<smithay::backend::renderer::element::memory::MemoryRenderBuffer>> {
        let mut t = self.walls.shown_thumbs();
        if self.gallery.available() {
            t.push(None);
            t.extend(self.gallery.thumbs.iter().cloned());
        }
        t
    }

    pub fn launch(&mut self, exec: &str, ids: &[String], icon: Option<smithay::backend::renderer::element::memory::MemoryRenderBuffer>, panel: usize) {
        // The frame turned (layout.rs): every launch is the screen's, from
        // whichever half of the taskbar; the curtain rises there.
        let panel = if layout::turned() { 0 } else { panel };
        // The Agent entry: the keyboard comes under its window, the agent's
        // keys up (kbd.rs).
        let agent = ids.iter().any(|id| id == "item-agent");
        // On its way warm already (warm_tick): claimed - its window is shown
        // under the curtain when it comes, no second process.
        let warming = ids.iter().any(|id| self.warming.get(id).is_some_and(|w| w.arrived.is_none()));
        if warming {
            let now = hybris_hwc::now_ns();
            for id in ids {
                if let Some(w) = self.warming.get_mut(id) {
                    w.claimed = true;
                }
            }
            if agent {
                self.kbd_wanted = Some(now);
            }
            self.launch_to = Some((panel, now));
            self.curtain.raise(panel, icon, ids.first().map(String::as_str).unwrap_or("app"), now);
            self.needs_redraw = true;
            tracing::info!("launch: {} is on its way warm, waited for", ids.first().map(String::as_str).unwrap_or("app"));
            return;
        }
        if self.bring_back(ids, panel) || self.call_over(ids, panel) {
            if agent {
                if let Some(w) = self.top_window(panel) {
                    self.keyboard_for(w, crate::kbd::Layer::Vibe);
                }
            }
            return;
        }
        let now = hybris_hwc::now_ns();
        if agent {
            self.kbd_wanted = Some(now);
        }
        self.launch_to = Some((panel, now));
        self.curtain.raise(panel, icon, ids.first().map(String::as_str).unwrap_or("app"), now);
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
        // Chromium kept resident (session/item-chromium.service): its window
        // asked of it over its socket, on a thread (the answer takes the
        // time of a window); the resident absent, the service is started
        // and asked again for a few seconds, and failing that the plain
        // launch is asked of the tick (spawn_later).
        if crate::kbd::is_browser(id) {
            let (exec, id, later, wake) = (exec.to_owned(), id.to_owned(), self.spawn_later.clone(), self.wake.clone());
            let _ = std::thread::Builder::new().name("chromium ask".into()).spawn(move || {
                if resident_chromium(&exec) {
                    tracing::info!("chromium: a window asked of the resident instance");
                } else {
                    later.lock().unwrap().push((exec, id));
                    wake.ping();
                }
            });
            return;
        }
        self.spawn_app_plain(exec, id);
    }

    /// `exec` started as a process of its own, in a scope.
    fn spawn_app_plain(&self, exec: &str, id: &str) {
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

    /// An agent's session opened from the system screen (sysfacts.rs's
    /// taps): a terminal on the right panel running `item-agents open`,
    /// the agent's keys under it.
    pub fn open_agent(&mut self, cmd: &str) {
        tracing::info!("agent: {cmd}");
        let now = hybris_hwc::now_ns();
        let panel = 1;
        self.launch_to = Some((panel, now));
        self.kbd_wanted = Some(now);
        let icon = crate::apps::icon("item-agent", crate::curtain::ICON);
        self.curtain.raise(panel, icon, "item-agent", now);
        let quoted = cmd.replace('\'', "'\\''");
        self.spawn_app(&format!("kgx -- sh -c '{quoted}; echo; echo [session ended]; sleep 2'"), "item-agent");
        self.needs_redraw = true;
    }

    /// The keyboard up under a window (kbd.rs), in a layer: the agent's for
    /// a terminal.
    pub fn keyboard_for(&mut self, window: Window, layer: crate::kbd::Layer) {
        let panel = (0..2).find(|&p| self.ribbon.on(p) == Some(&crate::ribbon::Page::App(window.clone()))).or_else(|| self.space.element_location(&window).and_then(|l| layout::panel_at(l.to_f64() + Point::from((1.0, 1.0)))));
        tracing::info!("keyboard: under {} on the {} panel", app_id(&window), match panel { Some(0) => "left", Some(_) => "right", None => "no" });
        let surface = window.toplevel().unwrap().wl_surface().clone();
        self.seat.get_keyboard().unwrap().set_focus(self, Some(surface), smithay::utils::SERIAL_COUNTER.next_serial());
        self.kbd_window = Some(window);
        self.kbd.show(panel, Some(layer));
        self.follow_keyboard();
        self.needs_redraw = true;
    }

    /// The power key: item's keyboard shown under the window with the
    /// focus, or hidden; stevia's when the phone uses stevia (kbd::choice).
    pub fn toggle_keyboard(&mut self) {
        if crate::kbd::choice() == crate::kbd::Choice::Stevia {
            crate::input::toggle_stevia();
            return;
        }
        self.kbd_auto = false;
        self.field_left_at = None;
        if self.kbd.out() {
            tracing::info!("power key: keyboard hidden");
            self.kbd.show(None, None);
        } else {
            let focused = self.seat.get_keyboard().unwrap().current_focus();
            let window = self.space.elements().find(|w| focused.as_ref() == Some(w.toplevel().unwrap().wl_surface())).cloned().or_else(|| self.top_window(1).or_else(|| self.top_window(0)));
            tracing::info!("power key: keyboard shown");
            match window {
                Some(window) => {
                    let layer = if crate::kbd::is_terminal(&app_id(&window)) { crate::kbd::Layer::Vibe } else { crate::kbd::Layer::Letters };
                    self.keyboard_for(window, layer);
                }
                // No window: on the right panel, as stevia came.
                None => self.kbd.show(Some(1), Some(crate::kbd::Layer::Letters)),
            }
        }
        self.follow_keyboard();
        self.needs_redraw = true;
    }

    /// A window gone (closed, put away): the keyboard under it goes when
    /// nothing is left on its panel.
    fn keyboard_gone_with(&mut self, window: &Window) {
        if self.kbd_window.as_ref() == Some(window) {
            self.kbd_window = None;
        }
        if let Some(p) = self.kbd.panel() {
            let left = self.space.elements().filter(|w| w != &window).any(|w| self.space.element_location(w).is_some_and(|l| layout::panel_at(l.to_f64() + Point::from((1.0, 1.0))) == Some(p)));
            if !left {
                self.kbd.show(None, None);
                self.follow_keyboard();
            }
        }
    }

    /// A text field of the window with the focus took the cursor (a
    /// text-input v3 enable committed, protocols.rs) or let it go: item's
    /// keyboard comes up by itself under the window, in its letters, when
    /// a finger put the cursor there (not a page focusing a field as it
    /// loads), and goes half a second after the field is left - unless a
    /// hand raised it (the power key, the Agent), which keeps it. A
    /// terminal's field is its whole screen: no keyboard by itself there.
    pub fn text_field(&mut self, on: bool, from: &smithay::reexports::wayland_server::backend::ObjectId) {
        if crate::kbd::choice() == crate::kbd::Choice::Stevia {
            return;
        }
        let focused = self.seat.get_keyboard().unwrap().current_focus();
        let Some(window) = focused.as_ref().and_then(|f| self.space.elements().find(|w| w.toplevel().unwrap().wl_surface() == f).cloned()) else {
            return;
        };
        if !from.same_client_as(&window.toplevel().unwrap().wl_surface().id()) {
            return;
        }
        let now = hybris_hwc::now_ns();
        if on {
            self.field_left_at = None;
            if self.kbd.out() || crate::kbd::is_terminal(&app_id(&window)) || !self.prefs.auto {
                return;
            }
            if now.saturating_sub(self.window_touched_at) > 2_000_000_000 {
                tracing::debug!("keyboard: a field in {} took the cursor by itself, left down", app_id(&window));
                return;
            }
            tracing::debug!("keyboard: a field in {} took the cursor", app_id(&window));
            self.keyboard_for(window, crate::kbd::Layer::Letters);
            self.kbd_auto = true;
        } else if self.kbd_auto && self.kbd.out() {
            self.field_left_at = Some(now);
        }
    }

    /// The keyboard that came up by itself goes half a second after its
    /// field was left (main.rs's tick).
    pub fn settle_field(&mut self, now: u64) {
        if self.field_left_at.is_some_and(|t| now.saturating_sub(t) >= 500_000_000) {
            self.field_left_at = None;
            // On the deck (the frame turned) the keyboard is at home: it
            // stays up across windows, down only by hand.
            if self.kbd_auto && self.kbd.out() && !layout::turned() {
                tracing::debug!("keyboard: the field was left, down");
                self.kbd.show(None, None);
                self.follow_keyboard();
                self.needs_redraw = true;
            }
            self.kbd_auto = false;
        }
    }

    /// What the keyboard asks (kbd.rs).
    pub fn keyboard_ask(&mut self, ask: crate::kbd::Ask) {
        match ask {
            crate::kbd::Ask::Key(k) => self.send_keys(&[k]),
            crate::kbd::Ask::Text(text) => self.type_text(&text),
        }
        self.needs_redraw = true;
    }

    /// The window the keyboard types into: the one with the focus, else
    /// the one on top of its panel.
    fn keyboard_window(&self) -> Option<Window> {
        let focused = self.seat.get_keyboard().unwrap().current_focus();
        // Turned (layout.rs), the window is the screen's whether or not the
        // keyboard is up: the agent's strip on the deck follows it.
        self.space.elements().find(|w| focused.as_ref() == Some(w.toplevel().unwrap().wl_surface())).cloned().or_else(|| self.kbd.panel().or(layout::turned().then_some(0)).and_then(|p| self.top_window(p)))
    }

    /// Keys pressed on the window above the keyboard, from the compositor's
    /// own keyboard, as go_back sends Alt+Left: each with its modifiers
    /// held round it, in the keymap's group it is read in.
    pub fn send_keys(&mut self, keys: &[crate::typing::Key]) {
        let Some(window) = self.keyboard_window() else {
            tracing::info!("keyboard: no window to type into");
            return;
        };
        let surface = window.toplevel().unwrap().wl_surface().clone();
        let keyboard = self.seat.get_keyboard().unwrap();
        keyboard.set_focus(self, Some(surface), smithay::utils::SERIAL_COUNTER.next_serial());
        use smithay::backend::input::KeyState;
        use smithay::input::keyboard::{FilterResult, Keycode, Layout};
        let mut time = (hybris_hwc::now_ns() / 1_000_000) as u32;
        let mut group = 0u8;
        for k in keys {
            if k.group != group {
                group = k.group;
                keyboard.with_xkb_state(self, |mut ctx| ctx.set_layout(Layout(group as u32)));
            }
            let mut seq = Vec::with_capacity(8);
            if k.ctrl {
                seq.push((crate::typing::LEFT_CTRL, KeyState::Pressed));
            }
            if k.alt {
                seq.push((crate::typing::LEFT_ALT, KeyState::Pressed));
            }
            if k.shift {
                seq.push((crate::typing::LEFT_SHIFT, KeyState::Pressed));
            }
            seq.push((k.code, KeyState::Pressed));
            seq.push((k.code, KeyState::Released));
            if k.shift {
                seq.push((crate::typing::LEFT_SHIFT, KeyState::Released));
            }
            if k.alt {
                seq.push((crate::typing::LEFT_ALT, KeyState::Released));
            }
            if k.ctrl {
                seq.push((crate::typing::LEFT_CTRL, KeyState::Released));
            }
            for (code, state) in seq {
                let serial = smithay::utils::SERIAL_COUNTER.next_serial();
                keyboard.input::<(), _>(self, Keycode::new(code + 8), state, serial, time, |_, _, _| FilterResult::Forward);
            }
            time = time.wrapping_add(1);
        }
        if group != 0 {
            keyboard.with_xkb_state(self, |mut ctx| ctx.set_layout(Layout(0)));
        }
    }

    /// Words typed into the window above the keyboard.
    pub fn type_text(&mut self, text: &str) {
        let keys = crate::typing::keys_for(text);
        tracing::debug!("keyboard: typing {} keys", keys.len());
        self.send_keys(&keys);
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
        self.warm_unopened.retain(|w| w != &window);
        let rect = layout::panels()[panel];
        let band = self.dock_band(panel);
        window.toplevel().unwrap().with_pending_state(|s| s.size = Some((rect.size.w, rect.size.h - band).into()));
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
        // The keyboard's bottom row for the window typed into.
        let (web, terminal) = self.keyboard_window().map(|w| (crate::kbd::is_browser(&app_id(&w)), crate::kbd::is_terminal(&app_id(&w)))).unwrap_or((false, false));
        self.kbd.set_window(web, terminal);
        // item's keyboard takes its half; stevia's height when it is stevia.
        let now = if std::env::var_os("NO_OSK_RESIZE").is_some() { None } else { self.kbd.cut().or_else(|| self.layers.keyboard_height()) };
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
                t.with_pending_state(|s| s.size = Some((rect.size.w, rect.size.h - cut - self.dock_band(p)).into()));
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
    // A picture on each panel keeps still: the seam stays in the hinge.
    if crate::walls::each_on() {
        return 0.0;
    }
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
        if let Some(sent) = self.key_sent.take() {
            let waited = hybris_hwc::now_ns().saturating_sub(sent);
            if waited < 500_000_000 {
                tracing::debug!("keyboard: the client answered the key in {:.1} ms", waited as f64 / 1e6);
                self.key_answered.get_or_insert(sent);
            }
        }
        let kind = with_renderer_surface_state(surface, |rs| rs.buffer().map(|b| format!("{:?}", buffer_type(b))))
            .flatten();
        // The launched app's window drew: the curtain over it lifts.
        if kind.is_some() && self.curtain.drawn(surface, hybris_hwc::now_ns()) {
            self.needs_redraw = true;
        }
        if kind.is_some() {
            if let Some(i) = self.warm_pending.iter().position(|(w, _)| w.toplevel().unwrap().wl_surface() == surface) {
                let (window, pi) = self.warm_pending.remove(i);
                self.warm_or_show(window, pi);
            }
        }
        if let Some(kind) = kind {
            let id = format!("{:?}", surface.id());
            if self.buffer_kinds.insert(id.clone(), kind.clone()).as_ref() != Some(&kind) {
                let app = self.space.elements().find(|w| w.toplevel().unwrap().wl_surface() == surface).map(app_id);
                tracing::debug!("surface {id}: buffer {kind}{}", app.map(|a| format!(", app id {a}")).unwrap_or_default());
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

        // A toplevel's first commit gets its first configure (a window
        // waiting for its app id too, draft 49: it is in no space yet).
        let shown = self.space.elements().find(|w| w.toplevel().unwrap().wl_surface() == surface).cloned();
        if let Some(window) = shown.or_else(|| self.warm_pending.iter().find(|(w, _)| w.toplevel().unwrap().wl_surface() == surface).map(|(w, _)| w.clone())) {
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
                if let Err(e) = xdg.send_configure() {
                    tracing::warn!("popup: its first configure: {e}");
                }
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
        // Not asked for (opened by another app): a free panel, so what is
        // shown stays where it is; none free, the next in turn.
        let free = (0..2).find(|&p| matches!(self.ribbon.on(p), Some(crate::ribbon::Page::Desk(_))));
        let mut pi = match asked.or(free) {
            Some(p) => p,
            None => {
                self.next_panel += 1;
                (self.next_panel - 1) % panels.len()
            }
        };
        // The frame turned (layout.rs): every window is the screen's; the
        // deck takes none.
        if layout::turned() {
            pi = 0;
        }
        let panel = panels[pi];
        let band = self.dock_band(pi);
        surface.with_pending_state(|state| {
            state.size = Some((panel.size.w, panel.size.h - band).into());
            state.states.set(xdg_toplevel::State::Maximized);
            state.states.set(xdg_toplevel::State::Activated);
        });
        // A warm launch pending (draft 49): every window waits unshown until
        // its first buffer tells its app id - a warming app's goes straight
        // to the windows put away, any other's is shown then (warm_or_show),
        // on the panel a launch asked for, under its curtain.
        if self.warming.values().any(|w| w.arrived.is_none()) {
            if let Some(p) = asked {
                // Not taken: warm_or_show reads it for the window shown.
                self.launch_to = Some((p, hybris_hwc::now_ns()));
            }
            let window = Window::new_wayland_window(surface);
            self.warm_pending.push((window, pi));
            if asked.is_none() && self.next_panel > 0 && free.is_none() {
                self.next_panel -= 1;
            }
            return;
        }
        if asked.is_some() {
            let p = panels.iter().position(|r| *r == panel).unwrap_or(0);
            self.curtain.adopt(p, surface.wl_surface());
        }
        let window = Window::new_wayland_window(surface);
        // The pages move for it: the clocks up before they do.
        self.boost.kick(hybris_hwc::now_ns());
        let p = panels.iter().position(|r| *r == panel).unwrap_or(0);
        if asked.is_none() && free == Some(p) {
            self.ribbon.open_free(window.clone(), p);
        } else {
            self.ribbon.open(window.clone(), p);
        }
        self.opened_onto(&window, p);
        self.space.map_element(window.clone(), panel.loc, true);
        if self.kbd_wanted.take_if(|t| hybris_hwc::now_ns().saturating_sub(*t) < 10_000_000_000).is_some() {
            self.keyboard_for(window.clone(), crate::kbd::Layer::Vibe);
            self.kbd_auto = false;
        }
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
        self.windows_gone += 1;
        self.boost.kick(hybris_hwc::now_ns());
        // Its page leaves the row.
        let page = self.ribbon.pages.iter().find_map(|p| match p {
            crate::ribbon::Page::App(w) if w.toplevel().unwrap().wl_surface() == surface.wl_surface() => Some(w.clone()),
            _ => None,
        });
        if let Some(w) = page {
            self.ribbon.remove(&w);
            self.keyboard_gone_with(&w);
        }
        self.put_away.retain(|(w, _)| w.toplevel().unwrap().wl_surface() != surface.wl_surface());
        self.warm_unopened.retain(|w| w.toplevel().unwrap().wl_surface() != surface.wl_surface());
        self.warm_pending.retain(|(w, _)| w.toplevel().unwrap().wl_surface() != surface.wl_surface());
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
            let panel = self
                .space
                .elements()
                .find(|w| w.toplevel().unwrap().wl_surface() == f)
                .and_then(|w| self.space.element_location(w))
                .and_then(|l| panels.iter().position(|r| r.contains(l)));
            if let Some(p) = panel {
                self.layers.panel = p;
            }
        }
        // The text inputs of the window losing the focus told `leave`, the
        // new one's `enter`: without an input method bound smithay tells
        // them nothing, and a client enables a field only after enter
        // (Chromium; GTK does without). The handle speaks to the surface
        // it holds, so the old one is put back for its leave.
        if crate::kbd::choice() == crate::kbd::Choice::Item {
            use smithay::wayland::text_input::TextInputSeat;
            let ti = self.seat.text_input();
            if self.text_entered.as_ref() != focused {
                if let Some(old) = self.text_entered.take().filter(|s| s.is_alive()) {
                    ti.set_focus(Some(old));
                    ti.leave();
                }
                ti.set_focus(focused.cloned());
                if focused.is_some() {
                    ti.enter();
                    self.text_entered = focused.cloned();
                }
            }
        }
        // The focus gone from the window a field raised the keyboard for:
        // the field is left (text_field), unless the next window's field
        // takes the cursor within the half second.
        if self.kbd_auto && self.kbd.out() && self.kbd_window.as_ref().is_some_and(|w| Some(w.toplevel().unwrap().wl_surface()) != focused) {
            self.field_left_at.get_or_insert(hybris_hwc::now_ns());
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

/// xdg-activation (GTK's gtk_window_present, an app told to show itself by
/// its second instance or a call coming in): a window of the app's that
/// is put away comes back onto its panel; a shown one takes the keyboard.
/// Only the token's own client reaches its surfaces, as the protocol has it.
impl XdgActivationHandler for State {
    fn activation_state(&mut self) -> &mut XdgActivationState {
        &mut self.xdg_activation_state
    }

    fn request_activation(&mut self, token: XdgActivationToken, _token_data: XdgActivationTokenData, surface: WlSurface) {
        self.xdg_activation_state.remove_token(&token);
        // Locked: nothing comes up under the lock screen for a window's own
        // asking (a call lights the screen by calls.rs, not by this).
        if self.lock.locked {
            return;
        }
        if let Some((window, panel)) = self.put_away.iter().find(|(w, _)| w.toplevel().unwrap().wl_surface() == &surface).cloned() {
            let panel = if layout::turned() { 0 } else { panel };
            tracing::info!("activation: {} asks to be shown, brought back", app_id(&window));
            let ids = [app_id(&window)];
            self.bring_back(&ids, panel);
            return;
        }
        let shown = self.space.elements().find(|w| w.toplevel().unwrap().wl_surface() == &surface).map(app_id);
        if let Some(id) = shown {
            let keyboard = self.seat.get_keyboard().unwrap();
            keyboard.set_focus(self, Some(surface), smithay::utils::SERIAL_COUNTER.next_serial());
            tracing::info!("activation: {id} asks to be shown, has the keyboard");
        }
    }
}
delegate_xdg_activation!(State);
delegate_seat!(State);
delegate_data_device!(State);
delegate_output!(State);
delegate_layer_shell!(State);
delegate_input_method_manager!(State);
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

/// A warm launch (draft 49): when it was spawned, and when its first
/// window came.
#[derive(Clone, Copy)]
struct Warming {
    since: u64,
    arrived: Option<u64>,
    /// The dock asked for it meanwhile (launch): its window is shown, not put away.
    claimed: bool,
}

/// `s` as one word for sh: in single quotes, its own quotes closed, escaped
/// and reopened. For anything that goes into a `sh -c` line and came from
/// outside (a session's path, a name from another device).
pub fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// A window asked of the Chromium kept resident (session/item-chromium.service,
/// `--no-startup-window --keep-alive-for-test`) over its singleton socket:
/// "START\0<cwd>\0<argv>..." with no delimiter after the last, the write
/// side shut, "ACK" back (chrome/browser/process_singleton_posix.cc). A
/// second chromium process would load its 300 MB only to do the same: 0.67 s
/// to the window this way against 1.5-1.8 (2026-10-23). `exec` is the
/// desktop entry's command, its arguments handed on. Whether it answered.
fn resident_chromium(exec: &str) -> bool {
    use std::io::{Read, Write};
    use std::os::unix::ffi::OsStrExt;
    let Some(home) = std::env::var_os("HOME") else { return false };
    let config = std::env::var_os("XDG_CONFIG_HOME").map(std::path::PathBuf::from).unwrap_or_else(|| std::path::Path::new(&home).join(".config"));
    let socket = config.join("chromium/SingletonSocket");
    // The resident answers at once; absent (the seconds after login, its
    // restart), its service is started and the socket tried for 6 s.
    let mut stream = std::os::unix::net::UnixStream::connect(&socket);
    if stream.is_err() {
        let _ = std::process::Command::new("systemctl").args(["--user", "start", "item-chromium.service"]).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status();
        for _ in 0..30 {
            std::thread::sleep(std::time::Duration::from_millis(200));
            stream = std::os::unix::net::UnixStream::connect(&socket);
            if stream.is_ok() {
                break;
            }
        }
    }
    let Ok(mut s) = stream else { return false };
    let _ = s.set_read_timeout(Some(std::time::Duration::from_secs(5)));
    let _ = s.set_write_timeout(Some(std::time::Duration::from_millis(500)));
    let mut message = b"START\0".to_vec();
    message.extend_from_slice(home.as_bytes());
    message.extend_from_slice(b"\0chromium\0--new-window");
    for arg in exec.split_whitespace().skip(1) {
        message.push(0);
        message.extend_from_slice(arg.as_bytes());
    }
    if s.write_all(&message).is_err() {
        return false;
    }
    let _ = s.shutdown(std::net::Shutdown::Write);
    // Written and taken: asked. The ACK comes once the window is made
    // (half a second); SHUTDOWN means the resident is going.
    let mut reply = [0u8; 8];
    let n = s.read(&mut reply).unwrap_or(0);
    &reply[..n] != b"SHUTDOWN"
}

