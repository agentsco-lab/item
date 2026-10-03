//! pen-split: the Duo's digitizer node as two - fingers and the pen - as the
//! port's sfduo-pen-split (Python) did, in Rust: every finger and pen event
//! passes through it, and the Python one added 1.5 ms on average and
//! 5-15 ms now and then on the pen's way to the screen (measured
//! 2026-10-03, item's pen->screen report). Same devices, same socket, same
//! behaviour; run at a real-time priority (its unit).
//!
//! The vendor touchpen HAL gives Linux one uinput node, surface_touchscreen,
//! for both: the pen is a multitouch contact like a finger, with tracking id
//! 65535 and a graded pressure. This grabs that node (EVIOCGRAB) and writes
//! two:
//!
//!   sfduo touchscreen   the fingers, as they came
//!   sfduo pen           the pen, as a tablet tool: proximity, touch,
//!                       pressure, the barrel button, the eraser
//!
//! The pen is a tablet tool only where it draws - over the pen's sheet while
//! it is out, which the sheet says with a datagram to /run/sfduo-pen.sock
//! ("1" out, "0" not); everywhere else it is one more finger, so the shell's
//! swipes take it, and its hovering is nothing. Which one a stroke is, is
//! decided as the pen comes near and kept until it leaves. If this stops,
//! the grab goes with it and the original node reaches the compositor again.
//!
//!   pen-split [--for SECONDS]

use std::collections::BTreeMap;
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixDatagram;
use std::time::{Duration, Instant};

use evdev::uinput::VirtualDevice;
use evdev::{AbsInfo, AbsoluteAxisCode as Abs, AttributeSet, Device, EventType, InputEvent, KeyCode as Key, PropType, UinputAbsSetup};

const SOURCE_NAME: &str = "surface_touchscreen";
const PEN_SOCK: &str = "/run/sfduo-pen.sock";
/// Where the right panel starts across the output (logical 717 of 1392):
/// the sheet lies over it.
const SHEET_FROM: f64 = 717.0 / 1392.0;
/// The tracking id the digitizer gives the pen.
const PEN_ID: i32 = 65535;
/// The panel's size, for the resolution a tablet has to have.
const WIDTH_MM: i32 = 175;
const HEIGHT_MM: i32 = 114;

fn log(msg: &str) {
    eprintln!("pen-split: {msg}");
}

fn find_source() -> Option<Device> {
    evdev::enumerate().map(|(_, d)| d).find(|d| d.name() == Some(SOURCE_NAME))
}

#[derive(Clone, Copy, PartialEq)]
enum Route {
    Tablet,
    Touch,
}

struct Splitter {
    touch: VirtualDevice,
    pen: VirtualDevice,
    sheet_out: bool,
    sheet_x: i32,
    pen_route: Option<Route>,
    /// The pen, as a finger: down.
    pen_touching: bool,
    touch_id: i32,
    slot: i32,
    /// Each slot's last values, by code.
    slots: BTreeMap<i32, BTreeMap<u16, i32>>,
    pen_slot: Option<i32>,
    rubber: bool,
    stylus: bool,
    /// This frame's changes, by slot.
    changed: BTreeMap<i32, Vec<(u16, i32)>>,
    keys: bool,
    fingers_down: usize,
    /// The pen as a tablet: in proximity (with which tool), touching.
    prox: Option<Key>,
    touching: bool,
}

fn abs(code: Abs, value: i32) -> InputEvent {
    InputEvent::new(EventType::ABSOLUTE.0, code.0, value)
}

fn key(code: Key, down: bool) -> InputEvent {
    InputEvent::new(EventType::KEY.0, code.0, down as i32)
}

impl Splitter {
    fn event(&mut self, ev: InputEvent) -> io::Result<()> {
        match ev.event_type() {
            EventType::ABSOLUTE => {
                if ev.code() == Abs::ABS_MT_SLOT.0 {
                    self.slot = ev.value();
                    return Ok(());
                }
                if ev.code() == Abs::ABS_MT_TRACKING_ID.0 && ev.value() == PEN_ID {
                    self.pen_slot = Some(self.slot);
                }
                self.slots.entry(self.slot).or_default().insert(ev.code(), ev.value());
                self.changed.entry(self.slot).or_default().push((ev.code(), ev.value()));
            }
            EventType::KEY => {
                if ev.code() == Key::BTN_TOOL_RUBBER.0 {
                    self.rubber = ev.value() != 0;
                    self.keys = true;
                } else if ev.code() == Key::BTN_STYLUS.0 {
                    self.stylus = ev.value() != 0;
                    self.keys = true;
                }
            }
            EventType::SYNCHRONIZATION if ev.code() == 0 => self.frame()?,
            _ => {}
        }
        Ok(())
    }

    fn frame(&mut self) -> io::Result<()> {
        let mut out = Vec::new();
        let mut pen_changed = self.keys;
        let changed = std::mem::take(&mut self.changed);
        for (slot, codes) in &changed {
            if Some(*slot) == self.pen_slot {
                pen_changed = true;
                continue;
            }
            out.push(abs(Abs::ABS_MT_SLOT, *slot));
            // The tracking id first, the rest after it.
            if let Some(&(_, v)) = codes.iter().rev().find(|(c, _)| *c == Abs::ABS_MT_TRACKING_ID.0) {
                out.push(abs(Abs::ABS_MT_TRACKING_ID, v));
            }
            for &(c, v) in codes.iter().filter(|(c, _)| *c != Abs::ABS_MT_TRACKING_ID.0) {
                out.push(InputEvent::new(EventType::ABSOLUTE.0, c, v));
            }
        }
        if !out.is_empty() {
            let down: Vec<i32> = self
                .slots
                .iter()
                .filter(|(s, st)| Some(**s) != self.pen_slot && st.get(&Abs::ABS_MT_TRACKING_ID.0).copied().unwrap_or(-1) >= 0)
                .map(|(s, _)| *s)
                .collect();
            if down.is_empty() != (self.fingers_down == 0) && !self.pen_touching {
                out.push(key(Key::BTN_TOUCH, !down.is_empty()));
            }
            if let Some(first) = down.first().and_then(|s| self.slots.get(s)) {
                if let Some(&x) = first.get(&Abs::ABS_MT_POSITION_X.0) {
                    out.push(abs(Abs::ABS_X, x));
                }
                if let Some(&y) = first.get(&Abs::ABS_MT_POSITION_Y.0) {
                    out.push(abs(Abs::ABS_Y, y));
                }
            }
            self.fingers_down = down.len();
            self.touch.emit(&out)?;
        }
        if pen_changed {
            self.pen_frame()?;
        }
        self.keys = false;
        Ok(())
    }

    fn pen_frame(&mut self) -> io::Result<()> {
        let st = self.pen_slot.and_then(|s| self.slots.get(&s)).cloned().unwrap_or_default();
        let get = |c: Abs| st.get(&c.0).copied();
        let present = get(Abs::ABS_MT_TRACKING_ID) == Some(PEN_ID);
        if present && self.pen_route.is_none() {
            let over_sheet = get(Abs::ABS_MT_POSITION_X).unwrap_or(0) >= self.sheet_x;
            self.pen_route = Some(if self.sheet_out && over_sheet { Route::Tablet } else { Route::Touch });
        }
        if self.pen_route == Some(Route::Touch) || (!present && self.pen_touching) {
            self.pen_as_finger(&st, present)?;
            if !present {
                self.forget_pen(&st);
            }
            return Ok(());
        }
        let tool = if self.rubber { Key::BTN_TOOL_RUBBER } else { Key::BTN_TOOL_PEN };
        let mut out = Vec::new();
        if present {
            match self.prox {
                None => out.push(key(tool, true)),
                Some(t) if t != tool => {
                    out.push(key(t, false));
                    out.push(key(tool, true));
                }
                _ => {}
            }
            self.prox = Some(tool);
            if let Some(x) = get(Abs::ABS_MT_POSITION_X) {
                out.push(abs(Abs::ABS_X, x));
            }
            if let Some(y) = get(Abs::ABS_MT_POSITION_Y) {
                out.push(abs(Abs::ABS_Y, y));
            }
            let pressure = get(Abs::ABS_MT_PRESSURE).unwrap_or(0);
            out.push(abs(Abs::ABS_PRESSURE, pressure));
            let touching = pressure > 0;
            if touching != self.touching {
                out.push(key(Key::BTN_TOUCH, touching));
                self.touching = touching;
            }
            out.push(key(Key::BTN_STYLUS, self.stylus));
        } else {
            if self.touching {
                out.push(abs(Abs::ABS_PRESSURE, 0));
                out.push(key(Key::BTN_TOUCH, false));
                self.touching = false;
            }
            if let Some(t) = self.prox.take() {
                out.push(key(t, false));
            }
            self.forget_pen(&st);
        }
        self.pen.emit(&out)
    }

    fn forget_pen(&mut self, st: &BTreeMap<u16, i32>) {
        if self.pen_slot.is_some() && st.get(&Abs::ABS_MT_TRACKING_ID.0).copied().unwrap_or(-1) < 0 {
            if let Some(s) = self.pen_slot.take() {
                self.slots.remove(&s);
            }
        }
        self.pen_route = None;
    }

    /// The pen as one more finger: down while it presses, up when it lifts
    /// or leaves; its hovering is nothing.
    fn pen_as_finger(&mut self, st: &BTreeMap<u16, i32>, present: bool) -> io::Result<()> {
        let pressing = present && st.get(&Abs::ABS_MT_PRESSURE.0).copied().unwrap_or(0) > 0;
        let slot = self.pen_slot.unwrap_or(0);
        let mut out = Vec::new();
        if pressing {
            out.push(abs(Abs::ABS_MT_SLOT, slot));
            if !self.pen_touching {
                self.touch_id = self.touch_id % 60000 + 1;
                out.push(abs(Abs::ABS_MT_TRACKING_ID, self.touch_id));
            }
            for c in [Abs::ABS_MT_POSITION_X, Abs::ABS_MT_POSITION_Y] {
                if let Some(&v) = st.get(&c.0) {
                    out.push(abs(c, v));
                }
            }
            if !self.pen_touching && self.fingers_down == 0 {
                out.push(key(Key::BTN_TOUCH, true));
            }
            if self.fingers_down == 0 {
                if let Some(&x) = st.get(&Abs::ABS_MT_POSITION_X.0) {
                    out.push(abs(Abs::ABS_X, x));
                }
                if let Some(&y) = st.get(&Abs::ABS_MT_POSITION_Y.0) {
                    out.push(abs(Abs::ABS_Y, y));
                }
            }
            self.pen_touching = true;
        } else if self.pen_touching {
            out.push(abs(Abs::ABS_MT_SLOT, slot));
            out.push(abs(Abs::ABS_MT_TRACKING_ID, -1));
            if self.fingers_down == 0 {
                out.push(key(Key::BTN_TOUCH, false));
            }
            self.pen_touching = false;
        }
        if out.is_empty() {
            return Ok(());
        }
        self.touch.emit(&out)
    }
}

fn make_devices(src: &Device) -> io::Result<(VirtualDevice, VirtualDevice, i32)> {
    let absinfo: BTreeMap<u16, AbsInfo> = src.get_absinfo()?.map(|(c, a)| (c.0, a)).collect();
    let need = |c: Abs| absinfo.get(&c.0).copied().ok_or_else(|| io::Error::other(format!("no {c:?}")));
    let (ax, ay) = (need(Abs::ABS_MT_POSITION_X)?, need(Abs::ABS_MT_POSITION_Y)?);
    let res_x = (ax.maximum() as f64 / WIDTH_MM as f64).round().max(1.0) as i32;
    let res_y = (ay.maximum() as f64 / HEIGHT_MM as f64).round().max(1.0) as i32;
    let info = |a: AbsInfo, res: Option<i32>| AbsInfo::new(0, a.minimum(), a.maximum(), 0, 0, res.unwrap_or(a.resolution()));
    let direct = AttributeSet::from_iter([PropType::DIRECT]);

    let mut b = VirtualDevice::builder()?.name("sfduo touchscreen").with_keys(&AttributeSet::from_iter([Key::BTN_TOUCH]))?.with_properties(&direct)?;
    b = b.with_absolute_axis(&UinputAbsSetup::new(Abs::ABS_X, info(ax, Some(res_x))))?;
    b = b.with_absolute_axis(&UinputAbsSetup::new(Abs::ABS_Y, info(ay, Some(res_y))))?;
    for c in [Abs::ABS_MT_SLOT, Abs::ABS_MT_TRACKING_ID, Abs::ABS_MT_POSITION_X, Abs::ABS_MT_POSITION_Y, Abs::ABS_MT_TOUCH_MAJOR, Abs::ABS_MT_TOUCH_MINOR, Abs::ABS_MT_ORIENTATION, Abs::ABS_MT_PRESSURE] {
        if let Some(a) = absinfo.get(&c.0) {
            let res = if c == Abs::ABS_MT_POSITION_X { Some(res_x) } else if c == Abs::ABS_MT_POSITION_Y { Some(res_y) } else { None };
            b = b.with_absolute_axis(&UinputAbsSetup::new(c, info(*a, res)))?;
        }
    }
    let touch = b.build()?;

    let pmax = absinfo.get(&Abs::ABS_MT_PRESSURE.0).map(|a| a.maximum()).unwrap_or(65535);
    let pen = VirtualDevice::builder()?
        .name("sfduo pen")
        .with_keys(&AttributeSet::from_iter([Key::BTN_TOOL_PEN, Key::BTN_TOOL_RUBBER, Key::BTN_TOUCH, Key::BTN_STYLUS]))?
        .with_properties(&direct)?
        .with_absolute_axis(&UinputAbsSetup::new(Abs::ABS_X, info(ax, Some(res_x))))?
        .with_absolute_axis(&UinputAbsSetup::new(Abs::ABS_Y, info(ay, Some(res_y))))?
        .with_absolute_axis(&UinputAbsSetup::new(Abs::ABS_PRESSURE, AbsInfo::new(0, 0, pmax, 0, 0, 0)))?
        .build()?;
    Ok((touch, pen, ax.maximum()))
}

fn main() {
    std::process::exit(run());
}

fn run() -> i32 {
    let args: Vec<String> = std::env::args().collect();
    let until = args.iter().position(|a| a == "--for").and_then(|i| args.get(i + 1)).and_then(|s| s.parse::<f64>().ok()).map(|s| Instant::now() + Duration::from_secs_f64(s));
    let mut src = None;
    for _ in 0..60 {
        src = find_source();
        if src.is_some() {
            break;
        }
        std::thread::sleep(Duration::from_secs(1));
    }
    let Some(mut src) = src else {
        log(&format!("no {SOURCE_NAME} node - nothing to split"));
        return 1;
    };
    let (touch, pen, max_x) = match make_devices(&src) {
        Ok(d) => d,
        Err(e) => {
            log(&format!("the devices: {e}"));
            return 1;
        }
    };
    // udev and the compositor meet the new devices first.
    std::thread::sleep(Duration::from_millis(500));
    if let Err(e) = src.grab() {
        log(&format!("the grab: {e}"));
        return 1;
    }
    log("surface_touchscreen split into 'sfduo touchscreen' and 'sfduo pen'");
    let _ = std::fs::remove_file(PEN_SOCK);
    let sheet = match UnixDatagram::bind(PEN_SOCK) {
        Ok(s) => s,
        Err(e) => {
            log(&format!("{PEN_SOCK}: {e}"));
            return 1;
        }
    };
    let _ = std::fs::set_permissions(PEN_SOCK, std::os::unix::fs::PermissionsExt::from_mode(0o666));
    let _ = sheet.set_nonblocking(true);
    let _ = src.set_nonblocking(true);
    let mut s = Splitter {
        touch,
        pen,
        sheet_out: false,
        sheet_x: (max_x as f64 * SHEET_FROM) as i32,
        pen_route: None,
        pen_touching: false,
        touch_id: 1,
        slot: 0,
        slots: BTreeMap::new(),
        pen_slot: None,
        rubber: false,
        stylus: false,
        changed: BTreeMap::new(),
        keys: false,
        fingers_down: 0,
        prox: None,
        touching: false,
    };
    let code = loop {
        if until.is_some_and(|u| Instant::now() >= u) {
            log("left after its time, the node given back");
            break 0;
        }
        let mut fds = [libc::pollfd { fd: src.as_raw_fd(), events: libc::POLLIN, revents: 0 }, libc::pollfd { fd: sheet.as_raw_fd(), events: libc::POLLIN, revents: 0 }];
        // SAFETY: two valid pollfds, their fds owned for the call.
        let n = unsafe { libc::poll(fds.as_mut_ptr(), 2, 1000) };
        if n < 0 {
            continue;
        }
        if fds[1].revents & libc::POLLIN != 0 {
            let mut buf = [0u8; 16];
            while let Ok(k) = sheet.recv(&mut buf) {
                s.sheet_out = buf[..k].iter().copied().filter(|b| !b.is_ascii_whitespace()).eq(*b"1");
            }
        }
        if fds[0].revents & (libc::POLLERR | libc::POLLHUP) != 0 {
            log("the node went away");
            break 1;
        }
        if fds[0].revents & libc::POLLIN == 0 {
            continue;
        }
        let events: Vec<InputEvent> = match src.fetch_events() {
            Ok(it) => it.collect(),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => continue,
            Err(e) => {
                // The HAL restarted: systemd starts this again.
                log(&format!("the node went away: {e}"));
                break 1;
            }
        };
        for ev in events {
            if let Err(e) = s.event(ev) {
                log(&format!("writing: {e}"));
            }
        }
    };
    let _ = src.ungrab();
    let _ = std::fs::remove_file(PEN_SOCK);
    code
}
