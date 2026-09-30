//! smithay's native EGL traits for libhybris on the Surface Duo's own display
//! path, which has no GBM:
//! - `HybrisDisplay`: the Android platform (`EGL_PLATFORM_ANDROID_KHR`) on the
//!   default display, which libhybris offers (`EGL_KHR_platform_android`);
//! - `HwcWindow`: an EGL window surface over the ANativeWindow that
//!   `hybris_hwc::Output` presents to hwcomposer's display 0.

use std::ffi::c_void;
use std::sync::Arc;

use smithay::backend::egl::display::EGLDisplayHandle;
use smithay::backend::egl::native::{EGLNativeDisplay, EGLNativeSurface, EGLPlatform};
use smithay::backend::egl::{ffi, wrap_egl_call_ptr, EGLError};

/// `EGL_PLATFORM_ANDROID_KHR` (EGL_KHR_platform_android).
const PLATFORM_ANDROID_KHR: u32 = 0x3141;

pub struct HybrisDisplay;

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
pub struct HwcWindow(pub *mut c_void);

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

