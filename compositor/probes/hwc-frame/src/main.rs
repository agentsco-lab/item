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

use hybris_hwc::{now_ns, take_stats, vsyncs, Output};

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

fn main() {
    let seconds: u64 = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(20);

    let (egl, gl) = unsafe {
        let libegl = open_lib("libEGL.so.1");
        let libgl = open_lib("libGLESv2.so.2");
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
        )
    };

    let out = Output::open(3);
    let (width, height, window) = (out.width, out.height, out.window);
    println!("display 0: {}x{}, vsync period {:.3} ms ({:.2} Hz)",
             width, height, out.vsync_period_ns as f64 / 1e6, 1e9 / out.vsync_period_ns as f64);

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
    let mut vsyncs_at_report = vsyncs();
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
            let v = vsyncs();
            let st = take_stats();
            let f = st.frames.max(1) as f64;
            println!("{:5.1} s: {:3} fps  mean {:5.2} ms  max {:6.2} ms  over 20 ms {:2}  swap {:5.2} ms  vsyncs {:3}  errors {}",
                     t, st.frames, st.mean_ms, st.max_ms, st.over_20ms,
                     swap_ns_sum as f64 / 1e6 / f, v - vsyncs_at_report, st.errors);
            swap_ns_sum = 0;
            vsyncs_at_report = v;
            next_report += 1_000_000_000;
        }
    }
    println!("done");
}
