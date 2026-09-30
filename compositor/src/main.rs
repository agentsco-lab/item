//! item-compositor: a Wayland compositor for the Surface Duo, drawing through
//! hwcomposer (libhybris' hwc2), on smithay. The base of item-shell.
//!
//! `item-compositor [--seconds N] [--spawn COMMAND]...`
//!
//! It runs in the user's session (see `tools/session-run.sh`), listens on a
//! Wayland socket in `$XDG_RUNTIME_DIR`, and spawns each COMMAND with
//! `WAYLAND_DISPLAY` set. Once a second it logs frames, the time between
//! presents, windows and touches (`RUST_LOG=item_compositor=debug` for each
//! touch).

mod input;
mod layout;
mod output;
mod state;

use std::sync::Arc;
use std::time::{Duration, Instant};

use hybris_hwc::{now_ns, take_stats};
use smithay::reexports::calloop::generic::Generic;
use smithay::reexports::calloop::{EventLoop, Interest, Mode as CalloopMode, PostAction};
use smithay::reexports::wayland_server::Display;
use smithay::wayland::socket::ListeningSocketSource;

use crate::output::Screen;
use crate::state::{ClientState, State};

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

    let mut event_loop: EventLoop<State> = EventLoop::try_new().expect("event loop");
    let display: Display<State> = Display::new().expect("display");
    let dh = display.handle();

    let mut screen = Screen::new(&dh);
    let _output_global = screen.output.create_global::<State>(&dh);

    let listening = ListeningSocketSource::new_auto().expect("wayland socket");
    let socket_name = listening.socket_name().to_os_string();
    let handle = event_loop.handle();
    handle
        .insert_source(listening, |stream, _, state: &mut State| {
            if let Err(e) = state.display_handle.insert_client(stream, Arc::new(ClientState::default())) {
                tracing::warn!("a client could not connect: {e}");
            }
        })
        .expect("socket source");
    handle
        .insert_source(Generic::new(display, Interest::READ, CalloopMode::Level), |_, display, state| {
            unsafe { display.get_mut().dispatch_clients(state).unwrap() };
            Ok(PostAction::Continue)
        })
        .expect("display source");
    input::init(&handle);

    let mut state = State::new(dh.clone(), event_loop.get_signal());
    state.space.map_output(&screen.output, (0, 0));
    tracing::info!("listening on {:?}", socket_name);

    for command in &args.spawn {
        // Clients take libhybris' Wayland EGL platform, not the compositor's
        // hwcomposer one. For now, in this session the portals and the
        // accessibility bus each hold a GTK app 25 s at start (a D-Bus
        // timeout); the clients are told to do without them until the session
        // provides both. CLIENT_ENV adds "KEY=VALUE ..." for tests.
        let child = std::process::Command::new("sh")
            .arg("-c")
            .arg(command)
            .env("WAYLAND_DISPLAY", &socket_name)
            .env("EGL_PLATFORM", "wayland")
            .env("GDK_DEBUG", "no-portals")
            .env("GTK_A11Y", "none")
            .env("NO_AT_BRIDGE", "1")
            .envs(std::env::var("CLIENT_ENV").ok().iter().flat_map(|e| {
                e.split_whitespace().filter_map(|kv| kv.split_once('=')).map(|(k, v)| (k.to_owned(), v.to_owned())).collect::<Vec<_>>()
            }))
            .spawn();
        match child {
            Ok(_) => tracing::info!("spawned: {command}"),
            Err(e) => tracing::warn!("could not spawn {command}: {e}"),
        }
    }

    let start = now_ns();
    let begin = Instant::now();
    let mut next_report = start + 1_000_000_000;
    loop {
        if let Some(s) = args.seconds {
            if (now_ns() - start) / 1_000_000_000 >= s {
                break;
            }
        }
        event_loop.dispatch(Some(Duration::ZERO), &mut state).expect("dispatch");

        screen.render(&state);
        screen.send_frames(&state, begin.elapsed());
        state.space.refresh();
        state.popups.cleanup();
        let _ = state.display_handle.flush_clients();

        let now = now_ns();
        if now >= next_report {
            let st = take_stats();
            tracing::info!(
                "{:3} fps  mean {:5.2} ms  max {:6.2} ms  over 20 ms {:2}  windows {}  touches {}  errors {}",
                st.frames, st.mean_ms, st.max_ms, st.over_20ms, state.space.elements().count(), state.touches, st.errors
            );
            next_report += 1_000_000_000;
        }
    }
    state.loop_signal.stop();
}
