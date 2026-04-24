//! Full-scene integration test: XNYS m1 data, every layer on.
//!
//! Asserts the scene produces the expected number of primitives of
//! each type. Exercises layer ordering across the whole `LayerZ` stack
//! and confirms sans-IO layers cooperate without touching real I/O.

use std::borrow::Cow;
use std::sync::Arc;

use chrono::TimeZone;
use midas_axis::{ContinuousAxis, PriceRange, TimeAxis, Viewport};
use midas_bars::{Candle, CandleSeries, Completeness, Ohlcv, Symbol};
use midas_calendar::{xnys, BarPeriod, Timestamp};
use midas_scene::layers::{
    CandleLayer, CrosshairLayer, GridLayer, HolidayMarkerLayer, LevelLayer, LevelView,
    OrderBracketLayer, OrderBracketView, PriceLineLayer, PriceLineView, SessionBandLayer,
    SessionSeparatorLayer, Side, VolumeLayer,
};
use midas_scene::{ChartScene, ScenePrimitives, ThemePalette};
use parking_lot::RwLock;

fn utc(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> Timestamp {
    chrono::Utc.with_ymd_and_hms(y, mo, d, h, mi, 0).unwrap()
}

fn mk_candle(ts: Timestamp, i: i64) -> Candle {
    let cal = xnys();
    let sym = Symbol::new("SPY", cal.id());
    let session = cal.classify(ts);
    let window = cal.bar_window(ts, BarPeriod::m1()).unwrap();
    let price = 100.0 + (i as f64) * 0.01;
    let ohlcv = Ohlcv::new(
        price,
        price + 0.05,
        price - 0.05,
        price + 0.02,
        1_000 + i as u64,
        10,
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

#[test]
fn full_xnys_scene_with_all_layers_emits_expected_primitives() {
    let cal = xnys();
    let mut series = CandleSeries::new(cal.id(), BarPeriod::m1(), Symbol::new("SPY", cal.id()));
    // 60 m1 bars starting 09:30 ET 2024-01-17.
    let start = utc(2024, 1, 17, 14, 30);
    const N: i64 = 60;
    for i in 0..N {
        series.push(mk_candle(start + chrono::Duration::minutes(i), i));
    }
    let series = Arc::new(RwLock::new(series));

    // Viewport spans pre-market through post-market for this day.
    let from = utc(2024, 1, 17, 9, 0); // 04:00 ET (PreMarket open)
    let to = utc(2024, 1, 18, 1, 0); // 20:00 ET (PostMarket close)

    let axis: Box<dyn TimeAxis> =
        Box::new(ContinuousAxis::new(from, to, 1200.0).expect("valid axis range"));
    let pr = PriceRange::new(99.0, 101.5).unwrap();
    let vp = Viewport::new(1200.0, 600.0);

    // Prepare calendar-driven layers.
    let mut bands = SessionBandLayer::new(cal);
    bands.update_sessions(from, to);
    let band_count = bands.cached_session_count();

    let mut separators = SessionSeparatorLayer::new(cal);
    separators.update_boundaries(from, to);
    let sep_count = separators.cached_boundary_count();

    let holidays = HolidayMarkerLayer::from_dates(
        xnys(),
        vec![(chrono::NaiveDate::from_ymd_opt(2024, 1, 15).unwrap(), "MLK")],
    );

    let brackets = OrderBracketLayer::new(vec![OrderBracketView {
        id: 1,
        entry_price: 100.5,
        tp_price: Some(101.0),
        sl_price: Some(100.0),
        side: Side::Long,
        label: Cow::Borrowed("E"),
    }]);

    let price_lines = PriceLineLayer::new(vec![PriceLineView {
        id: 1,
        price: 100.8,
        label: Cow::Borrowed("Target"),
        color: [255, 255, 0, 255],
    }]);

    let levels = LevelLayer::new(vec![LevelView {
        id: 1,
        price: 99.5,
        label: Cow::Borrowed("S1"),
        color: [255, 0, 0, 255],
        locked: false,
    }]);

    let crosshair = CrosshairLayer::with_position((500.0, 300.0));

    let scene = ChartScene::builder()
        .axis_boxed(axis)
        .price_range(pr)
        .viewport(vp)
        .palette(ThemePalette::dark_default())
        .layer(bands)
        .layer(GridLayer::with_defaults())
        .layer(separators)
        .layer(VolumeLayer::with_defaults(series.clone()))
        .layer(CandleLayer::with_defaults(series.clone()))
        .layer(holidays)
        .layer(price_lines)
        .layer(brackets)
        .layer(levels)
        .layer(crosshair)
        .build()
        .unwrap();

    let mut out = ScenePrimitives::default();
    scene.paint(&mut out);

    // --- Candles: exactly N candle instances.
    assert_eq!(out.candles.len(), N as usize);

    // --- Quads: bands (one per session) + volume bars (one per candle).
    assert_eq!(out.quads.len(), band_count + N as usize);

    // --- Lines: separators + grid vertical (axis.ticks) + grid horizontal
    //     (>= 2) + price-lines (1) + bracket (3 legs) + levels (1) +
    //     crosshair (2).
    let grid_ticks = scene.axis().ticks(midas_axis::TickDensity::Normal).len();
    let expected_line_min =
        sep_count + grid_ticks + 2 /* grid h */ + 1 /* price line */ + 3 /* bracket */ + 1 /* level */ + 2 /* crosshair */;
    assert!(
        out.lines.len() >= expected_line_min,
        "got {} lines, expected >= {}",
        out.lines.len(),
        expected_line_min
    );

    // --- Badges: bracket entry badge (1) + holiday marker inside viewport.
    //     MLK (2024-01-15) sits BEFORE from=01-17; badge skipped.
    //     So: 1 badge.
    assert_eq!(out.badges.len(), 1);

    // --- Text: 1 price-line label + 1 level label + 2 crosshair
    //     axis labels (price at right margin, time at bottom margin).
    //     The crosshair has no series attached in this fixture, so no
    //     OHLC box emits — see slice 3 of the chart-transition plan.
    assert_eq!(out.text.len(), 4);

    // Sanity: every primitive fits inside a finite range.
    for c in &out.candles {
        assert!(c.x_center.is_finite());
    }
    for q in &out.quads {
        assert!(q.w >= 0.0);
        assert!(q.h >= 0.0);
    }
}

#[test]
fn repeated_paint_does_not_accumulate() {
    let cal = xnys();
    let mut series = CandleSeries::new(cal.id(), BarPeriod::m1(), Symbol::new("SPY", cal.id()));
    let start = utc(2024, 1, 17, 14, 30);
    for i in 0..5 {
        series.push(mk_candle(start + chrono::Duration::minutes(i), i));
    }
    let series = Arc::new(RwLock::new(series));

    let axis = ContinuousAxis::new(start, start + chrono::Duration::hours(1), 1000.0).unwrap();
    let pr = PriceRange::new(99.0, 101.0).unwrap();
    let vp = Viewport::new(1000.0, 400.0);

    let scene = ChartScene::builder()
        .axis(axis)
        .price_range(pr)
        .viewport(vp)
        .layer(CandleLayer::with_defaults(series))
        .build()
        .unwrap();

    let mut out = ScenePrimitives::default();
    scene.paint(&mut out);
    let first = out.candles.len();
    scene.paint(&mut out);
    let second = out.candles.len();
    // Same primitive count — `paint` clears the buffer before each pass.
    assert_eq!(first, second);
    assert_eq!(first, 5);
}
