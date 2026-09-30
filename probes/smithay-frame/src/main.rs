//! Probe C, stage 1: smithay's GLES renderer drawing into hwcomposer's window.
//!
//! smithay normally makes its EGL display over GBM; the Duo's own display path
//! has no GBM. So smithay gets two small pieces of its native EGL traits:
//! - `HybrisDisplay`: the Android platform (`EGL_PLATFORM_ANDROID_KHR`) on the
//!   default display, which libhybris offers (`EGL_KHR_platform_android`);
//! - `HwcWindow`: an EGL window surface over the ANativeWindow that
//!   `hybris_hwc::Output` presents to hwcomposer's display 0.
//!
//! Then `GlesRenderer` draws the same frame as probe B (a drifting background,
//! the hinge's columns black, a bar sweeping across both panels) and
//! `EGLSurface::swap_buffers` hands it to hwcomposer. Once a second: frames,
//! time between presents, frames over 20 ms, render and swap times, vsyncs.
//!
//! Run it with the shell stopped: `EGL_PLATFORM=hwcomposer smithay-frame [s]`.

use std::ffi::c_void;
use std::sync::Arc;

use hybris_hwc::{now_ns, take_stats, vsyncs, Output};
use smithay::backend::egl::display::EGLDisplayHandle;
use smithay::backend::egl::native::{EGLNativeDisplay, EGLNativeSurface, EGLPlatform};
use smithay::backend::egl::context::{GlAttributes, PixelFormatRequirements};
use smithay::backend::egl::{ffi, wrap_egl_call_ptr, EGLContext, EGLDisplay, EGLError, EGLSurface};
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::renderer::{Bind, Color32F, Frame, Renderer};
use smithay::utils::{Physical, Rectangle, Size, Transform};

/// `EGL_PLATFORM_ANDROID_KHR` (EGL_KHR_platform_android).
const PLATFORM_ANDROID_KHR: u32 = 0x3141;

struct HybrisDisplay;

impl EGLNativeDisplay for HybrisDisplay {
    fn supported_platforms(&self) -> Vec<EGLPlatform<'_>> {
        vec![EGLPlatform::new(
            PLATFORM_ANDROID_KHR,
            "PLATFORM_ANDROID_KHR",
            std::ptr::null_mut(), // EGL_DEFAULT_DISPLAY
            vec![ffi::egl::NONE as ffi::EGLint],
            &["EGL_KHR_platform_android"],
        )]
    }

    fn identifier(&self) -> Option<String> {
        Some("hybris/android".into())
    }
}

/// The ANativeWindow of `hybris_hwc::Output`.
struct HwcWindow(*mut c_void);

// The window is libhybris' and lives as long as the program; EGL is its only
// user after creation.
unsafe impl Send for HwcWindow {}

unsafe impl EGLNativeSurface for HwcWindow {
    unsafe fn create(
        &self,
        display: &Arc<EGLDisplayHandle>,
        config_id: ffi::egl::types::EGLConfig,
    ) -> Result<*const c_void, EGLError> {
        wrap_egl_call_ptr(|| unsafe {
            ffi::egl::CreateWindowSurface(***display, config_id, self.0 as _, std::ptr::null())
        })
    }

    fn identifier(&self) -> Option<String> {
        Some("hybris/hwcomposer-window".into())
    }
}

extern "C" {
    fn dlopen(file: *const std::ffi::c_char, flags: i32) -> *mut c_void;
    fn dlsym(handle: *mut c_void, name: *const std::ffi::c_char) -> *mut c_void;
}

fn libc_dlopen(name: &str) -> *mut c_void {
    let c = std::ffi::CString::new(name).unwrap();
    let h = unsafe { dlopen(c.as_ptr(), 2) };
    assert!(!h.is_null(), "dlopen {name}");
    h
}

unsafe fn libc_dlsym<T: Copy>(lib: *mut c_void, name: &str) -> T {
    let c = std::ffi::CString::new(name).unwrap();
    let p = dlsym(lib, c.as_ptr());
    assert!(!p.is_null(), "dlsym {name}");
    std::mem::transmute_copy(&p)
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();
    let seconds: u64 = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(20);

    let out = Output::open(3);
    let (width, height) = (out.width, out.height);
    println!("display 0: {}x{}, vsync period {:.3} ms", width, height, out.vsync_period_ns as f64 / 1e6);

    let display = unsafe { EGLDisplay::new(HybrisDisplay) }.expect("EGLDisplay");
    // A context with a config: a window surface needs one (EGLContext::new
    // makes a surfaceless one). GLES 3 as the Adreno offers; vsync, so a swap
    // waits for the display.
    let attributes = GlAttributes { version: (3, 0), profile: None, debug: false, vsync: true };
    let context = EGLContext::new_with_config(&display, attributes, PixelFormatRequirements::_8_bit())
        .expect("EGLContext");
    let pixel_format = context.pixel_format().expect("the context has no pixel format");
    println!("EGL context: {:?}", pixel_format);
    let mut surface = unsafe { EGLSurface::new(&display, pixel_format, context.config_id(), HwcWindow(out.window)) }
        .expect("EGLSurface");
    let config_id = context.config_id();
    let mut renderer = unsafe { GlesRenderer::new(context) }.expect("GlesRenderer");

    // Diagnostics: what EGL made of the window and the config.
    let dh = display.get_display_handle();
    let mut visual = 0;
    unsafe { ffi::egl::GetConfigAttrib(**dh, config_id, ffi::egl::NATIVE_VISUAL_ID as i32, &mut visual) };
    println!("EGL config native visual id {visual} (HAL format: 1 RGBA_8888, 2 RGBX_8888, 5 BGRA_8888); surface size {:?}",
             surface.get_size());
    let read_pixels: extern "C" fn(i32, i32, i32, i32, u32, u32, *mut c_void) = unsafe {
        let lib = libc_dlopen("libGLESv2.so.2");
        libc_dlsym(lib, "glReadPixels")
    };

    let size: Size<i32, Physical> = (width, height).into();
    let full = Rectangle::from_size(size);
    let hinge = Rectangle::new((1350, 0).into(), (84, height).into());
    let bar_w = 48;

    let start = now_ns();
    let mut next_report = start + 1_000_000_000;
    let mut vsyncs_at_report = vsyncs();
    let (mut render_ns, mut swap_ns) = (0u64, 0u64);
    let mut pixel_report = String::new();
    loop {
        let t = (now_ns() - start) as f64 / 1e9;
        if t > seconds as f64 {
            break;
        }
        let bg = Color32F::new(0.15 + 0.1 * (t * 0.7).sin() as f32, 0.2 + 0.1 * (t * 0.5).cos() as f32, 0.35, 1.0);
        let x = ((t / 2.0).fract() * (width - bar_w) as f64) as i32;
        let bar = Rectangle::new((x, 0).into(), (bar_w, height).into());

        let r0 = now_ns();
        {
            let mut target = renderer.bind(&mut surface).expect("bind");
            let mut frame = renderer.render(&mut target, size, Transform::Normal).expect("render");
            frame.clear(bg, &[full]).expect("clear");
            // draw_solid's damage is relative to its destination, not to the output.
            frame.draw_solid(hinge, &[Rectangle::from_size(hinge.size)], Color32F::new(0.0, 0.0, 0.0, 1.0)).expect("hinge");
            frame.draw_solid(bar, &[Rectangle::from_size(bar.size)], Color32F::new(1.0, 1.0, 1.0, 1.0)).expect("bar");
            let _ = frame.finish().expect("finish");
        }
        let r1 = now_ns();
        if now_ns() >= next_report {
                // What the frame holds: colours along the middle row, and where
                // the white bar is (read before the swap: after it the back buffer is undefined).
                let probe_row = height / 2;
                let mut samples = String::new();
                for sx in [100, 700, 1300, 1390, 1500, 2100, 2700] {
                    let mut px = [0u8; 4];
                    read_pixels(sx, probe_row, 1, 1, 0x1908, 0x1401, px.as_mut_ptr() as *mut c_void);
                    samples += &format!(" {sx}:{:02x}{:02x}{:02x}", px[0], px[1], px[2]);
                }
                let mut white = Vec::new();
                let mut sx = 0;
                while sx < width {
                    let mut px = [0u8; 4];
                    read_pixels(sx, probe_row, 1, 1, 0x1908, 0x1401, px.as_mut_ptr() as *mut c_void);
                    if px[0] > 0xf0 && px[1] > 0xf0 && px[2] > 0xf0 {
                        white.push(sx);
                    }
                    sx += 8;
                }
                pixel_report = format!("        pixels{samples}  white at {:?} (bar drawn at x {x})", (white.first(), white.last()));
        }
        surface.swap_buffers(None).expect("swap_buffers");
        let r2 = now_ns();
        render_ns += r1 - r0;
        swap_ns += r2 - r1;

        if r2 >= next_report {
            println!("{pixel_report}");
            let v = vsyncs();
            let st = take_stats();
            let f = st.frames.max(1) as f64;
            println!("{:5.1} s: {:3} fps  mean {:5.2} ms  max {:6.2} ms  over 20 ms {:2}  render {:5.2} ms  swap {:5.2} ms  vsyncs {:3}  errors {}",
                     t, st.frames, st.mean_ms, st.max_ms, st.over_20ms,
                     render_ns as f64 / 1e6 / f, swap_ns as f64 / 1e6 / f, v - vsyncs_at_report, st.errors);
            render_ns = 0;
            swap_ns = 0;
            vsyncs_at_report = v;
            next_report += 1_000_000_000;
        }
    }
    println!("done");
}
