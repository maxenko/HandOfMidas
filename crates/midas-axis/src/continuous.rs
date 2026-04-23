//! [`ContinuousAxis`] — linear UTC mapping.
//!
//! Every pixel in `[0, width_px]` maps to a UTC instant in
//! `[start, end]` by linear interpolation. Used for calendars whose
//! `time_axis_policy()` is `Continuous` (i.e. crypto spot).

use midas_calendar::{TimeAxisPolicy, Timestamp};

use crate::ticks::{enumerate_ticks, pick_step};
use crate::{AxisError, SnapDirection, TickDensity, TimeAxis, TimeTick};

/// Linear UTC-time axis.
///
/// Internally we cache `start_ns` and `end_ns` as i64 epoch-nanoseconds so
/// the hot-path `to_x` / `from_x` reduce to two f64 multiplies. The
/// projection span is computed once at construction.
#[derive(Clone, Debug)]
pub struct ContinuousAxis {
    start: Timestamp,
    end: Timestamp,
    width: f32,
    /// Cached `end - start` in nanoseconds as an f64. Positive by
    /// construction (validated in `new`).
    span_ns: f64,
}

impl ContinuousAxis {
    /// Build an axis spanning `[start, end]` across `width_px` pixels.
    /// Fails if `start >= end` or `width_px` is not finite-positive.
    pub fn new(start: Timestamp, end: Timestamp, width_px: f32) -> Result<Self, AxisError> {
        if start >= end {
            return Err(AxisError::InvalidTimeRange);
        }
        if !width_px.is_finite() || width_px <= 0.0 {
            return Err(AxisError::InvalidWidth(width_px));
        }
        let span_ns = (end - start)
            .num_nanoseconds()
            .map(|n| n as f64)
            // Fallback for extraordinarily wide ranges (>292y) that
            // overflow i64 nanos. Drop to microseconds for the span.
            .unwrap_or_else(|| (end - start).num_microseconds().unwrap_or(1) as f64 * 1_000.0);
        Ok(Self {
            start,
            end,
            width: width_px,
            span_ns,
        })
    }

    #[inline]
    pub fn start(&self) -> Timestamp {
        self.start
    }

    #[inline]
    pub fn end(&self) -> Timestamp {
        self.end
    }

    /// Raw x for a timestamp, without clamping. Useful internally.
    #[inline]
    fn raw_x(&self, ts: Timestamp) -> f32 {
        let delta_ns = (ts - self.start)
            .num_nanoseconds()
            .map(|n| n as f64)
            .unwrap_or_else(|| (ts - self.start).num_microseconds().unwrap_or(0) as f64 * 1_000.0);
        ((delta_ns / self.span_ns) * self.width as f64) as f32
    }
}

impl TimeAxis for ContinuousAxis {
    fn to_x(&self, ts: Timestamp) -> f32 {
        let x = self.raw_x(ts);
        x.clamp(0.0, self.width)
    }

    fn from_x(&self, x: f32) -> Option<Timestamp> {
        if !(x.is_finite() && (0.0..=self.width).contains(&x)) {
            return None;
        }
        let frac = (x as f64) / (self.width as f64);
        let offset_ns = (frac * self.span_ns).round() as i64;
        Some(self.start + chrono::Duration::nanoseconds(offset_ns))
    }

    fn from_x_snapped(&self, x: f32, _dir: SnapDirection) -> (Timestamp, bool) {
        // No gaps on a continuous axis — clamp x and call from_x.
        let clamped = if x.is_finite() {
            x.clamp(0.0, self.width)
        } else {
            0.0
        };
        let was_snapped = !x.is_finite() || !(0.0..=self.width).contains(&x);
        let ts = self.from_x(clamped).unwrap_or(self.start);
        (ts, was_snapped)
    }

    fn ticks(&self, density: TickDensity) -> Vec<TimeTick> {
        let span_secs = (self.end - self.start).num_seconds().max(1);
        let step = pick_step(span_secs, self.width, density);
        enumerate_ticks(self.start, self.end, step, |ts| {
            if ts < self.start || ts > self.end {
                None
            } else {
                Some(self.raw_x(ts).clamp(0.0, self.width))
            }
        })
    }

    fn width_px(&self) -> f32 {
        self.width
    }

    fn policy(&self) -> TimeAxisPolicy {
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

    #[test]
    fn to_x_at_endpoints() {
        let axis =
            ContinuousAxis::new(ts(2024, 1, 1, 0, 0, 0), ts(2024, 1, 2, 0, 0, 0), 1000.0).unwrap();
        assert!((axis.to_x(ts(2024, 1, 1, 0, 0, 0)) - 0.0).abs() < 1e-3);
        assert!((axis.to_x(ts(2024, 1, 2, 0, 0, 0)) - 1000.0).abs() < 1e-3);
    }

    #[test]
    fn to_x_midpoint() {
        let axis =
            ContinuousAxis::new(ts(2024, 1, 1, 0, 0, 0), ts(2024, 1, 2, 0, 0, 0), 1000.0).unwrap();
        let mid = axis.to_x(ts(2024, 1, 1, 12, 0, 0));
        assert!((mid - 500.0).abs() < 1e-3, "mid={mid}");
    }

    #[test]
    fn to_x_clamps_outside() {
        let axis =
            ContinuousAxis::new(ts(2024, 1, 1, 0, 0, 0), ts(2024, 1, 2, 0, 0, 0), 1000.0).unwrap();
        assert_eq!(axis.to_x(ts(2023, 12, 31, 0, 0, 0)), 0.0);
        assert_eq!(axis.to_x(ts(2024, 1, 3, 0, 0, 0)), 1000.0);
    }

    #[test]
    fn from_x_round_trips_to_x() {
        let axis =
            ContinuousAxis::new(ts(2024, 1, 1, 0, 0, 0), ts(2024, 1, 2, 0, 0, 0), 1000.0).unwrap();
        for &x_in in &[0.0f32, 123.4, 500.0, 750.25, 1000.0] {
            let ts = axis.from_x(x_in).expect("in-range");
            let x_out = axis.to_x(ts);
            assert!((x_in - x_out).abs() < 0.05, "x_in={x_in}, x_out={x_out}");
        }
    }

    #[test]
    fn from_x_returns_none_outside_viewport() {
        let axis =
            ContinuousAxis::new(ts(2024, 1, 1, 0, 0, 0), ts(2024, 1, 2, 0, 0, 0), 1000.0).unwrap();
        assert!(axis.from_x(-0.001).is_none());
        assert!(axis.from_x(1000.001).is_none());
        assert!(axis.from_x(f32::NAN).is_none());
        assert!(axis.from_x(f32::INFINITY).is_none());
    }

    #[test]
    fn from_x_snapped_inside_viewport_reports_not_snapped() {
        let axis =
            ContinuousAxis::new(ts(2024, 1, 1, 0, 0, 0), ts(2024, 1, 2, 0, 0, 0), 1000.0).unwrap();
        let (_ts, snapped) = axis.from_x_snapped(500.0, SnapDirection::Nearest);
        assert!(!snapped);
    }

    #[test]
    fn from_x_snapped_outside_viewport_clamps() {
        let axis =
            ContinuousAxis::new(ts(2024, 1, 1, 0, 0, 0), ts(2024, 1, 2, 0, 0, 0), 1000.0).unwrap();
        let (ts_lo, snapped_lo) = axis.from_x_snapped(-100.0, SnapDirection::Nearest);
        assert!(snapped_lo);
        assert_eq!(ts_lo, ts(2024, 1, 1, 0, 0, 0));
        let (ts_hi, snapped_hi) = axis.from_x_snapped(2000.0, SnapDirection::Nearest);
        assert!(snapped_hi);
        assert_eq!(ts_hi, ts(2024, 1, 2, 0, 0, 0));
    }

    #[test]
    fn ticks_for_one_day_yield_reasonable_count() {
        let axis =
            ContinuousAxis::new(ts(2024, 1, 1, 0, 0, 0), ts(2024, 1, 2, 0, 0, 0), 1000.0).unwrap();
        let ticks = axis.ticks(TickDensity::Normal);
        assert!(
            (5..=20).contains(&ticks.len()),
            "normal day ticks = {}",
            ticks.len()
        );
        // Ticks are x-sorted.
        let xs: Vec<f32> = ticks.iter().map(|t| t.x).collect();
        assert!(xs.windows(2).all(|w| w[0] <= w[1]));
    }

    #[test]
    fn ticks_for_one_hour_are_minute_level() {
        let axis = ContinuousAxis::new(ts(2024, 1, 1, 10, 0, 0), ts(2024, 1, 1, 11, 0, 0), 1000.0)
            .unwrap();
        let ticks = axis.ticks(TickDensity::Normal);
        // Labels should be HH:MM.
        assert!(!ticks.is_empty());
        let any_minute_label = ticks
            .iter()
            .any(|t| t.label.primary().contains(':') && t.label.primary().len() == 5);
        assert!(any_minute_label, "expected HH:MM labels, got {:?}", ticks);
    }

    #[test]
    fn ticks_for_one_year_are_month_level_with_secondary() {
        let axis =
            ContinuousAxis::new(ts(2025, 1, 1, 0, 0, 0), ts(2026, 1, 1, 0, 0, 0), 1000.0).unwrap();
        let ticks = axis.ticks(TickDensity::Normal);
        assert!(!ticks.is_empty());
        let has_secondary_year = ticks
            .iter()
            .any(|t| t.label.secondary() == Some("2025") || t.label.secondary() == Some("2026"));
        assert!(
            has_secondary_year,
            "expected a month tick with year-secondary, got {:?}",
            ticks
        );
    }

    #[test]
    fn policy_is_continuous() {
        let axis =
            ContinuousAxis::new(ts(2024, 1, 1, 0, 0, 0), ts(2024, 1, 2, 0, 0, 0), 1000.0).unwrap();
        assert_eq!(axis.policy(), TimeAxisPolicy::Continuous);
    }

    #[test]
    fn new_rejects_inverted_range() {
        let err = ContinuousAxis::new(ts(2024, 1, 2, 0, 0, 0), ts(2024, 1, 1, 0, 0, 0), 1000.0);
        assert!(matches!(err, Err(AxisError::InvalidTimeRange)));
    }

    #[test]
    fn new_rejects_bad_width() {
        let err = ContinuousAxis::new(ts(2024, 1, 1, 0, 0, 0), ts(2024, 1, 2, 0, 0, 0), -5.0);
        assert!(matches!(err, Err(AxisError::InvalidWidth(_))));
        let err = ContinuousAxis::new(ts(2024, 1, 1, 0, 0, 0), ts(2024, 1, 2, 0, 0, 0), f32::NAN);
        assert!(matches!(err, Err(AxisError::InvalidWidth(_))));
    }
}
