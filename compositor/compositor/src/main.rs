//! item-compositor: a Wayland compositor for the Surface Duo, drawing through
//! hwcomposer (libhybris' hwc2), on smithay. The base of item-shell.
//!
//! `item-compositor [--seconds N] [--session COMMAND] [--spawn COMMAND]...`
//!
//! It runs in the user's session (see `tools/session-run.sh`), listens on
//! `wayland-item` in `$XDG_RUNTIME_DIR`, and spawns each COMMAND with
//! `WAYLAND_DISPLAY` set.
//!
//! `--session COMMAND` is the session's manager, as phoc's `-E`: started
//! once the socket is up (`gnome-session --session=item` from
//! `session/item-session`), and the compositor ends when it does. It starts
//! the on-screen keyboard itself then.
//!
//! Frames are paced by hwcomposer's vsync: the event loop sleeps until there
//! is something to do - a client, a touch, a vsync - and at a vsync draws a
//! frame only if something changed. The swap does not wait for the display,
//! so input and clients are served while a frame is on its way. Once a second
//! it logs frames drawn, vsyncs, time between presents, windows, touches, and
//! the time from a touch to the first frame showing a client's answer to it.

mod apps;
mod back;
mod boost;
mod clock;
mod curtain;
mod dock;
mod gesture;
mod glass;
mod grid;
mod input;
mod layers;
mod lock;
mod pinpad;
mod door;
mod logind;
mod fingerprint;
mod follow;
mod status;
mod keyring;
mod setup;
mod layout;
mod notify;
mod output;
mod pace;
mod pam;
mod pensheet;
mod protocols;
mod quick;
mod ribbon;
mod sched;
mod polkit;
mod alert;
mod calls;
mod dialog;
mod display;
mod keys;
mod nm;
mod idle;
mod sysfacts;
mod walls;
mod picker;
mod privacy;
mod shade;
mod sleep;
mod state;
mod sysscreen;
mod text;
mod tour;

use std::sync::Arc;
use std::time::Instant;

use hybris_hwc::{now_ns, take_stats, vsyncs};
use smithay::reexports::calloop::generic::Generic;
use smithay::reexports::calloop::ping::make_ping;
use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
use smithay::reexports::calloop::{EventLoop, Interest, LoopHandle, Mode as CalloopMode, PostAction};
use smithay::reexports::wayland_server::Display;
use smithay::wayland::socket::ListeningSocketSource;

use crate::output::Screen;
use crate::state::{ClientState, State};

/// The event loop's data: the Wayland state and the screen it is drawn on.
pub struct Data {
    /// The volume bar was up at the last frame.
    volume_bar_up: bool,
    /// FRAMES_DOCK's run was taken.
    frames_dock_done: bool,
    /// Whether the screen was lit, as last told to the system screen.
    lit_was: bool,
    /// Whether the screen was dimmed for idleness at the last look.
    dim_was: bool,
    /// A call lit the dark screen: 0 while it lasts, then when it ended.
    call_lit: Option<u64>,
    alert_was: bool,
    pub state: State,
    pub screen: Screen,
    started: Instant,
    report: Report,
    handle: LoopHandle<'static, Data>,
    pacing: Pacing,
    /// Swapped frames' presentation feedback, with the vsync each shows at.
    feedback: Vec<(u64, smithay::desktop::utils::OutputPresentationFeedback)>,
}

/// When in the frame to draw.
///
/// A frame drawn right at a vsync shows at the next one, and anything a client
/// commits after that vsync waits a whole frame more. Drawn late - at the next
/// vsync less what a frame takes and a margin for hwcomposer's present - it
/// takes in whatever came during the frame. Too late, and it misses the vsync
/// and shows a frame later: a stutter. `LATE_MARGIN_MS` sets the margin
/// (default 4), `LATE=0` draws at the vsync as before.
struct Pacing {
    late: bool,
    margin_ns: u64,
    /// A running average of the time a frame takes to render and hand over.
    render_ns: f64,
    /// The heaviest frames lately (a peak, slowly forgotten): a frame that
    /// redraws the screen after a rest costs much more than those before,
    /// and drawn by the average alone it misses its vsync.
    peak_ns: f64,
    /// The vsync the frame being drawn aims for.
    target_ns: u64,
    /// When the last frame went to hwcomposer.
    last_swap_ns: u64,
    /// Draw a client's new frame at once (`ASAP=0` waits for the vsync).
    asap: bool,
    /// When clients hear their frame was taken (`CALLBACKS`).
    callbacks: Callbacks,
    /// A frame went to hwcomposer; its clients hear of it at the vsync that
    /// puts it on screen (`Callbacks::Vsync`).
    callbacks_due: bool,
}

/// When frame callbacks go out.
#[derive(PartialEq, Clone, Copy, Debug)]
enum Callbacks {
    /// Each client when it suits it (`adaptive`, the default): the quick
    /// ones at the vsync, the rest as the frame is drawn (pace.rs).
    Adaptive,
    /// As the frame is drawn (`draw`, the default before step 23): a client starts its
    /// next frame while ours waits for the vsync, as under phoc. Settings
    /// scrolls at 60 fps with the GPU boosted (43 without). A client quicker
    /// than a frame commits while ours still waits, and its frame shows a
    /// vsync later: with the boost the calculator answers a tap in 36-37 ms,
    /// 28 without.
    Draw,
    /// At the vsync that puts the frame on screen (`vsync`): the client
    /// draws right after it, and its frame is drawn at once for the next.
    /// Taps 34 ms with the boost, but Settings, which needs 12-14 ms a frame
    /// even boosted, scrolls at 39-48 fps (26 without the boost).
    Vsync,
    /// After the frame is drawn and presented (`after`).
    After,
}

impl Pacing {
    fn from_env() -> Pacing {
        let late = std::env::var("LATE").map(|v| v != "0").unwrap_or(true);
        let margin_ms: f64 = std::env::var("LATE_MARGIN_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(4.0);
        Pacing { late, margin_ns: (margin_ms * 1e6) as u64, render_ns: 2e6, peak_ns: 0.0, target_ns: 0, last_swap_ns: 0, asap: std::env::var("ASAP").map(|v| v != "0").unwrap_or(true),
            callbacks: match std::env::var("CALLBACKS").as_deref() {
                Ok("vsync") => Callbacks::Vsync,
                Ok("after") => Callbacks::After,
                Ok("draw") => Callbacks::Draw,
                _ => Callbacks::Adaptive,
            },
            callbacks_due: false,
        }
    }

    fn budget_ns(&self) -> u64 {
        (self.render_ns * 1.5).max(self.peak_ns * 1.1) as u64 + self.margin_ns
    }

    /// Whether the last frame handed to hwcomposer is still waiting for a
    /// vsync to go on screen. Nothing is drawn meanwhile: a frame presented
    /// behind a waiting one queues, hwcomposer's present then blocks until
    /// the vsync (11-14 ms), and every frame after it is a frame late for
    /// good (step 5c). If vsyncs stop, 50 ms ends the wait.
    fn pending() -> bool {
        let presented = hybris_hwc::last_present_ns();
        presented != 0
            && presented >= hybris_hwc::last_vsync_ns()
            && hybris_hwc::now_ns().saturating_sub(presented) < 50_000_000
    }
}

/// What is logged once a second.
#[derive(Default)]
struct Report {
    drawn: u32,
    missed: u32,
    vsync_ticks: u32,
    vsyncs_at_last: u64,
    render_max_ms: f64,
    draw_ms: Vec<f64>,
    elements_ms: Vec<f64>,
    swap_ms: Vec<f64>,
    present_ms: Vec<f64>,
    touch_to_screen_ms: Vec<f64>,
    /// From a finger's move on the shade (the kernel's time) to the frame
    /// showing it.
    shade_ms: Vec<f64>,
    /// The same, from when the move reached the compositor.
    shade_loop_ms: Vec<f64>,
    /// Frames by buffer age.
    ages: std::collections::BTreeMap<usize, u32>,
    /// Share of the screen redrawn, %.
    damaged_share: Vec<f64>,
    /// Frames drawn with nothing damaged (no swap).
    no_damage: u32,
    /// Frames drawn at once for a client's commit.
    asap: u32,
}

/// "mean/max" of a second's samples.
fn mean_max(v: &[f64]) -> String {
    if v.is_empty() {
        return "-".into();
    }
    let mean = v.iter().sum::<f64>() / v.len() as f64;
    format!("{:.1}/{:.1}", mean, v.iter().cloned().fold(0.0, f64::max))
}

impl Data {
    /// At each vsync: aim the next frame at the next vsync, and draw it now
    /// or late in the frame.
    fn on_vsync(&mut self) {
        self.report.vsync_ticks += 1;
        let vsync = hybris_hwc::last_vsync_ns();
        let period = self.screen.vsync_period_ns;
        self.pacing.target_ns = vsync + period;
        // The frame presented before this vsync is on screen now.
        if self.pacing.callbacks_due && hybris_hwc::last_present_ns() < vsync {
            self.pacing.callbacks_due = false;
            let time = std::time::Duration::from_nanos(vsync.saturating_sub(self.screen.clock_origin_ns));
            self.screen.send_frames(&self.state, time, output::Group::All);
            let _ = self.state.display_handle.flush_clients();
        }
        // Callbacks::Adaptive: the quick clients hear at the vsync, once the
        // last frame is out, and their next is drawn at once (pace.rs).
        if self.pacing.callbacks == Callbacks::Adaptive && hybris_hwc::last_present_ns() < vsync {
            let time = std::time::Duration::from_nanos(vsync.saturating_sub(self.screen.clock_origin_ns));
            if self.screen.send_frames(&self.state, time, output::Group::Quick) {
                let _ = self.state.display_handle.flush_clients();
            }
        }
        // wp_presentation: the frames that were to show at this vsync are on
        // screen; their time is the vsync's (CLOCK_MONOTONIC, hwcomposer's).
        if self.feedback.first().is_some_and(|(at, _)| *at <= vsync + period as u64 / 2) {
            use smithay::reexports::wayland_protocols::wp::presentation_time::server::wp_presentation_feedback::Kind;
            let refresh = smithay::wayland::presentation::Refresh::fixed(std::time::Duration::from_nanos(period as u64));
            let seq = hybris_hwc::vsyncs();
            while self.feedback.first().is_some_and(|(at, _)| *at <= vsync + period as u64 / 2) {
                let (_, mut f) = self.feedback.remove(0);
                f.presented::<_, smithay::utils::Monotonic>(std::time::Duration::from_nanos(vsync), refresh, seq, Kind::Vsync);
            }
            let _ = self.state.display_handle.flush_clients();
        }
        // The first frame after a pause is drawn at once: the GPU wakes slowly,
        // and drawn late such a frame missed its vsync.
        let idle = vsync.saturating_sub(self.pacing.last_swap_ns) > period * 3 / 2;
        if !self.pacing.late || (idle && self.state.needs_redraw) {
            self.draw_if_needed();
            return;
        }
        let draw_at = self.pacing.target_ns.saturating_sub(self.pacing.budget_ns());
        let now = hybris_hwc::now_ns();
        if draw_at <= now {
            self.draw_if_needed();
            return;
        }
        let delay = std::time::Duration::from_nanos(draw_at - now);
        let _ = self.handle.insert_source(Timer::from_duration(delay), |_, _, data: &mut Data| {
            data.draw_if_needed();
            TimeoutAction::Drop
        });
    }

    /// hwcomposer sends no vsync before the first frame it is given, and may
    /// stop between frames: if something changed and no vsync came for 50 ms,
    /// draw anyway.
    fn on_watchdog(&mut self) {
        let now = hybris_hwc::now_ns();
        // The idle delay as the settings have it, none while something
        // holds the screen on; dimmed before it goes dark.
        let held = self.state.idle_held();
        // The tour: what the user did, and the next step when back.
        if self.state.tour.active() {
            use crate::ribbon::Page;
            let home = self.state.ribbon.at(self.state.ribbon.view);
            let seen = crate::tour::Seen {
                grid: self.state.grid.panel().is_some(),
                shade: self.state.shade.visible(),
                ribbon_moved: self.state.ribbon.moving(),
                ribbon_home: !self.state.ribbon.moving() && home == [Some(&Page::Desk(0)), Some(&Page::Desk(1))],
                carried: self.state.dock.carrying(),
            };
            if self.state.tour.watch(&seen, now) {
                self.state.needs_redraw = true;
            }
        }
        // Woken from sleep: by the power key, the screen lights (the press
        // was spent on waking the phone); a call lights its own; a packet
        // leaves it dark.
        if let Some(woken) = self.state.sleep.take_woken(now) {
            if woken == sleep::Woken::PowerKey && self.state.lock.blank && !self.state.lid_shut {
                self.state.lock.set_blank(false);
                self.state.needs_redraw = true;
            }
        }
        // An alarm ringing (alert.rs): the screen lit, held lit and awake
        // until it is answered or taken back.
        let urgent = self.state.notes.urgent();
        if self.state.alert.set(urgent.as_ref(), now) {
            if self.state.lock.blank && !self.state.lid_shut {
                self.state.lock.set_blank(false);
            }
        }
        if self.state.alert.holds_screen() {
            self.state.lock.last_touch_ns = now;
        }
        if urgent.is_some() != self.alert_was {
            self.alert_was = urgent.is_some();
            self.state.needs_redraw = true;
        }
        // Asleep when nothing needs it awake (sleep.rs).
        let playing = self.state.shade.quick.media.lock().unwrap().as_ref().is_some_and(|m| m.playing);
        let may = self.state.lock.locked && self.state.lock.blank && !self.state.calls.any() && !self.state.alert.holds_screen() && !playing && !self.state.setup.active && !self.state.idle_held();
        self.state.sleep.tick(now, may);
        // What gsd-power asks on the bus is not done: the screen is item's -
        // the lid, the power key, its own idleness (gsd lit it after every
        // resume, and darkened the unlocked screen after 15 s, as if locked).
        if let Some(dark) = self.state.display_config.take_asked() {
            if dark && !self.state.lock.blank {
                tracing::info!("display: gsd-power asked it dark: left lit");
            }
        }
        // Pages over their leaders (follow.rs): those asked to be shown
        // again, and windows of a follower not yet over theirs.
        if self.state.follow_pages() {
            self.state.needs_redraw = true;
        }
        // Calls: one coming in lights the screen and holds it lit while it
        // rings; locked, it shows over the lock screen.
        if let Some(rang) = self.state.calls.take(now) {
            if rang && self.state.lock.blank && !self.state.lid_shut {
                self.state.lock.set_blank(false);
                self.call_lit = Some(0);
            }
            self.state.needs_redraw = true;
        }
        // A call lit the dark screen and is over: dark again after 4 s if
        // nobody touched it (as the port's lid daemon did under phosh).
        match self.call_lit {
            Some(0) if !self.state.calls.any() => self.call_lit = Some(now),
            Some(t) if t > 0 => {
                if !self.state.lock.locked || self.state.lock.blank || self.state.calls.any() || self.state.lock.last_touch_ns > t {
                    self.call_lit = None;
                } else if now >= t + 4_000_000_000 {
                    tracing::info!("calls: over, the screen dark again");
                    self.state.lock.set_blank(true);
                    self.call_lit = None;
                }
            }
            _ => {}
        }
        if self.state.calls.ringing() {
            self.state.lock.last_touch_ns = now;
        }
        let shown = self.state.calls.holds_screen();
        self.state.calls.show(self.state.lock.locked, now);
        if shown != self.state.calls.holds_screen() {
            self.state.needs_redraw = true;
        }
        self.state.lock.idle_ns = if held { 0 } else { self.state.idle.delay_ns() };
        let dim = self.state.lock.dimming(now);
        if dim != self.dim_was {
            self.dim_was = dim;
            self.state.needs_redraw = true;
        }
        // The system screen samples the load only while the screen is lit;
        // lit, the kernel does not sleep (sleep.rs).
        let lit = !self.state.lock.blank;
        if lit != self.lit_was {
            self.lit_was = lit;
            self.state.sleep.awake(lit);
            self.state.display_config.set_dark(!lit);
            self.state.system.display(lit);
        }
        // A finger held still on a desk: the wallpaper's choosing.
        if !self.state.picker.is_open() && !self.state.lock.holds_screen() && !self.state.setup.holds_screen() {
            if let Some(slot) = self.state.grid.long_press(now) {
                let current = self.state.walls.names.iter().position(|n| *n == self.state.walls.current).unwrap_or(0);
                let count = self.state.walls.names.len();
                self.state.picker.open(slot, current, count);
                self.state.walls.ask_thumbs();
                crate::fingerprint::buzz("button-pressed");
                self.state.boost.kick(now);
                self.state.needs_redraw = true;
            }
        }
        // A finger held still on an app in the grid, or on the dock: the
        // icon is carried (dock.rs), onto the dock, along it or off it.
        if !self.state.lock.holds_screen() && !self.state.setup.holds_screen() {
            if let Some((slot, desktop, pos)) = self.state.grid.lift(now) {
                self.state.dock.carry_new(slot, &desktop, pos, now);
                self.state.boost.kick(now);
                self.state.needs_redraw = true;
            }
            // Held still in a window's bottom band: the window is carried.
            if let Some((slot, window, panel)) = self.state.gestures.long_press(now) {
                let x = crate::layout::panels()[panel].loc.x as f64 + crate::layout::panels()[panel].size.w as f64 / 2.0;
                self.state.carrying = Some((slot, window, panel, x));
                self.state.bottom_start = None;
                crate::fingerprint::buzz("button-pressed");
                self.state.needs_redraw = true;
            }
            if self.state.dock.long_press(now) {
                self.state.boost.kick(now);
                self.state.needs_redraw = true;
            }
        }
        self.state.lock.idle(now);
        self.state.boost.tick(now);
        if now.saturating_sub(hybris_hwc::last_vsync_ns()) > 50_000_000 {
            self.pacing.target_ns = now + self.screen.vsync_period_ns;
            self.draw_if_needed();
        }
    }

    /// A client committed a frame: draw it now, rather than at the next vsync,
    /// unless a frame is still waiting in hwcomposer. It shows at the same
    /// vsync either way (or one later, if it came too late for this one), but
    /// the client hears it was taken (the frame callback, sent as a frame is
    /// drawn) half a frame sooner on average, and starts its next one. A GTK app that takes 15-20 ms a
    /// frame scrolled at 30 fps waiting for our vsync, at 40 under phoc, which
    /// answers a commit in 3 ms. The shell's own motion still draws late, at
    /// the vsync, with the finger's latest place.
    fn on_client_frame(&mut self) {
        if std::env::var_os("LOG_TIMES").is_some() && self.state.needs_redraw {
            tracing::info!("times: client frame at {:.6}, pending {}", hybris_hwc::now_ns() as f64 / 1e9, Pacing::pending());
        }
        if !self.pacing.asap || !self.state.needs_redraw || Pacing::pending() {
            return;
        }
        let now = hybris_hwc::now_ns();
        let period = self.screen.vsync_period_ns;
        let last = hybris_hwc::last_vsync_ns();
        // vsyncs may have stopped while nothing was drawn: aim at the next
        // one on their grid.
        self.pacing.target_ns = last + (now.saturating_sub(last) / period + 1) * period;
        self.report.asap += 1;
        self.draw_if_needed();
    }

    /// What logind and the screen saver's callers asked (logind.rs), done.
    fn take_logind_asks(&mut self) {
        for ask in self.state.logind.take_asks() {
            let lock = &mut self.state.lock;
            match ask {
                logind::Ask::Lock => lock.lock_now(),
                logind::Ask::Unlock => lock.unlock(),
                logind::Ask::Blank(true) => {
                    lock.lock_now();
                    lock.set_blank(true);
                }
                logind::Ask::Blank(false) => lock.set_blank(false),
            }
            self.state.needs_redraw = true;
        }
        for event in self.state.fingerprint.take_events() {
            // The first setup's, while it is on.
            if self.state.setup.active {
                self.state.setup.reader(event);
                self.state.needs_redraw = true;
                continue;
            }
            match event {
                fingerprint::Event::Identified => self.state.lock.unlock(),
                fingerprint::Event::NotRecognized => self.state.lock.fingerprint_failed(),
                other => self.state.setup.reader(other),
            }
            self.state.needs_redraw = true;
        }
        // The first setup: its threads' answers; the PIN it settled on for
        // the keyring; its fingers for the lock's mark.
        if self.state.setup.poll(&self.state.fingerprint) {
            self.state.needs_redraw = true;
        }
        // The setup gone: the dock is born of its drop.
        if let Some((at, centre, r)) = self.state.setup.take_drop() {
            self.state.dock.born(at, centre, r);
            self.state.tour.start(at + 2_500_000_000);
            self.state.needs_redraw = true;
        }
        if let Some(pin) = self.state.setup.take_pin() {
            self.state.keyring.unlocked_with(pin);
        }
        if self.state.setup.active {
            let n = self.state.setup.fingers;
            self.state.lock.set_fingers(n);
        }
        // The keyring's prompts: the PIN that unlocked answers those waiting.
        if let Some(pin) = self.state.lock.take_verified_pin() {
            self.state.keyring.unlocked_with(pin);
        }
        self.state.keyring.service(self.state.lock.locked || self.state.setup.holds_screen());
        // polkit's asks: the dialog, once the phone is unlocked; the PIN's
        // answer; an ask taken back.
        if !self.state.lock.locked && !self.state.setup.holds_screen() && !self.state.dialog.is_open() {
            if let Some((message, _action)) = self.state.polkit.waiting() {
                self.state.asker = state::Asker::Polkit;
                self.state.dialog.ask_pin("Authentication required", &message);
                self.state.needs_redraw = true;
            } else if let Some((network, again)) = self.state.wifi.waiting() {
                // NetworkManager's: a Wi-Fi network's password.
                self.state.asker = state::Asker::Wifi;
                let hint = if again { "That password did not work. Try again." } else { "Tap the field to see what you type." };
                self.state.dialog.ask_text("Wi-Fi password", &format!("“{network}” needs a password to connect."), "Connect", hint);
                self.state.needs_redraw = true;
            }
        }
        if self.state.wifi.take_cancelled() && self.state.wifi.waiting().is_none() && self.state.asker == state::Asker::Wifi && self.state.dialog.is_open() {
            self.state.dialog.close();
            self.state.needs_redraw = true;
        }
        if let Some(ok) = self.state.polkit.take_checked() {
            if ok {
                self.state.dialog.close();
            } else {
                self.state.dialog.wrong();
            }
            self.state.needs_redraw = true;
        }
        if self.state.polkit.take_cancelled() && self.state.polkit.waiting().is_none() && self.state.asker == state::Asker::Polkit && self.state.dialog.is_open() {
            self.state.dialog.close();
            self.state.needs_redraw = true;
        }
        if let Some(facts) = self.state.status.take() {
            self.state.lock.set_status(&facts);
            self.state.shade.set_status(&facts);
            self.state.needs_redraw = true;
        }
        if self.state.lock.notice_done(hybris_hwc::now_ns()) {
            self.state.needs_redraw = true;
        }
        let (locked, blank) = (self.state.lock.locked, self.state.lock.blank);
        self.state.logind.report(locked, blank);
        // The reader listens while the phone is locked and lit.
        self.state.fingerprint.want(self.state.lock.wants_reader() || self.state.setup.wants_reader());
    }

    fn draw_if_needed(&mut self) {
        if !self.state.needs_redraw {
            return;
        }
        // A dark screen draws nothing; it is drawn again when lit.
        if self.state.lock.blank {
            return;
        }
        if self.state.lock.relit() {
            self.screen.reprime = 3;
        }
        if Pacing::pending() {
            return;
        }
        self.state.needs_redraw = false;
        let started = hybris_hwc::now_ns();
        self.state.follow_keyboard();
        // The dock stands on the panels whose pages are desks, nor under a
        // launch curtain or the grid; while the ribbon moves under a finger,
        // it moves with it between the pages on either side (dock.rs).
        let _ = started;
        self.state.place_dock(self.pacing.target_ns);
        // Callbacks::Draw: frame callbacks before the frame is drawn: the
        // clients' buffers for it are already taken, and a client starts on
        // its next frame while this one is drawn and presented (8-9 ms).
        // Sent after, a GTK app had 7 ms left before the next vsync, missed
        // it, and scrolled at 28 fps. Their time is the vsync the frame aims
        // for.
        if matches!(self.pacing.callbacks, Callbacks::Draw | Callbacks::Adaptive) {
            let time = std::time::Duration::from_nanos(self.pacing.target_ns.saturating_sub(self.screen.clock_origin_ns));
            let group = if self.pacing.callbacks == Callbacks::Draw { output::Group::All } else { output::Group::Slow };
            self.screen.send_frames(&self.state, time, group);
            let _ = self.state.display_handle.flush_clients();
        }
        let cost = self.screen.render(&self.state, self.pacing.target_ns);
        let (draw_ns, swap_ns) = (cost.draw_ns, cost.swap_ns);
        let took = draw_ns + swap_ns;
        *self.report.ages.entry(cost.age).or_default() += 1;
        self.report.damaged_share.push(cost.damaged_px as f64 / self.screen.pixels as f64 * 100.0);
        if !cost.swapped {
            self.report.no_damage += 1;
        }
        if cost.swapped {
            self.pacing.last_swap_ns = hybris_hwc::now_ns();
            // Only a frame that went to hwcomposer says what the next will
            // take (one with nothing damaged costs nothing), and not one of
            // more than two periods (the first, a stall). A slower frame
            // raises the estimate at once, faster ones lower it slowly: a
            // budget too short misses vsyncs.
            if took < 2 * self.screen.vsync_period_ns {
                let weight = if took as f64 > self.pacing.render_ns { 0.5 } else { 0.1 };
                self.pacing.render_ns += (took as f64 - self.pacing.render_ns) * weight;
                // The peak: up at once, down by 0.5 % a frame (half in some
                // 2.3 s of motion), never past the period.
                self.pacing.peak_ns = (self.pacing.peak_ns * 0.995).max(took as f64).min(self.screen.vsync_period_ns as f64 * 0.8);
            }
        }
        self.report.render_max_ms = self.report.render_max_ms.max(took as f64 / 1e6);
        self.report.draw_ms.push(draw_ns as f64 / 1e6);
        self.report.elements_ms.push(cost.elements_ns as f64 / 1e6);
        self.report.swap_ms.push(swap_ns as f64 / 1e6);
        if cost.swapped {
            self.report.present_ms.push(hybris_hwc::last_present_took_ns() as f64 / 1e6);
        }
        self.report.drawn += 1;

        // The frame shows at the vsync it aimed for, or one later if its
        // present came after that vsync.
        let target = self.pacing.target_ns;
        let presented = hybris_hwc::last_present_ns();
        let shown_at = if presented <= target {
            target
        } else {
            self.report.missed += 1;
            // Each missed frame, with where its time went.
            let d = hybris_hwc::last_present_detail();
            let ms = |ns: u64| ns as f64 / 1e6;
            tracing::info!(
                "missed: drawn {:.1} ms before the vsync, draw {:.1} swap {:.1}, presented {:.1} ms late: {:.1} after the last present, validate {:.1}, present {:.1}, GPU {}; redrawn {:.0}%, shade {}",
                target as f64 / 1e6 - started as f64 / 1e6, ms(draw_ns), ms(swap_ns), ms(presented - target),
                ms(d.since_last_ns), ms(d.validate_ns), ms(d.present_ns), if d.gpu_pending { "busy" } else { "done" },
                cost.damaged_px as f64 / self.screen.pixels as f64 * 100.0, self.state.shade.describe()
            );
            target + self.screen.vsync_period_ns
        };
        if cost.swapped {
            let feedback = self.screen.take_feedback(&self.state);
            self.feedback.push((shown_at, feedback));
        }
        if let Some(touched) = self.state.touch_answered.take() {
            self.report.touch_to_screen_ms.push(shown_at.saturating_sub(touched) as f64 / 1e6);
        }
        if let Some((moved_us, reached_ns)) = self.state.shade.take_moved() {
            let moved_ns = moved_us * 1000;
            // libinput's time is the kernel's CLOCK_MONOTONIC; skip it if not.
            if moved_ns <= reached_ns {
                self.report.shade_ms.push(shown_at.saturating_sub(moved_ns) as f64 / 1e6);
            }
            self.report.shade_loop_ms.push(shown_at.saturating_sub(reached_ns) as f64 / 1e6);
        }
        // hwcomposer does not show the first frame after it powers the
        // display on: with nothing moving, the screen stayed black. Every
        // buffer gets a frame at start.
        if !self.screen.primed() || self.screen.reprime > 0 {
            self.state.needs_redraw = true;
        }
        // FRAMES_DOCK: a run of frames from when a drop first reaches the
        // hinge, for looking at its crossing.
        if std::env::var_os("FRAMES_DOCK").is_some() && self.state.dock.at_hinge.get() && !self.frames_dock_done {
            self.frames_dock_done = true;
            self.screen.frames_left = 40;
        }
        if self.state.dock.settle(self.pacing.target_ns) {
            self.state.needs_redraw = true;
            self.state.boost.kick(hybris_hwc::now_ns());
        }
        let now = hybris_hwc::now_ns();
        for done in self.state.gestures.settle(self.pacing.target_ns) {
            let crate::gesture::Done::PutAway(window, panel) = done;
            self.state.put_window_away(window, panel);
        }
        if self.state.gestures.moving() {
            self.state.needs_redraw = true;
            self.state.boost.kick(now);
        }
        // Windows closing: frames until they are gone, then their last frames
        // forgotten with those of windows no longer there.
        self.state.closing.retain(|(_, since)| now < since + crate::output::CLOSE_NS);
        if !self.state.closing.is_empty() {
            self.state.needs_redraw = true;
        }
        let mut keep: Vec<_> = self.state.closing.iter().map(|(id, _)| id.clone()).collect();
        keep.extend(self.state.space.elements().map(|w| smithay::reexports::wayland_server::Resource::id(w.toplevel().unwrap().wl_surface())));
        self.screen.forget(&keep);
        // The volume bar: frames while it fades, one more when it is gone.
        match self.state.shade.quick.volume_bar_state(now) {
            (true, true) => self.state.needs_redraw = true,
            (true, false) => self.volume_bar_up = true,
            (false, _) if std::mem::take(&mut self.volume_bar_up) => self.state.needs_redraw = true,
            _ => {}
        }
        // The dock's halves come in from the sides after an unlock.
        if let Some(t) = self.state.lock.take_unlocked() {
            self.state.dock.rise(t + 250_000_000);
        }
        // A notification's banner slides in and out.
        if let Some(n) = self.state.notes.banner(now) {
            let age = now.saturating_sub(n.at_ns);
            if age < 300_000_000 || age + 300_000_000 > crate::notify::BANNER_NS {
                self.state.needs_redraw = true;
            }
        }
        if self.state.setup.settle(self.pacing.target_ns) {
            self.state.needs_redraw = true;
            self.state.boost.kick(now);
        }
        if self.state.lock.settle(self.pacing.target_ns) {
            self.state.needs_redraw = true;
        }
        // The ribbon: frames while it moves; where it stopped, applied.
        if self.state.ribbon.settle(self.pacing.target_ns) {
            self.state.needs_redraw = true;
            self.state.boost.kick(now);
        }
        // The setup's drop flowing on (over the hinge, its wet spot drying).
        if self.state.setup.active && self.state.dock.lone_moving(self.pacing.target_ns) {
            self.state.needs_redraw = true;
        }
        if self.state.dialog.settle(self.pacing.target_ns) || self.state.calls.settle(self.pacing.target_ns) || self.state.alert.settle(self.pacing.target_ns) || self.state.layers.sliding() || self.state.tour.settle() {
            self.state.needs_redraw = true;
        }
        let count = self.state.walls.names.len();
        if self.state.picker.settle(self.pacing.target_ns, count) {
            self.state.needs_redraw = true;
            self.state.boost.kick(now);
        }
        if self.state.clock.fading(now) || self.screen.wall_fading(now) {
            self.state.needs_redraw = true;
        }
        if self.state.ribbon.unsettled && !self.state.ribbon.moving() {
            self.state.apply_ribbon();
        }
        if self.state.system.settle(self.pacing.target_ns) {
            self.state.needs_redraw = true;
            self.state.boost.kick(now);
        }
        if self.state.pen.settle(self.pacing.target_ns) {
            self.state.needs_redraw = true;
            self.state.boost.kick(now);
        }
        if self.state.grid.settle(self.pacing.target_ns) {
            self.state.needs_redraw = true;
            self.state.boost.kick(now);
        }
        if self.state.curtain.settle(self.pacing.target_ns) {
            self.state.needs_redraw = true;
            self.state.boost.kick(hybris_hwc::now_ns());
        }
        if self.state.shade.settle(self.pacing.target_ns) {
            self.state.needs_redraw = true;
            self.state.boost.kick(hybris_hwc::now_ns());
        }
        // Nothing went to hwcomposer (nothing changed): no vsync will show
        // it, so the clients hear now.
        if self.pacing.callbacks == Callbacks::After || (self.pacing.callbacks == Callbacks::Vsync && !cost.swapped) {
            let time = std::time::Duration::from_nanos(shown_at.saturating_sub(self.screen.clock_origin_ns));
            self.screen.send_frames(&self.state, time, output::Group::All);
            let _ = self.state.display_handle.flush_clients();
        } else if self.pacing.callbacks == Callbacks::Vsync {
            self.pacing.callbacks_due = true;
        }
        let (locked, blank) = (self.state.lock.locked, self.state.lock.blank);
        self.state.logind.report(locked, blank);
        self.state.fingerprint.want(self.state.lock.wants_reader() || self.state.setup.wants_reader());
        self.state.paces.busy_until(hybris_hwc::now_ns());
    }

    fn log_report(&mut self) {
        let st = take_stats();
        let v = vsyncs();
        let r = std::mem::take(&mut self.report);
        let lat = &r.touch_to_screen_ms;
        let lat_text = if lat.is_empty() {
            String::from("-")
        } else {
            let mean = lat.iter().sum::<f64>() / lat.len() as f64;
            let max = lat.iter().cloned().fold(0.0, f64::max);
            format!("{} mean {:.1} max {:.1} ms", lat.len(), mean, max)
        };
        tracing::info!(
            "drawn {:3} of {:3}  missed {:2}  commits {:3} (at once {})  draw {} (elements {}) swap {} (present {}, fast {}) ms  redrawn {}%  no damage {}  budget {:4.1}  touches {}  touch->screen {}  shade->screen {} (from the loop {}) ms  GPU boost {:.0}%  errors {}",
            r.drawn, v - r.vsyncs_at_last, r.missed, std::mem::take(&mut self.state.commits), r.asap,
            mean_max(&r.draw_ms), mean_max(&r.elements_ms), mean_max(&r.swap_ms), mean_max(&r.present_ms), st.fast, mean_max(&r.damaged_share),
            r.no_damage, self.pacing.budget_ns() as f64 / 1e6, self.state.touches, lat_text,
            mean_max(&r.shade_ms), mean_max(&r.shade_loop_ms),
            self.state.boost.take_share(hybris_hwc::now_ns(), 1_000_000_000), st.errors
        );
        self.report.vsyncs_at_last = v;
    }
}

struct Args {
    seconds: Option<u64>,
    session: Option<String>,
    spawn: Vec<String>,
}

fn args() -> Args {
    let mut a = Args { seconds: None, session: None, spawn: Vec::new() };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--seconds" => a.seconds = it.next().and_then(|s| s.parse().ok()),
            "--spawn" => a.spawn.extend(it.next()),
            "--session" => a.session = it.next(),
            other => eprintln!("unknown argument: {other}"),
        }
    }
    a
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn,item_compositor=info")),
        )
        .init();
    sleep::prepare();
    let args = args();
    // This thread is the loop: on the big cores, ahead of the apps.
    sched::favour_this_thread();

    let mut event_loop: EventLoop<Data> = EventLoop::try_new().expect("event loop");
    let display: Display<State> = Display::new().expect("display");
    let dh = display.handle();

    let screen = Screen::new(&dh);

    // A fixed name, so the session can hand it to the portals before we run.
    let listening = ListeningSocketSource::with_name("wayland-item").expect("wayland socket");
    let socket_name = listening.socket_name().to_os_string();
    let handle = event_loop.handle();
    handle
        .insert_source(listening, |stream, _, data: &mut Data| {
            if let Err(e) = data.state.display_handle.insert_client(stream, Arc::new(ClientState::default())) {
                tracing::warn!("a client could not connect: {e}");
            }
        })
        .expect("socket source");
    handle
        .insert_source(Generic::new(display, Interest::READ, CalloopMode::Level), |_, display, data: &mut Data| {
            unsafe { display.get_mut().dispatch_clients(&mut data.state).unwrap() };
            if std::mem::take(&mut data.state.client_frame) {
                data.on_client_frame();
            }
            Ok(PostAction::Continue)
        })
        .expect("display source");
    input::init(&handle);

    // hwcomposer's vsync, on its own thread, wakes the loop.
    let (ping, vsync_source) = make_ping().expect("ping");
    hybris_hwc::set_vsync_handler(move |_| ping.ping());
    handle.insert_source(vsync_source, |_, _, data: &mut Data| data.on_vsync()).expect("vsync source");

    handle
        .insert_source(Timer::from_duration(std::time::Duration::from_millis(50)), |_, _, data: &mut Data| {
            data.on_watchdog();
            // The screen dark: once a second is enough (it woke the phone 20
            // times a second all night).
            TimeoutAction::ToDuration(std::time::Duration::from_millis(if data.state.lock.blank { 1000 } else { 50 }))
        })
        .expect("watchdog timer");
    handle
        .insert_source(Timer::from_duration(std::time::Duration::from_secs(1)), |_, _, data: &mut Data| {
            data.log_report();
            // The lock's state to logind, also while the screen is dark (no
            // frames then).
            data.take_logind_asks();
            // `touch /tmp/item-shot` asks for a screenshot of the next frame.
            if std::fs::remove_file("/tmp/item-shot").is_ok() {
                data.screen.shot = Some(format!("/tmp/item-shot-{}.rgba", data.started.elapsed().as_secs()));
                data.state.needs_redraw = true;
            }
            // `touch /tmp/item-frames` asks for the next 40 frames
            // (FRAMES_COUNT).
            if std::fs::remove_file("/tmp/item-frames").is_ok() {
                data.screen.frames_left = std::env::var("FRAMES_COUNT").ok().and_then(|n| n.parse().ok()).unwrap_or(40);
            }
            // `touch /tmp/item-birth`: the dock born of a drop where the
            // setup's last one stands, for tests.
            if std::fs::remove_file("/tmp/item-birth").is_ok() {
                let left = crate::layout::panels()[0];
                data.state.dock.born(hybris_hwc::now_ns(), (left.loc.x as f64 + left.size.w as f64 / 2.0, 250.0), 36.0);
                data.state.needs_redraw = true;
            }
            if data.state.shade.visible() && data.state.shade.refresh_text() {
                data.state.needs_redraw = true;
            }
            // For tests: `touch /tmp/item-rise` is the dock's rise after an
            // unlock.
            if std::fs::remove_file("/tmp/item-rise").is_ok() {
                data.state.dock.rise(hybris_hwc::now_ns());
                data.state.needs_redraw = true;
            }
            // `touch /tmp/item-stroke` draws a wave with the pen's pressure
            // rising along it on the open sheet, for tests.
            if std::fs::remove_file("/tmp/item-stroke").is_ok() {
                data.state.pen.test_stroke();
                data.state.needs_redraw = true;
            }
            // `touch /tmp/item-power` is the power key, for tests.
            if std::fs::remove_file("/tmp/item-power").is_ok() {
                data.state.lock.power_key();
                data.state.needs_redraw = true;
            }
            if data.state.lock.relit() {
                data.screen.reprime = 3;
                data.state.needs_redraw = true;
            }
            if data.state.lock.locked && data.state.lock.refresh() {
                data.state.needs_redraw = true;
            }
            // Locked and lit: the last notifications and what is playing,
            // the player read again every few seconds.
            if data.state.lock.locked && !data.state.lock.blank {
                let mut list = data.state.notes.list();
                list.sort_by(|a, b| b.at_ns.cmp(&a.at_ns).then(b.id.cmp(&a.id)));
                let notes: Vec<(String, String)> = list.iter().map(|n| (n.app.clone(), n.summary.clone())).collect();
                let media = data.state.shade.quick.media.lock().unwrap().clone().map(|m| (m.title, m.artist, m.playing));
                if data.state.lock.set_notes(notes) | data.state.lock.set_media(media) {
                    data.state.needs_redraw = true;
                }
                if data.started.elapsed().as_secs() % 3 == 0 {
                    data.state.shade.quick.read();
                }
            }
            // The system screen's page is drawn on this thread (some 8 ms):
            // not while the ribbon moves, which would miss a frame for it.
            if !data.state.ribbon.moving() && data.state.system.refresh() {
                data.screen.warm_pages(&data.state);
                data.state.needs_redraw = true;
            }
            if data.state.clock.refresh() {
                data.state.needs_redraw = true;
            }
            let weather = data.state.system.weather();
            if data.state.clock.set_weather(weather) {
                data.state.needs_redraw = true;
            }
            // A call's time talked.
            if data.state.calls.tick(hybris_hwc::now_ns()) {
                data.state.needs_redraw = true;
            }
            // A banner up, or just gone: a frame, so it goes.
            if data.state.shade.quick.volume_bar_state(hybris_hwc::now_ns()).0 || data.volume_bar_up {
                data.state.needs_redraw = true;
            }
            if data.state.notes.banner(hybris_hwc::now_ns()).is_some() || data.state.shade.quick.banner_at.get().is_some() {
                data.state.needs_redraw = true;
            }
            if let Some(s) = data.state.stop_after {
                if data.started.elapsed().as_secs() >= s {
                    data.state.loop_signal.stop();
                }
            }
            TimeoutAction::ToDuration(std::time::Duration::from_secs(1))
        })
        .expect("report timer");

    // The quick settings' thread wakes the loop when it has read the system.
    let (wake, wake_source) = make_ping().expect("ping");
    handle
        .insert_source(wake_source, |_, _, data: &mut Data| {
            data.state.lock.poll();
            data.take_logind_asks();
            // A wallpaper decoded, to the GPU; the picker's small pictures.
            if let Some(picture) = data.state.walls.take_loaded() {
                data.screen.set_wallpaper(picture);
            }
            if data.state.walls.take_thumbs() {
                data.state.needs_redraw = true;
            }
            if !data.state.ribbon.moving() && data.state.system.refresh() {
                data.screen.warm_pages(&data.state);
            }
            data.state.needs_redraw = true;
            data.on_client_frame();
        })
        .expect("wake source");
    let mut state = State::new(dh.clone(), event_loop.get_signal(), wake);
    // The output's global after xdg-output's manager (made in State::new):
    // phosh-osk-stevia asks the manager for each wl_output as it is
    // announced, and crashed on a null manager when the output came first.
    let _output_global = screen.output.create_global::<State>(&dh);
    state.stop_after = args.seconds;
    state.space.map_output(&screen.output, (0, 0));
    tracing::info!("listening on {:?}", socket_name);

    state.socket_name = socket_name.clone();
    // The on-screen keyboard, the port's (layers.rs): its user unit, which
    // phosh's session starts, restarted now that our socket is up - it takes
    // the session's WAYLAND_DISPLAY, ours for the run. A second stevia of our
    // own fought it for the input method (the second one gets "unavailable").
    // No unit: stevia started directly. NO_OSK=1 leaves it out.
    // Under a session manager the keyboard is the session's (its target).
    if std::env::var_os("NO_OSK").is_none() && args.session.is_none() {
        state.spawn("systemctl --user restart mobi.phosh.OSK.service 2>/dev/null || exec phosh-osk-stevia --replace");
    }
    // A real session starts locked, as phosh's does after a boot: the first
    // PIN also opens the login keyring (PAM's phosh service).
    let fingers = state.fingerprint.fingers;
    state.lock.set_fingers(fingers);
    // A real session: the first setup if it was not done, else locked as
    // after a boot.
    if args.session.is_some() && setup::wanted() {
        state.setup.start(fingers);
    } else if args.session.is_some() && !lock::opened_since_boot() {
        state.lock.after_boot();
    } else if args.session.is_some() {
        // A session again since the phone was opened with the PIN: locked,
        // the reader there as for any lock.
        state.lock.lock_now();
    } else if std::env::var_os("SETUP").is_some() || std::env::var_os("SETUP_DRY").is_some() {
        state.setup.start(fingers);
    }
    // The session's manager: when it ends (a log out, or it failed), so
    // does the compositor, and the unit that started it decides what next.
    if let Some(command) = &args.session {
        if let Some(mut child) = state.spawn_child(command) {
            let (tx, rx) = smithay::reexports::calloop::channel::channel::<std::process::ExitStatus>();
            std::thread::spawn(move || {
                if let Ok(status) = child.wait() {
                    let _ = tx.send(status);
                }
            });
            handle
                .insert_source(rx, |event, _, data: &mut Data| {
                    if let smithay::reexports::calloop::channel::Event::Msg(status) = event {
                        tracing::info!("session: its manager ended ({status}); ending");
                        data.state.loop_signal.stop();
                    }
                })
                .expect("session watch");
        }
    }
    for command in &args.spawn {
        state.spawn(command);
    }

    let pacing = Pacing::from_env();
    tracing::info!(
        "pacing: {}, margin {:.1} ms, clients' frames {}, frame callbacks {:?}",
        if pacing.late { "late in the frame" } else { "at the vsync" },
        pacing.margin_ns as f64 / 1e6,
        if pacing.asap { "at once" } else { "at the vsync" },
        pacing.callbacks
    );
    let mut data = Data {
        volume_bar_up: false, frames_dock_done: false, lit_was: false, dim_was: false, call_lit: None, alert_was: false, state, screen, started: Instant::now(), report: Report::default(), handle: handle.clone(), pacing, feedback: Vec::new() };
    data.report.vsyncs_at_last = vsyncs();
    let _ = now_ns();
    // The shade's text goes to the GPU now, not at the first pull.
    data.screen.warm_up(&data.state);
    // The system screen's page read and drawn now, at rest: the first swipe
    // to it does not wait for it.
    data.state.system.ready();
    data.state.pen.ready();
    state::check_scopes();
    // DIALOG_TEST=pin|text: the system's dialog up, to look at it (its
    // answers go nowhere).
    match std::env::var("DIALOG_TEST").as_deref() {
        Ok("pin") => data.state.dialog.ask_pin("Authentication required", "Authentication is required to change the system's settings."),
        Ok("text") => data.state.dialog.ask_text("Wi-Fi password", "“Home network” needs a password to connect.", "Connect", "Tap the field to see what you type."),
        _ => {}
    }
    // TOUR=1: the tour, as after the setup.
    if std::env::var_os("TOUR").is_some() {
        data.state.tour.start(hybris_hwc::now_ns() + 2_000_000_000);
    }
    // CALL_TEST=5 (incoming) or 1 (active): the phone locked and a call over
    // it, to look at it (its buttons go to no call).
    if let Some(n) = std::env::var("CALL_TEST").ok().and_then(|v| v.parse::<u32>().ok()) {
        if std::env::var_os("CALL_UNLOCKED").is_none() { data.state.lock.lock_now(); }
        data.state.calls.pretend(n, hybris_hwc::now_ns());
    }
    data.screen.warm_pages(&data.state);
    // The first frame, which starts hwcomposer's vsyncs.
    data.draw_if_needed();
    event_loop
        .run(None, &mut data, |data| {
            data.state.space.refresh();
            data.state.popups.cleanup();
            let _ = data.state.display_handle.flush_clients();
        })
        .expect("event loop");
}
