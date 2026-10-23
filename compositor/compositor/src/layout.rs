//! The Surface Duo's geometry, in logical px at the port's scale of 2.
//!
//! hwcomposer gives one display of 2784x1800: two panels of 1350 columns and
//! the hinge's 84 between them. At scale 2 the left panel is x 0-675, the
//! hinge 675-717 and the right panel 717-1392. The touchscreen covers the
//! same 2784x1800, hinge and all, so it maps onto this layout one to one.
//!
//! Held with the hinge horizontal (posture.rs: a laptop, or the open book
//! turned - "stacked"), the frame is turned: 900 wide and 1392 tall, the
//! panels one above the other, the hinge between. Which physical panel is
//! the upper one is the turn's direction; in the frame the panels are
//! always index 0 first (left, or upper) and 1 second. The renderer turns
//! the frame onto the screen (output.rs) and touches are turned into it
//! here, so the shell draws in the frame's own space either way.

use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};

use smithay::utils::{Logical, Point, Rectangle, Transform};

pub const SCALE: i32 = 2;

/// The screen as hwcomposer and the touchscreen have it: the hinge vertical.
pub const LANDSCAPE: (i32, i32) = (1392, 900);
const PANEL: i32 = 675;
const HINGE: i32 = 42;

/// How the frame is turned: 0 the book (the hinge vertical), 1 the right
/// panel up, 2 the left panel up.
static TURN: AtomicU8 = AtomicU8::new(0);

pub fn turn() -> u8 {
    TURN.load(Ordering::Relaxed)
}

/// Turns the frame; whether that changed it.
pub fn set_turn(t: u8) -> bool {
    TURN.swap(t.min(2), Ordering::Relaxed) != t.min(2)
}

pub fn turned() -> bool {
    turn() != 0
}

/// The whole frame, hinge included.
pub fn size() -> (i32, i32) {
    if turned() { (LANDSCAPE.1, LANDSCAPE.0) } else { LANDSCAPE }
}

/// The panels: left then right, or upper then lower.
pub fn panels() -> [Rectangle<i32, Logical>; 2] {
    if turned() {
        [
            Rectangle::new((0, 0).into(), (LANDSCAPE.1, PANEL).into()),
            Rectangle::new((0, PANEL + HINGE).into(), (LANDSCAPE.1, PANEL).into()),
        ]
    } else {
        [
            Rectangle::new((0, 0).into(), (PANEL, LANDSCAPE.1).into()),
            Rectangle::new((PANEL + HINGE, 0).into(), (PANEL, LANDSCAPE.1).into()),
        ]
    }
}

/// The panel under a point, if it is not on the hinge.
pub fn panel_at(point: Point<f64, Logical>) -> Option<usize> {
    panels().iter().position(|p| p.to_f64().contains(point))
}

/// A point of the screen (a touch, in LANDSCAPE) in the frame.
pub fn from_landscape(p: Point<f64, Logical>) -> Point<f64, Logical> {
    let (w, h) = (LANDSCAPE.0 as f64, LANDSCAPE.1 as f64);
    match turn() {
        // The right panel up: the user's up is the screen's right.
        1 => (p.y, w - p.x).into(),
        // The left panel up: the user's up is the screen's left.
        2 => (h - p.y, p.x).into(),
        _ => p,
    }
}

/// A point of the frame turned by `turn` back on the screen (LANDSCAPE):
/// the inverse of `from_landscape` for that turn.
pub fn to_landscape(p: Point<f64, Logical>, turn: u8) -> Point<f64, Logical> {
    let (w, h) = (LANDSCAPE.0 as f64, LANDSCAPE.1 as f64);
    match turn {
        1 => (w - p.y, p.x).into(),
        2 => (p.y, h - p.x).into(),
        _ => p,
    }
}

/// A point of the frame as it was turned (`from_turn`) in the frame as it
/// is now: where a piece of the shell stood, for it to set off from.
pub fn carry(p: Point<f64, Logical>, from_turn: u8) -> Point<f64, Logical> {
    from_landscape(to_landscape(p, from_turn))
}

/// The output's transform as clients are told it.
pub fn output_transform() -> Transform {
    match turn() {
        1 => Transform::_270,
        2 => Transform::_90,
        _ => Transform::Normal,
    }
}

/// The transform the frame is drawn with into the screen's buffer: the
/// buffer is upside down through hybris (Flipped180), and turned with the frame.
pub fn frame_transform() -> Transform {
    match turn() {
        1 => Transform::Flipped270,
        2 => Transform::Flipped90,
        _ => Transform::Flipped180,
    }
}

/// The panels as the screen has them (physical px, the hinge vertical),
/// whichever way the frame is turned: the turn's animation works in these.
pub fn screen_panels() -> [Rectangle<i32, smithay::utils::Physical>; 2] {
    [
        Rectangle::new((0, 0).into(), (PANEL * SCALE, LANDSCAPE.1 * SCALE).into()),
        Rectangle::new(((PANEL + HINGE) * SCALE, 0).into(), (PANEL * SCALE, LANDSCAPE.1 * SCALE).into()),
    ]
}

/// Until when (ns) the turn's animation runs (output.rs sets it): touches
/// are dropped meanwhile (input.rs).
static TURNING_UNTIL: AtomicU64 = AtomicU64::new(0);

pub fn set_turning_until(ns: u64) {
    TURNING_UNTIL.store(ns, Ordering::Relaxed);
}

pub fn turning_now(now_ns: u64) -> bool {
    now_ns < TURNING_UNTIL.load(Ordering::Relaxed)
}
