//! Probe B: a GL frame on the Surface Duo's panels through libhybris' hwc2,
//! at vsync, from Rust.
//!
//! The recipe is libhybris' own `test_hwcomposer` (tests/test_common.cpp):
//! display 0 from hwc2, one client layer over it, a native window from
//! `HWCNativeWindowCreate` under an EGL surface, and in the window's present
//! callback: the buffer's acquire fence into `set_client_target`, `present`,
//! and the release fences back onto the buffers.
//!
//! It draws a white bar sweeping across all 2784 columns (both panels and the
//! hinge) over a slowly changing background, and prints once a second: frames,
//! the mean and longest time between presents, the frames over 20 ms, the time
//! spent in eglSwapBuffers, and hwcomposer's vsyncs.
//!
//! The libraries are opened with dlopen, so the binary cross-builds on the PC
//! with nothing of the phone's but glibc.
//!
//! Run it with the shell stopped (it takes hwcomposer's display):
//!   EGL_PLATFORM=hwcomposer hwc-frame [seconds]

use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::ptr::null_mut;
use std::sync::atomic::{AtomicPtr, AtomicU64, Ordering};
use std::sync::Mutex;

// ---- libc ------------------------------------------------------------------

#[repr(C)]
struct Timespec {
    sec: i64,
    nsec: i64,
}

extern "C" {
    fn dlopen(file: *const c_char, flags: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, name: *const c_char) -> *mut c_void;
    fn dlerror() -> *const c_char;
    fn close(fd: c_int) -> c_int;
    fn clock_gettime(clock: c_int, ts: *mut Timespec) -> c_int;
}

const RTLD_NOW: c_int = 2;
const RTLD_GLOBAL: c_int = 0x100;
const CLOCK_MONOTONIC: c_int = 1;

fn now_ns() -> u64 {
    let mut ts = Timespec { sec: 0, nsec: 0 };
    unsafe { clock_gettime(CLOCK_MONOTONIC, &mut ts) };
    ts.sec as u64 * 1_000_000_000 + ts.nsec as u64
}

unsafe fn open_lib(name: &str) -> *mut c_void {
    let c = CString::new(name).unwrap();
    let h = dlopen(c.as_ptr(), RTLD_NOW | RTLD_GLOBAL);
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

// ---- hwc2 (libhwc2, hwc2_compatibility_layer.h) -----------------------------

type Dev = c_void;
type Disp = c_void;
type Layer = c_void;
type Fences = c_void;

#[repr(C)]
struct Listener {
    on_vsync: extern "C" fn(*mut Listener, i32, u64, i64),
    on_hotplug: extern "C" fn(*mut Listener, i32, u64, bool, bool),
    on_refresh: extern "C" fn(*mut Listener, i32, u64),
}

#[repr(C)]
struct DisplayConfig {
    id: u32,
    display: u64,
    width: i32,
    height: i32,
    vsync_period: i64,
    dpi_x: f32,
    dpi_y: f32,
}

const HWC2_ERROR_NONE: i32 = 0;
const HWC2_ERROR_HAS_CHANGES: i32 = 5;
const HWC2_POWER_MODE_ON: i32 = 2;
const HWC2_VSYNC_ENABLE: i32 = 1;
const HWC2_COMPOSITION_CLIENT: i32 = 1;
const HWC2_BLEND_MODE_NONE: i32 = 1;
const HAL_PIXEL_FORMAT_RGBA_8888: u32 = 1;
const HAL_DATASPACE_UNKNOWN: i32 = 0;

#[derive(Clone, Copy)]
struct Hwc {
    device_new: extern "C" fn(bool) -> *mut Dev,
    register_callback: extern "C" fn(*mut Dev, *mut Listener, i32),
    on_hotplug: extern "C" fn(*mut Dev, u64, bool),
    display_by_id: extern "C" fn(*mut Dev, u64) -> *mut Disp,
    active_config: extern "C" fn(*mut Disp) -> *mut DisplayConfig,
    set_power_mode: extern "C" fn(*mut Disp, i32) -> i32,
    set_vsync_enabled: extern "C" fn(*mut Disp, i32) -> i32,
    create_layer: extern "C" fn(*mut Disp) -> *mut Layer,
    layer_composition: extern "C" fn(*mut Layer, i32) -> i32,
    layer_blend: extern "C" fn(*mut Layer, i32) -> i32,
    layer_crop: extern "C" fn(*mut Layer, f32, f32, f32, f32) -> i32,
    layer_frame: extern "C" fn(*mut Layer, i32, i32, i32, i32) -> i32,
    layer_visible: extern "C" fn(*mut Layer, i32, i32, i32, i32) -> i32,
    validate: extern "C" fn(*mut Disp, *mut u32, *mut u32) -> i32,
    accept_changes: extern "C" fn(*mut Disp) -> i32,
    set_client_target: extern "C" fn(*mut Disp, u32, *mut c_void, i32, i32) -> i32,
    present: extern "C" fn(*mut Disp, *mut i32) -> i32,
    release_fences: extern "C" fn(*mut Disp, *mut *mut Fences) -> i32,
    fence_of: extern "C" fn(*mut Fences, *mut Layer) -> i32,
    fences_destroy: extern "C" fn(*mut Fences),
}

// ---- the native window (libhybris-hwcomposerwindow, hwcomposer.h) ------------

type PresentCb = extern "C" fn(*mut c_void, *mut c_void, *mut c_void);

#[derive(Clone, Copy)]
struct Win {
    create: extern "C" fn(u32, u32, u32, PresentCb, *mut c_void) -> *mut c_void,
    set_buffer_count: extern "C" fn(*mut c_void, c_int) -> c_int,
    get_fence: extern "C" fn(*mut c_void) -> c_int,
    set_fence: extern "C" fn(*mut c_void, c_int),
}

/// The start of an ANativeWindowBuffer: android_native_base_t, whose incRef
/// and decRef keep a presented buffer alive until the next present.
#[repr(C)]
struct NativeBase {
    magic: i32,
    version: i32,
    reserved: [*mut c_void; 4],
    inc_ref: extern "C" fn(*mut NativeBase),
    dec_ref: extern "C" fn(*mut NativeBase),
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

// ---- shared state for the callbacks -------------------------------------------

static DEVICE: AtomicPtr<Dev> = AtomicPtr::new(null_mut());
static VSYNCS: AtomicU64 = AtomicU64::new(0);

struct Presenter {
    hwc: Hwc,
    win: Win,
    display: *mut Disp,
    layer: *mut Layer,
    last: *mut c_void,
    last_ns: u64,
    stats: Stats,
}
unsafe impl Send for Presenter {}

#[derive(Default)]
struct Stats {
    frames: u32,
    long: u32,
    sum_ms: f64,
    max_ms: f64,
    errors: u32,
}

static PRESENTER: Mutex<Option<Presenter>> = Mutex::new(None);
static HWC: Mutex<Option<Hwc>> = Mutex::new(None);

extern "C" fn on_vsync(_: *mut Listener, _: i32, _: u64, _: i64) {
    VSYNCS.fetch_add(1, Ordering::Relaxed);
}

extern "C" fn on_hotplug(_: *mut Listener, _: i32, display: u64, connected: bool, primary: bool) {
    println!("hotplug: display {display} {} {}",
             if connected { "connected" } else { "disconnected" },
             if primary { "primary" } else { "external" });
    let dev = DEVICE.load(Ordering::Acquire);
    if let (false, Some(hwc)) = (dev.is_null(), *HWC.lock().unwrap()) {
        (hwc.on_hotplug)(dev, display, connected);
    }
}

extern "C" fn on_refresh(_: *mut Listener, _: i32, _: u64) {}

/// The window's present callback: called from inside eglSwapBuffers with the
/// buffer the GPU was given, whose fence says when it is drawn.
extern "C" fn present(_: *mut c_void, _window: *mut c_void, buffer: *mut c_void) {
    let mut guard = PRESENTER.lock().unwrap();
    let p = guard.as_mut().expect("presenter");
    let (h, w) = (p.hwc, p.win);

    let mut types = 0u32;
    let mut requests = 0u32;
    let err = (h.validate)(p.display, &mut types, &mut requests);
    if err != HWC2_ERROR_NONE && err != HWC2_ERROR_HAS_CHANGES {
        p.stats.errors += 1;
        return;
    }
    if types != 0 || requests != 0 {
        (h.accept_changes)(p.display);
    }

    let acquire = (w.get_fence)(buffer);
    (h.set_client_target)(p.display, 0, buffer, acquire, HAL_DATASPACE_UNKNOWN);

    let mut present_fence: i32 = -1;
    if (h.present)(p.display, &mut present_fence) != HWC2_ERROR_NONE {
        p.stats.errors += 1;
    }

    let mut fences: *mut Fences = null_mut();
    if (h.release_fences)(p.display, &mut fences) == HWC2_ERROR_NONE && !fences.is_null() {
        let release = (h.fence_of)(fences, p.layer);
        (w.set_fence)(buffer, release);
        (h.fences_destroy)(fences);
    }

    // The present fence signals when this frame is on screen and the last
    // one is free again.
    unsafe {
        if !p.last.is_null() {
            let old = (w.get_fence)(p.last);
            if old != -1 {
                close(old);
            }
            (w.set_fence)(p.last, present_fence);
            let base = p.last as *mut NativeBase;
            ((*base).dec_ref)(base);
        } else if present_fence != -1 {
            close(present_fence);
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
        if ms > 20.0 {
            p.stats.long += 1;
        }
    }
    p.last_ns = now;
}

fn main() {
    let seconds: u64 = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(20);

    let (hwc, win, egl, gl) = unsafe {
        let hwc2 = open_lib("libhwc2.so.1");
        let hwcw = open_lib("libhybris-hwcomposerwindow.so.1");
        let libegl = open_lib("libEGL.so.1");
        let libgl = open_lib("libGLESv2.so.2");
        (
            Hwc {
                device_new: sym(hwc2, "hwc2_compat_device_new"),
                register_callback: sym(hwc2, "hwc2_compat_device_register_callback"),
                on_hotplug: sym(hwc2, "hwc2_compat_device_on_hotplug"),
                display_by_id: sym(hwc2, "hwc2_compat_device_get_display_by_id"),
                active_config: sym(hwc2, "hwc2_compat_display_get_active_config"),
                set_power_mode: sym(hwc2, "hwc2_compat_display_set_power_mode"),
                set_vsync_enabled: sym(hwc2, "hwc2_compat_display_set_vsync_enabled"),
                create_layer: sym(hwc2, "hwc2_compat_display_create_layer"),
                layer_composition: sym(hwc2, "hwc2_compat_layer_set_composition_type"),
                layer_blend: sym(hwc2, "hwc2_compat_layer_set_blend_mode"),
                layer_crop: sym(hwc2, "hwc2_compat_layer_set_source_crop"),
                layer_frame: sym(hwc2, "hwc2_compat_layer_set_display_frame"),
                layer_visible: sym(hwc2, "hwc2_compat_layer_set_visible_region"),
                validate: sym(hwc2, "hwc2_compat_display_validate"),
                accept_changes: sym(hwc2, "hwc2_compat_display_accept_changes"),
                set_client_target: sym(hwc2, "hwc2_compat_display_set_client_target"),
                present: sym(hwc2, "hwc2_compat_display_present"),
                release_fences: sym(hwc2, "hwc2_compat_display_get_release_fences"),
                fence_of: sym(hwc2, "hwc2_compat_out_fences_get_fence"),
                fences_destroy: sym(hwc2, "hwc2_compat_out_fences_destroy"),
            },
            Win {
                create: sym(hwcw, "HWCNativeWindowCreate"),
                set_buffer_count: sym(hwcw, "HWCNativeWindowSetBufferCount"),
                get_fence: sym(hwcw, "HWCNativeBufferGetFence"),
                set_fence: sym(hwcw, "HWCNativeBufferSetFence"),
            },
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
        )
    };
    *HWC.lock().unwrap() = Some(hwc);

    // hwc2: the device, its callbacks, display 0 once hotplugged.
    let listener = Box::leak(Box::new(Listener { on_vsync, on_hotplug, on_refresh }));
    let dev = (hwc.device_new)(false);
    assert!(!dev.is_null(), "hwc2_compat_device_new failed");
    DEVICE.store(dev, Ordering::Release);
    (hwc.register_callback)(dev, listener, 0);

    let mut display = null_mut();
    for _ in 0..5000 {
        display = (hwc.display_by_id)(dev, 0);
        if !display.is_null() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert!(!display.is_null(), "no hwc display 0 after 5 s");

    (hwc.set_power_mode)(display, HWC2_POWER_MODE_ON);
    let cfg = unsafe { &*(hwc.active_config)(display) };
    let (width, height) = (cfg.width, cfg.height);
    println!("display 0: {}x{}, vsync period {:.3} ms ({:.2} Hz)",
             width, height, cfg.vsync_period as f64 / 1e6, 1e9 / cfg.vsync_period as f64);

    let layer = (hwc.create_layer)(display);
    (hwc.layer_composition)(layer, HWC2_COMPOSITION_CLIENT);
    (hwc.layer_blend)(layer, HWC2_BLEND_MODE_NONE);
    (hwc.layer_crop)(layer, 0.0, 0.0, width as f32, height as f32);
    (hwc.layer_frame)(layer, 0, 0, width, height);
    (hwc.layer_visible)(layer, 0, 0, width, height);
    (hwc.set_vsync_enabled)(display, HWC2_VSYNC_ENABLE);

    *PRESENTER.lock().unwrap() = Some(Presenter {
        hwc,
        win,
        display,
        layer,
        last: null_mut(),
        last_ns: 0,
        stats: Stats::default(),
    });

    // The native window and EGL on it.
    let window = (win.create)(width as u32, height as u32, HAL_PIXEL_FORMAT_RGBA_8888, present, null_mut());
    assert!(!window.is_null(), "HWCNativeWindowCreate failed");
    (win.set_buffer_count)(window, 3);

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
        println!("GL: {} / {}",
                 CStr::from_ptr((gl.get_string)(GL_VERSION)).to_string_lossy(),
                 CStr::from_ptr((gl.get_string)(GL_RENDERER)).to_string_lossy());
    }
    (gl.viewport)(0, 0, width, height);

    // Draw: a background that drifts in colour, the hinge's columns black, and
    // a bar sweeping across all columns once every two seconds.
    let start = now_ns();
    let mut next_report = start + 1_000_000_000;
    let mut swap_ns_sum = 0u64;
    let mut vsyncs_at_report = VSYNCS.load(Ordering::Relaxed);
    let hinge = (1350, 1434);
    let bar = 48;
    loop {
        let t = (now_ns() - start) as f64 / 1e9;
        if t > seconds as f64 {
            break;
        }
        let r = 0.15 + 0.1 * (t * 0.7).sin() as f32;
        let g = 0.2 + 0.1 * (t * 0.5).cos() as f32;
        (gl.disable)(GL_SCISSOR_TEST);
        (gl.clear_color)(r, g, 0.35, 1.0);
        (gl.clear)(GL_COLOR_BUFFER_BIT);

        (gl.enable)(GL_SCISSOR_TEST);
        (gl.scissor)(hinge.0, 0, hinge.1 - hinge.0, height);
        (gl.clear_color)(0.0, 0.0, 0.0, 1.0);
        (gl.clear)(GL_COLOR_BUFFER_BIT);

        let x = ((t / 2.0).fract() * (width - bar) as f64) as i32;
        (gl.scissor)(x, 0, bar, height);
        (gl.clear_color)(1.0, 1.0, 1.0, 1.0);
        (gl.clear)(GL_COLOR_BUFFER_BIT);

        let s = now_ns();
        (egl.swap_buffers)(dpy, surface);
        swap_ns_sum += now_ns() - s;

        let now = now_ns();
        if now >= next_report {
            let v = VSYNCS.load(Ordering::Relaxed);
            let mut guard = PRESENTER.lock().unwrap();
            let st = std::mem::take(&mut guard.as_mut().unwrap().stats);
            drop(guard);
            let f = st.frames.max(1) as f64;
            println!("{:5.1} s: {:3} fps  mean {:5.2} ms  max {:6.2} ms  over 20 ms {:2}  swap {:5.2} ms  vsyncs {:3}  errors {}",
                     t, st.frames, st.sum_ms / f, st.max_ms, st.long,
                     swap_ns_sum as f64 / 1e6 / f, v - vsyncs_at_report, st.errors);
            swap_ns_sum = 0;
            vsyncs_at_report = v;
            next_report += 1_000_000_000;
        }
    }
    println!("done");
}
