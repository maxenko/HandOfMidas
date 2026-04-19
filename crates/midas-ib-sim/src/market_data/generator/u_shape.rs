//! Intraday U-shape intensity table. Stage 03 fills in.

use crate::engine::clock::VirtualInstant;

/// Returns the intensity multiplier for the given virtual instant relative to
/// a session anchor. Stage 03 implements using the calibrated U-shape.
pub fn u_shape_multiplier(_now: VirtualInstant) -> f64 {
    todo!("Stage 03 — U-shape multiplier")
}
