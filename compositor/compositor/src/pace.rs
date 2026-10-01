//! How quickly each client answers its frame callback, and so when it
//! should hear it (`Callbacks::Adaptive`).
//!
//! Told as our frame is drawn, a client draws its next while ours waits for
//! the vsync; a client that needs most of a frame (Settings, 12-14 ms) keeps
//! 60 fps that way. But a quick one commits within a millisecond, while our
//! frame is still being drawn without it: its frame shows two vsyncs after
//! its commit. Told at the vsync instead, a quick client commits early in
//! the frame, its frame is drawn at once and shows at the next vsync: one
//! frame sooner. A slow one told at the vsync would miss the frame and drop
//! to 40 fps (step 7b).
//!
//! So each surface's callback-to-commit time is followed (a moving average)
//! and the quick ones hear at the vsync, the rest as the frame is drawn.

use std::cell::RefCell;
use std::collections::HashMap;

use smithay::reexports::wayland_server::backend::ObjectId;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::Resource;
use smithay::wayland::compositor::{get_parent, with_states, SurfaceAttributes};

/// Quick below this (ms, the average from callback to commit), slow again
/// above the second: a margin so a client does not flip every frame. A quick
/// client's commit is drawn at once (about 6 ms to hwcomposer's present), so
/// it has to come early enough in the frame for the present to make the
/// next vsync.
const QUICK_MS: f64 = 6.0;
const SLOW_MS: f64 = 9.0;

#[derive(Default)]
struct Pace {
    sent_ns: Option<u64>,
    average_ms: f64,
    quick: bool,
    /// Answers in a row that came while the loop was busy drawing: their
    /// time is not known, only that it was shorter than the drawing.
    unknown: u32,
    /// After a trial at the vsync that showed it slow, none before this.
    retry_ns: u64,
}

/// A commit read within this of the loop coming free came while it was busy.
const BUSY_NS: u64 = 500_000;
/// Answers of unknown time in a row before a trial at the vsync.
const TRIAL_AFTER: u32 = 3;
/// After a trial showed a client slow, the next not before this.
const RETRY_NS: u64 = 5_000_000_000;

#[derive(Default)]
pub struct Paces(RefCell<HashMap<ObjectId, Pace>>, std::cell::Cell<u64>);

/// The surface a subsurface belongs to.
pub fn root(surface: &WlSurface) -> WlSurface {
    let mut s = surface.clone();
    while let Some(parent) = get_parent(&s) {
        s = parent;
    }
    s
}

impl Paces {
    pub fn quick(&self, surface: &WlSurface) -> bool {
        self.0.borrow().get(&surface.id()).is_some_and(|p| p.quick)
    }

    /// Whether the surface asked for a frame callback with its last commit.
    pub fn waiting(surface: &WlSurface) -> bool {
        with_states(surface, |states| !states.cached_state.get::<SurfaceAttributes>().current().frame_callbacks.is_empty())
    }

    /// A callback was sent to a surface that was waiting for one.
    pub fn sent(&self, surface: &WlSurface, now_ns: u64) {
        if std::env::var_os("PACE_DEBUG").is_some() {
            tracing::info!("pace: sent {}", surface.id());
        }
        self.0.borrow_mut().entry(surface.id()).or_default().sent_ns = Some(now_ns);
    }

    /// The loop was busy drawing until now: a commit that came meanwhile is
    /// only read now, so its answer is timed from here at the latest.
    pub fn busy_until(&self, now_ns: u64) {
        self.1.set(now_ns);
    }

    /// A commit: if it answers a callback, how long the answer took.
    pub fn committed(&self, surface: &WlSurface, now_ns: u64) {
        let root = root(surface);
        let mut paces = self.0.borrow_mut();
        let Some(p) = paces.get_mut(&root.id()) else { return };
        let Some(sent) = p.sent_ns.take() else { return };
        // While the loop was drawing, a commit waits unread: its answer's
        // time is unknown. A client told as the frame is drawn answers then if
        // it is quick, so a run of those earns it a trial at the vsync, where
        // its answer is timed for certain.
        let busy_end = self.1.get();
        if busy_end > sent && now_ns < busy_end + BUSY_NS {
            p.unknown += 1;
            if !p.quick && p.unknown >= TRIAL_AFTER && now_ns > p.retry_ns {
                tracing::info!("pace: {} tried at the vsync", root.id());
                p.quick = true;
                p.average_ms = 0.0;
                p.unknown = 0;
            }
            return;
        }
        p.unknown = 0;
        let ms = now_ns.saturating_sub(sent) as f64 / 1e6;
        if std::env::var_os("PACE_DEBUG").is_some() {
            tracing::info!("pace: {} answered in {ms:.2} ms, average {:.2}", root.id(), p.average_ms);
        }
        // A client that was idle, not drawing: not a measure of its pace.
        if ms > 100.0 {
            return;
        }
        p.average_ms = if p.average_ms == 0.0 { ms } else { p.average_ms * 0.7 + ms * 0.3 };
        let quick = if p.quick { p.average_ms < SLOW_MS } else { p.average_ms < QUICK_MS };
        if p.quick && !quick {
            p.retry_ns = now_ns + RETRY_NS;
        }
        if quick != p.quick {
            tracing::info!("pace: {} {} ({:.1} ms from callback to commit)", root.id(), if quick { "quick: told at the vsync" } else { "slow: told as the frame is drawn" }, p.average_ms);
            p.quick = quick;
        }
    }

    /// Forgets surfaces gone.
    pub fn retain(&self, alive: impl Fn(&ObjectId) -> bool) {
        self.0.borrow_mut().retain(|id, _| alive(id));
    }
}
