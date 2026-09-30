//! The launch curtain, item's: black over the panel an app is launched
//! onto, the app's icon in the middle, from the tap until the new window's
//! first frame. An app takes 1.5-2 s to show a window; the curtain answers
//! the tap in the next frame. The other panel stays as it was.
//!
//! It is up at once and fades out over 150 ms when the window has drawn,
//! or after 20 s if no window comes (item's WAIT_TIMEOUT).

use smithay::backend::renderer::element::memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement};
use smithay::backend::renderer::element::solid::SolidColorRenderElement;
use smithay::backend::renderer::element::{Id, Kind};
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::renderer::utils::CommitCounter;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Physical, Rectangle};

use crate::layout::{self, SCALE};
use crate::shade::ShellElement;

/// The icon's side on the curtain, logical px (item's CURTAIN_ICON).
pub const ICON: i32 = 96;
const FADE_NS: u64 = 150_000_000;
const WAIT_NS: u64 = 20_000_000_000;

struct Up {
    panel: usize,
    icon: Option<MemoryRenderBuffer>,
    since_ns: u64,
    /// The launched app's window, once it is mapped.
    window: Option<WlSurface>,
    /// When it started to fade.
    lifting_ns: Option<u64>,
}

pub struct Curtain {
    up: Option<Up>,
    id: Id,
}

impl Curtain {
    pub fn new() -> Curtain {
        Curtain { up: None, id: Id::new() }
    }

    /// Up over `panel` with this icon.
    pub fn raise(&mut self, panel: usize, icon: Option<MemoryRenderBuffer>, now_ns: u64) {
        self.up = Some(Up { panel, icon, since_ns: now_ns, window: None, lifting_ns: None });
    }

    /// Whether a window mapped on `panel` is the one the curtain waits for.
    pub fn adopt(&mut self, panel: usize, surface: &WlSurface) {
        if let Some(up) = &mut self.up {
            if up.panel == panel && up.window.is_none() && up.lifting_ns.is_none() {
                up.window = Some(surface.clone());
            }
        }
    }

    /// A commit with a buffer: if it is the awaited window's, the curtain
    /// starts to lift. Returns whether it did.
    pub fn drawn(&mut self, surface: &WlSurface, now_ns: u64) -> bool {
        if let Some(up) = &mut self.up {
            if up.lifting_ns.is_none() && up.window.as_ref() == Some(surface) {
                up.lifting_ns = Some(now_ns);
                tracing::info!("curtain: the window drew {:.2} s after the tap", (now_ns - up.since_ns) as f64 / 1e9);
                return true;
            }
        }
        false
    }

    pub fn is_up(&self) -> bool {
        self.up.is_some()
    }

    /// After a frame for `frame_ns`: whether it still needs frames (fading),
    /// or is gone.
    pub fn settle(&mut self, frame_ns: u64) -> bool {
        let Some(up) = &mut self.up else { return false };
        if up.lifting_ns.is_none() && frame_ns > up.since_ns + WAIT_NS {
            tracing::info!("curtain: no window after {} s", WAIT_NS / 1_000_000_000);
            up.lifting_ns = Some(frame_ns);
        }
        match up.lifting_ns {
            Some(t) if frame_ns >= t + FADE_NS => {
                self.up = None;
                true
            }
            Some(_) => true,
            None => false,
        }
    }

    pub fn elements(&self, renderer: &mut GlesRenderer, frame_ns: u64) -> Vec<ShellElement> {
        let Some(up) = &self.up else { return Vec::new() };
        let alpha = match up.lifting_ns {
            Some(t) => 1.0 - (frame_ns.saturating_sub(t) as f32 / FADE_NS as f32).clamp(0.0, 1.0),
            None => 1.0,
        };
        if alpha <= 0.0 {
            return Vec::new();
        }
        let panel = layout::panels()[up.panel].to_physical(SCALE);
        let mut out = Vec::new();
        if let Some(icon) = &up.icon {
            let side = ICON * SCALE;
            let loc = ((panel.loc.x + (panel.size.w - side) / 2) as f64, ((panel.size.h - side) / 2) as f64);
            if let Ok(e) = MemoryRenderBufferRenderElement::from_buffer(renderer, loc, icon, Some(alpha), None, None, Kind::Unspecified) {
                out.push(ShellElement::Text(e));
            }
        }
        // Black, premultiplied; its alpha changes as it fades, so the
        // commit counter follows the frame, not left at its default.
        let rect = Rectangle::<i32, Physical>::new(panel.loc, panel.size);
        let commit = CommitCounter::from((alpha * 1000.0) as usize);
        out.push(ShellElement::Solid(SolidColorRenderElement::new(self.id.clone(), rect, commit, [0.0, 0.0, 0.0, alpha], Kind::Unspecified)));
        out
    }
}
