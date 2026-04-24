//! [`CrosshairLayer`] — mouse-following horizontal + vertical guides
//! + OHLC/time/price labels.
//!
//! ## Slice 3 upgrades (chart-transition plan)
//!
//! Slice 2's `CrosshairLayer` emitted two `LineInstance`s at the mouse
//! position. Slice 3 keeps that core geometry but adds:
//!
//! - **Time label at bottom margin**: the wall-clock time of the bar
//!   under the cursor, formatted via
//!   [`LabelFormatter::time`][midas_axis::LabelFormatter::time].
//! - **Price label at right margin**: the price corresponding to the
//!   cursor y, formatted via
//!   [`LabelFormatter::price`][midas_axis::LabelFormatter::price].
//! - **OHLC box near the cursor**: four `TextInstance`s stacked
//!   vertically (`O:`, `H:`, `L:`, `C:`), only emitted when the cursor
//!   sits over an actual candle in the attached
//!   [`SharedCandleSeries`].
//!
//! ## Series resolution
//!
//! The layer optionally carries an `Arc<RwLock<CandleSeries>>`. When
//! absent (the construction path used by today's `scene_builder.rs` —
//! slice 6/7 own the file edit to wire it up), only the arms + axis
//! labels emit; no OHLC. When present, the paint path resolves the
//! cursor timestamp via
//! [`TimeAxis::from_x`][midas_axis::TimeAxis::from_x] and then
//! binary-searches the series by `ts_open` to find the bar containing
//! the cursor timestamp — `O(log n)` per paint.
//!
//! ## API contract preservation
//!
//! [`CrosshairLayer::new`] and [`CrosshairLayer::with_position`] keep
//! their old signatures so slice 2 construction sites compile
//! unchanged. The new fields default to "no labels beyond the arms"
//! — callers enable them via builder-style setters
//! ([`CrosshairLayer::with_series`], [`CrosshairLayer::with_timezone`],
//! [`CrosshairLayer::with_tick_size`]).

use std::borrow::Cow;

use chrono_tz::Tz;
use midas_axis::TickDensity;
use midas_bars::CandleRef;

use crate::layer::{LayerId, LayerZ, SceneLayer};
use crate::layers::candle::SharedCandleSeries;
use crate::paint::PaintContext;
use crate::primitives::{LineInstance, TextAnchor, TextInstance};

/// Padding between axis labels and the viewport edge, in logical pixels.
const AXIS_LABEL_MARGIN_PX: f32 = 4.0;

/// Font size for every crosshair label (arms tooltip + OHLC box).
/// Matches the 11 px convention used by `PriceLineLayer` / `LevelLayer`.
const LABEL_FONT_SIZE_PX: f32 = 11.0;

/// Horizontal offset between the cursor and the OHLC box. Positive =
/// box sits to the right of the cursor. Negative values shift it to
/// the left when the cursor is near the right viewport edge so the
/// box stays on-screen.
const OHLC_BOX_OFFSET_PX: f32 = 8.0;

/// Approximate width of the OHLC box used for the right-edge spill
/// check. The real width is font-metrics-dependent — this constant is
/// a conservative upper bound that keeps the box on-screen without
/// requiring a measurement pass.
const OHLC_BOX_WIDTH_PX: f32 = 110.0;

/// Vertical spacing between the four OHLC rows.
const OHLC_ROW_STRIDE_PX: f32 = 14.0;

/// Default price tick-size when the caller doesn't override via
/// [`CrosshairLayer::with_tick_size`]. `0.01` matches US equities.
const DEFAULT_TICK_SIZE: f64 = 0.01;

/// Two thin lines tracking the mouse + optional axis labels + OHLC
/// box. Emits only when `position` is `Some`. The shader layer
/// updates `position` on pointer-move via the widget's
/// [`SessionChart::set_crosshair`] / `clear_crosshair` pair.
///
/// Clone is `O(1)` (the `Arc` handle is cheap to clone); the layer
/// itself carries no bulky state. `Send + Sync` drops out of the
/// `Arc<RwLock<_>>` + `Copy` field composition.
#[derive(Clone, Debug)]
pub struct CrosshairLayer {
    pub position: Option<(f32, f32)>,
    pub line_width_px: f32,
    /// Optional shared candle series. When `Some`, the paint path
    /// binary-searches for the candle under the cursor and emits an
    /// OHLC box. When `None`, only arms + axis labels emit.
    series: Option<SharedCandleSeries>,
    /// Timezone used for time-label formatting. Defaults to UTC; the
    /// session-chart widget overrides this to the calendar-local tz
    /// for equities (follow-up wiring in slice 6/7 where
    /// `scene_builder.rs` gains the `.with_timezone(calendar.tz())`
    /// call).
    timezone: Tz,
    /// Instrument tick size. Drives
    /// [`LabelFormatter::price`][midas_axis::LabelFormatter::price]
    /// decimal-place selection. Defaults to `0.01`.
    tick_size: f64,
}

impl Default for CrosshairLayer {
    fn default() -> Self {
        Self::new()
    }
}

impl CrosshairLayer {
    /// Construct an empty crosshair layer with no cursor position. The
    /// layer is a no-op until [`CrosshairLayer::with_position`] (or a
    /// direct `position` write) sets a pixel anchor.
    pub fn new() -> Self {
        Self {
            position: None,
            line_width_px: 1.0,
            series: None,
            timezone: chrono_tz::UTC,
            tick_size: DEFAULT_TICK_SIZE,
        }
    }

    /// Convenience constructor that installs a cursor position. Kept
    /// for slice 2 call-site compatibility — tests constructed from
    /// this path still compile unchanged.
    pub fn with_position(position: (f32, f32)) -> Self {
        Self {
            position: Some(position),
            line_width_px: 1.0,
            series: None,
            timezone: chrono_tz::UTC,
            tick_size: DEFAULT_TICK_SIZE,
        }
    }

    /// Attach a shared candle series. The paint path will binary-
    /// search this series for the bar under the cursor and emit an
    /// OHLC box near the cursor when a bar is found. Returns `self`
    /// for builder-style chaining.
    #[must_use]
    pub fn with_series(mut self, series: SharedCandleSeries) -> Self {
        self.series = Some(series);
        self
    }

    /// Override the time-label timezone (default: UTC). Exchange-local
    /// timezones (e.g. `America::New_York` for XNYS) are set by the
    /// caller using knowledge of the calendar.
    #[must_use]
    pub fn with_timezone(mut self, tz: Tz) -> Self {
        self.timezone = tz;
        self
    }

    /// Override the instrument tick size used for price formatting.
    #[must_use]
    pub fn with_tick_size(mut self, tick_size: f64) -> Self {
        self.tick_size = tick_size;
        self
    }

    /// Currently attached candle series, if any. Exposed for tests +
    /// diagnostics; the scene pipeline does not consume it.
    pub fn series(&self) -> Option<&SharedCandleSeries> {
        self.series.as_ref()
    }

    /// Set the cursor position directly. Mirrors the free-field
    /// access (`layer.position = Some(pt)`) but keeps the idiom
    /// builder-style-friendly for callers that pipeline mutations.
    pub fn set_position(&mut self, pt: Option<(f32, f32)>) {
        self.position = pt;
    }
}

impl SceneLayer for CrosshairLayer {
    fn id(&self) -> LayerId {
        LayerId("crosshair")
    }

    fn z(&self) -> LayerZ {
        LayerZ::CROSSHAIR
    }

    fn paint(&self, ctx: &mut PaintContext<'_>) {
        let Some((x, y)) = self.position else {
            return;
        };
        let vp = ctx.viewport;
        // Guard: cursor off-chart → emit nothing. The shader layer
        // already clears `position` on `CursorLeft`, but a rebuilt
        // scene may carry a stale pixel coordinate from the previous
        // frame for one tick — clip defensively so tests that
        // construct the layer with out-of-bounds coordinates don't
        // flood the scene with off-screen primitives.
        if !x.is_finite()
            || !y.is_finite()
            || x < 0.0
            || x > vp.width_px
            || y < 0.0
            || y > vp.height_px
        {
            tracing::debug!(
                target: "midas_scene::crosshair::off_chart",
                x, y,
                vp_w = vp.width_px,
                vp_h = vp.height_px,
                "cursor off-chart; no emission",
            );
            return;
        }

        let color = ctx.palette.crosshair;
        let w = self.line_width_px.max(0.5);
        // Horizontal arm.
        ctx.out.lines.push(LineInstance {
            x0: 0.0,
            y0: y,
            x1: vp.width_px,
            y1: y,
            width_px: w,
            color,
        });
        // Vertical arm.
        ctx.out.lines.push(LineInstance {
            x0: x,
            y0: 0.0,
            x1: x,
            y1: vp.height_px,
            width_px: w,
            color,
        });

        // Price label at the right margin, vertically centred on the
        // horizontal arm.
        let price = ctx.y_to_price(y);
        if price.is_finite() {
            let label = ctx.formatter.price(price, self.tick_size);
            ctx.out.text.push(TextInstance {
                x: vp.width_px - AXIS_LABEL_MARGIN_PX,
                y,
                color: ctx.palette.text,
                text: Cow::Owned(label),
                size_px: LABEL_FONT_SIZE_PX,
                anchor: TextAnchor::MiddleRight,
            });
        }

        // Time label at the bottom margin, horizontally centred on
        // the vertical arm. `axis.from_x` returns `None` when the
        // cursor sits in a compressed gap — skip emission there.
        if let Some(ts) = ctx.axis.from_x(x) {
            let text = ctx
                .formatter
                .time(ts, self.timezone, TickDensity::Dense);
            ctx.out.text.push(TextInstance {
                x,
                y: vp.height_px - AXIS_LABEL_MARGIN_PX,
                color: ctx.palette.text,
                text: Cow::Owned(text),
                size_px: LABEL_FONT_SIZE_PX,
                anchor: TextAnchor::BottomCenter,
            });
        }

        // OHLC box — only when a series is attached and the cursor
        // sits over a resolvable candle.
        self.emit_ohlc_box(ctx, x, y);
    }
}

impl CrosshairLayer {
    /// Resolve the candle under the cursor and emit a 4-row OHLC
    /// tooltip. No-op when `self.series` is `None`, the series is
    /// empty, or `axis.from_x` lands inside a compressed gap.
    fn emit_ohlc_box(&self, ctx: &mut PaintContext<'_>, cursor_x: f32, cursor_y: f32) {
        let Some(series) = &self.series else {
            return;
        };
        let Some(ts) = ctx.axis.from_x(cursor_x) else {
            return;
        };
        let guard = series.read();
        if guard.is_empty() {
            return;
        }

        // Binary search by `ts_open`. We want the largest `idx` such
        // that `ts_open(idx) <= ts`. `partition_point` with `<=`
        // returns the first idx strictly greater than `ts`; subtract
        // one to land on the active bar. An empty prefix (cursor
        // before first bar) → no emission.
        //
        // `partition_point` walks `0..guard.len()` with an
        // `O(log n)` predicate, which costs exactly one read-guard
        // hit per probe. At 5K bars that is ~13 probes — negligible
        // inside the render hot path.
        let len = guard.len();
        let idx_after = (0..len).collect::<Vec<_>>().partition_point(|&i| {
            guard
                .at(i)
                .map(|c| c.ts_open() <= ts)
                .unwrap_or(false)
        });
        if idx_after == 0 {
            // Cursor before the first bar.
            return;
        }
        let idx = idx_after - 1;
        let Some(bar) = guard.at(idx) else {
            return;
        };

        // Emit four rows: O / H / L / C. Colour from palette.text so
        // the labels pick up dark/light theme automatically.
        let rows = ohlc_rows(&bar, ctx, self.tick_size);

        // Spill check: if the cursor is close to the right edge,
        // flip the box to the left of the cursor so it stays inside
        // the viewport.
        let spill_right = cursor_x + OHLC_BOX_OFFSET_PX + OHLC_BOX_WIDTH_PX > ctx.viewport.width_px;
        let (anchor_x, anchor) = if spill_right {
            (cursor_x - OHLC_BOX_OFFSET_PX, TextAnchor::TopRight)
        } else {
            (cursor_x + OHLC_BOX_OFFSET_PX, TextAnchor::TopLeft)
        };

        // Stack rows downward from the cursor. If the stack would
        // spill below the viewport, flip it upward.
        let total_h = OHLC_ROW_STRIDE_PX * rows.len() as f32;
        let stack_downward = cursor_y + total_h < ctx.viewport.height_px;
        let y_base = if stack_downward {
            cursor_y + OHLC_BOX_OFFSET_PX
        } else {
            cursor_y - OHLC_BOX_OFFSET_PX - total_h
        };

        for (row_idx, label) in rows.into_iter().enumerate() {
            let row_y = y_base + OHLC_ROW_STRIDE_PX * row_idx as f32;
            ctx.out.text.push(TextInstance {
                x: anchor_x,
                y: row_y,
                color: ctx.palette.text,
                text: Cow::Owned(label),
                size_px: LABEL_FONT_SIZE_PX,
                anchor,
            });
        }

        tracing::debug!(
            target: "midas_scene::crosshair::ohlc",
            idx,
            ts = %bar.ts_open(),
            "emit ohlc box",
        );
    }
}

/// Build the four OHLC row strings in `[O, H, L, C]` order. Separate
/// function for direct unit testing without constructing a full
/// `PaintContext`.
fn ohlc_rows(bar: &CandleRef<'_>, ctx: &PaintContext<'_>, tick_size: f64) -> [String; 4] {
    [
        format!("O: {}", ctx.formatter.price(bar.open(), tick_size)),
        format!("H: {}", ctx.formatter.price(bar.high(), tick_size)),
        format!("L: {}", ctx.formatter.price(bar.low(), tick_size)),
        format!("C: {}", ctx.formatter.price(bar.close(), tick_size)),
    ]
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use chrono::TimeZone;
    use midas_axis::{ContinuousAxis, DefaultFormatter, LinearPriceAxis, PriceRange, Viewport};
    use midas_bars::{BarPeriod, Candle, CandleSeries, Completeness, Ohlcv, Symbol};
    use midas_calendar::{crypto_spot, Timestamp};
    use parking_lot::RwLock;

    use super::*;
    use crate::primitives::ScenePrimitives;
    use crate::ThemePalette;

    fn ts(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> Timestamp {
        chrono::Utc.with_ymd_and_hms(y, mo, d, h, mi, s).unwrap()
    }

    fn harness() -> (
        ContinuousAxis,
        LinearPriceAxis,
        PriceRange,
        Viewport,
        ThemePalette,
        DefaultFormatter,
    ) {
        let axis =
            ContinuousAxis::new(ts(2024, 1, 1, 0, 0, 0), ts(2024, 1, 2, 0, 0, 0), 1000.0).unwrap();
        let pr = PriceRange::new(90.0, 110.0).unwrap();
        let vp = Viewport::new(1000.0, 400.0);
        let paxis = LinearPriceAxis::new(pr, vp.height_px);
        (
            axis,
            paxis,
            pr,
            vp,
            ThemePalette::dark_default(),
            DefaultFormatter::new(),
        )
    }

    fn mk_crypto_candle(start: Timestamp, minute_offset: i64, open: f64) -> Candle {
        let cal = crypto_spot();
        let sym = Symbol::new("BTC-USD", cal.id());
        let ts = start + chrono::Duration::minutes(minute_offset);
        let session = cal.classify(ts);
        let window = cal.bar_window(ts, BarPeriod::m1()).unwrap();
        let ohlcv = Ohlcv::new(
            open,
            open + 5.0,
            open - 3.0,
            open + 2.5,
            100,
            1,
            None,
        )
        .unwrap();
        Candle::new(
            sym,
            cal,
            BarPeriod::m1(),
            session,
            window,
            ohlcv,
            Completeness::Completed,
        )
        .unwrap()
    }

    /// Build a crypto series with `n` contiguous M1 bars starting at
    /// `2024-01-01T00:00:00Z`. Prices linearly ramp from `base`.
    fn fill_crypto_series(n: usize, base: f64) -> SharedCandleSeries {
        let cal = crypto_spot();
        let sym = Symbol::new("BTC-USD", cal.id());
        let mut s = CandleSeries::new(cal.id(), BarPeriod::m1(), sym);
        let start = ts(2024, 1, 1, 0, 0, 0);
        for i in 0..n {
            s.push(mk_crypto_candle(start, i as i64, base + i as f64));
        }
        Arc::new(RwLock::new(s))
    }

    fn empty_crypto_series() -> SharedCandleSeries {
        let cal = crypto_spot();
        let sym = Symbol::new("BTC-USD", cal.id());
        Arc::new(RwLock::new(CandleSeries::new(
            cal.id(),
            BarPeriod::m1(),
            sym,
        )))
    }

    // ── Test 1 — slice 2 API contract preservation ───────────────────

    /// Old `CrosshairLayer::new()` + `with_position(pt)` constructors
    /// still compile. Plan hard-requirement: "tests that construct
    /// it today must still compile".
    #[test]
    fn legacy_constructors_compile() {
        let _empty = CrosshairLayer::new();
        let _positioned = CrosshairLayer::with_position((100.0, 50.0));
    }

    /// `None` position emits nothing.
    #[test]
    fn none_position_emits_zero_primitives() {
        let (axis, paxis, pr, vp, pal, fmt) = harness();
        let mut out = ScenePrimitives::default();
        let mut ctx = PaintContext {
            axis: &axis,
            viewport: vp,
            price_range: pr,
            palette: &pal,
            price_axis: &paxis,
            formatter: &fmt,
            out: &mut out,
        };
        CrosshairLayer::new().paint(&mut ctx);
        assert!(ctx.out.is_empty());
    }

    // ── Test 2 — arms (regression from slice 2) ──────────────────────

    /// With a position and NO series, two arm lines + two axis
    /// labels (price + time) emit — no OHLC rows.
    #[test]
    fn cursor_over_empty_series_emits_only_arms_and_axis_labels() {
        let (axis, paxis, pr, vp, pal, fmt) = harness();
        let mut out = ScenePrimitives::default();
        let mut ctx = PaintContext {
            axis: &axis,
            viewport: vp,
            price_range: pr,
            palette: &pal,
            price_axis: &paxis,
            formatter: &fmt,
            out: &mut out,
        };
        let layer = CrosshairLayer::with_position((500.0, 200.0))
            .with_series(empty_crypto_series());
        layer.paint(&mut ctx);
        assert_eq!(out.lines.len(), 2, "arms must always emit");
        // With an empty series no OHLC rows land — only the two
        // axis labels (price + time).
        assert_eq!(out.text.len(), 2);
    }

    /// Arms span the full viewport (preserved from slice 2).
    #[test]
    fn arms_span_full_viewport() {
        let (axis, paxis, pr, vp, pal, fmt) = harness();
        let mut out = ScenePrimitives::default();
        let mut ctx = PaintContext {
            axis: &axis,
            viewport: vp,
            price_range: pr,
            palette: &pal,
            price_axis: &paxis,
            formatter: &fmt,
            out: &mut out,
        };
        CrosshairLayer::with_position((300.0, 100.0)).paint(&mut ctx);
        // Horizontal: x0 == 0, x1 == viewport.width_px.
        assert_eq!(out.lines[0].x0, 0.0);
        assert!((out.lines[0].x1 - 1000.0).abs() < 1e-3);
        assert!((out.lines[0].y0 - out.lines[0].y1).abs() < 1e-3);
        // Vertical: y0 == 0, y1 == viewport.height_px.
        assert_eq!(out.lines[1].y0, 0.0);
        assert!((out.lines[1].y1 - 400.0).abs() < 1e-3);
        assert!((out.lines[1].x0 - out.lines[1].x1).abs() < 1e-3);
    }

    // ── Test 3 — axis labels (price at right, time at bottom) ────────

    /// Without a series the crosshair still emits one price label at
    /// the right margin + one time label at the bottom margin.
    #[test]
    fn cursor_with_no_series_emits_price_and_time_axis_labels() {
        let (axis, paxis, pr, vp, pal, fmt) = harness();
        let mut out = ScenePrimitives::default();
        let mut ctx = PaintContext {
            axis: &axis,
            viewport: vp,
            price_range: pr,
            palette: &pal,
            price_axis: &paxis,
            formatter: &fmt,
            out: &mut out,
        };
        CrosshairLayer::with_position((500.0, 200.0)).paint(&mut ctx);
        // Exactly two text primitives: price + time.
        assert_eq!(out.text.len(), 2);
    }

    /// Price label sits at the viewport's right edge (minus the
    /// margin) and uses `MiddleRight` anchor so the text grows
    /// leftward from the edge.
    #[test]
    fn price_label_anchors_at_right_margin() {
        let (axis, paxis, pr, vp, pal, fmt) = harness();
        let mut out = ScenePrimitives::default();
        let mut ctx = PaintContext {
            axis: &axis,
            viewport: vp,
            price_range: pr,
            palette: &pal,
            price_axis: &paxis,
            formatter: &fmt,
            out: &mut out,
        };
        CrosshairLayer::with_position((500.0, 200.0)).paint(&mut ctx);
        // The price label lands at x == width - margin.
        let price_label = &out
            .text
            .iter()
            .find(|t| matches!(t.anchor, TextAnchor::MiddleRight))
            .expect("expected price label at right margin");
        assert!(
            (price_label.x - (vp.width_px - AXIS_LABEL_MARGIN_PX)).abs() < 1e-3,
            "price label x={} expected {}",
            price_label.x,
            vp.width_px - AXIS_LABEL_MARGIN_PX
        );
        // y centred on the horizontal arm.
        assert!((price_label.y - 200.0).abs() < 1e-3);
        // The text is the formatter's rendering of the midpoint
        // price (y=200 → centre of 90..110 → 100.0 → "100.00").
        assert_eq!(price_label.text, "100.00");
    }

    /// Time label sits at the viewport's bottom edge (minus the
    /// margin) and uses `BottomCenter` anchor so the text sits
    /// horizontally centred above the cursor's x.
    #[test]
    fn time_label_anchors_at_bottom_margin() {
        let (axis, paxis, pr, vp, pal, fmt) = harness();
        let mut out = ScenePrimitives::default();
        let mut ctx = PaintContext {
            axis: &axis,
            viewport: vp,
            price_range: pr,
            palette: &pal,
            price_axis: &paxis,
            formatter: &fmt,
            out: &mut out,
        };
        CrosshairLayer::with_position((500.0, 200.0)).paint(&mut ctx);
        let time_label = out
            .text
            .iter()
            .find(|t| matches!(t.anchor, TextAnchor::BottomCenter))
            .expect("expected time label at bottom margin");
        assert!(
            (time_label.y - (vp.height_px - AXIS_LABEL_MARGIN_PX)).abs() < 1e-3,
            "time label y={} expected {}",
            time_label.y,
            vp.height_px - AXIS_LABEL_MARGIN_PX
        );
        // x centred on the vertical arm.
        assert!((time_label.x - 500.0).abs() < 1e-3);
        // `from_x(500)` on a 24h axis lands at the 12h mark ⇒
        // `12:00:00` under `Dense` density.
        assert_eq!(time_label.text, "12:00:00");
    }

    // ── Test 4 — OHLC box over a candle ──────────────────────────────

    /// Cursor directly over bar `idx=5` of a 60-bar M1 series: four
    /// TextInstances emit (O/H/L/C) plus the two axis labels, for
    /// six text primitives total.
    #[test]
    fn cursor_over_candle_emits_ohlc_box_plus_axis_labels() {
        let (axis, paxis, pr, vp, pal, fmt) = harness();
        let mut out = ScenePrimitives::default();
        let mut ctx = PaintContext {
            axis: &axis,
            viewport: vp,
            price_range: pr,
            palette: &pal,
            price_axis: &paxis,
            formatter: &fmt,
            out: &mut out,
        };
        // Axis spans 24 h → 1000 px → 24 × 60 bars = 1440 bars.
        // x=500 → midnight + 12 h → bar index 720. Build a series
        // spanning 721 bars so the cursor lands on the last bar.
        let series = fill_crypto_series(721, 100.0);
        let layer = CrosshairLayer::with_position((500.0, 200.0))
            .with_series(series);
        layer.paint(&mut ctx);

        let ohlc_count = out
            .text
            .iter()
            .filter(|t| {
                t.text.starts_with("O: ")
                    || t.text.starts_with("H: ")
                    || t.text.starts_with("L: ")
                    || t.text.starts_with("C: ")
            })
            .count();
        assert_eq!(ohlc_count, 4, "expected 4 OHLC rows, got {}", ohlc_count);
        // Plus the two axis labels (right-margin price +
        // bottom-margin time).
        assert_eq!(out.text.len(), 6);
        // Arms still present.
        assert_eq!(out.lines.len(), 2);
    }

    /// OHLC rows appear in `[O, H, L, C]` order. The row containing
    /// `O:` lands first (smallest y), `C:` last (largest y) when the
    /// cursor is in the top half of the viewport (stack-downward
    /// path).
    #[test]
    fn ohlc_rows_are_ordered_o_h_l_c_top_to_bottom() {
        let (axis, paxis, pr, vp, pal, fmt) = harness();
        let mut out = ScenePrimitives::default();
        let mut ctx = PaintContext {
            axis: &axis,
            viewport: vp,
            price_range: pr,
            palette: &pal,
            price_axis: &paxis,
            formatter: &fmt,
            out: &mut out,
        };
        let series = fill_crypto_series(721, 100.0);
        let layer = CrosshairLayer::with_position((500.0, 50.0))
            .with_series(series);
        layer.paint(&mut ctx);

        // Collect only the OHLC rows, sorted by their y coordinate.
        let mut rows: Vec<(f32, &str)> = out
            .text
            .iter()
            .filter(|t| {
                t.text.starts_with("O: ")
                    || t.text.starts_with("H: ")
                    || t.text.starts_with("L: ")
                    || t.text.starts_with("C: ")
            })
            .map(|t| (t.y, t.text.as_ref()))
            .collect();
        rows.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        let order: Vec<&str> = rows
            .iter()
            .map(|(_y, s)| s.split_whitespace().next().unwrap())
            .collect();
        assert_eq!(order, vec!["O:", "H:", "L:", "C:"]);
    }

    /// Each OHLC row carries the expected formatted value. Bar 720
    /// opens at `100.0 + 720 = 820.0`, high `+5`, low `-3`,
    /// close `+2.5` — formatted under the penny tick.
    #[test]
    fn ohlc_rows_carry_formatted_values() {
        let (axis, _paxis, _pr, vp, pal, fmt) = harness();
        let mut out = ScenePrimitives::default();
        // Widen the price range so `y_to_price` doesn't interfere
        // with the payload check.
        let pr = PriceRange::new(0.0, 2000.0).unwrap();
        let paxis = LinearPriceAxis::new(pr, vp.height_px);
        let mut ctx = PaintContext {
            axis: &axis,
            viewport: vp,
            price_range: pr,
            palette: &pal,
            price_axis: &paxis,
            formatter: &fmt,
            out: &mut out,
        };
        let series = fill_crypto_series(721, 100.0);
        let layer = CrosshairLayer::with_position((500.0, 200.0))
            .with_series(series);
        layer.paint(&mut ctx);

        let rows: std::collections::HashMap<&str, &str> = out
            .text
            .iter()
            .filter_map(|t| {
                let s = t.text.as_ref();
                if s.starts_with("O: ")
                    || s.starts_with("H: ")
                    || s.starts_with("L: ")
                    || s.starts_with("C: ")
                {
                    let prefix = &s[0..2];
                    let rest = &s[3..];
                    Some((prefix, rest))
                } else {
                    None
                }
            })
            .collect();
        // Bar 720 (0-indexed) → open = 100 + 720 = 820.0.
        assert_eq!(rows.get("O:"), Some(&"820.00"));
        assert_eq!(rows.get("H:"), Some(&"825.00"));
        assert_eq!(rows.get("L:"), Some(&"817.00"));
        assert_eq!(rows.get("C:"), Some(&"822.50"));
    }

    // ── Test 5 — anchor / spill logic ────────────────────────────────

    /// Cursor far from the right edge → OHLC box anchors `TopLeft`.
    #[test]
    fn ohlc_box_anchors_top_left_when_cursor_left_of_centre() {
        let (axis, paxis, pr, vp, pal, fmt) = harness();
        let mut out = ScenePrimitives::default();
        let mut ctx = PaintContext {
            axis: &axis,
            viewport: vp,
            price_range: pr,
            palette: &pal,
            price_axis: &paxis,
            formatter: &fmt,
            out: &mut out,
        };
        // 1000 px wide. Box width ≈ 110. 200 + 8 + 110 = 318 < 1000.
        let series = fill_crypto_series(721, 100.0);
        let layer = CrosshairLayer::with_position((200.0, 100.0))
            .with_series(series);
        layer.paint(&mut ctx);
        for t in out.text.iter() {
            if t.text.starts_with("O: ") {
                assert!(matches!(t.anchor, TextAnchor::TopLeft));
                // x = cursor + offset = 208.
                assert!((t.x - 208.0).abs() < 1e-3);
                return;
            }
        }
        panic!("no OHLC row found");
    }

    /// Cursor near the right edge → OHLC box flips to `TopRight`
    /// anchor so it grows leftward and stays on-screen.
    #[test]
    fn ohlc_box_spills_left_when_cursor_near_right_edge() {
        let (axis, paxis, pr, vp, pal, fmt) = harness();
        let mut out = ScenePrimitives::default();
        let mut ctx = PaintContext {
            axis: &axis,
            viewport: vp,
            price_range: pr,
            palette: &pal,
            price_axis: &paxis,
            formatter: &fmt,
            out: &mut out,
        };
        // 950 + 8 + 110 = 1068 > 1000 → spill check fires.
        let series = fill_crypto_series(1440, 100.0);
        let layer = CrosshairLayer::with_position((950.0, 100.0))
            .with_series(series);
        layer.paint(&mut ctx);
        for t in out.text.iter() {
            if t.text.starts_with("O: ") {
                assert!(matches!(t.anchor, TextAnchor::TopRight));
                // x = cursor - offset = 942.
                assert!((t.x - 942.0).abs() < 1e-3);
                return;
            }
        }
        panic!("no OHLC row found");
    }

    // ── Test 6 — off-chart / defensive guards ────────────────────────

    /// Cursor outside viewport bounds emits zero primitives. Tests
    /// the defensive clip the paint path runs to cover the one-frame
    /// stale-position race.
    #[test]
    fn cursor_off_chart_emits_no_primitives() {
        let (axis, paxis, pr, vp, pal, fmt) = harness();
        let mut out = ScenePrimitives::default();
        let mut ctx = PaintContext {
            axis: &axis,
            viewport: vp,
            price_range: pr,
            palette: &pal,
            price_axis: &paxis,
            formatter: &fmt,
            out: &mut out,
        };
        // Negative x is off-chart.
        let layer = CrosshairLayer::with_position((-10.0, 200.0));
        layer.paint(&mut ctx);
        assert!(ctx.out.is_empty());

        // Past-right-edge x is off-chart.
        let layer = CrosshairLayer::with_position((1200.0, 200.0));
        layer.paint(&mut ctx);
        assert!(ctx.out.is_empty());

        // Below bottom edge.
        let layer = CrosshairLayer::with_position((500.0, 500.0));
        layer.paint(&mut ctx);
        assert!(ctx.out.is_empty());

        // NaN is off-chart.
        let layer = CrosshairLayer::with_position((f32::NAN, 200.0));
        layer.paint(&mut ctx);
        assert!(ctx.out.is_empty());
    }

    /// `set_position(None)` clears previously-set position → next
    /// paint emits nothing. Mirrors the widget's
    /// `SessionChart::clear_crosshair` → `CrosshairLayer` wiring.
    #[test]
    fn clear_position_via_set_position_emits_no_primitives() {
        let (axis, paxis, pr, vp, pal, fmt) = harness();
        let mut out = ScenePrimitives::default();
        let mut ctx = PaintContext {
            axis: &axis,
            viewport: vp,
            price_range: pr,
            palette: &pal,
            price_axis: &paxis,
            formatter: &fmt,
            out: &mut out,
        };
        let mut layer = CrosshairLayer::with_position((500.0, 200.0));
        layer.set_position(None);
        layer.paint(&mut ctx);
        assert!(ctx.out.is_empty());
    }

    /// Cursor BEFORE the first bar in a populated series emits no
    /// OHLC rows (only arms + axis labels).
    #[test]
    fn cursor_before_first_bar_emits_no_ohlc() {
        let (axis, paxis, pr, vp, pal, fmt) = harness();
        let mut out = ScenePrimitives::default();
        let mut ctx = PaintContext {
            axis: &axis,
            viewport: vp,
            price_range: pr,
            palette: &pal,
            price_axis: &paxis,
            formatter: &fmt,
            out: &mut out,
        };
        // Axis starts at 2024-01-01T00:00:00Z. Series' first bar is
        // at the same instant (minute 0). We need the cursor strictly
        // BEFORE the first bar. Build a series whose first bar sits
        // at minute 5 of the axis → cursor at x=0 falls before it.
        let cal = crypto_spot();
        let sym = Symbol::new("BTC-USD", cal.id());
        let mut s = CandleSeries::new(cal.id(), BarPeriod::m1(), sym);
        let start = ts(2024, 1, 1, 0, 5, 0);
        for i in 0..10 {
            s.push(mk_crypto_candle(start, i, 100.0 + i as f64));
        }
        let series: SharedCandleSeries = Arc::new(RwLock::new(s));
        let layer =
            CrosshairLayer::with_position((0.5, 200.0)).with_series(series);
        layer.paint(&mut ctx);

        let ohlc_count = out
            .text
            .iter()
            .filter(|t| t.text.starts_with("O: "))
            .count();
        assert_eq!(ohlc_count, 0);
    }

    // ── Test 7 — builder-style setters ───────────────────────────────

    /// `with_tick_size` affects price-label precision.
    #[test]
    fn with_tick_size_tunes_price_decimals() {
        let (axis, paxis, pr, vp, pal, fmt) = harness();
        let mut out = ScenePrimitives::default();
        let mut ctx = PaintContext {
            axis: &axis,
            viewport: vp,
            price_range: pr,
            palette: &pal,
            price_axis: &paxis,
            formatter: &fmt,
            out: &mut out,
        };
        // Integer tick → 0-decimal price label.
        let layer = CrosshairLayer::with_position((500.0, 200.0)).with_tick_size(1.0);
        layer.paint(&mut ctx);
        let price_label = out
            .text
            .iter()
            .find(|t| matches!(t.anchor, TextAnchor::MiddleRight))
            .expect("price label");
        assert_eq!(price_label.text, "100");
    }

    /// `with_timezone` is stored on the layer.
    #[test]
    fn with_timezone_retains_tz() {
        let layer = CrosshairLayer::new().with_timezone(chrono_tz::America::New_York);
        assert_eq!(layer.timezone, chrono_tz::America::New_York);
    }

    /// `with_series` installs the series reference.
    #[test]
    fn with_series_installs_reference() {
        let series = empty_crypto_series();
        let layer = CrosshairLayer::new().with_series(Arc::clone(&series));
        assert!(layer.series().is_some());
    }

    // ── Test 8 — atlas sharing: multiple crosshair layers compose ────

    /// Two crosshair layers in the same scene both emit text into
    /// the SHARED `ScenePrimitives.text` vector — no separate atlas
    /// per layer. Verifies the contract that the renderer's text
    /// atlas is shared across layers via the common output buffer.
    #[test]
    fn multiple_layers_share_text_buffer() {
        let (axis, paxis, pr, vp, pal, fmt) = harness();
        let mut out = ScenePrimitives::default();
        let mut ctx = PaintContext {
            axis: &axis,
            viewport: vp,
            price_range: pr,
            palette: &pal,
            price_axis: &paxis,
            formatter: &fmt,
            out: &mut out,
        };
        let a = CrosshairLayer::with_position((300.0, 100.0));
        let b = CrosshairLayer::with_position((700.0, 200.0));
        a.paint(&mut ctx);
        let after_a = ctx.out.text.len();
        b.paint(&mut ctx);
        let after_b = ctx.out.text.len();
        assert!(after_b > after_a, "layer b must append to shared buffer");
        // Each layer emits 2 axis labels (price + time) → 4 total.
        assert_eq!(after_b, 4);
        // Arms count doubles too.
        assert_eq!(ctx.out.lines.len(), 4);
    }

    /// Clearing the shared buffer between frames resets all layers'
    /// emissions. Regression guard for the "paint clears out" contract.
    #[test]
    fn scene_primitives_clear_resets_layer_output() {
        let (axis, paxis, pr, vp, pal, fmt) = harness();
        let mut out = ScenePrimitives::default();
        let mut ctx = PaintContext {
            axis: &axis,
            viewport: vp,
            price_range: pr,
            palette: &pal,
            price_axis: &paxis,
            formatter: &fmt,
            out: &mut out,
        };
        let layer = CrosshairLayer::with_position((500.0, 200.0));
        layer.paint(&mut ctx);
        let first = ctx.out.text.len();
        ctx.out.clear();
        layer.paint(&mut ctx);
        let second = ctx.out.text.len();
        assert_eq!(first, second, "same layer → same emission count per frame");
    }
}
