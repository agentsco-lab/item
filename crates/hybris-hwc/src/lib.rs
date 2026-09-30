//! hwcomposer through libhybris, for the Surface Duo: display 0 from hwc2 with
//! one client layer over it, and a native window whose buffers go to that
//! layer with their fences. EGL draws into the window; this crate presents.
//!
//! The recipe is libhybris' own `test_hwcomposer` (tests/test_common.cpp).
//! The libraries are opened with dlopen, so a program using this crate
//! cross-builds on the PC with nothing of the phone's but glibc.

#![allow(clippy::missing_safety_doc)]

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
    present_or_validate: extern "C" fn(*mut Disp, *mut u32, *mut u32, *mut i32, *mut u32) -> i32,
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
    fast: u32,
    frames: u32,
    long: u32,
    sum_ms: f64,
    max_ms: f64,
    errors: u32,
}

static PRESENTER: Mutex<Option<Presenter>> = Mutex::new(None);
static HWC: Mutex<Option<Hwc>> = Mutex::new(None);

/// Called on hwcomposer's thread at each vsync, with its timestamp (ns).
static VSYNC_HANDLER: std::sync::OnceLock<Box<dyn Fn(i64) + Send + Sync>> = std::sync::OnceLock::new();

extern "C" fn on_vsync(_: *mut Listener, _: i32, _: u64, timestamp: i64) {
    VSYNCS.fetch_add(1, Ordering::Relaxed);
    LAST_VSYNC_NS.store(timestamp as u64, Ordering::Relaxed);
    if let Some(handler) = VSYNC_HANDLER.get() {
        handler(timestamp);
    }
}

static LAST_VSYNC_NS: AtomicU64 = AtomicU64::new(0);

/// Sets what runs at each vsync, on hwcomposer's thread; once.
pub fn set_vsync_handler(handler: impl Fn(i64) + Send + Sync + 'static) {
    let _ = VSYNC_HANDLER.set(Box::new(handler));
}

/// The last vsync's timestamp, in hwcomposer's clock (CLOCK_MONOTONIC, ns).
pub fn last_vsync_ns() -> u64 {
    LAST_VSYNC_NS.load(Ordering::Relaxed)
}

/// The time of the last present to hwcomposer (CLOCK_MONOTONIC, ns).
pub fn last_present_ns() -> u64 {
    LAST_PRESENT_NS.load(Ordering::Relaxed)
}

static LAST_PRESENT_NS: AtomicU64 = AtomicU64::new(0);
static LAST_PRESENT_TOOK_NS: AtomicU64 = AtomicU64::new(0);

/// Whether to try presentOrValidate first (`HWC_PRESENT_OR_VALIDATE=1`). Off:
/// on the Surface Duo it killed the compositor at its first frame (the
/// Android side of libhybris' hwc2 layer, Halium 11).
static PRESENT_OR_VALIDATE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// How long the last present callback took: validate, set the client target,
/// present, and the fences (ns).
pub fn last_present_took_ns() -> u64 {
    LAST_PRESENT_TOOK_NS.load(Ordering::Relaxed)
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

    let t_start = now_ns();
    let acquire = (w.get_fence)(buffer);
    (h.set_client_target)(p.display, 0, buffer, acquire, HAL_DATASPACE_UNKNOWN);

    // presentOrValidate presents at once when nothing in the layers changed
    // (state 1) - one client layer here, which never does - and only
    // validates otherwise (state 0), when accept and present follow as before.
    let mut types = 0u32;
    let mut requests = 0u32;
    let mut present_fence: i32 = -1;
    let mut state = 0u32;
    let fast = PRESENT_OR_VALIDATE.load(Ordering::Relaxed)
        && (h.present_or_validate)(p.display, &mut types, &mut requests, &mut present_fence, &mut state)
            == HWC2_ERROR_NONE
        && state == 1;
    if fast {
        p.stats.fast += 1;
    } else {
        if !PRESENT_OR_VALIDATE.load(Ordering::Relaxed) {
            let err = (h.validate)(p.display, &mut types, &mut requests);
            if err != HWC2_ERROR_NONE && err != HWC2_ERROR_HAS_CHANGES {
                p.stats.errors += 1;
                return;
            }
        }
        if types != 0 || requests != 0 {
            (h.accept_changes)(p.display);
        }
        present_fence = -1;
        if (h.present)(p.display, &mut present_fence) != HWC2_ERROR_NONE {
            p.stats.errors += 1;
        }
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
    LAST_PRESENT_NS.store(now, Ordering::Relaxed);
    LAST_PRESENT_TOOK_NS.store(now - t_start, Ordering::Relaxed);
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


/// hwcomposer's display 0, ready for a native window to present into.
pub struct Output {
    pub width: i32,
    pub height: i32,
    /// hwcomposer's vsync period in nanoseconds.
    pub vsync_period_ns: i64,
    /// An ANativeWindow for `eglCreateWindowSurface`: each buffer EGL swaps
    /// into it is presented on display 0.
    pub window: *mut c_void,
}

/// Frames presented since the last call: count, times between them, errors.
#[derive(Default, Debug, Clone, Copy)]
pub struct FrameStats {
    /// Frames presented at once by presentOrValidate, without a validate.
    pub fast: u32,
    pub frames: u32,
    pub over_20ms: u32,
    pub mean_ms: f64,
    pub max_ms: f64,
    pub errors: u32,
}

/// vsyncs hwcomposer has reported so far.
pub fn vsyncs() -> u64 {
    VSYNCS.load(Ordering::Relaxed)
}

pub fn take_stats() -> FrameStats {
    let mut guard = PRESENTER.lock().unwrap();
    let st = std::mem::take(&mut guard.as_mut().expect("presenter").stats);
    let f = st.frames.max(1) as f64;
    FrameStats { fast: st.fast, frames: st.frames, over_20ms: st.long, mean_ms: st.sum_ms / f, max_ms: st.max_ms, errors: st.errors }
}

pub fn now_ns() -> u64 {
    let mut ts = Timespec { sec: 0, nsec: 0 };
    unsafe { clock_gettime(CLOCK_MONOTONIC, &mut ts) };
    ts.sec as u64 * 1_000_000_000 + ts.nsec as u64
}

impl Output {
    /// Opens hwc2, waits for display 0, powers it on, puts one client layer
    /// over it and makes a native window with `buffers` buffers.
    pub fn open(buffers: i32) -> Output {
        let (hwc, win) = unsafe {
            let hwc2 = open_lib("libhwc2.so.1");
            let hwcw = open_lib("libhybris-hwcomposerwindow.so.1");
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
                    present_or_validate: sym(hwc2, "hwc2_compat_display_present_or_validate"),
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
            )
        };
        *HWC.lock().unwrap() = Some(hwc);
        PRESENT_OR_VALIDATE.store(
            std::env::var("HWC_PRESENT_OR_VALIDATE").map(|v| v == "1").unwrap_or(false),
            Ordering::Relaxed,
        );

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
        let (width, height, vsync_period_ns) = (cfg.width, cfg.height, cfg.vsync_period);

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

        let window = (win.create)(width as u32, height as u32, HAL_PIXEL_FORMAT_RGBA_8888, present, null_mut());
        assert!(!window.is_null(), "HWCNativeWindowCreate failed");
        (win.set_buffer_count)(window, buffers);

        Output { width, height, vsync_period_ns, window }
    }
}
