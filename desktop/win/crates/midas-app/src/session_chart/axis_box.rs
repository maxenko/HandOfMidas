//! [`AxisKind`] + [`AxisBox`] — sum type over the pluggable
//! [`TimeAxis`](midas_axis::TimeAxis) implementations the widget can
//! hold.
//!
//! Split out of `widget.rs` in the R3 refactor (arch-audit F2) so the
//! widget file can focus on the scaffold widget itself. All
//! axis-construction choices (policy dispatch, continuous vs. compressed)
//! live here; the widget just holds an [`AxisBox`] and matches on it at
//! paint time.

use midas_axis::{AxisError, CompressedAxis, ContinuousAxis, TimeAxis};
use midas_calendar::{ExchangeCalendar, TimeAxisPolicy, Timestamp};

/// Which concrete [`TimeAxis`] implementation this chart uses.
///
/// Phase C needs both `Continuous` (crypto) and `Compressed` (XNYS);
/// `SessionIndex` is reserved for Phase F's analytical zoom.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum AxisKind {
    Continuous,
    Compressed,
    SessionIndex,
}

impl AxisKind {
    /// Pick the axis kind implied by the calendar's
    /// [`TimeAxisPolicy`][midas_calendar::TimeAxisPolicy].
    ///
    /// Maps:
    /// - [`TimeAxisPolicy::Continuous`] → [`AxisKind::Continuous`].
    /// - [`TimeAxisPolicy::CompressedSessionBoundaries`] →
    ///   [`AxisKind::Compressed`].
    ///
    /// `SessionIndex` is never returned here — it's a user-driven
    /// override reserved for later. Phase F R2-NM-6's "zoom threshold
    /// auto-switch" is the first code-path that would return it.
    pub fn for_calendar(calendar: &'static dyn ExchangeCalendar) -> Self {
        match calendar.time_axis_policy() {
            TimeAxisPolicy::Continuous => AxisKind::Continuous,
            TimeAxisPolicy::CompressedSessionBoundaries => AxisKind::Compressed,
        }
    }
}

/// Small internal sum-axis — lets the widget hold a single runtime
/// value without a generic type parameter bleed into the composition
/// root. Each variant owns its concrete axis value; the widget's
/// `paint_buckets` matches on the variant to pass the right axis into
/// [`build_scene`](super::scene_builder::build_scene).
///
/// `CompressedAxis` is `Box`-wrapped — its inline `SmallVec` buffers
/// push the enum past 1 KiB, which clippy's `large_enum_variant` lint
/// flags. Boxing the compressed variant keeps the enum small without
/// affecting the uncommon-axis hot-path (the box is only dereferenced
/// once per paint, behind an already-non-trivial scene-builder
/// traversal).
#[derive(Clone)]
pub(super) enum AxisBox {
    Continuous(ContinuousAxis),
    Compressed(Box<CompressedAxis>),
    // `SessionIndex(SessionIndexAxis)` reserved; not constructed in
    // Phase C.
}

impl AxisBox {
    pub(super) fn kind(&self) -> AxisKind {
        match self {
            AxisBox::Continuous(_) => AxisKind::Continuous,
            AxisBox::Compressed(_) => AxisKind::Compressed,
        }
    }

    pub(super) fn width_px(&self) -> f32 {
        match self {
            AxisBox::Continuous(a) => a.width_px(),
            AxisBox::Compressed(a) => a.width_px(),
        }
    }

    /// Construct the axis implied by `calendar.time_axis_policy()` for
    /// the given time window and viewport width. Returns an
    /// [`AxisError`] on invalid inputs (degenerate time window, NaN
    /// width, etc.) — callers that used to panic via `.expect` now
    /// propagate this up to the host for graceful degradation.
    pub(super) fn try_for_calendar(
        calendar: &'static dyn ExchangeCalendar,
        window: (Timestamp, Timestamp),
        width_px: f32,
    ) -> Result<Self, AxisError> {
        match calendar.time_axis_policy() {
            TimeAxisPolicy::Continuous => {
                ContinuousAxis::new(window.0, window.1, width_px).map(AxisBox::Continuous)
            }
            TimeAxisPolicy::CompressedSessionBoundaries => {
                let mut buf: midas_calendar::SessionBuf = midas_calendar::SessionBuf::new();
                calendar.sessions_between(window.0, window.1, &mut buf);
                CompressedAxis::new(buf, width_px, 0.0).map(|a| AxisBox::Compressed(Box::new(a)))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use midas_calendar::{crypto_spot, xnys};

    fn utc(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> Timestamp {
        use chrono::TimeZone;
        chrono::Utc.with_ymd_and_hms(y, mo, d, h, mi, 0).unwrap()
    }

    #[test]
    fn axis_kind_for_calendar_matches_policy() {
        assert_eq!(AxisKind::for_calendar(crypto_spot()), AxisKind::Continuous);
        assert_eq!(AxisKind::for_calendar(xnys()), AxisKind::Compressed);
    }

    #[test]
    fn try_for_calendar_crypto_yields_continuous() {
        let start = utc(2024, 3, 1, 0, 0);
        let end = utc(2024, 3, 2, 0, 0);
        let ax = AxisBox::try_for_calendar(crypto_spot(), (start, end), 1000.0).unwrap();
        assert_eq!(ax.kind(), AxisKind::Continuous);
    }

    #[test]
    fn try_for_calendar_xnys_yields_compressed() {
        let start = utc(2024, 1, 17, 0, 0);
        let end = utc(2024, 1, 19, 0, 0);
        let ax = AxisBox::try_for_calendar(xnys(), (start, end), 1000.0).unwrap();
        assert_eq!(ax.kind(), AxisKind::Compressed);
    }

    #[test]
    fn try_for_calendar_rejects_inverted_time_range() {
        let start = utc(2024, 3, 2, 0, 0);
        let end = utc(2024, 3, 1, 0, 0);
        let err = AxisBox::try_for_calendar(crypto_spot(), (start, end), 1000.0);
        assert!(matches!(err, Err(AxisError::InvalidTimeRange)));
    }
}
