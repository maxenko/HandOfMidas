//! [`SessionIndexAxis`] — x-coord is fractional bar index.
//!
//! Each of the `N` timestamps occupies `width_px / N` pixels. This axis
//! reports `TimeAxisPolicy::Continuous` because, within its domain, there
//! are no compressed gaps — the axis is a "lens" on bar space.

use std::sync::Arc;

use chrono::Duration;

use midas_calendar::{TimeAxisPolicy, Timestamp};

use crate::ticks::{label_for, pick_step};
use crate::{AxisError, Importance, SnapDirection, TickDensity, TimeAxis, TimeTick};

/// Fractional-bar-index axis.
///
/// Storage is `Arc<[Timestamp]>` so two consumers (main chart +
/// thumbnail) can share the column without allocation.
#[derive(Clone, Debug)]
pub struct SessionIndexAxis {
    timestamps: Arc<[Timestamp]>,
    width: f32,
    /// Pixels per bar. Cached; strictly positive.
    px_per_bar: f32,
}

impl SessionIndexAxis {
    /// Build a session-index axis. `timestamps` must be non-empty and
    /// weakly monotonic (callers already guarantee this from
    /// `CandleSeries`).
    pub fn new(timestamps: Arc<[Timestamp]>, width_px: f32) -> Result<Self, AxisError> {
        if !width_px.is_finite() || width_px <= 0.0 {
            return Err(AxisError::InvalidWidth(width_px));
        }
        if timestamps.is_empty() {
            return Err(AxisError::EmptyTimestamps);
        }
        let px_per_bar = width_px / timestamps.len() as f32;
        Ok(Self {
            timestamps,
            width: width_px,
            px_per_bar,
        })
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.timestamps.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.timestamps.is_empty()
    }

    /// Bar index at `ts`, using binary search. Returns the index of the
    /// largest timestamp <= `ts` (saturating). `None` only if the series
    /// is empty (ruled out by the constructor).
    fn index_for_ts(&self, ts: Timestamp) -> usize {
        // partition_point returns the first index whose ts > target;
        // subtract 1 for the floor-index.
        let n = self.timestamps.len();
        if ts < self.timestamps[0] {
            return 0;
        }
        if ts >= self.timestamps[n - 1] {
            return n - 1;
        }
        let idx = self.timestamps.partition_point(|t| *t <= ts);
        if idx == 0 {
            0
        } else {
            idx - 1
        }
    }
}

impl TimeAxis for SessionIndexAxis {
    fn to_x(&self, ts: Timestamp) -> f32 {
        let n = self.timestamps.len();
        if n == 0 {
            return 0.0;
        }
        if ts <= self.timestamps[0] {
            return 0.0;
        }
        if ts > self.timestamps[n - 1] {
            return self.width;
        }
        // Fractional index: floor-index + (ts - timestamps[i]) /
        //   (timestamps[i+1] - timestamps[i]).
        // When `ts == timestamps[n-1]`, `index_for_ts` returns n-1 and
        // frac falls through to 0, so x = (n-1) * px_per_bar — the left
        // edge of the last bar, NOT `width`. `width` is reserved for
        // "past the series."
        let i = self.index_for_ts(ts);
        let base = i as f32;
        let frac = if i + 1 < n {
            let span_ns = (self.timestamps[i + 1] - self.timestamps[i])
                .num_nanoseconds()
                .map(|n| n as f64)
                .unwrap_or(1.0);
            let delta_ns = (ts - self.timestamps[i])
                .num_nanoseconds()
                .map(|n| n as f64)
                .unwrap_or(0.0);
            (delta_ns / span_ns.max(1.0)) as f32
        } else {
            0.0
        };
        ((base + frac) * self.px_per_bar).clamp(0.0, self.width)
    }

    fn from_x(&self, x: f32) -> Option<Timestamp> {
        if !x.is_finite() || !(0.0..=self.width).contains(&x) {
            return None;
        }
        let n = self.timestamps.len();
        if n == 0 {
            return None;
        }
        let idx_f = (x / self.px_per_bar).clamp(0.0, (n - 1) as f32);
        let i = idx_f.floor() as usize;
        let frac = (idx_f - i as f32) as f64;
        if i + 1 < n {
            let span_ns = (self.timestamps[i + 1] - self.timestamps[i])
                .num_nanoseconds()
                .map(|n| n as f64)
                .unwrap_or(1.0);
            let offset_ns = (frac * span_ns).round() as i64;
            Some(self.timestamps[i] + Duration::nanoseconds(offset_ns))
        } else {
            Some(self.timestamps[i])
        }
    }

    fn from_x_snapped(&self, x: f32, _dir: SnapDirection) -> (Timestamp, bool) {
        let clamped = if x.is_finite() {
            x.clamp(0.0, self.width)
        } else {
            0.0
        };
        let was_snapped = !x.is_finite() || !(0.0..=self.width).contains(&x);
        let ts = self.from_x(clamped).unwrap_or_else(|| self.timestamps[0]);
        (ts, was_snapped)
    }

    fn ticks(&self, density: TickDensity) -> Vec<TimeTick> {
        let n = self.timestamps.len();
        if n == 0 {
            return Vec::new();
        }
        let first = self.timestamps[0];
        let last = self.timestamps[n - 1];
        let span_secs = (last - first).num_seconds().max(1);
        let step = pick_step(span_secs, self.width, density);

        // Emit one tick per relevant bar-boundary. Because x = index *
        // px_per_bar, we iterate bar indices rather than wall time — this
        // guarantees labels sit exactly at candle centers and bar spacing
        // remains uniform regardless of wall-clock density.
        let mut out = Vec::new();
        let target_count = (self.width / density.target_px()).max(2.0).min(n as f32) as usize;
        // Bar stride to hit roughly `target_count` ticks.
        let stride = n.div_ceil(target_count).max(1);
        let importance_step = step; // for label_for / importance inference
        let major_every = stride * 4; // every 4th tick is "major-ish"
        let mut i = 0_usize;
        while i < n {
            let ts = self.timestamps[i];
            let x = (i as f32 + 0.5) * self.px_per_bar;
            let is_major = i == 0 || i.is_multiple_of(major_every);
            out.push(TimeTick {
                x: x.clamp(0.0, self.width),
                ts,
                label: label_for(ts, importance_step),
                importance: if is_major {
                    Importance::Major
                } else {
                    Importance::Minor
                },
            });
            i += stride;
        }
        out
    }

    fn width_px(&self) -> f32 {
        self.width
    }

    fn policy(&self) -> TimeAxisPolicy {
        // Session-index is a "lens"; within its domain, no gaps exist.
        // Reporting Continuous lets downstream code drive tick rendering
        // uniformly.
        TimeAxisPolicy::Continuous
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    fn ts(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> Timestamp {
        chrono::Utc.with_ymd_and_hms(y, mo, d, h, mi, s).unwrap()
    }

    fn uniform_series(n: usize) -> Arc<[Timestamp]> {
        let mut v = Vec::with_capacity(n);
        let start = ts(2024, 1, 17, 14, 30, 0);
        for i in 0..n {
            v.push(start + Duration::minutes(i as i64));
        }
        v.into()
    }

    #[test]
    fn to_x_uniform_spacing() {
        let ts_arr = uniform_series(10);
        let axis = SessionIndexAxis::new(ts_arr.clone(), 100.0).unwrap();
        // First bar: 0..10 px range, second: 10..20 px range, etc.
        // Midpoint of each bar is at (i + 0.5) * 10.
        for i in 0..10 {
            let x = axis.to_x(ts_arr[i]);
            // Integer-aligned timestamps at index boundaries: bar i
            // starts at x = i * 10.
            assert!(
                (x - i as f32 * 10.0).abs() < 0.5,
                "bar {}: x={x}, expected ~{}",
                i,
                i * 10
            );
        }
    }

    #[test]
    fn from_x_round_trips_in_range() {
        let ts_arr = uniform_series(10);
        let axis = SessionIndexAxis::new(ts_arr, 100.0).unwrap();
        for &x_in in &[0.0f32, 12.5, 25.0, 49.9, 89.0] {
            let t = axis.from_x(x_in).expect("in-range");
            let x_out = axis.to_x(t);
            assert!(
                (x_in - x_out).abs() < 1.0,
                "round-trip at x={x_in}: got x_out={x_out}"
            );
        }
    }

    #[test]
    fn from_x_returns_none_outside_viewport() {
        let axis = SessionIndexAxis::new(uniform_series(4), 100.0).unwrap();
        assert!(axis.from_x(-1.0).is_none());
        assert!(axis.from_x(101.0).is_none());
        assert!(axis.from_x(f32::NAN).is_none());
    }

    #[test]
    fn to_x_clamps_outside_range() {
        let ts_arr = uniform_series(5);
        let axis = SessionIndexAxis::new(ts_arr.clone(), 100.0).unwrap();
        assert_eq!(axis.to_x(ts_arr[0] - Duration::days(1)), 0.0);
        assert_eq!(axis.to_x(ts_arr[4] + Duration::days(1)), 100.0);
    }

    #[test]
    fn from_x_snapped_outside_clamps() {
        let axis = SessionIndexAxis::new(uniform_series(4), 100.0).unwrap();
        let (_t, snapped) = axis.from_x_snapped(-5.0, SnapDirection::Nearest);
        assert!(snapped);
        let (_t, snapped) = axis.from_x_snapped(50.0, SnapDirection::Nearest);
        assert!(!snapped);
    }

    #[test]
    fn ticks_at_sparse_yield_manageable_count() {
        let axis = SessionIndexAxis::new(uniform_series(100), 600.0).unwrap();
        let ticks = axis.ticks(TickDensity::Sparse);
        // Target ~ width/160 ≈ 3.75 → floor to stride; expect [2..=8].
        assert!(
            (2..=10).contains(&ticks.len()),
            "sparse yield: {}",
            ticks.len()
        );
    }

    #[test]
    fn ticks_at_normal_density() {
        let axis = SessionIndexAxis::new(uniform_series(100), 1000.0).unwrap();
        let ticks = axis.ticks(TickDensity::Normal);
        assert!(!ticks.is_empty());
        // Ticks should be x-sorted.
        let xs: Vec<f32> = ticks.iter().map(|t| t.x).collect();
        assert!(xs.windows(2).all(|w| w[0] <= w[1]));
    }

    #[test]
    fn rejects_empty_timestamps() {
        let empty: Arc<[Timestamp]> = Arc::from(Vec::<Timestamp>::new());
        let err = SessionIndexAxis::new(empty, 100.0);
        assert!(matches!(err, Err(AxisError::EmptyTimestamps)));
    }

    #[test]
    fn rejects_bad_width() {
        let err = SessionIndexAxis::new(uniform_series(5), -1.0);
        assert!(matches!(err, Err(AxisError::InvalidWidth(_))));
    }

    #[test]
    fn policy_reports_continuous() {
        let axis = SessionIndexAxis::new(uniform_series(5), 100.0).unwrap();
        assert_eq!(axis.policy(), TimeAxisPolicy::Continuous);
    }
}
