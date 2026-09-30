//! item-compositor: a Wayland compositor for the Surface Duo, drawing through
//! hwcomposer (libhybris' hwc2), on smithay. The base of item-shell.
//!
//! `item-compositor [--seconds N] [--spawn COMMAND]...`
//!
//! It runs in the user's session (see `tools/session-run.sh`), listens on
//! `wayland-item` in `$XDG_RUNTIME_DIR`, and spawns each COMMAND with
//! `WAYLAND_DISPLAY` set.
//!
//! Frames are paced by hwcomposer's vsync: the event loop sleeps until there
//! is something to do - a client, a touch, a vsync - and at a vsync draws a
//! frame only if something changed. The swap does not wait for the display,
//! so input and clients are served while a frame is on its way. Once a second
//! it logs frames drawn, vsyncs, time between presents, windows, touches, and
//! the time from a touch to the first frame showing a client's answer to it.

mod input;
mod layout;
mod output;
mod state;

use std::sync::Arc;
use std::time::Instant;

use hybris_hwc::{now_ns, take_stats, vsyncs};
use smithay::reexports::calloop::generic::Generic;
use smithay::reexports::calloop::ping::make_ping;
use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
use smithay::reexports::calloop::{EventLoop, Interest, Mode as CalloopMode, PostAction};
use smithay::reexports::wayland_server::Display;
use smithay::wayland::socket::ListeningSocketSource;

use crate::output::Screen;
use crate::state::{ClientState, State};

/// The event loop's data: the Wayland state and the screen it is drawn on.
pub struct Data {
    pub state: State,
    pub screen: Screen,
    started: Instant,
    report: Report,
}

/// What is logged once a second.
#[derive(Default)]
struct Report {
    drawn: u32,
    vsync_ticks: u32,
    vsyncs_at_last: u64,
    touch_to_screen_ms: Vec<f64>,
}

impl Data {
    /// At each vsync: draw a frame if something changed since the last one.
    fn on_vsync(&mut self) {
        self.report.vsync_ticks += 1;
        self.draw_if_needed();
    }

    /// hwcomposer sends no vsync before the first frame it is given, and may
    /// stop between frames: if something changed and no vsync came for 50 ms,
    /// draw anyway.
    fn on_watchdog(&mut self) {
        let since_vsync = hybris_hwc::now_ns().saturating_sub(hybris_hwc::last_vsync_ns());
        if since_vsync > 50_000_000 {
            self.draw_if_needed();
        }
    }

    fn draw_if_needed(&mut self) {
        if !self.state.needs_redraw {
            return;
        }
        self.state.needs_redraw = false;
        self.screen.render(&self.state);
        self.report.drawn += 1;

        // The frame goes up at the next vsync: that is the time clients get
        // with their frame callbacks, and the end of a touch's round trip.
        let shown_at = hybris_hwc::last_vsync_ns() + self.screen.vsync_period_ns;
        if let Some(touched) = self.state.touch_answered.take() {
            self.report.touch_to_screen_ms.push(shown_at.saturating_sub(touched) as f64 / 1e6);
        }
        let time = std::time::Duration::from_nanos(shown_at.saturating_sub(self.screen.clock_origin_ns));
        self.screen.send_frames(&self.state, time);
        let _ = self.state.display_handle.flush_clients();
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
            "drawn {:3} of {:3} vsyncs  commits {:3}  presents {:3}  max between {:6.2} ms  windows {}  touches {}  touch->screen {}  errors {}",
            r.drawn, v - r.vsyncs_at_last, std::mem::take(&mut self.state.commits), st.frames, st.max_ms,
            self.state.space.elements().count(), self.state.touches, lat_text, st.errors
        );
        self.report.vsyncs_at_last = v;
    }
}

struct Args {
    seconds: Option<u64>,
    spawn: Vec<String>,
}

fn args() -> Args {
    let mut a = Args { seconds: None, spawn: Vec::new() };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--seconds" => a.seconds = it.next().and_then(|s| s.parse().ok()),
            "--spawn" => a.spawn.extend(it.next()),
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
    let args = args();

    let mut event_loop: EventLoop<Data> = EventLoop::try_new().expect("event loop");
    let display: Display<State> = Display::new().expect("display");
    let dh = display.handle();

    let screen = Screen::new(&dh);
    let _output_global = screen.output.create_global::<State>(&dh);

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
            TimeoutAction::ToDuration(std::time::Duration::from_millis(50))
        })
        .expect("watchdog timer");
    handle
        .insert_source(Timer::from_duration(std::time::Duration::from_secs(1)), |_, _, data: &mut Data| {
            data.log_report();
            if let Some(s) = data.state.stop_after {
                if data.started.elapsed().as_secs() >= s {
                    data.state.loop_signal.stop();
                }
            }
            TimeoutAction::ToDuration(std::time::Duration::from_secs(1))
        })
        .expect("report timer");

    let mut state = State::new(dh.clone(), event_loop.get_signal());
    state.stop_after = args.seconds;
    state.space.map_output(&screen.output, (0, 0));
    tracing::info!("listening on {:?}", socket_name);

    for command in &args.spawn {
        // Clients take libhybris' Wayland EGL platform, not the compositor's
        // hwcomposer one, and our desktop name. CLIENT_ENV adds "KEY=VALUE ..."
        // for tests.
        let child = std::process::Command::new("sh")
            .arg("-c")
            .arg(command)
            .env("WAYLAND_DISPLAY", &socket_name)
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

    let mut data = Data { state, screen, started: Instant::now(), report: Report::default() };
    data.report.vsyncs_at_last = vsyncs();
    let _ = now_ns();
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
