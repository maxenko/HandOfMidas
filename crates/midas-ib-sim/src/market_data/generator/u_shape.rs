//! Intraday U-shape intensity multiplier.
//!
//! Derived from Hasbrouck (2007) ch. 8 Table 8.4 for NYSE/NASDAQ US equities.
//! 13 half-hour buckets cover the RTH window 09:30–16:00 ET. Open and close
//! buckets are ~2× the midday baseline, producing the characteristic
//! volume + spread "smile".
//!
//! Lookup is keyed off `VirtualInstant` as elapsed time since session start.
//! Session start is defined as 09:30 ET — the sim's canonical anchor.

use crate::engine::clock::VirtualInstant;

/// Half-hour buckets of the U-shape multiplier, 09:30 ET onward.
/// Index 0 = 09:30-10:00, index 12 = 15:30-16:00.
pub const U_SHAPE_TABLE: [f64; 13] = [
    2.10, // 09:30-10:00  open burst
    1.55, // 10:00-10:30
    1.20, // 10:30-11:00
    0.95, // 11:00-11:30
    0.80, // 11:30-12:00
    0.72, // 12:00-12:30  midday trough
    0.70, // 12:30-13:00  midday trough
    0.72, // 13:00-13:30
    0.80, // 13:30-14:00
    0.95, // 14:00-14:30
    1.20, // 14:30-15:00
    1.55, // 15:00-15:30
    2.10, // 15:30-16:00  close burst
];

/// Length of one U-shape bucket = 30 minutes in seconds.
pub const U_SHAPE_BUCKET_SECS: u64 = 30 * 60;

/// Total RTH span covered by the table (6½ hours = 23 400 s).
pub const U_SHAPE_SPAN_SECS: u64 = U_SHAPE_BUCKET_SECS * U_SHAPE_TABLE.len() as u64;

/// Return the U-shape multiplier for the given virtual time (interpreted as
/// seconds since session start = 09:30 ET). Values outside the RTH window
/// clamp to the first / last bucket, so pre-open and after-hours still
/// produce plausible arrival rates.
pub fn u_shape_multiplier(now: VirtualInstant) -> f64 {
    let secs = now.as_duration().as_secs();
    let idx = (secs / U_SHAPE_BUCKET_SECS).min(U_SHAPE_TABLE.len() as u64 - 1) as usize;
    U_SHAPE_TABLE[idx]
}

/// Average of the U-shape table. Used by tests to reason about "midday mean".
pub fn u_shape_mean() -> f64 {
    U_SHAPE_TABLE.iter().sum::<f64>() / U_SHAPE_TABLE.len() as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_and_close_are_heavier_than_midday() {
        let mid_idx = U_SHAPE_TABLE.len() / 2;
        let midday = U_SHAPE_TABLE[mid_idx];
        assert!(U_SHAPE_TABLE[0] > 1.5 * midday);
        assert!(U_SHAPE_TABLE[U_SHAPE_TABLE.len() - 1] > 1.5 * midday);
    }

    #[test]
    fn multiplier_picks_open_bucket_at_time_zero() {
        let m = u_shape_multiplier(VirtualInstant::ZERO);
        assert!((m - U_SHAPE_TABLE[0]).abs() < 1e-12);
    }

    #[test]
    fn multiplier_picks_close_bucket_near_end() {
        // 6h15m after open = inside bucket index 12 (15:30–16:00).
        let t = VirtualInstant::from_secs(6 * 3600 + 15 * 60);
        let m = u_shape_multiplier(t);
        assert!((m - U_SHAPE_TABLE[12]).abs() < 1e-12);
    }

    #[test]
    fn after_hours_clamps_to_last_bucket() {
        let t = VirtualInstant::from_secs(U_SHAPE_SPAN_SECS + 3600);
        let m = u_shape_multiplier(t);
        assert!((m - U_SHAPE_TABLE[12]).abs() < 1e-12);
    }
}
