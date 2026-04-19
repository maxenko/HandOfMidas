//! Roll bid-ask bounce + spread model. Stage 03 fills in.

use crate::engine::types::Side;

/// Apply the Roll bounce to observed prices given the mid.
pub fn observed_price(_mid: f64, _half_spread: f64, _side: Side) -> f64 {
    todo!("Stage 03 — Roll bounce observed_price")
}
