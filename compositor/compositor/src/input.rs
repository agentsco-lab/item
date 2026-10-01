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

use smithay::backend::input::{
    AbsolutePositionEvent, Event, InputEvent, KeyState, KeyboardKeyEvent, Switch, SwitchState, SwitchToggleEvent,
    ButtonState, TabletToolButtonEvent, TabletToolEvent, TabletToolTipEvent, TabletToolTipState, TabletToolType,
    TouchEvent, TouchSlot,
};
use smithay::utils::{Logical, Point};
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

/// An input device by its name, as a node.
fn find(name: &str) -> Option<String> {
    std::fs::read_dir("/sys/class/input").ok()?.flatten().find_map(|d| {
        let n = std::fs::read_to_string(d.path().join("device/name")).ok()?;
        let node = d.file_name().to_string_lossy().into_owned();
        (n.trim() == name && node.starts_with("event")).then(|| format!("/dev/input/{node}"))
    })
}

pub fn init(handle: &LoopHandle<'static, Data>) {
    let mut libinput = Libinput::new_from_path(OpenDirect);
    // The power key and the volume keys, the lid (lock.rs), the pen.
    for name in ["qpnp_pon", "gpio-keys", "Surface Duo Lid Switch", "sfduo pen"] {
        match find(name).and_then(|p| libinput.path_add_device(&p).map(|_| p)) {
            Some(p) => tracing::info!("keys: {name} ({p})"),
            None => tracing::warn!("keys: no {name}"),
        }
    }
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

/// A contact on the screen, from a finger or the pen: the pen is a touch of
/// its own slot, so it taps, drags and scrolls as a finger does.
enum Contact {
    Down(TouchSlot, Point<f64, Logical>, u64),
    Motion(TouchSlot, Point<f64, Logical>, u64),
    Up(TouchSlot, u64),
    Frame,
    Cancel,
}

/// The pen's slot, clear of the fingers' (the touchscreen has ten).
fn pen_slot() -> TouchSlot {
    TouchSlot::from(Some(31u32))
}

impl State {
    /// A finger takes the ribbon at x: the pages that may come are made
    /// ready to be drawn.
    fn ribbon_grab(&mut self, slot: TouchSlot, x: f64, time: u64) {
        self.system.ready();
        self.pen.ready();
        self.ribbon.grab(slot, x, time);
        self.needs_redraw = true;
    }

    fn on_input(&mut self, event: InputEvent<LibinputInputBackend>) {
        let contact = match event {
            // The keys: power, volume (evdev codes, +8 for xkb).
            InputEvent::Keyboard { event } => {
                if event.state() == KeyState::Pressed {
                    match event.key_code().raw().saturating_sub(8) {
                        116 => self.lock.power_key(),
                        115 => self.shade.quick.volume_step(true),
                        114 => self.shade.quick.volume_step(false),
                        _ => {}
                    }
                    self.needs_redraw = true;
                }
                return;
            }
            // The lid: the Duo folded shut goes dark and locks, as logind
            // was told to (HandleLidSwitch=lock); opened, it is lit, locked.
            InputEvent::SwitchToggle { event } => {
                if SwitchToggleEvent::<LibinputInputBackend>::switch(&event) == Some(Switch::Lid) {
                    let shut = SwitchToggleEvent::<LibinputInputBackend>::state(&event) == SwitchState::On;
                    tracing::info!("lid: {}", if shut { "shut" } else { "open" });
                    if shut {
                        self.lock.lock_now();
                        self.lock.set_blank(true);
                    } else {
                        self.lock.set_blank(false);
                    }
                    self.needs_redraw = true;
                }
                return;
            }
            InputEvent::TouchDown { event } => Contact::Down(event.slot(), event.position_transformed(LAYOUT.into()), event.time()),
            InputEvent::TouchMotion { event } => Contact::Motion(event.slot(), event.position_transformed(LAYOUT.into()), event.time()),
            InputEvent::TouchUp { event } => Contact::Up(event.slot(), event.time()),
            InputEvent::TouchFrame { .. } => Contact::Frame,
            InputEvent::TouchCancel { .. } => Contact::Cancel,
            // The pen: its tip down, moved, up.
            InputEvent::TabletToolTip { event } => {
                let pos = event.position_transformed(LAYOUT.into());
                // On the pen's sheet the pen draws, with its pressure; its
                // other end erases.
                let down = TabletToolTipEvent::<LibinputInputBackend>::tip_state(&event) == TabletToolTipState::Down;
                let eraser = TabletToolEvent::<LibinputInputBackend>::tool(&event).tool_type == TabletToolType::Eraser;
                let pressure = TabletToolEvent::<LibinputInputBackend>::pressure(&event);
                if down && !self.lock.holds_screen() && self.pen.takes_pen(pos) {
                    self.pen_drawing = true;
                    self.pen.pen_down(pos, pressure, eraser);
                    self.boost.kick(hybris_hwc::now_ns());
                    self.needs_redraw = true;
                    return;
                }
                if !down && self.pen_drawing {
                    self.pen_drawing = false;
                    self.pen.pen_up();
                    self.needs_redraw = true;
                    return;
                }
                let c = match TabletToolTipEvent::<LibinputInputBackend>::tip_state(&event) {
                    TabletToolTipState::Down => Contact::Down(pen_slot(), pos, event.time()),
                    TabletToolTipState::Up => Contact::Up(pen_slot(), event.time()),
                };
                self.contact(c);
                self.contact(Contact::Frame);
                return;
            }
            InputEvent::TabletToolAxis { event } => {
                if self.pen_drawing {
                    let pos = event.position_transformed(LAYOUT.into());
                    let eraser = TabletToolEvent::<LibinputInputBackend>::tool(&event).tool_type == TabletToolType::Eraser;
                    self.pen.pen_motion(pos, TabletToolEvent::<LibinputInputBackend>::pressure(&event), eraser);
                    self.boost.kick(hybris_hwc::now_ns());
                    self.needs_redraw = true;
                    return;
                }
                if !self.pen_down {
                    return;
                }
                self.contact(Contact::Motion(pen_slot(), event.position_transformed(LAYOUT.into()), event.time()));
                Contact::Frame
            }
            // The pen's barrel button, held: it erases on the sheet.
            InputEvent::TabletToolButton { event } => {
                self.pen.set_barrel(TabletToolButtonEvent::<LibinputInputBackend>::button_state(&event) == ButtonState::Pressed);
                return;
            }
            _ => return,
        };
        self.contact(contact);
    }

    fn contact(&mut self, contact: Contact) {
        match contact {
            Contact::Down(slot, _, _) if slot == pen_slot() => self.pen_down = true,
            Contact::Up(slot, _) if slot == pen_slot() => self.pen_down = false,
            _ => {}
        }
        let Some(touch) = self.seat.get_touch() else { return };
        // A dark screen takes no touches; any other touch keeps it lit.
        if self.lock.blank {
            return;
        }
        self.lock.last_touch_ns = hybris_hwc::now_ns();
        // The first setup: every touch is its own (setup.rs).
        if self.setup.holds_screen() {
            match contact {
                Contact::Down(slot, pos, _) if self.setup_touch.is_none() => {
                    self.setup.down(pos.x, pos.y);
                    self.setup_touch = Some((slot, pos.x, pos.y, pos.x, pos.y));
                }
                Contact::Motion(slot, pos, _) => {
                    if let Some(t) = self.setup_touch.as_mut().filter(|t| t.0 == slot) {
                        (t.3, t.4) = (pos.x, pos.y);
                        if (t.3 - t.1).abs() > 12.0 || (t.4 - t.2).abs() > 12.0 {
                            self.setup.motion();
                        }
                    }
                }
                Contact::Up(slot, _) => {
                    if let Some(t) = self.setup_touch.take_if(|t| t.0 == slot) {
                        let fingerprint = &self.fingerprint;
                        self.setup.up(t.3, t.4, fingerprint);
                    }
                }
                Contact::Cancel => self.setup_touch = None,
                _ => {}
            }
            self.needs_redraw = true;
            return;
        }
        // Choosing the wallpaper: every touch is the picker's.
        let count = self.walls.names.len();
        match &contact {
            Contact::Down(slot, pos, t) if self.picker.is_open() => {
                self.picker.down(*slot, *pos, *t, count);
                self.needs_redraw = true;
                return;
            }
            Contact::Motion(slot, pos, t) if self.picker.holds(*slot) => {
                self.picker.motion(*slot, *pos, *t);
                self.needs_redraw = true;
                return;
            }
            Contact::Up(slot, _) if self.picker.holds(*slot) => {
                if let Some(i) = self.picker.up(*slot, count) {
                    let name = self.walls.names[i].clone();
                    self.walls.choose(&name, true);
                }
                self.needs_redraw = true;
                return;
            }
            _ => {}
        }
        // Locked: every touch is the lock screen's.
        match &contact {
            Contact::Down(slot, pos, t) if self.lock.holds_screen() => {
                self.lock.down(*slot, pos.x, pos.y, *t);
                self.needs_redraw = true;
                return;
            }
            Contact::Motion(slot, pos, t) if self.lock.holds(*slot) => {
                self.lock.motion(*slot, pos.y, *t);
                self.needs_redraw = true;
                return;
            }
            Contact::Up(slot, _) if self.lock.holds(*slot) => {
                self.lock.up(*slot);
                self.needs_redraw = true;
                return;
            }
            _ => {}
        }
        // Anything the finger does wakes the GPU's clock (boost.rs).
        if matches!(contact, Contact::Down(..) | Contact::Motion(..) | Contact::Up(..)) {
            self.boost.kick(hybris_hwc::now_ns());
        }
        let msec = |t: u64| (t / 1000) as u32;
        match contact {
            Contact::Down(slot, pos, time) => {
                // A tap on a notification's banner: its action.
                if let Some((rect, id)) = self.shade.quick.banner_at.get() {
                    if rect.contains(pos) {
                        self.notes.invoke(id);
                        self.shade.quick.banner_at.set(None);
                        self.needs_redraw = true;
                        return;
                    }
                }
                // A touch at a panel's top edge, or on its shade, is the shade's.
                if self.shade.down(slot, pos.x, pos.y, time) {
                    self.needs_redraw = true;
                    return;
                }
                // The ribbon caught while it runs: it stops under the finger.
                if self.ribbon.moving() {
                    self.ribbon_grab(slot, pos.x, time);
                    return;
                }
                // The pen's sheet's buttons (pensheet.rs).
                if self.pen.down(slot, pos, time) {
                    self.needs_redraw = true;
                    return;
                }
                // The system screen shown: a finger on it scrolls it, or
                // moves the ribbon sideways (sysscreen.rs).
                if self.system.touch_down(slot, pos, time) {
                    self.needs_redraw = true;
                    return;
                }
                // The pen's sheet shown: a finger on it moves the ribbon.
                let panel = crate::layout::panel_at(pos);
                if self.pen.out() && panel == Some(1) {
                    self.ribbon_grab(slot, pos.x, time);
                    return;
                }
                if self.dock.down(slot, pos) {
                    self.needs_redraw = true;
                    return;
                }
                // The outer edge of a panel with a window: back (back.rs).
                if let Some(panel) = crate::back::Back::strip_at(pos) {
                    if self.top_window(panel).is_some() && self.grid.panel() != Some(panel) {
                        self.back.down(slot, pos, panel);
                        self.needs_redraw = true;
                        return;
                    }
                }
                // The bottom band of a panel with a window: the swipe that
                // puts it away (gesture.rs).
                if pos.y >= LAYOUT.1 as f64 - crate::gesture::EDGE {
                    if let Some(panel) = crate::layout::panel_at(pos) {
                        if let Some(window) = self.top_window(panel) {
                            self.gestures.down(slot, pos, time, window, panel);
                            self.bottom_start = Some((slot, pos));
                            return;
                        }
                    }
                }
                // The app grid, and an empty panel's swipes (grid.rs).
                let empty = crate::layout::panel_at(pos).is_some_and(|p| self.top_window(p).is_none());
                if self.grid.down(slot, pos, empty) {
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
                tracing::debug!("touch down {slot:?} at {:.0},{:.0}", pos.x, pos.y);
                touch.down(self, under, &DownEvent { slot, location: pos, serial, time: msec(time) });
            }
            Contact::Motion(slot, pos, time) => {
                if self.ribbon.holds(slot) {
                    self.ribbon.motion(slot, pos.x, time);
                    self.needs_redraw = true;
                    return;
                }
                if self.system.holds(slot) {
                    if let crate::sysscreen::TouchAsk::Ribbon(x) = self.system.touch_motion(slot, pos, time) {
                        self.ribbon_grab(slot, x, time);
                        self.ribbon.motion(slot, pos.x, time);
                    }
                    self.needs_redraw = true;
                    return;
                }
                if self.shade.holds(slot) {
                    self.shade.motion_at(slot, pos.x, pos.y, time);
                    self.needs_redraw = true;
                    return;
                }
                if self.pen.holds(slot) {
                    self.pen.motion(slot, pos, time);
                    self.needs_redraw = true;
                    return;
                }
                if self.back.holds(slot) {
                    self.back.motion(slot, pos);
                    self.needs_redraw = true;
                    return;
                }
                if self.grid.holds(slot) {
                    match self.grid.motion(slot, pos, time) {
                        crate::grid::Ask::Shade(start) => {
                            self.shade.grab_from(slot, start.x, start.y, time);
                            self.shade.motion(slot, pos.y, time);
                        }
                        // Sideways on a desktop: the ribbon, with the finger.
                        crate::grid::Ask::Ribbon(start) => {
                            self.ribbon_grab(slot, start.x, time);
                            self.ribbon.motion(slot, pos.x, time);
                        }
                        _ => {}
                    }
                    self.needs_redraw = true;
                    return;
                }
                if self.gestures.holds(slot) {
                    // Sideways along the bottom edge: the ribbon, not away.
                    if let Some((_, start)) = self.bottom_start.filter(|(s, _)| *s == slot) {
                        let (dx, dy) = (pos.x - start.x, pos.y - start.y);
                        if dx.abs() > 14.0 && dx.abs() > dy.abs() * 1.2 {
                            self.bottom_start = None;
                            self.gestures.let_go(slot);
                            self.ribbon_grab(slot, start.x, time);
                            self.ribbon.motion(slot, pos.x, time);
                            self.needs_redraw = true;
                            return;
                        }
                        if dy.abs() > 14.0 {
                            self.bottom_start = None;
                        }
                    }
                    self.gestures.motion(slot, pos.y, time);
                    self.needs_redraw = true;
                    return;
                }
                if self.dock.holds(slot) {
                    self.dock.motion(slot, pos);
                    self.needs_redraw = true;
                    return;
                }
                let under = self.surface_under(pos);
                touch.motion(self, under, &MotionEvent { slot, location: pos, time: msec(time) });
            }
            Contact::Up(slot, time) => {
                if self.ribbon.holds(slot) {
                    self.ribbon.up(slot);
                    self.needs_redraw = true;
                    return;
                }
                if self.bottom_start.is_some_and(|(s, _)| s == slot) {
                    self.bottom_start = None;
                }
                if self.system.holds(slot) {
                    self.system.touch_up(slot);
                    self.needs_redraw = true;
                    return;
                }
                if self.shade.holds(slot) {
                    if let Some(ask) = self.shade.up(slot) {
                        self.shade_ask(ask);
                    }
                    self.needs_redraw = true;
                    return;
                }
                if self.pen.holds(slot) {
                    self.pen.up(slot);
                    self.needs_redraw = true;
                    return;
                }
                if self.back.holds(slot) {
                    if let Some(panel) = self.back.up(slot) {
                        self.go_back(panel);
                    }
                    self.needs_redraw = true;
                    return;
                }
                if self.grid.holds(slot) {
                    if let crate::grid::Ask::Launch { exec, ids, icon, panel } = self.grid.up(slot) {
                        let icon = icon.as_deref().and_then(|n| crate::apps::icon(n, crate::curtain::ICON));
                        self.launch(&exec, &ids, icon, panel);
                    }
                    self.needs_redraw = true;
                    return;
                }
                if self.gestures.holds(slot) {
                    self.gestures.up(slot, hybris_hwc::now_ns());
                    self.needs_redraw = true;
                    return;
                }
                if self.dock.holds(slot) {
                    if let Some(Tap::Launch(command, panel, icon, ids)) = self.dock.up(slot) {
                        self.launch(&command, &ids, icon, panel);
                    }
                    self.needs_redraw = true;
                    return;
                }
                touch.up(self, &UpEvent { slot, serial: SERIAL_COUNTER.next_serial(), time: msec(time) });
            }
            Contact::Frame => touch.frame(self),
            Contact::Cancel => {
                self.shade.cancel();
                self.dock.cancel();
                self.gestures.cancel(hybris_hwc::now_ns());
                self.grid.cancel();
                self.back.cancel();
                self.system.cancel();
                self.pen.cancel();
                self.ribbon.cancel();
                self.bottom_start = None;
                self.needs_redraw = true;
                touch.cancel(self)
            }
        }
    }
}
