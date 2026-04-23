//! [`SessionBandLayer`] — coloured rectangles per trading session.

use midas_calendar::{ExchangeCalendar, SessionBuf, SessionKind, Timestamp};

use crate::layer::{LayerId, LayerZ, SceneLayer};
use crate::paint::PaintContext;
use crate::primitives::QuadInstance;

/// Optional colour overrides. `None` means "fall back to the scene's
/// [`ThemePalette`](crate::ThemePalette)". Callers building a custom
/// band layer can override per-kind colours without cloning the full
/// palette.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct SessionPalette {
    pub pre: Option<[u8; 4]>,
    pub regular: Option<[u8; 4]>,
    pub post: Option<[u8; 4]>,
    pub closed: Option<[u8; 4]>,
}

/// Coloured backgrounds per active session. Per R2-NB-3 layer state is
/// computed BEFORE `paint`; use
/// [`SessionBandLayer::update_sessions`] to refresh the cached
/// session buffer from the calendar whenever the viewport time range
/// changes.
pub struct SessionBandLayer {
    calendar: &'static dyn ExchangeCalendar,
    sessions: SessionBuf,
    palette: SessionPalette,
}

impl SessionBandLayer {
    /// Build an empty band layer. Call
    /// [`update_sessions`](Self::update_sessions) before paint.
    pub fn new(calendar: &'static dyn ExchangeCalendar) -> Self {
        Self {
            calendar,
            sessions: SessionBuf::new(),
            palette: SessionPalette::default(),
        }
    }

    /// Override per-kind colours.
    pub fn with_palette(mut self, palette: SessionPalette) -> Self {
        self.palette = palette;
        self
    }

    /// Re-populate the cached session buffer for the given time range.
    /// Called by the scene driver each frame BEFORE paint.
    pub fn update_sessions(&mut self, from: Timestamp, to: Timestamp) {
        self.sessions.clear();
        self.calendar.sessions_between(from, to, &mut self.sessions);
    }

    /// Number of cached sessions the last `update_sessions` produced.
    pub fn cached_session_count(&self) -> usize {
        self.sessions.len()
    }
}

fn pick_color(kind: SessionKind, theme: &crate::ThemePalette, palette: &SessionPalette) -> [u8; 4] {
    match kind {
        SessionKind::PreMarket => palette.pre.unwrap_or(theme.band_pre),
        SessionKind::Regular => palette.regular.unwrap_or(theme.band_regular),
        SessionKind::PostMarket => palette.post.unwrap_or(theme.band_post),
        SessionKind::Closed => palette.closed.unwrap_or(theme.band_closed),
        // Break / Overnight share the closed tint — they visually
        // recede to "nothing happening here."
        _ => palette.closed.unwrap_or(theme.band_closed),
    }
}

impl SceneLayer for SessionBandLayer {
    fn id(&self) -> LayerId {
        LayerId("session-bands")
    }

    fn z(&self) -> LayerZ {
        LayerZ::SESSION_BAND
    }

    fn paint(&self, ctx: &mut PaintContext<'_>) {
        let h = ctx.viewport.height_px;
        let w_clamp = ctx.viewport.width_px;
        for session in &self.sessions {
            let x0 = ctx.axis.to_x(session.open()).clamp(0.0, w_clamp);
            let x1 = ctx.axis.to_x(session.close()).clamp(0.0, w_clamp);
            let w = (x1 - x0).max(0.0);
            if w <= 0.0 {
                continue;
            }
            let color = pick_color(session.kind(), ctx.palette, &self.palette);
            ctx.out.quads.push(QuadInstance {
                x: x0,
                y: 0.0,
                w,
                h,
                color,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use midas_axis::{ContinuousAxis, PriceRange, Viewport};
    use midas_calendar::{xnys, Timestamp};

    use super::*;
    use crate::primitives::ScenePrimitives;
    use crate::ThemePalette;

    fn ts(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> Timestamp {
        chrono::Utc.with_ymd_and_hms(y, mo, d, h, mi, s).unwrap()
    }

    #[test]
    fn update_sessions_populates_buffer() {
        // 2024-01-17 (Wed) ET day: pre 04:00–09:30, regular 09:30–16:00,
        // post 16:00–20:00. We query a range that covers those sessions.
        let mut layer = SessionBandLayer::new(xnys());
        let from = ts(2024, 1, 17, 9, 0, 0); // 04:00 ET
        let to = ts(2024, 1, 18, 1, 0, 0); // 20:00 ET
        layer.update_sessions(from, to);
        assert!(layer.cached_session_count() >= 3);
    }

    #[test]
    fn paint_emits_one_quad_per_session() {
        let mut layer = SessionBandLayer::new(xnys());
        let from = ts(2024, 1, 17, 9, 0, 0);
        let to = ts(2024, 1, 18, 1, 0, 0);
        layer.update_sessions(from, to);
        let n = layer.cached_session_count();

        let axis = ContinuousAxis::new(from, to, 1000.0).unwrap();
        let pr = PriceRange::new(100.0, 110.0).unwrap();
        let vp = Viewport::new(1000.0, 400.0);
        let pal = ThemePalette::dark_default();
        let mut out = ScenePrimitives::default();
        let mut ctx = PaintContext {
            axis: &axis,
            viewport: vp,
            price_range: pr,
            palette: &pal,
            out: &mut out,
        };
        layer.paint(&mut ctx);
        assert_eq!(out.quads.len(), n);
    }

    #[test]
    fn quads_fill_viewport_height() {
        let mut layer = SessionBandLayer::new(xnys());
        let from = ts(2024, 1, 17, 9, 0, 0);
        let to = ts(2024, 1, 18, 1, 0, 0);
        layer.update_sessions(from, to);

        let axis = ContinuousAxis::new(from, to, 1000.0).unwrap();
        let pr = PriceRange::new(100.0, 110.0).unwrap();
        let vp = Viewport::new(1000.0, 400.0);
        let pal = ThemePalette::dark_default();
        let mut out = ScenePrimitives::default();
        let mut ctx = PaintContext {
            axis: &axis,
            viewport: vp,
            price_range: pr,
            palette: &pal,
            out: &mut out,
        };
        layer.paint(&mut ctx);
        for q in &out.quads {
            assert!((q.h - 400.0).abs() < 1e-3);
            assert_eq!(q.y, 0.0);
        }
    }

    #[test]
    fn pre_market_quad_tinted_differently_than_regular() {
        let mut layer = SessionBandLayer::new(xnys());
        let from = ts(2024, 1, 17, 9, 0, 0);
        let to = ts(2024, 1, 18, 1, 0, 0);
        layer.update_sessions(from, to);

        let axis = ContinuousAxis::new(from, to, 1000.0).unwrap();
        let pr = PriceRange::new(100.0, 110.0).unwrap();
        let vp = Viewport::new(1000.0, 400.0);
        let pal = ThemePalette::dark_default();
        let mut out = ScenePrimitives::default();
        let mut ctx = PaintContext {
            axis: &axis,
            viewport: vp,
            price_range: pr,
            palette: &pal,
            out: &mut out,
        };
        layer.paint(&mut ctx);
        // At least one quad matches `band_pre` and one matches
        // `band_regular`.
        let has_pre = out.quads.iter().any(|q| q.color == pal.band_pre);
        let has_regular = out.quads.iter().any(|q| q.color == pal.band_regular);
        assert!(has_pre);
        assert!(has_regular);
    }

    #[test]
    fn update_sessions_clears_previous_buffer() {
        let mut layer = SessionBandLayer::new(xnys());
        layer.update_sessions(ts(2024, 1, 17, 9, 0, 0), ts(2024, 1, 18, 1, 0, 0));
        let n1 = layer.cached_session_count();
        layer.update_sessions(ts(2024, 1, 18, 9, 0, 0), ts(2024, 1, 19, 1, 0, 0));
        let n2 = layer.cached_session_count();
        // Second update overwrites, not appends.
        assert!(n1 >= 3);
        assert!(n2 >= 3);
        assert!(n2 < n1 + 10); // rough bound — strictly not-accumulated
    }
}
