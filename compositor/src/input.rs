//! Touch: the Duo's touchscreen through libinput's path interface, to the
//! window under the finger over `wl_touch`.
//!
//! The port's `sfduo-pen-split` grabs the raw digitizer (`surface_touchscreen`)
//! and gives the fingers a device of their own, `sfduo touchscreen`; that one
//! is taken when it is there. The touchscreen covers the output hinge and all,
//! so a position transformed to the layout is the logical point.

use std::os::fd::OwnedFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

use smithay::backend::input::{AbsolutePositionEvent, Event, InputEvent, TouchEvent};
use smithay::backend::libinput::LibinputInputBackend;
use smithay::input::touch::{DownEvent, MotionEvent, UpEvent};
use smithay::reexports::calloop::LoopHandle;
use smithay::reexports::input::{Libinput, LibinputInterface};
use smithay::utils::SERIAL_COUNTER;

use crate::layout::LAYOUT;
use crate::state::State;

/// libinput's path interface opens the device itself; the session's user is
/// in the device's group (android_input).
struct OpenDirect;

impl LibinputInterface for OpenDirect {
    fn open_restricted(&mut self, path: &Path, flags: i32) -> Result<OwnedFd, i32> {
        std::fs::OpenOptions::new()
            .read(true)
            .write(flags & 3 != 0)
            .custom_flags(flags)
            .open(path)
            .map(OwnedFd::from)
            .map_err(|e| e.raw_os_error().unwrap_or(-1))
    }

    fn close_restricted(&mut self, fd: OwnedFd) {
        drop(fd);
    }
}

/// The fingers' device: `$TOUCHSCREEN`, else the port's, else the raw one.
fn find_touchscreen() -> Option<String> {
    if let Ok(path) = std::env::var("TOUCHSCREEN") {
        return Some(path);
    }
    ["sfduo touchscreen", "surface_touchscreen"].iter().find_map(|want| {
        std::fs::read_dir("/sys/class/input").ok()?.flatten().find_map(|d| {
            let name = std::fs::read_to_string(d.path().join("device/name")).ok()?;
            let node = d.file_name().to_string_lossy().into_owned();
            (name.trim() == *want && node.starts_with("event")).then(|| format!("/dev/input/{node}"))
        })
    })
}

pub fn init(handle: &LoopHandle<'static, State>) {
    let mut libinput = Libinput::new_from_path(OpenDirect);
    match find_touchscreen() {
        Some(path) => match libinput.path_add_device(&path) {
            Some(d) => tracing::info!("touch: {} ({path})", d.name()),
            None => tracing::warn!("touch: could not open {path}"),
        },
        None => tracing::warn!("touch: no touchscreen found"),
    }
    handle
        .insert_source(LibinputInputBackend::new(libinput), |event, _, state: &mut State| state.on_input(event))
        .expect("libinput source");
}

impl State {
    fn on_input(&mut self, event: InputEvent<LibinputInputBackend>) {
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
                tracing::debug!("touch down {:?} at {:.0},{:.0}", event.slot(), pos.x, pos.y);
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
