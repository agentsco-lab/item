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

use crate::dock::Tap;
use crate::layout::LAYOUT;
use crate::state::State;
use crate::Data;

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

pub fn init(handle: &LoopHandle<'static, Data>) {
    let mut libinput = Libinput::new_from_path(OpenDirect);
    match find_touchscreen() {
        Some(path) => match libinput.path_add_device(&path) {
            Some(d) => tracing::info!("touch: {} ({path})", d.name()),
            None => tracing::warn!("touch: could not open {path}"),
        },
        None => tracing::warn!("touch: no touchscreen found"),
    }
    handle
        .insert_source(LibinputInputBackend::new(libinput), |event, _, data: &mut Data| data.state.on_input(event))
        .expect("libinput source");
}

impl State {
    fn on_input(&mut self, event: InputEvent<LibinputInputBackend>) {
        let Some(touch) = self.seat.get_touch() else { return };
        // Anything the finger does wakes the GPU's clock (boost.rs).
        if matches!(event, InputEvent::TouchDown { .. } | InputEvent::TouchMotion { .. } | InputEvent::TouchUp { .. }) {
            self.boost.kick(hybris_hwc::now_ns());
        }
        match event {
            InputEvent::TouchDown { event } => {
                let pos = event.position_transformed(LAYOUT.into());
                // A touch at a panel's top edge, or on its shade, is the shade's.
                if self.shade.down(event.slot(), pos.x, pos.y, event.time()) {
                    self.needs_redraw = true;
                    return;
                }
                if self.dock.down(event.slot(), pos) {
                    self.needs_redraw = true;
                    return;
                }
                // The outer edge of a panel with a window: back (back.rs).
                if let Some(panel) = crate::back::Back::strip_at(pos) {
                    if self.top_window(panel).is_some() && self.grid.panel() != Some(panel) {
                        self.back.down(event.slot(), pos, panel);
                        self.needs_redraw = true;
                        return;
                    }
                }
                // The bottom band of a panel with a window: the swipe that
                // puts it away (gesture.rs).
                if pos.y >= LAYOUT.1 as f64 - crate::gesture::EDGE {
                    if let Some(panel) = crate::layout::panel_at(pos) {
                        if let Some(window) = self.top_window(panel) {
                            self.gestures.down(event.slot(), pos, event.time(), window, panel);
                            return;
                        }
                    }
                }
                // The app grid, and an empty panel's swipes (grid.rs).
                let empty = crate::layout::panel_at(pos).is_some_and(|p| self.top_window(p).is_none());
                if self.grid.down(event.slot(), pos, empty) {
                    self.needs_redraw = true;
                    return;
                }
                let serial = SERIAL_COUNTER.next_serial();
                // The window under the finger comes up and takes the keyboard.
                if let Some(window) = self.space.element_under(pos).map(|(w, _)| w.clone()) {
                    self.space.raise_element(&window, true);
                    let surface = window.toplevel().unwrap().wl_surface().clone();
                    self.seat.get_keyboard().unwrap().set_focus(self, Some(surface), serial);
                }
                let under = self.surface_under(pos);
                self.touches += 1;
                // A touch no client answers within half a second is not waited for.
                let now = hybris_hwc::now_ns();
                if self.touch_pending.is_some_and(|t| now - t > 500_000_000) {
                    self.touch_pending = None;
                }
                self.touch_pending.get_or_insert(now);
                tracing::debug!("touch down {:?} at {:.0},{:.0}", event.slot(), pos.x, pos.y);
                touch.down(self, under, &DownEvent { slot: event.slot(), location: pos, serial, time: event.time_msec() });
            }
            InputEvent::TouchMotion { event } => {
                let pos = event.position_transformed(LAYOUT.into());
                if self.shade.holds(event.slot()) {
                    self.shade.motion(event.slot(), pos.y, event.time());
                    self.needs_redraw = true;
                    return;
                }
                if self.back.holds(event.slot()) {
                    self.back.motion(event.slot(), pos);
                    self.needs_redraw = true;
                    return;
                }
                if self.grid.holds(event.slot()) {
                    if let crate::grid::Ask::Shade(start) = self.grid.motion(event.slot(), pos, event.time()) {
                        self.shade.grab_from(event.slot(), start.x, start.y, event.time());
                        self.shade.motion(event.slot(), pos.y, event.time());
                    }
                    self.needs_redraw = true;
                    return;
                }
                if self.gestures.holds(event.slot()) {
                    self.gestures.motion(event.slot(), pos.y, event.time());
                    self.needs_redraw = true;
                    return;
                }
                if self.dock.holds(event.slot()) {
                    self.dock.motion(event.slot(), pos);
                    self.needs_redraw = true;
                    return;
                }
                let under = self.surface_under(pos);
                touch.motion(self, under, &MotionEvent { slot: event.slot(), location: pos, time: event.time_msec() });
            }
            InputEvent::TouchUp { event } => {
                if self.shade.holds(event.slot()) {
                    self.shade.up(event.slot());
                    self.needs_redraw = true;
                    return;
                }
                if self.back.holds(event.slot()) {
                    if let Some(panel) = self.back.up(event.slot()) {
                        self.go_back(panel);
                    }
                    self.needs_redraw = true;
                    return;
                }
                if self.grid.holds(event.slot()) {
                    if let crate::grid::Ask::Launch { exec, ids, icon, panel } = self.grid.up(event.slot()) {
                        let icon = icon.as_deref().and_then(|n| crate::apps::icon(n, crate::curtain::ICON));
                        self.launch(&exec, &ids, icon, panel);
                    }
                    self.needs_redraw = true;
                    return;
                }
                if self.gestures.holds(event.slot()) {
                    self.gestures.up(event.slot(), hybris_hwc::now_ns());
                    self.needs_redraw = true;
                    return;
                }
                if self.dock.holds(event.slot()) {
                    if let Some(Tap::Launch(command, panel, icon, ids)) = self.dock.up(event.slot()) {
                        self.launch(&command, &ids, icon, panel);
                    }
                    self.needs_redraw = true;
                    return;
                }
                touch.up(self, &UpEvent { slot: event.slot(), serial: SERIAL_COUNTER.next_serial(), time: event.time_msec() });
            }
            InputEvent::TouchFrame { .. } => touch.frame(self),
            InputEvent::TouchCancel { .. } => {
                self.shade.cancel();
                self.dock.cancel();
                self.gestures.cancel(hybris_hwc::now_ns());
                self.grid.cancel();
                self.back.cancel();
                self.needs_redraw = true;
                touch.cancel(self)
            }
            _ => {}
        }
    }
}
