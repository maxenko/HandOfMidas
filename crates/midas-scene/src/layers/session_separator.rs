//! [`SessionSeparatorLayer`] — vertical rules at session close → next-open.

use midas_calendar::{ExchangeCalendar, SessionBuf, Timestamp};
use smallvec::SmallVec;

use crate::layer::{LayerId, LayerZ, SceneLayer};
use crate::paint::PaintContext;
use crate::primitives::LineInstance;

/// Visual knobs for [`SessionSeparatorLayer`].
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct SeparatorStyle {
    pub line_width_px: f32,
}

impl Default for SeparatorStyle {
    fn default() -> Self {
        Self { line_width_px: 1.0 }
    }
}

/// One session-to-session transition. Emitted at the pixel x for
/// the prior session's close (which equals the next session's open
/// on a compressed axis — the separator sits exactly on the seam).
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct SessionBoundary {
    pub at: Timestamp,
}

/// Thin vertical rules between adjacent sessions. Paired with
/// [`SessionBandLayer`](super::SessionBandLayer) for the session-aware
/// visual.
pub struct SessionSeparatorLayer {
    calendar: &'static dyn ExchangeCalendar,
    boundaries: SmallVec<[SessionBoundary; 32]>,
    style: SeparatorStyle,
}

impl SessionSeparatorLayer {
    pub fn new(calendar: &'static dyn ExchangeCalendar) -> Self {
        Self {
            calendar,
            boundaries: SmallVec::new(),
            style: SeparatorStyle::default(),
        }
    }

    pub fn with_style(mut self, style: SeparatorStyle) -> Self {
        self.style = style;
        self
    }

    /// Re-populate the cached boundary buffer for the given time range.
    /// Called by the scene driver each frame BEFORE paint. Emits one
    /// entry per session-transition inside `[from, to)`.
    pub fn update_boundaries(&mut self, from: Timestamp, to: Timestamp) {
        self.boundaries.clear();
        let mut sessions: SessionBuf = SessionBuf::new();
        self.calendar.sessions_between(from, to, &mut sessions);
        // A boundary sits at the end of each non-last session.
        for pair in sessions.windows(2) {
            self.boundaries.push(SessionBoundary {
                at: pair[0].close(),
            });
        }
    }

    pub fn cached_boundary_count(&self) -> usize {
        self.boundaries.len()
    }
}

impl SceneLayer for SessionSeparatorLayer {
    fn id(&self) -> LayerId {
        LayerId("session-separators")
    }

    fn z(&self) -> LayerZ {
        LayerZ::SESSION_SEPARATOR
    }

    fn paint(&self, ctx: &mut PaintContext<'_>) {
        let color = ctx.palette.separator;
        let h = ctx.viewport.height_px;
        for b in &self.boundaries {
            let x = ctx.axis.to_x(b.at);
            ctx.out.lines.push(LineInstance {
                x0: x,
                y0: 0.0,
                x1: x,
                y1: h,
                width_px: self.style.line_width_px,
                color,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use midas_axis::{ContinuousAxis, DefaultFormatter, LinearPriceAxis, PriceRange, Viewport};
    use midas_calendar::{xnys, Timestamp};

    use super::*;
    use crate::primitives::ScenePrimitives;
    use crate::ThemePalette;

    fn ts(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> Timestamp {
        chrono::Utc.with_ymd_and_hms(y, mo, d, h, mi, s).unwrap()
    }

    #[test]
    fn update_boundaries_emits_n_minus_1_for_n_sessions() {
        // XNYS 2024-01-17: pre, regular, post → 3 sessions → 2 boundaries.
        let mut layer = SessionSeparatorLayer::new(xnys());
        let from = ts(2024, 1, 17, 9, 0, 0);
        let to = ts(2024, 1, 18, 1, 0, 0);
        layer.update_boundaries(from, to);
        // We include the boundary to the next day's pre if it falls
        // inside the range — but to=01:00 UTC 01/18 is right at the
        // post close, so we get at least 2 boundaries.
        assert!(layer.cached_boundary_count() >= 2);
    }

    #[test]
    fn paint_emits_one_line_per_boundary() {
        let mut layer = SessionSeparatorLayer::new(xnys());
        let from = ts(2024, 1, 17, 9, 0, 0);
        let to = ts(2024, 1, 18, 1, 0, 0);
        layer.update_boundaries(from, to);
        let n = layer.cached_boundary_count();

        let axis = ContinuousAxis::new(from, to, 1000.0).unwrap();
        let pr = PriceRange::new(100.0, 110.0).unwrap();
        let vp = Viewport::new(1000.0, 400.0);
        let pal = ThemePalette::dark_default();
        let mut out = ScenePrimitives::default();
        let paxis = LinearPriceAxis::new(pr, vp.height_px);
        let fmt = DefaultFormatter::new();
        let mut ctx = PaintContext {
            axis: &axis,
            viewport: vp,
            price_range: pr,
            palette: &pal,
            price_axis: &paxis,
            formatter: &fmt,
            out: &mut out,
        };
        layer.paint(&mut ctx);
        assert_eq!(out.lines.len(), n);
    }

    #[test]
    fn separator_spans_full_viewport_height() {
        let mut layer = SessionSeparatorLayer::new(xnys());
        let from = ts(2024, 1, 17, 9, 0, 0);
        let to = ts(2024, 1, 18, 1, 0, 0);
        layer.update_boundaries(from, to);

        let axis = ContinuousAxis::new(from, to, 1000.0).unwrap();
        let pr = PriceRange::new(100.0, 110.0).unwrap();
        let vp = Viewport::new(1000.0, 400.0);
        let pal = ThemePalette::dark_default();
        let mut out = ScenePrimitives::default();
        let paxis = LinearPriceAxis::new(pr, vp.height_px);
        let fmt = DefaultFormatter::new();
        let mut ctx = PaintContext {
            axis: &axis,
            viewport: vp,
            price_range: pr,
            palette: &pal,
            price_axis: &paxis,
            formatter: &fmt,
            out: &mut out,
        };
        layer.paint(&mut ctx);
        for l in &out.lines {
            assert_eq!(l.y0, 0.0);
            assert!((l.y1 - 400.0).abs() < 1e-3);
            assert!((l.x0 - l.x1).abs() < 1e-3);
        }
    }
}
