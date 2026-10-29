//! Probe: a GL frame on the Surface Duo's panels through the kernel's DRM,
//! with no hwcomposer - item's own display output's first step.
//!
//! The GPU's side is the compositor's as it is: EGL through libhybris on a
//! native window from `HWCNativeWindowCreate`. Only the window's present
//! callback differs: instead of handing the buffer to hwcomposer, it waits
//! for the GPU (the buffer's acquire fence), imports the buffer's dmabuf
//! into DRM as a framebuffer (once a buffer), and flips it onto both
//! panels' CRTCs, each its half - waiting for both flips done before the
//! window has the buffer back. The panels' factory calibration goes onto
//! their colour hardware first, as item-recovery puts it (R2's file the left
//! panel's).
//!
//! It draws a test picture on each panel (grey steps, the darks, colour
//! bars and their halves) and a white bar sweeping across both, and prints
//! once a second: frames, the mean and longest time between them, the time
//! waiting for the GPU and for the flips.
//!
//! Run it with item and hwcomposer stopped (tools/kms-run.sh):
//!   EGL_PLATFORM=hwcomposer kms-frame [seconds] [srgb|vivid|off]
//! KMS_SETCRTC=1: every frame set on the CRTCs (as the recovery's screen
//! does) instead of flipped. KMS_UBWC=0/1: the framebuffer's compression
//! forced off or on, instead of as gralloc's handle says. KMS_PERF=0: the
//! CRTCs' clock and bandwidth votes left as hwcomposer left them.

mod colour;
mod kms;

use std::collections::HashMap;
use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::ptr::null_mut;
use std::sync::Mutex;

extern "C" {
    fn dlopen(file: *const c_char, flags: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, name: *const c_char) -> *mut c_void;
    fn dlerror() -> *const c_char;
}

unsafe fn open_lib(name: &str) -> *mut c_void {
    let c = CString::new(name).unwrap();
    let h = dlopen(c.as_ptr(), 2 | 0x100);
    if h.is_null() {
        panic!("dlopen {name}: {}", CStr::from_ptr(dlerror()).to_string_lossy());
    }
    h
}

unsafe fn sym<T: Copy>(lib: *mut c_void, name: &str) -> T {
    let c = CString::new(name).unwrap();
    let p = dlsym(lib, c.as_ptr());
    if p.is_null() {
        panic!("dlsym {name}: missing");
    }
    std::mem::transmute_copy(&p)
}

pub fn now_ns() -> u64 {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: a timespec of ours.
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64
}

// ---- the window's buffers ---------------------------------------------------

/// android_native_base_t: what an ANativeWindowBuffer begins with.
#[repr(C)]
struct NativeBase {
    magic: i32,
    version: i32,
    reserved: [*mut c_void; 4],
    inc_ref: extern "C" fn(*mut NativeBase),
    dec_ref: extern "C" fn(*mut NativeBase),
}

/// ANativeWindowBuffer as libhybris lays it out: the base, then width,
/// height, stride (pixels), format, usage; two reserved words; the
/// gralloc handle.
#[repr(C)]
struct WindowBuffer {
    base: NativeBase,
    width: i32,
    height: i32,
    stride: i32,
    format: i32,
    usage: i32,
    reserved: [*mut c_void; 2],
    handle: *const NativeHandle,
}

/// native_handle_t: then `num_fds` fds and `num_ints` ints. Qualcomm's
/// gralloc (private_handle_t): fds the buffer's and its metadata's, then
/// the magic ('gmsm') and the flags (UBWC: 0x08000000) first of the ints.
#[repr(C)]
struct NativeHandle {
    version: i32,
    num_fds: i32,
    num_ints: i32,
    data: [i32; 0],
}

const GRALLOC_MAGIC: i32 = 0x676d_736d;
const PRIV_FLAGS_UBWC_ALIGNED: i32 = 0x0800_0000;

#[derive(Clone, Copy)]
struct Win {
    create: extern "C" fn(u32, u32, u32, extern "C" fn(*mut c_void, *mut c_void, *mut c_void), *mut c_void) -> *mut c_void,
    set_buffer_count: extern "C" fn(*mut c_void, i32) -> i32,
    get_fence: extern "C" fn(*mut c_void) -> i32,
    set_fence: extern "C" fn(*mut c_void, i32),
}

#[derive(Default)]
struct Stats {
    frames: u32,
    sum_ms: f64,
    max_ms: f64,
    gpu_ns: u64,
    flip_ns: u64,
    errors: u32,
}

struct Presenter {
    win: Win,
    card: kms::Card,
    /// A framebuffer a buffer (by its address): imported once.
    fbs: HashMap<usize, u32>,
    setcrtc: bool,
    ubwc: Option<bool>,
    last: *mut c_void,
    last_ns: u64,
    order: Vec<usize>,
    stats: Stats,
}

// The presenter is only touched from the present callback, on the drawing
// thread, and from main between frames.
unsafe impl Send for Presenter {}

static PRESENTER: Mutex<Option<Presenter>> = Mutex::new(None);

/// The buffer's framebuffer: imported the first time it is seen.
fn framebuffer(p: &mut Presenter, buffer: *mut c_void) -> Result<u32, String> {
    if let Some(&fb) = p.fbs.get(&(buffer as usize)) {
        return Ok(fb);
    }
    // SAFETY: the window's buffers are ANativeWindowBuffers whose handle is
    // gralloc's native_handle_t, `num_fds + num_ints` ints after its head.
    let (w, h, stride, fd, ints) = unsafe {
        let b = &*(buffer as *const WindowBuffer);
        let nh = &*b.handle;
        let n = (nh.num_fds + nh.num_ints) as usize;
        let data = std::slice::from_raw_parts(nh.data.as_ptr(), n);
        println!(
            "buffer {:#x}: {}x{}, stride {}, format {}, usage {:#x}; handle v{} fds {} ints {}: {:?}",
            buffer as usize, b.width, b.height, b.stride, b.format, b.usage, nh.version, nh.num_fds, nh.num_ints,
            &data[..n.min(12)]
        );
        (b.width as u32, b.height as u32, b.stride as u32, data[0], data[nh.num_fds as usize..].to_vec())
    };
    let gralloc_ubwc = ints.first() == Some(&GRALLOC_MAGIC) && ints.get(1).is_some_and(|f| f & PRIV_FLAGS_UBWC_ALIGNED != 0);
    let ubwc = p.ubwc.unwrap_or(gralloc_ubwc);
    let fb = p.card.framebuffer(fd, w, h, stride * 4, ubwc.then_some(kms::MOD_QCOM_COMPRESSED))?;
    println!("buffer {:#x}: framebuffer {fb} (UBWC {ubwc}, gralloc says {gralloc_ubwc})", buffer as usize);
    p.fbs.insert(buffer as usize, fb);
    Ok(fb)
}

/// The window's present callback: called from inside eglSwapBuffers with the
/// buffer the GPU was given, whose fence says when it is drawn.
extern "C" fn present(_: *mut c_void, _window: *mut c_void, buffer: *mut c_void) {
    let mut guard = PRESENTER.lock().unwrap();
    let p = guard.as_mut().expect("presenter");
    if p.order.len() < 12 {
        p.order.push(buffer as usize);
    }

    // The GPU done with it.
    let t = now_ns();
    let fence = (p.win.get_fence)(buffer);
    if fence >= 0 {
        let mut pfd = libc::pollfd { fd: fence, events: libc::POLLIN, revents: 0 };
        // SAFETY: one pollfd; the fence is the window's to us, closed after.
        unsafe {
            libc::poll(&mut pfd, 1, 1000);
            libc::close(fence);
        }
        (p.win.set_fence)(buffer, -1);
    }
    p.stats.gpu_ns += now_ns() - t;

    let shown = framebuffer(p, buffer).and_then(|fb| {
        let setcrtc = p.setcrtc;
        p.card.show(fb, setcrtc)
    });
    match shown {
        Ok(ns) => p.stats.flip_ns += ns,
        Err(e) => {
            if p.stats.errors < 5 {
                eprintln!("kms-frame: {e}");
            }
            p.stats.errors += 1;
        }
    }

    // The buffer on screen kept until the next one is: the window may not
    // hand it out again while it shows.
    // SAFETY: both are the window's ANativeWindowBuffers (NativeBase
    // first); the kept one lives from its inc_ref to the dec_ref here.
    unsafe {
        if !p.last.is_null() {
            let base = p.last as *mut NativeBase;
            ((*base).dec_ref)(base);
        }
        let base = buffer as *mut NativeBase;
        ((*base).inc_ref)(base);
    }
    p.last = buffer;

    let now = now_ns();
    if p.last_ns != 0 {
        let ms = (now - p.last_ns) as f64 / 1e6;
        p.stats.frames += 1;
        p.stats.sum_ms += ms;
        p.stats.max_ms = p.stats.max_ms.max(ms);
    }
    p.last_ns = now;
}

// ---- EGL and GLES -------------------------------------------------------------

const EGL_BUFFER_SIZE: i32 = 0x3020;
const EGL_RENDERABLE_TYPE: i32 = 0x3040;
const EGL_OPENGL_ES2_BIT: i32 = 0x0004;
const EGL_NONE: i32 = 0x3038;
const EGL_CONTEXT_CLIENT_VERSION: i32 = 0x3098;
const GL_COLOR_BUFFER_BIT: u32 = 0x4000;
const GL_SCISSOR_TEST: u32 = 0x0C11;
const GL_VERSION: u32 = 0x1F02;
const GL_RENDERER: u32 = 0x1F01;
const HAL_PIXEL_FORMAT_RGBA_8888: u32 = 1;

#[derive(Clone, Copy)]
struct Egl {
    get_display: extern "C" fn(*mut c_void) -> *mut c_void,
    initialize: extern "C" fn(*mut c_void, *mut i32, *mut i32) -> u32,
    choose_config: extern "C" fn(*mut c_void, *const i32, *mut *mut c_void, i32, *mut i32) -> u32,
    create_window_surface: extern "C" fn(*mut c_void, *mut c_void, *mut c_void, *const i32) -> *mut c_void,
    create_context: extern "C" fn(*mut c_void, *mut c_void, *mut c_void, *const i32) -> *mut c_void,
    make_current: extern "C" fn(*mut c_void, *mut c_void, *mut c_void, *mut c_void) -> u32,
    swap_interval: extern "C" fn(*mut c_void, i32) -> u32,
    swap_buffers: extern "C" fn(*mut c_void, *mut c_void) -> u32,
    get_error: extern "C" fn() -> i32,
}

#[derive(Clone, Copy)]
struct Gl {
    clear_color: extern "C" fn(f32, f32, f32, f32),
    clear: extern "C" fn(u32),
    enable: extern "C" fn(u32),
    disable: extern "C" fn(u32),
    scissor: extern "C" fn(i32, i32, i32, i32),
    viewport: extern "C" fn(i32, i32, i32, i32),
    get_string: extern "C" fn(u32) -> *const c_char,
}

impl Gl {
    /// A rectangle in `rgb`, from the frame's top-left (GL's y is up).
    fn rect(&self, h: i32, x: i32, y: i32, w: i32, rh: i32, rgb: [f32; 3]) {
        (self.scissor)(x, h - y - rh, w, rh);
        (self.clear_color)(rgb[0], rgb[1], rgb[2], 1.0);
        (self.clear)(GL_COLOR_BUFFER_BIT);
    }
}

/// The test picture on one panel at `x0`, `pw` x `h`: as item-recovery's
/// (grey steps, the darks 0..15 of 255, colour bars, their halves).
fn test_picture(gl: &Gl, x0: i32, pw: i32, h: i32) {
    for i in 0..16 {
        let (x, w) = (x0 + i * pw / 16, pw / 16 + 1);
        let v = i as f32 / 15.0;
        gl.rect(h, x, 0, w, h / 4, [v, v, v]);
        let d = i as f32 / 255.0;
        gl.rect(h, x, h / 4, w, h / 4, [d, d, d]);
    }
    const BARS: [[f32; 3]; 7] = [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.], [0., 1., 1.], [1., 0., 1.], [1., 1., 0.], [1., 1., 1.]];
    for (i, c) in BARS.iter().enumerate() {
        let (x, w) = (x0 + i as i32 * pw / 7, pw / 7 + 1);
        gl.rect(h, x, h / 2, w, h / 4, *c);
        gl.rect(h, x, h * 3 / 4, w, h - h * 3 / 4, [c[0] * 0.5, c[1] * 0.5, c[2] * 0.5]);
    }
}

fn main() {
    let seconds: u64 = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(20);
    let mode = std::env::args().nth(2).unwrap_or_else(|| "vivid".into());
    let setcrtc = std::env::var("KMS_SETCRTC").is_ok_and(|v| v == "1");
    let ubwc = std::env::var("KMS_UBWC").ok().map(|v| v == "1");

    let card = kms::Card::open().unwrap_or_else(|e| panic!("{e}"));
    for (i, p) in card.panels.iter().enumerate() {
        println!("panel {i}: connector {}, crtc {}, {}x{} at {} Hz, from x {}", p.connector, p.crtc, p.mode.hdisplay, p.mode.vdisplay, p.mode.vrefresh, p.x);
    }
    // The factory calibration: R2's file the left panel's (seen so on the
    // owner's Duo, 2026-10-29), C3's the right's.
    for (i, p) in card.panels.iter().enumerate() {
        let calib = if mode == "off" { None } else { colour::load(if i == 0 { "r2" } else { "c3" }, &mode).map_err(|e| eprintln!("kms-frame: {e}")).ok() };
        let blobs = [
            ("SDE_DSPP_IGC_V3", calib.as_ref().map(|c| colour::igc_blob(c, "rgb"))),
            ("SDE_DSPP_PCC_V4", calib.as_ref().map(colour::pcc_blob)),
            ("SDE_DSPP_GC_V1", calib.as_ref().map(|c| colour::gc_blob(c, "rgb"))),
        ];
        let said: Vec<String> = blobs.iter().map(|(name, b)| match card.set_blob(p.crtc, name, b.as_deref()) {
            Ok(()) => format!("{} {}", &name[9..], if b.is_some() { "on" } else { "off" }),
            Err(e) => e,
        }).collect();
        println!("panel {i}: colour {mode}: {}", said.join(", "));
    }
    // The display's clock and its share of the memory's bandwidth: what
    // hwcomposer votes each frame on each CRTC (read under item,
    // 2026-10-29). Without them every commit after the first was refused
    // ("exceeds bandwidth: 45600000kb > 22800000kb", E2BIG).
    const PERF: [(&str, u64); 7] = [
        ("core_clk", 187_920_000),
        ("core_ab", 1_164_041_856),
        ("core_ib", 2_500_000_000),
        ("llcc_ab", 1_164_041_856),
        ("llcc_ib", 0),
        ("dram_ab", 1_164_041_856),
        ("dram_ib", 1_600_000_000),
    ];
    let perf = std::env::var("KMS_PERF").map_or(true, |v| v != "0");
    for (i, p) in card.panels.iter().enumerate() {
        let left: Vec<String> = PERF.iter().map(|(n, _)| format!("{n} {}", card.get(p.crtc, n).unwrap_or_default())).collect();
        println!("panel {i}: as hwcomposer left it: {}", left.join(", "));
        if perf {
            for (n, v) in PERF {
                if let Err(e) = card.set(p.crtc, n, v) {
                    eprintln!("kms-frame: {e}");
                }
            }
        }
    }
    let width = card.panels.iter().map(|p| p.mode.hdisplay as i32).sum::<i32>();
    let height = card.panels.iter().map(|p| p.mode.vdisplay as i32).max().unwrap();
    let halves: Vec<(i32, i32)> = card.panels.iter().map(|p| (p.x as i32, p.mode.hdisplay as i32)).collect();

    let (egl, gl, win) = unsafe {
        let libegl = open_lib("libEGL.so.1");
        let libgl = open_lib("libGLESv2.so.2");
        let hwcw = open_lib("libhybris-hwcomposerwindow.so.1");
        (
            Egl {
                get_display: sym(libegl, "eglGetDisplay"),
                initialize: sym(libegl, "eglInitialize"),
                choose_config: sym(libegl, "eglChooseConfig"),
                create_window_surface: sym(libegl, "eglCreateWindowSurface"),
                create_context: sym(libegl, "eglCreateContext"),
                make_current: sym(libegl, "eglMakeCurrent"),
                swap_interval: sym(libegl, "eglSwapInterval"),
                swap_buffers: sym(libegl, "eglSwapBuffers"),
                get_error: sym(libegl, "eglGetError"),
            },
            Gl {
                clear_color: sym(libgl, "glClearColor"),
                clear: sym(libgl, "glClear"),
                enable: sym(libgl, "glEnable"),
                disable: sym(libgl, "glDisable"),
                scissor: sym(libgl, "glScissor"),
                viewport: sym(libgl, "glViewport"),
                get_string: sym(libgl, "glGetString"),
            },
            Win {
                create: sym(hwcw, "HWCNativeWindowCreate"),
                set_buffer_count: sym(hwcw, "HWCNativeWindowSetBufferCount"),
                get_fence: sym(hwcw, "HWCNativeBufferGetFence"),
                set_fence: sym(hwcw, "HWCNativeBufferSetFence"),
            },
        )
    };

    *PRESENTER.lock().unwrap() = Some(Presenter { win, card, fbs: HashMap::new(), setcrtc, ubwc, last: null_mut(), last_ns: 0, order: Vec::new(), stats: Stats::default() });
    let window = (win.create)(width as u32, height as u32, HAL_PIXEL_FORMAT_RGBA_8888, present, null_mut());
    assert!(!window.is_null(), "HWCNativeWindowCreate failed");
    (win.set_buffer_count)(window, 3);
    println!("frame {width}x{height}, {} panels; flips: {}", halves.len(), if setcrtc { "SETCRTC every frame" } else { "page flips" });

    let dpy = (egl.get_display)(null_mut());
    assert!(!dpy.is_null(), "eglGetDisplay failed");
    assert!((egl.initialize)(dpy, null_mut(), null_mut()) == 1, "eglInitialize: {:#x}", (egl.get_error)());
    let attrs = [EGL_BUFFER_SIZE, 32, EGL_RENDERABLE_TYPE, EGL_OPENGL_ES2_BIT, EGL_NONE];
    let mut config = null_mut();
    let mut n = 0;
    (egl.choose_config)(dpy, attrs.as_ptr(), &mut config, 1, &mut n);
    assert!(n > 0, "no EGL config: {:#x}", (egl.get_error)());
    let surface = (egl.create_window_surface)(dpy, config, window, std::ptr::null());
    assert!(!surface.is_null(), "eglCreateWindowSurface: {:#x}", (egl.get_error)());
    let ctx_attrs = [EGL_CONTEXT_CLIENT_VERSION, 2, EGL_NONE];
    let ctx = (egl.create_context)(dpy, config, null_mut(), ctx_attrs.as_ptr());
    assert!(!ctx.is_null(), "eglCreateContext: {:#x}", (egl.get_error)());
    assert!((egl.make_current)(dpy, surface, surface, ctx) == 1, "eglMakeCurrent: {:#x}", (egl.get_error)());
    (egl.swap_interval)(dpy, 1);
    unsafe {
        println!("GL: {} / {}", CStr::from_ptr((gl.get_string)(GL_VERSION)).to_string_lossy(), CStr::from_ptr((gl.get_string)(GL_RENDERER)).to_string_lossy());
    }
    (gl.viewport)(0, 0, width, height);

    let start = now_ns();
    let mut next_report = start + 1_000_000_000;
    let bar = 24;
    loop {
        let t = (now_ns() - start) as f64 / 1e9;
        if t > seconds as f64 {
            break;
        }
        (gl.enable)(GL_SCISSOR_TEST);
        for &(x0, pw) in &halves {
            test_picture(&gl, x0, pw, height);
        }
        // The bar: across both panels once every two seconds, over the
        // darks only (the steps and bars left whole to look at).
        let x = ((t / 2.0).fract() * (width - bar) as f64) as i32;
        gl.rect(height, x, height / 4, bar, height / 4, [1.0, 1.0, 1.0]);
        (gl.disable)(GL_SCISSOR_TEST);
        (egl.swap_buffers)(dpy, surface);

        let now = now_ns();
        if now >= next_report {
            let mut guard = PRESENTER.lock().unwrap();
            let p = guard.as_mut().unwrap();
            let st = std::mem::take(&mut p.stats);
            let f = st.frames.max(1) as f64;
            println!(
                "{:5.1} s: {:3} fps  mean {:5.2} ms  max {:6.2} ms  gpu wait {:5.2} ms  flip wait {:5.2} ms  errors {}",
                t, st.frames, st.sum_ms / f, st.max_ms, st.gpu_ns as f64 / 1e6 / f, st.flip_ns as f64 / 1e6 / f, st.errors
            );
            if t < 1.5 {
                println!("buffers in the order presented: {:x?}", p.order);
            }
            next_report += 1_000_000_000;
        }
    }
    println!("done");
}
