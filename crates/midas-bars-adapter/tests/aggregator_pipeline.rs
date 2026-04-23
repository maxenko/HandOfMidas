//! S7 `SessionedBarAggregator` integration test.
//!
//! Feeds a stream of synthetic trade ticks spanning the XNYS
//! PreMarket → Regular boundary (09:28 → 09:32 ET on 2024-01-17) into
//! a `SessionedBarStream` over an `mpsc<Arc<Tick>>` channel. Asserts:
//!
//! - Bars before 09:30 ET carry `SessionKind::PreMarket`.
//! - A boundary rollover lands at 09:30 ET; the first bar after it is
//!   `SessionKind::Regular`.
//! - Each bar's `window` matches `calendar.bar_window(first_tick_ts,
//!   Clock(M1))`.
//! - Candle `window.open` timestamps are strictly non-decreasing
//!   across the drained sequence.

use std::sync::Arc;

use chrono::TimeZone;
use midas_bars::Symbol;
use midas_bars_adapter::{AggregatorConfig, SessionedBarAggregator, SessionedBarStream};
use midas_broker_core::market_data::{ReqId, Tick, TickAttributes, TickKind, TickType, TickValue};
use midas_broker_core::SymbolKey;
use midas_calendar::{xnys, BarPeriod, SessionKind};
use midas_clock::SystemClock;
use midas_stream::BarStream;
use tokio::sync::mpsc;

fn utc(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> chrono::DateTime<chrono::Utc> {
    chrono::Utc.with_ymd_and_hms(y, mo, d, h, mi, s).unwrap()
}

fn ps_tick(price: f64, size: i64, ts: chrono::DateTime<chrono::Utc>) -> Tick {
    Tick {
        symbol: SymbolKey {
            contract_id: 265598,
            symbol: "AAPL".into(),
        },
        req_id: ReqId(1),
        kind: TickKind::PriceSize,
        tick_type: TickType::Last,
        value: TickValue::PriceSize { price, size },
        attrs: TickAttributes::default(),
        ts,
    }
}

#[tokio::test]
async fn pre_to_regular_m1_pipeline_yields_correctly_tagged_bars() {
    let cal = xnys();
    let sym = Symbol::new("AAPL", cal.id());
    let cfg = AggregatorConfig::new(sym, cal, BarPeriod::m1(), Arc::new(SystemClock))
        .with_partial_emit_rate_hz(0);
    let agg = SessionedBarAggregator::new(cfg).unwrap();
    let (tx, rx) = mpsc::channel::<Arc<Tick>>(16);
    let mut stream = SessionedBarStream::new(agg, rx);

    // Ticks spanning 09:28:00 ET → 09:32:30 ET (winter → UTC-5, so
    // 14:28 → 14:32:30 UTC). Two ticks per minute bucket so we can
    // exercise folding within a bar. Produce from a spawned task so a
    // small mpsc buffer doesn't deadlock the test.
    tokio::spawn(async move {
        let ticks: Vec<chrono::DateTime<chrono::Utc>> = (0..10)
            .map(|i| utc(2024, 1, 17, 14, 28, 0) + chrono::Duration::seconds(i * 30))
            .collect();
        let base_price = 100.0;
        for (i, ts) in ticks.iter().enumerate() {
            let price = base_price + (i as f64) * 0.1;
            if tx.send(Arc::new(ps_tick(price, 10, *ts))).await.is_err() {
                return;
            }
        }
    });

    // Drain every candle.
    let mut candles = Vec::new();
    while let Some(c) = stream.next().await {
        candles.push(c);
    }

    assert!(
        !candles.is_empty(),
        "pipeline must yield at least one candle"
    );

    // Non-decreasing window.open.
    let opens: Vec<_> = candles.iter().map(|c| c.window.open).collect();
    for pair in opens.windows(2) {
        assert!(
            pair[0] <= pair[1],
            "window.open timestamps must be non-decreasing: {:?}",
            pair
        );
    }

    // Every bar's window must match bar_window(first_tick_ts, M1).
    for c in &candles {
        let expected = cal.bar_window(c.window.open, BarPeriod::m1()).unwrap();
        assert_eq!(c.window, expected, "candle window must match bar_window");
    }

    // Find the Rollover seam: the first Completed bar's window should
    // close at ≤ 14:30 UTC, and its session should be PreMarket. The
    // next candle starts at 14:30 UTC and sits in Regular.
    let completed_pre = candles
        .iter()
        .find(|c| {
            c.completeness == midas_bars::Completeness::Completed
                && c.session.kind() == SessionKind::PreMarket
        })
        .expect("pipeline must close at least one PreMarket bar");
    assert!(completed_pre.window.close <= utc(2024, 1, 17, 14, 30, 0));

    // At least one candle must be Regular, opening exactly at 14:30 UTC.
    let regular = candles
        .iter()
        .find(|c| c.session.kind() == SessionKind::Regular)
        .expect("pipeline must include a Regular-session candle");
    assert_eq!(regular.window.open, utc(2024, 1, 17, 14, 30, 0));
}

#[tokio::test]
async fn drained_bar_count_spans_expected_minutes() {
    // Sanity: ticks across 5 minute-buckets → expect at least 5 distinct
    // window.open values in the drained candle stream.
    let cal = xnys();
    let sym = Symbol::new("AAPL", cal.id());
    let cfg = AggregatorConfig::new(sym, cal, BarPeriod::m1(), Arc::new(SystemClock))
        .with_partial_emit_rate_hz(0);
    let agg = SessionedBarAggregator::new(cfg).unwrap();
    let (tx, rx) = mpsc::channel::<Arc<Tick>>(16);
    let mut stream = SessionedBarStream::new(agg, rx);
    // One tick per second across 14:28:00 → 14:32:59 UTC (5 minutes). The
    // small mpsc buffer forces back-pressure; produce from a task so the
    // draining `stream.next()` can make progress concurrently.
    tokio::spawn(async move {
        for s in 0..(5 * 60) {
            let ts = utc(2024, 1, 17, 14, 28, 0) + chrono::Duration::seconds(s);
            if tx
                .send(Arc::new(ps_tick(100.0 + (s as f64) * 0.001, 1, ts)))
                .await
                .is_err()
            {
                return;
            }
        }
    });
    let mut opens = std::collections::BTreeSet::new();
    while let Some(c) = stream.next().await {
        opens.insert(c.window.open);
    }
    // 5 minute-buckets: [14:28, 14:29, 14:30, 14:31, 14:32).
    assert_eq!(opens.len(), 5);
}
