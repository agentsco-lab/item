//! The Surface Duo's geometry, in logical px at the port's scale of 2.
//!
//! hwcomposer gives one display of 2784x1800: two panels of 1350 columns and
//! the hinge's 84 between them. At scale 2 the left panel is x 0-675, the
//! hinge 675-717 and the right panel 717-1392. The touchscreen covers the
//! same 2784x1800, hinge and all, so it maps onto this layout one to one.

use smithay::utils::{Logical, Point, Rectangle};

pub const SCALE: i32 = 2;

/// The whole output, hinge included.
pub const LAYOUT: (i32, i32) = (1392, 900);

/// The panels, left then right.
pub fn panels() -> [Rectangle<i32, Logical>; 2] {
    [
        Rectangle::new((0, 0).into(), (675, 900).into()),
        Rectangle::new((717, 0).into(), (675, 900).into()),
    ]
}

/// The panel under a point, if it is not on the hinge.
pub fn panel_at(point: Point<f64, Logical>) -> Option<usize> {
    panels().iter().position(|p| p.to_f64().contains(point))
}
