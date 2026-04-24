//! # midas-axis
//!
//! Slice S4 of the `session-aware-charts` stack: pluggable time-axis
//! projections. The trait [`TimeAxis`] exposes the three operations the
//! rest of the chart stack needs on the x-dimension:
//!
//! - [`TimeAxis::to_x`] — timestamp to pixel.
//! - [`TimeAxis::from_x`] — pixel to timestamp, `None` inside compressed
//!   gaps.
//! - [`TimeAxis::from_x_snapped`] — pixel to timestamp with snap direction,
//!   always returns a timestamp plus a flag indicating whether a snap
//!   occurred.
//!
//! Three concrete implementations ship:
//!
//! - [`ContinuousAxis`] — linear UTC mapping. Picked for calendars whose
//!   [`TimeAxisPolicy`] is [`TimeAxisPolicy::Continuous`] (e.g. crypto).
//! - [`CompressedAxis`] — session-compressed. Closed time collapses;
//!   consecutive sessions visually butt against one another with an
//!   optional pixel gap. Picked for equities / futures / FX.
//! - [`SessionIndexAxis`] — x is the fractional bar index. Used for
//!   extreme zoom, indicator alignment, or analytical work; wall-clock
//!   labels derive from a parallel `timestamps` array.
//!
//! ## Ideal-design references
//!
//! - `plan/session-aware-charts/00a-ideal-design.md` → "Time axis
//!   (first-class)" and "Ideal behaviours → Time axis continuity".
//! - R2-NM-5 `from_x` policy: `Option` on raw, infallible snap variant.
//!
//! ## Non-goals
//!
//! - No auto-switching compressed ↔ continuous at a zoom threshold. That
//!   is Phase F.
//! - No GPU or framework dependencies — this crate is sans-IO.
//! - No `Camera2D`. Pan/zoom is axis-domain (shift / scale the range).

use std::borrow::Cow;
use std::sync::Arc;

use midas_calendar::{ExchangeCalendar, SessionBuf, TimeAxisPolicy, Timestamp};

mod compressed;
mod continuous;
mod format;
mod price;
mod session_index;
mod ticks;

pub use crate::compressed::CompressedAxis;
pub use crate::continuous::ContinuousAxis;
pub use crate::format::{DefaultFormatter, LabelFormatter};
pub use crate::price::{LinearPriceAxis, PriceAxis};
pub use crate::session_index::SessionIndexAxis;

// Re-export the bar-period for convenience; downstream crates should be
// able to depend on `midas-axis` without a second `midas-bars` import just
// to reach `BarPeriod`.
pub use midas_bars::BarPeriod;

/// Physical viewport measurements in device-independent pixels.
///
/// `width_px` and `height_px` are the chart-drawable rectangle; DPI scale
/// is carried separately so GPU code can scale logical pixels without the
/// axis itself needing to know about device resolution.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Viewport {
    pub width_px: f32,
    pub height_px: f32,
    pub dpi_scale: f32,
}

impl Viewport {
    /// Build a viewport with a default `dpi_scale = 1.0`.
    #[inline]
    pub fn new(width_px: f32, height_px: f32) -> Self {
        Self {
            width_px,
            height_px,
            dpi_scale: 1.0,
        }
    }
}

/// Price range shown in the vertical dimension. Smart-constructed;
/// `low < high`, both finite.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct PriceRange {
    low: f64,
    high: f64,
}

impl PriceRange {
    /// Build a range. Returns [`AxisError::InvalidPriceRange`] if `low` or
    /// `high` is non-finite, if `low >= high`, or if either value is NaN.
    pub fn new(low: f64, high: f64) -> Result<Self, AxisError> {
        if !low.is_finite() || !high.is_finite() || low >= high {
            return Err(AxisError::InvalidPriceRange { low, high });
        }
        Ok(Self { low, high })
    }

    #[inline]
    pub fn low(&self) -> f64 {
        self.low
    }

    #[inline]
    pub fn high(&self) -> f64 {
        self.high
    }

    #[inline]
    pub fn span(&self) -> f64 {
        self.high - self.low
    }
}

/// How to resolve an x-coordinate that lies inside a compressed gap.
///
/// See ideal-design §"Time axis (first-class)" — `from_x_snapped`:
///
/// - [`SnapDirection::Nearest`]: picks the closer session-edge pixel.
/// - [`SnapDirection::Forward`]: picks the next session-open (strictly
///   forward in wall-clock time). The rule of thumb for placement tools
///   (`BracketTool`, annotation drop) per R2-NM-5.
/// - [`SnapDirection::Backward`]: picks the previous session-close.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum SnapDirection {
    Nearest,
    Forward,
    Backward,
}

/// Caller-facing tick-density hint. Implementations translate it to a
/// minimum pixel spacing between ticks.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum TickDensity {
    Sparse,
    Normal,
    Dense,
}

impl TickDensity {
    /// Minimum pixel spacing between ticks for this density.
    #[inline]
    fn target_px(self) -> f32 {
        match self {
            TickDensity::Sparse => 160.0,
            TickDensity::Normal => 90.0,
            TickDensity::Dense => 55.0,
        }
    }
}

/// A single tick emitted by [`TimeAxis::ticks`]. Positional + textual +
/// visual-weight only — the axis emits no GPU / styling primitives.
#[derive(Clone, Debug)]
pub struct TimeTick {
    pub x: f32,
    pub ts: Timestamp,
    pub label: TickLabel,
    pub importance: Importance,
}

/// Primary / optional-secondary label pair. The secondary label exists so
/// renderers can stack context (e.g. "Jan" / "2025") without re-parsing the
/// timestamp.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TickLabel {
    Primary(Cow<'static, str>),
    WithSecondary {
        primary: Cow<'static, str>,
        secondary: Cow<'static, str>,
    },
}

impl TickLabel {
    #[inline]
    pub fn primary(&self) -> &str {
        match self {
            TickLabel::Primary(s) => s.as_ref(),
            TickLabel::WithSecondary { primary, .. } => primary.as_ref(),
        }
    }

    #[inline]
    pub fn secondary(&self) -> Option<&str> {
        match self {
            TickLabel::Primary(_) => None,
            TickLabel::WithSecondary { secondary, .. } => Some(secondary.as_ref()),
        }
    }
}

/// Tick importance — renderers typically draw `Major` ticks thicker /
/// with a stronger label weight.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Importance {
    Minor,
    Major,
}

/// Errors returned by axis constructors and (rarely) projection methods.
#[derive(Debug, thiserror::Error, PartialEq)]
pub enum AxisError {
    #[error("invalid price range: low={low}, high={high} (require finite and low < high)")]
    InvalidPriceRange { low: f64, high: f64 },
    #[error("invalid time range: start >= end")]
    InvalidTimeRange,
    #[error("invalid viewport width: {0}")]
    InvalidWidth(f32),
    #[error("sessions must be sorted by open and non-overlapping")]
    UnsortedSessions,
    #[error("SessionIndexAxis requires at least one timestamp")]
    EmptyTimestamps,
}

/// Pluggable time-axis projection. Every chart consumer routes x-axis
/// arithmetic through this trait; the concrete axis (continuous /
/// compressed / index) is chosen at chart construction and may be swapped
/// without touching candle data.
#[allow(clippy::wrong_self_convention)] // `from_x`/`from_x_snapped` are
                                        // axis-direction names, not
                                        // type-constructor conversions.
pub trait TimeAxis: Send + Sync {
    /// Timestamp to pixel. Clamping behaviour is axis-specific:
    ///
    /// - [`ContinuousAxis`][]: clamps `ts` below `start` to `0.0` and
    ///   above `end` to `width_px`.
    /// - [`CompressedAxis`][]: before-first clamps to `0.0`; after-last
    ///   clamps to `width_px`; mid-gap clamps to the preceding
    ///   session-close x.
    /// - [`SessionIndexAxis`][]: nearest-index-clamp.
    fn to_x(&self, ts: Timestamp) -> f32;

    /// Pixel to timestamp. `None` iff `x` lies in a visual gap
    /// ([`CompressedAxis`] only) or strictly outside `[0, width_px]` for
    /// [`ContinuousAxis`]/[`SessionIndexAxis`].
    fn from_x(&self, x: f32) -> Option<Timestamp>;

    /// Pixel to timestamp with snap. Always returns a timestamp. The
    /// `bool` flag is `true` iff a snap was performed (i.e. `from_x`
    /// would have returned `None`).
    fn from_x_snapped(&self, x: f32, dir: SnapDirection) -> (Timestamp, bool);

    /// Ticks for the current viewport at the requested density. May
    /// allocate; not called from the hot render path.
    fn ticks(&self, density: TickDensity) -> Vec<TimeTick>;

    /// Viewport width in pixels.
    fn width_px(&self) -> f32;

    /// Policy this axis implements. Reported by [`ContinuousAxis`] and
    /// [`SessionIndexAxis`] as [`TimeAxisPolicy::Continuous`]; by
    /// [`CompressedAxis`] as [`TimeAxisPolicy::CompressedSessionBoundaries`].
    fn policy(&self) -> TimeAxisPolicy;
}

/// Convenience builder that inspects the calendar and picks the right
/// concrete axis.
///
/// - If `calendar.time_axis_policy()` is [`TimeAxisPolicy::Continuous`]
///   (crypto), returns a [`ContinuousAxis`] spanning
///   `[viewport_range.0, viewport_range.1)`.
/// - Else (equities / futures / FX), queries
///   `calendar.sessions_between(from, to, &mut buf)` and returns a
///   [`CompressedAxis`] with `gap_px = 0.0`.
///
/// Panics if `viewport_range.0 >= viewport_range.1` or if `width_px` is
/// not finite / positive.
pub fn for_calendar(
    calendar: &'static dyn ExchangeCalendar,
    viewport_range: (Timestamp, Timestamp),
    width_px: f32,
) -> Box<dyn TimeAxis> {
    let (start, end) = viewport_range;
    assert!(start < end, "viewport_range.start must be strictly < end");
    assert!(
        width_px.is_finite() && width_px > 0.0,
        "width_px must be finite and positive"
    );
    match calendar.time_axis_policy() {
        TimeAxisPolicy::Continuous => {
            // ContinuousAxis::new validates (start, end) again; unwrap is
            // safe given the asserts above.
            Box::new(ContinuousAxis::new(start, end, width_px).expect("valid continuous range"))
        }
        TimeAxisPolicy::CompressedSessionBoundaries => {
            let mut buf: SessionBuf = SessionBuf::new();
            calendar.sessions_between(start, end, &mut buf);
            Box::new(
                CompressedAxis::new(buf, width_px, 0.0)
                    .expect("calendar emits sorted non-overlapping sessions"),
            )
        }
    }
}

// ---------------------------------------------------------------------------
// `Arc<[Timestamp]>` helper type alias — `SessionIndexAxis` takes it, but
// users building one directly from a `Vec<Timestamp>` want the ergonomic
// conversion.
// ---------------------------------------------------------------------------

/// Shared timestamp column; [`SessionIndexAxis`] stores this.
pub type TimestampSeries = Arc<[Timestamp]>;

// ---------------------------------------------------------------------------
// Tests for top-level surface (PriceRange, Viewport, `for_calendar` builder)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use midas_calendar::{crypto_spot, xnys};

    use super::*;

    fn ts(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> Timestamp {
        chrono::Utc.with_ymd_and_hms(y, mo, d, h, mi, s).unwrap()
    }

    #[test]
    fn price_range_rejects_inverted_or_nan() {
        assert!(PriceRange::new(10.0, 5.0).is_err());
        assert!(PriceRange::new(f64::NAN, 1.0).is_err());
        assert!(PriceRange::new(0.0, f64::INFINITY).is_err());
        let r = PriceRange::new(1.0, 10.0).unwrap();
        assert_eq!(r.low(), 1.0);
        assert_eq!(r.high(), 10.0);
        assert_eq!(r.span(), 9.0);
    }

    #[test]
    fn viewport_defaults_dpi_to_one() {
        let v = Viewport::new(800.0, 600.0);
        assert_eq!(v.dpi_scale, 1.0);
    }

    #[test]
    fn for_calendar_crypto_returns_continuous() {
        let axis = for_calendar(
            crypto_spot(),
            (ts(2024, 1, 1, 0, 0, 0), ts(2024, 1, 8, 0, 0, 0)),
            1000.0,
        );
        assert_eq!(axis.policy(), TimeAxisPolicy::Continuous);
        assert!((axis.width_px() - 1000.0).abs() < 1e-3);
    }

    #[test]
    fn for_calendar_xnys_returns_compressed() {
        let axis = for_calendar(
            xnys(),
            (ts(2024, 1, 17, 14, 0, 0), ts(2024, 1, 18, 22, 0, 0)),
            1000.0,
        );
        assert_eq!(axis.policy(), TimeAxisPolicy::CompressedSessionBoundaries);
    }

    #[test]
    fn tick_label_accessors() {
        let p = TickLabel::Primary(std::borrow::Cow::Borrowed("foo"));
        assert_eq!(p.primary(), "foo");
        assert_eq!(p.secondary(), None);

        let ws = TickLabel::WithSecondary {
            primary: std::borrow::Cow::Borrowed("Jan"),
            secondary: std::borrow::Cow::Borrowed("2025"),
        };
        assert_eq!(ws.primary(), "Jan");
        assert_eq!(ws.secondary(), Some("2025"));
    }
}
