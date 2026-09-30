//! latency-probe: a Wayland client that times its own frames to the screen
//! with `wp_presentation`, so the same numbers come from any compositor
//! (item-compositor, phoc).
//!
//!     latency-probe tap    a square in the window's middle changes colour at
//!                          each touch down; prints the time from the touch
//!                          (the kernel's, as wl_touch gives it) to the
//!                          frame's presentation
//!     latency-probe anim   the square changes colour every frame; prints,
//!                          each second, the frames presented, the gaps of
//!                          more than one vsync, and commit to screen
//!
//! A third argument `full` asks for the whole output (fullscreen), so a
//! finger anywhere on it lands on the probe.
//!
//! Only the square is damaged, so the compositor uploads little of the
//! window's shm buffers: what is timed is the compositor's path, not the
//! copy. Times are CLOCK_MONOTONIC; the compositor's presentation clock must
//! be it too (checked).

use std::os::fd::{AsFd, FromRawFd, OwnedFd};
use std::time::Duration;

use wayland_client::protocol::{wl_buffer, wl_callback, wl_compositor, wl_registry, wl_seat, wl_shm, wl_shm_pool, wl_surface, wl_touch};
use wayland_client::{delegate_noop, Connection, Dispatch, QueueHandle, WEnum};
use wayland_protocols::wp::presentation_time::client::{wp_presentation, wp_presentation_feedback};
use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base};

const BOX: i32 = 200;
const BUFFERS: usize = 3;
const COLOURS: [u32; 4] = [0xff_f0_f0_f0, 0xff_20_60_d0, 0xff_d0_40_30, 0xff_30_a0_50];

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Tap,
    Anim,
}

struct Buffer {
    buffer: wl_buffer::WlBuffer,
    busy: bool,
    colour: usize,
}

struct State {
    mode: Mode,
    compositor: Option<wl_compositor::WlCompositor>,
    shm: Option<wl_shm::WlShm>,
    wm: Option<xdg_wm_base::XdgWmBase>,
    presentation: Option<wp_presentation::WpPresentation>,
    seat: Option<wl_seat::WlSeat>,
    touch: Option<wl_touch::WlTouch>,
    surface: Option<wl_surface::WlSurface>,
    size: (i32, i32),
    configured: bool,
    buffers: Vec<Buffer>,
    /// The memory the buffers live in, one after another.
    map: Option<(*mut u8, usize)>,
    colour: usize,
    clock_ok: Option<bool>,
    // anim: this second's frames
    second_start: u64,
    frames: u32,
    late: u32,
    last_presented: u64,
    commit_to_screen: Vec<f64>,
    refresh_ns: u64,
    running: bool,
}

fn now_ns() -> u64 {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64
}

/// What a feedback was asked for.
#[derive(Clone, Copy)]
enum Asked {
    /// A touch down at this time (ms, wl_touch's), received at this time (ns).
    Tap { touch_ms: u32, received_ns: u64 },
    /// A frame committed at this time (ns).
    Frame { committed_ns: u64 },
}

impl State {
    fn make_buffers(&mut self, qh: &QueueHandle<State>) {
        let (w, h) = self.size;
        let stride = w * 4;
        let one = (stride * h) as usize;
        let total = one * BUFFERS;
        let fd = unsafe { libc::memfd_create(c"latency-probe".as_ptr(), libc::MFD_CLOEXEC) };
        assert!(fd >= 0, "memfd_create");
        let fd = unsafe { OwnedFd::from_raw_fd(fd) };
        unsafe {
            libc::ftruncate(std::os::fd::AsRawFd::as_raw_fd(&fd), total as i64);
        }
        let ptr = unsafe { libc::mmap(std::ptr::null_mut(), total, libc::PROT_READ | libc::PROT_WRITE, libc::MAP_SHARED, std::os::fd::AsRawFd::as_raw_fd(&fd), 0) };
        assert!(ptr != libc::MAP_FAILED, "mmap");
        let pool = self.shm.as_ref().unwrap().create_pool(fd.as_fd(), total as i32, qh, ());
        let pixels = unsafe { std::slice::from_raw_parts_mut(ptr as *mut u32, total / 4) };
        pixels.fill(0xff_30_30_30);
        self.buffers.clear();
        for i in 0..BUFFERS {
            let buffer = pool.create_buffer((i * one) as i32, w, h, stride, wl_shm::Format::Argb8888, qh, i);
            self.buffers.push(Buffer { buffer, busy: false, colour: usize::MAX });
        }
        pool.destroy();
        self.map = Some((ptr as *mut u8, total));
    }

    /// The square in buffer `i` painted `colour`.
    fn paint(&mut self, i: usize, colour: usize) {
        let (w, h) = self.size;
        let one = (w * h) as usize;
        let Some((ptr, total)) = self.map else { return };
        let pixels = unsafe { std::slice::from_raw_parts_mut(ptr as *mut u32, total / 4) };
        let (x0, y0) = ((w - BOX) / 2, (h - BOX) / 2);
        for y in y0.max(0)..(y0 + BOX).min(h) {
            let row = i * one + (y * w) as usize;
            pixels[row + x0.max(0) as usize..row + (x0 + BOX).min(w) as usize].fill(COLOURS[colour]);
        }
        self.buffers[i].colour = colour;
    }

    /// A new frame: the next colour in a free buffer, the square damaged, a
    /// presentation feedback asked for.
    fn frame(&mut self, qh: &QueueHandle<State>, asked: Asked) -> bool {
        let Some(i) = self.buffers.iter().position(|b| !b.busy) else { return false };
        self.colour = (self.colour + 1) % COLOURS.len();
        if self.buffers[i].colour != self.colour {
            self.paint(i, self.colour);
        }
        let (w, h) = self.size;
        let surface = self.surface.as_ref().unwrap();
        surface.attach(Some(&self.buffers[i].buffer), 0, 0);
        self.buffers[i].busy = true;
        surface.damage_buffer((w - BOX) / 2, (h - BOX) / 2, BOX, BOX);
        if let Some(p) = &self.presentation {
            p.feedback(surface, qh, asked);
        }
        if self.mode == Mode::Anim {
            surface.frame(qh, ());
        }
        surface.commit();
        true
    }
}

impl Dispatch<wl_registry::WlRegistry, ()> for State {
    fn event(state: &mut Self, registry: &wl_registry::WlRegistry, event: wl_registry::Event, _: &(), _: &Connection, qh: &QueueHandle<Self>) {
        if let wl_registry::Event::Global { name, interface, version } = event {
            match interface.as_str() {
                "wl_compositor" => state.compositor = Some(registry.bind(name, version.min(4), qh, ())),
                "wl_shm" => state.shm = Some(registry.bind(name, 1, qh, ())),
                "xdg_wm_base" => state.wm = Some(registry.bind(name, 1, qh, ())),
                "wp_presentation" => state.presentation = Some(registry.bind(name, 1, qh, ())),
                "wl_seat" => state.seat = Some(registry.bind(name, version.min(5), qh, ())),
                _ => {}
            }
        }
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for State {
    fn event(state: &mut Self, seat: &wl_seat::WlSeat, event: wl_seat::Event, _: &(), _: &Connection, qh: &QueueHandle<Self>) {
        if let wl_seat::Event::Capabilities { capabilities: WEnum::Value(c) } = event {
            if c.contains(wl_seat::Capability::Touch) && state.touch.is_none() {
                state.touch = Some(seat.get_touch(qh, ()));
            }
        }
    }
}

impl Dispatch<wl_touch::WlTouch, ()> for State {
    fn event(state: &mut Self, _: &wl_touch::WlTouch, event: wl_touch::Event, _: &(), _: &Connection, qh: &QueueHandle<Self>) {
        if let wl_touch::Event::Down { time, .. } = event {
            if state.mode == Mode::Tap && state.configured {
                let asked = Asked::Tap { touch_ms: time, received_ns: now_ns() };
                if state.frame(qh, asked) {
                    // For timing below the compositor (ab-latency.sh): the
                    // first present after this holds the frame.
                    println!("commit {:.6}", now_ns() as f64 / 1e9);
                } else {
                    println!("tap: no free buffer");
                }
            }
        }
    }
}

impl Dispatch<xdg_wm_base::XdgWmBase, ()> for State {
    fn event(_: &mut Self, wm: &xdg_wm_base::XdgWmBase, event: xdg_wm_base::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let xdg_wm_base::Event::Ping { serial } = event {
            wm.pong(serial);
        }
    }
}

impl Dispatch<xdg_toplevel::XdgToplevel, ()> for State {
    fn event(state: &mut Self, _: &xdg_toplevel::XdgToplevel, event: xdg_toplevel::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match event {
            xdg_toplevel::Event::Configure { width, height, .. } if width > 0 && height > 0 => state.size = (width, height),
            xdg_toplevel::Event::Close => state.running = false,
            _ => {}
        }
    }
}

impl Dispatch<xdg_surface::XdgSurface, ()> for State {
    fn event(state: &mut Self, xdg: &xdg_surface::XdgSurface, event: xdg_surface::Event, _: &(), _: &Connection, qh: &QueueHandle<Self>) {
        if let xdg_surface::Event::Configure { serial } = event {
            xdg.ack_configure(serial);
            let resized = state.buffers.is_empty() || state.map.is_some_and(|(_, total)| total != (state.size.0 * state.size.1 * 4) as usize * BUFFERS);
            if resized {
                state.make_buffers(qh);
            }
            if !state.configured || resized {
                state.configured = true;
                println!("size {}x{}", state.size.0, state.size.1);
                state.frame(qh, Asked::Frame { committed_ns: now_ns() });
            }
        }
    }
}

impl Dispatch<wl_buffer::WlBuffer, usize> for State {
    fn event(state: &mut Self, _: &wl_buffer::WlBuffer, event: wl_buffer::Event, i: &usize, _: &Connection, _: &QueueHandle<Self>) {
        if let wl_buffer::Event::Release = event {
            if let Some(b) = state.buffers.get_mut(*i) {
                b.busy = false;
            }
        }
    }
}

impl Dispatch<wl_callback::WlCallback, ()> for State {
    fn event(state: &mut Self, _: &wl_callback::WlCallback, event: wl_callback::Event, _: &(), _: &Connection, qh: &QueueHandle<Self>) {
        if let wl_callback::Event::Done { .. } = event {
            if state.mode == Mode::Anim && !state.frame(qh, Asked::Frame { committed_ns: now_ns() }) {
                // No buffer free: try again at the next frame.
                state.surface.as_ref().unwrap().frame(qh, ());
                state.surface.as_ref().unwrap().commit();
            }
        }
    }
}

impl Dispatch<wp_presentation::WpPresentation, ()> for State {
    fn event(state: &mut Self, _: &wp_presentation::WpPresentation, event: wp_presentation::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let wp_presentation::Event::ClockId { clk_id } = event {
            state.clock_ok = Some(clk_id == libc::CLOCK_MONOTONIC as u32);
            println!("presentation clock {clk_id}{}", if clk_id == libc::CLOCK_MONOTONIC as u32 { " (monotonic)" } else { ": not monotonic, times are wrong" });
        }
    }
}

impl Dispatch<wp_presentation_feedback::WpPresentationFeedback, Asked> for State {
    fn event(state: &mut Self, _: &wp_presentation_feedback::WpPresentationFeedback, event: wp_presentation_feedback::Event, asked: &Asked, _: &Connection, _: &QueueHandle<Self>) {
        match event {
            wp_presentation_feedback::Event::Presented { tv_sec_hi, tv_sec_lo, tv_nsec, refresh, .. } => {
                let at = ((tv_sec_hi as u64) << 32 | tv_sec_lo as u64) * 1_000_000_000 + tv_nsec as u64;
                if refresh > 0 {
                    state.refresh_ns = refresh as u64;
                }
                match *asked {
                    Asked::Tap { touch_ms, received_ns } => {
                        let at_ms = (at / 1_000_000) as u32;
                        println!(
                            "tap {} ms (to the client {} ms, client to screen {:.1} ms)",
                            at_ms.wrapping_sub(touch_ms),
                            ((received_ns / 1_000_000) as u32).wrapping_sub(touch_ms),
                            at.saturating_sub(received_ns) as f64 / 1e6
                        );
                    }
                    Asked::Frame { committed_ns } => {
                        if state.mode != Mode::Anim {
                            return;
                        }
                        state.frames += 1;
                        let period = if state.refresh_ns > 0 { state.refresh_ns } else { 16_666_667 };
                        if state.last_presented > 0 && at - state.last_presented > period * 3 / 2 {
                            state.late += ((at - state.last_presented + period / 2) / period - 1) as u32;
                        }
                        state.last_presented = at;
                        state.commit_to_screen.push(at.saturating_sub(committed_ns) as f64 / 1e6);
                        let now = now_ns();
                        if state.second_start == 0 {
                            state.second_start = now;
                        } else if now - state.second_start >= 1_000_000_000 {
                            let c = &mut state.commit_to_screen;
                            c.sort_by(|a, b| a.partial_cmp(b).unwrap());
                            println!(
                                "anim {} frames, {} vsyncs missed, commit to screen median {:.1} ms",
                                state.frames,
                                state.late,
                                c.get(c.len() / 2).copied().unwrap_or(0.0)
                            );
                            state.frames = 0;
                            state.late = 0;
                            c.clear();
                            state.second_start = now;
                        }
                    }
                }
            }
            wp_presentation_feedback::Event::Discarded => {
                if let Asked::Tap { .. } = asked {
                    println!("tap discarded");
                }
            }
            _ => {}
        }
    }
}

delegate_noop!(State: ignore wl_compositor::WlCompositor);
delegate_noop!(State: ignore wl_shm::WlShm);
delegate_noop!(State: ignore wl_shm_pool::WlShmPool);
delegate_noop!(State: ignore wl_surface::WlSurface);

fn main() {
    let mode = match std::env::args().nth(1).as_deref() {
        Some("anim") => Mode::Anim,
        _ => Mode::Tap,
    };
    let seconds: u64 = std::env::args().nth(2).and_then(|s| s.parse().ok()).unwrap_or(0);
    let conn = Connection::connect_to_env().expect("no Wayland display");
    let mut queue = conn.new_event_queue();
    let qh = queue.handle();
    conn.display().get_registry(&qh, ());
    let mut state = State {
        mode,
        compositor: None,
        shm: None,
        wm: None,
        presentation: None,
        seat: None,
        touch: None,
        surface: None,
        size: (600, 800),
        configured: false,
        buffers: Vec::new(),
        map: None,
        colour: 0,
        clock_ok: None,
        second_start: 0,
        frames: 0,
        late: 0,
        last_presented: 0,
        commit_to_screen: Vec::new(),
        refresh_ns: 0,
        running: true,
    };
    queue.roundtrip(&mut state).expect("roundtrip");
    if state.presentation.is_none() {
        eprintln!("latency-probe: the compositor has no wp_presentation");
        std::process::exit(1);
    }
    let surface = state.compositor.as_ref().expect("wl_compositor").create_surface(&qh, ());
    let xdg = state.wm.as_ref().expect("xdg_wm_base").get_xdg_surface(&surface, &qh, ());
    let toplevel = xdg.get_toplevel(&qh, ());
    toplevel.set_title("latency probe".into());
    toplevel.set_app_id("latency-probe".into());
    if std::env::args().nth(3).as_deref() == Some("full") {
        toplevel.set_fullscreen(None);
    }
    surface.commit();
    state.surface = Some(surface);
    let end = (seconds > 0).then(|| std::time::Instant::now() + Duration::from_secs(seconds));
    while state.running && end.is_none_or(|e| std::time::Instant::now() < e) {
        use std::io::Write;
        let _ = std::io::stdout().flush();
        queue.flush().ok();
        if let Some(guard) = queue.prepare_read() {
            let fd = guard.connection_fd();
            let mut pfd = libc::pollfd { fd: std::os::fd::AsRawFd::as_raw_fd(&fd), events: libc::POLLIN, revents: 0 };
            unsafe { libc::poll(&mut pfd, 1, 200) };
            if pfd.revents & libc::POLLIN != 0 {
                let _ = guard.read();
            }
        }
        queue.dispatch_pending(&mut state).expect("dispatch");
    }
}
