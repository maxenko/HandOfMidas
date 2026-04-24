//! [`HolidayMarkerLayer`] — small badges at market holidays.

use std::ops::Range;

use chrono::{Datelike, NaiveDate, TimeZone};
use midas_calendar::ExchangeCalendar;

use crate::layer::{LayerId, LayerZ, SceneLayer};
use crate::paint::PaintContext;
use crate::primitives::BadgeInstance;

/// Tiny top-of-viewport badges anchored at each holiday inside the
/// cached year range.
///
/// Holds an exchange-calendar reference so `paint` can anchor each
/// holiday at 12:00 LOCAL time (not UTC midnight — which, for XNYS in
/// winter, is the previous ET trading day). Matches the pattern used
/// by [`SessionBandLayer`](crate::layers::SessionBandLayer).
pub struct HolidayMarkerLayer {
    /// Calendar used to resolve the anchor timezone (see `paint`).
    calendar: &'static dyn ExchangeCalendar,
    /// `(date, holiday_name)` pairs — pre-populated at construction via
    /// [`HolidayMarkerLayer::new`].
    pub holidays: Vec<(NaiveDate, &'static str)>,
}

impl HolidayMarkerLayer {
    /// Pre-populate the holiday cache by iterating `year_range` and
    /// picking dates where `calendar.trading_day(date)` reports
    /// `is_holiday`. Dates with a missing `holiday_name` fall back to
    /// the string `"Holiday"`.
    pub fn new(calendar: &'static dyn ExchangeCalendar, year_range: Range<i32>) -> Self {
        let mut out: Vec<(NaiveDate, &'static str)> = Vec::new();
        for year in year_range {
            // Walk every date in the year. Cheap: ~365 iterations per
            // year, and this runs once at chart-build.
            let start = NaiveDate::from_ymd_opt(year, 1, 1).unwrap();
            let end = NaiveDate::from_ymd_opt(year + 1, 1, 1).unwrap();
            let mut d = start;
            while d < end {
                if let Ok(day) = calendar.trading_day(d) {
                    if day.is_holiday {
                        let name = day.holiday_name.unwrap_or("Holiday");
                        out.push((d, name));
                    }
                }
                d = d.succ_opt().unwrap_or(end);
                if d.year() != year {
                    break;
                }
            }
        }
        Self {
            calendar,
            holidays: out,
        }
    }

    /// Alternate constructor for tests / fixtures — inject an explicit
    /// list of holidays without walking a calendar. Still needs a
    /// calendar reference for the tz-aware anchor in `paint`.
    pub fn from_dates(
        calendar: &'static dyn ExchangeCalendar,
        holidays: Vec<(NaiveDate, &'static str)>,
    ) -> Self {
        Self { calendar, holidays }
    }
}

impl SceneLayer for HolidayMarkerLayer {
    fn id(&self) -> LayerId {
        LayerId("holiday-markers")
    }

    fn z(&self) -> LayerZ {
        LayerZ::HOLIDAY_MARKER
    }

    fn paint(&self, ctx: &mut PaintContext<'_>) {
        let color = ctx.palette.separator;
        let width = ctx.viewport.width_px;
        // Axis time range bracket: map the two viewport edges back to
        // timestamps and reject holidays falling outside. Compressed
        // axes return `None` for edge x's inside a gap; fall back to
        // snapping in those cases.
        let (ts_start, _) = ctx
            .axis
            .from_x_snapped(0.0, midas_axis::SnapDirection::Forward);
        let (ts_end, _) = ctx
            .axis
            .from_x_snapped(width, midas_axis::SnapDirection::Backward);
        let tz = self.calendar.tz();
        for (date, name) in &self.holidays {
            // Anchor the badge at 12:00 LOCAL (exchange tz) so the
            // marker sits INSIDE the would-be trading day. Using UTC
            // midnight on `date` is wrong for XNYS in winter (05:00
            // UTC = 00:00 ET is still the previous trading day).
            //
            // DST ambiguity at 12:00 local is impossible — spring-
            // forward is 02:00→03:00, fall-back is 02:00→01:00. Noon
            // always resolves unambiguously.
            let naive_noon = date.and_hms_opt(12, 0, 0).unwrap();
            let local = match tz.from_local_datetime(&naive_noon).single() {
                Some(dt) => dt,
                None => match tz.from_local_datetime(&naive_noon).earliest() {
                    Some(dt) => dt,
                    None => continue,
                },
            };
            let ts = local.with_timezone(&chrono::Utc);
            if ts < ts_start || ts > ts_end {
                continue;
            }
            let x = ctx.axis.to_x(ts);
            if x < 0.0 || x > width {
                continue;
            }
            ctx.out.badges.push(BadgeInstance {
                x: x - 6.0,
                y: 2.0,
                w: 12.0,
                h: 12.0,
                color,
                text: (*name).into(),
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
    fn from_dates_populates_buffer() {
        let layer = HolidayMarkerLayer::from_dates(
            xnys(),
            vec![(
                NaiveDate::from_ymd_opt(2024, 1, 1).unwrap(),
                "New Year's Day",
            )],
        );
        assert_eq!(layer.holidays.len(), 1);
    }

    #[test]
    fn paint_emits_badge_inside_viewport() {
        let layer = HolidayMarkerLayer::from_dates(
            xnys(),
            vec![(NaiveDate::from_ymd_opt(2024, 1, 1).unwrap(), "New Year")],
        );
        let axis = ContinuousAxis::new(ts(2023, 12, 20, 0, 0, 0), ts(2024, 1, 20, 0, 0, 0), 1000.0)
            .unwrap();
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
        assert_eq!(out.badges.len(), 1);
        assert_eq!(out.badges[0].text.as_ref(), "New Year");
    }

    #[test]
    fn walking_calendar_finds_nyse_holidays() {
        let layer = HolidayMarkerLayer::new(xnys(), 2024..2025);
        assert!(!layer.holidays.is_empty());
        // New Year's Day, MLK, Presidents', Good Friday, Memorial, Juneteenth,
        // Independence, Labor, Thanksgiving, Christmas — 10 in 2024.
        assert!(layer.holidays.len() >= 8);
    }

    /// Regression: H1 bug-hunt finding (see
    /// `plan/session-aware-charts/99-diagnostic-findings-r2.md`). Prior
    /// behaviour anchored each holiday at `date.and_hms_opt(0,0,0)`
    /// interpreted as UTC — which for XNYS in winter is 00:00 UTC =
    /// 19:00 PREVIOUS-day ET, i.e. a different trading day. The fix
    /// anchors at 12:00 LOCAL (America/New_York for XNYS) so the
    /// emitted badge x matches the to_x of ET-noon on the holiday
    /// date.
    #[test]
    fn badge_x_matches_et_noon_not_utc_midnight() {
        // 2024-01-01 New Year's Day. In winter (EST, UTC-5), 12:00 ET =
        // 17:00 UTC.
        let holiday_date = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        let layer = HolidayMarkerLayer::from_dates(xnys(), vec![(holiday_date, "New Year")]);

        let axis = ContinuousAxis::new(ts(2023, 12, 20, 0, 0, 0), ts(2024, 1, 20, 0, 0, 0), 1000.0)
            .unwrap();
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
        assert_eq!(out.badges.len(), 1);

        // Expected: badge.x computed from 2024-01-01 17:00 UTC (= 12:00
        // ET), minus the 6-pixel badge half-width offset the layer
        // applies.
        use midas_axis::TimeAxis;
        let expected_ts = ts(2024, 1, 1, 17, 0, 0);
        let expected_x = axis.to_x(expected_ts) - 6.0;
        let got_x = out.badges[0].x;
        assert!(
            (got_x - expected_x).abs() < 1e-3,
            "badge anchored at ET noon, not UTC midnight: got {got_x}, expected {expected_x}"
        );

        // Defense: make sure the old-broken computation (UTC midnight
        // = 2024-01-01 00:00 UTC) would land at a DIFFERENT x.
        let old_broken_ts = ts(2024, 1, 1, 0, 0, 0);
        let old_broken_x = axis.to_x(old_broken_ts) - 6.0;
        assert!(
            (got_x - old_broken_x).abs() > 1.0,
            "fix must produce a different x than UTC-midnight anchor"
        );
    }
}
