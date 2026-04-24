//! Crosshair label parity-harness fixtures.
//!
//! Slice 3 of the chart-transition plan. Slice 0 ships the real
//! PNG-diff harness; slice 9a wires the per-panel backend toggle that
//! lets dev-harness fire a real parity compare. Until then we land
//! two `ScenePrimitives` fixture tests (`crosshair_aapl_m1`,
//! `crosshair_btc_m1`) that pin the synthetic output for regression
//! purposes.
//!
//! The fixture shape is deliberately narrow: we assert on primitive
//! counts + the text payloads of the OHLC rows, NOT on pixel colors
//! or font-metric-sensitive placement. AA flap (R5 in the plan) is
//! absorbed by keeping tolerances on screen coords ≥ 1 px and never
//! comparing antialiased edges.

use std::sync::Arc;

use chrono::TimeZone;
use midas_axis::{for_calendar, DefaultFormatter, LinearPriceAxis, PriceRange, Viewport};
use midas_bars::{BarPeriod, Candle, CandleSeries, Completeness, Ohlcv, Symbol};
use midas_calendar::{crypto_spot, xnys, Timestamp};
use midas_scene::layers::{CrosshairLayer, SharedCandleSeries};
use midas_scene::{
    PaintContext, SceneLayer, ScenePrimitives, TextAnchor, TextInstance, ThemePalette,
};
use parking_lot::RwLock;

fn utc(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> Timestamp {
    chrono::Utc.with_ymd_and_hms(y, mo, d, h, mi, 0).unwrap()
}

fn mk_crypto_candle(start: Timestamp, minute_offset: i64, price: f64) -> Candle {
    let cal = crypto_spot();
    let sym = Symbol::new("BTC-USD", cal.id());
    let ts = start + chrono::Duration::minutes(minute_offset);
    let session = cal.classify(ts);
    let window = cal.bar_window(ts, BarPeriod::m1()).unwrap();
    let ohlcv = Ohlcv::new(
        price,
        price + 100.0,
        price - 80.0,
        price + 40.0,
        1_000,
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

fn mk_aapl_candle(start: Timestamp, minute_offset: i64, price: f64) -> Candle {
    let cal = xnys();
    let sym = Symbol::new("AAPL", cal.id());
    let ts = start + chrono::Duration::minutes(minute_offset);
    let session = cal.classify(ts);
    let window = cal.bar_window(ts, BarPeriod::m1()).unwrap();
    let ohlcv = Ohlcv::new(
        price,
        price + 0.10,
        price - 0.08,
        price + 0.04,
        100,
        5,
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

fn btc_series(n: usize, base: f64) -> SharedCandleSeries {
    let cal = crypto_spot();
    let sym = Symbol::new("BTC-USD", cal.id());
    let mut s = CandleSeries::new(cal.id(), BarPeriod::m1(), sym);
    let start = utc(2024, 1, 1, 0, 0);
    for i in 0..n {
        s.push(mk_crypto_candle(start, i as i64, base + i as f64));
    }
    Arc::new(RwLock::new(s))
}

fn aapl_series(n: usize, base: f64) -> SharedCandleSeries {
    let cal = xnys();
    let sym = Symbol::new("AAPL", cal.id());
    let mut s = CandleSeries::new(cal.id(), BarPeriod::m1(), sym);
    let start = utc(2024, 1, 17, 14, 30); // 09:30 ET
    for i in 0..n {
        s.push(mk_aapl_candle(start, i as i64, base + i as f64 * 0.01));
    }
    Arc::new(RwLock::new(s))
}

/// Fixture: `crosshair_btc_m1`. Crypto spot calendar, 12-hour axis,
/// cursor at the horizontal midpoint → lands on bar 360 of a 500-bar
/// 1-min series.
#[test]
fn crosshair_btc_m1_fixture() {
    let axis = for_calendar(
        crypto_spot(),
        (utc(2024, 1, 1, 0, 0), utc(2024, 1, 1, 12, 0)),
        1000.0,
    );
    let pr = PriceRange::new(49_000.0, 51_000.0).unwrap();
    let vp = Viewport::new(1000.0, 400.0);
    let paxis = LinearPriceAxis::new(pr, vp.height_px);
    let palette = ThemePalette::dark_default();
    let fmt = DefaultFormatter::new();
    let mut out = ScenePrimitives::default();
    let mut ctx = PaintContext {
        axis: axis.as_ref(),
        viewport: vp,
        price_range: pr,
        palette: &palette,
        price_axis: &paxis,
        formatter: &fmt,
        out: &mut out,
    };

    let series = btc_series(500, 50_000.0);
    let layer = CrosshairLayer::with_position((500.0, 200.0))
        .with_series(series)
        .with_tick_size(0.01);
    layer.paint(&mut ctx);

    // Fixture assertions.
    // - 2 arms.
    assert_eq!(ctx.out.lines.len(), 2, "expected 2 crosshair arms");
    // - 2 axis labels + 4 OHLC rows = 6 text primitives.
    assert_eq!(ctx.out.text.len(), 6, "expected 6 text primitives");

    // Bar 360 (0-indexed) → open = 50_000 + 360 = 50_360.0.
    // High = 50_460.0, low = 50_280.0, close = 50_400.0.
    let ohlc = collect_ohlc(&ctx.out.text);
    assert_eq!(ohlc.get("O:"), Some(&"50360.00".to_string()));
    assert_eq!(ohlc.get("H:"), Some(&"50460.00".to_string()));
    assert_eq!(ohlc.get("L:"), Some(&"50280.00".to_string()));
    assert_eq!(ohlc.get("C:"), Some(&"50400.00".to_string()));

    // Price label at right margin — middle of 49_000..51_000 at y=200
    // → 50_000.00.
    let price_label = ctx
        .out
        .text
        .iter()
        .find(|t| matches!(t.anchor, TextAnchor::MiddleRight))
        .expect("right-margin price label");
    assert_eq!(price_label.text, "50000.00");
    assert!(
        (price_label.x - (vp.width_px - 4.0)).abs() < 1e-3,
        "price label must sit at right margin",
    );

    // Time label at bottom margin — x=500 on a 12h axis → 6h mark.
    let time_label = ctx
        .out
        .text
        .iter()
        .find(|t| matches!(t.anchor, TextAnchor::BottomCenter))
        .expect("bottom-margin time label");
    assert_eq!(time_label.text, "06:00:00");
    assert!((time_label.y - (vp.height_px - 4.0)).abs() < 1e-3);
}

/// Fixture: `crosshair_aapl_m1`. XNYS compressed axis, RTH session,
/// 60 M1 bars. Cursor over the mid-session bar.
#[test]
fn crosshair_aapl_m1_fixture() {
    // The compressed axis for XNYS walks sessions over the window; 1
    // weekday (2024-01-17, RTH 09:30–16:00 ET = 14:30–21:00 UTC).
    let axis = for_calendar(
        xnys(),
        (utc(2024, 1, 17, 14, 30), utc(2024, 1, 17, 21, 0)),
        1000.0,
    );
    let pr = PriceRange::new(99.0, 101.0).unwrap();
    let vp = Viewport::new(1000.0, 400.0);
    let paxis = LinearPriceAxis::new(pr, vp.height_px);
    let palette = ThemePalette::dark_default();
    let fmt = DefaultFormatter::new();
    let mut out = ScenePrimitives::default();
    let mut ctx = PaintContext {
        axis: axis.as_ref(),
        viewport: vp,
        price_range: pr,
        palette: &palette,
        price_axis: &paxis,
        formatter: &fmt,
        out: &mut out,
    };

    let series = aapl_series(60, 100.0);
    // Cursor at x=50 — near the left edge so we fall on an early bar.
    let layer = CrosshairLayer::with_position((50.0, 100.0))
        .with_series(series)
        .with_tick_size(0.01);
    layer.paint(&mut ctx);

    // 2 arms.
    assert_eq!(ctx.out.lines.len(), 2);
    // 2 axis labels + 4 OHLC rows — 6 total. (Time label may be
    // absent in a compressed gap, but x=50 is inside an RTH session
    // so `axis.from_x` returns `Some`.)
    assert_eq!(ctx.out.text.len(), 6);

    // OHLC payloads exist.
    let ohlc = collect_ohlc(&ctx.out.text);
    assert!(ohlc.contains_key("O:"));
    assert!(ohlc.contains_key("H:"));
    assert!(ohlc.contains_key("L:"));
    assert!(ohlc.contains_key("C:"));
}

/// Fixture: crosshair over an empty series. Arms emit, axis labels
/// emit, but no OHLC rows land.
#[test]
fn crosshair_empty_series_fixture() {
    let axis = for_calendar(
        crypto_spot(),
        (utc(2024, 1, 1, 0, 0), utc(2024, 1, 1, 12, 0)),
        1000.0,
    );
    let pr = PriceRange::new(49_000.0, 51_000.0).unwrap();
    let vp = Viewport::new(1000.0, 400.0);
    let paxis = LinearPriceAxis::new(pr, vp.height_px);
    let palette = ThemePalette::dark_default();
    let fmt = DefaultFormatter::new();
    let mut out = ScenePrimitives::default();
    let mut ctx = PaintContext {
        axis: axis.as_ref(),
        viewport: vp,
        price_range: pr,
        palette: &palette,
        price_axis: &paxis,
        formatter: &fmt,
        out: &mut out,
    };

    // Empty series attached.
    let cal = crypto_spot();
    let sym = Symbol::new("BTC-USD", cal.id());
    let series: SharedCandleSeries = Arc::new(RwLock::new(CandleSeries::new(
        cal.id(),
        BarPeriod::m1(),
        sym,
    )));
    let layer = CrosshairLayer::with_position((500.0, 200.0)).with_series(series);
    layer.paint(&mut ctx);

    assert_eq!(ctx.out.lines.len(), 2, "arms emit even with empty series");
    // Two axis labels (price + time). No OHLC rows.
    assert_eq!(ctx.out.text.len(), 2);
    assert!(
        ctx.out
            .text
            .iter()
            .all(|t| !t.text.starts_with("O: ")
                && !t.text.starts_with("H: ")
                && !t.text.starts_with("L: ")
                && !t.text.starts_with("C: ")),
    );
}

/// Collect the four OHLC rows from a `TextInstance` slice into a
/// `(prefix, value)` map for assertion.
fn collect_ohlc(text: &[TextInstance]) -> std::collections::HashMap<String, String> {
    text.iter()
        .filter_map(|t| {
            let s = t.text.as_ref();
            if s.starts_with("O: ")
                || s.starts_with("H: ")
                || s.starts_with("L: ")
                || s.starts_with("C: ")
            {
                let prefix = s[..2].to_string();
                let value = s[3..].to_string();
                Some((prefix, value))
            } else {
                None
            }
        })
        .collect()
}
