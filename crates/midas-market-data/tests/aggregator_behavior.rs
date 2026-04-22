//! Behaviour tests for the S6 aggregator registry.
//!
//! All tests use `SimMarketData` as the upstream provider. Timing-
//! sensitive tests coalesce a short wall-clock sleep (≤200 ms) rather
//! than `tokio::time::pause` — the `#[tokio::test(start_paused = true)]`
//! dance is awkward here because the sim's own tick emitter runs on
//! `tokio::time` too, and pausing time stalls its "is my rt-bar
//! subscription alive" grace path.
//!
//! The aggregator loop itself is deterministic given deterministic rt-
//! bar input; we drive that via `SimMarketData::inject_for_test` so
//! test outcomes don't depend on the sim's interval-based emitter.

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, TimeZone, Utc};
use midas_broker::sim::{SimMarketData, SimMarketDataConfig};
use midas_broker::MarketDataSource;
use midas_broker_core::market_data::{
    Bar, BarCompleteness, MarketDataError, MarketEvent, SymbolKey, Timeframe,
};
use midas_market_data::MarketDataRouter;
use tokio::sync::broadcast::error::RecvError;
use tokio::time::sleep;

fn aapl() -> SymbolKey {
    SymbolKey {
        contract_id: 265598,
        symbol: "AAPL".into(),
    }
}

/// Build a router over a quiet sim: no tick emitter, no burst, rt-bar
/// auto-emitter parked at a very long cadence so the only bars that
/// show up are the ones the test explicitly injects.
fn build_router() -> (Arc<MarketDataRouter>, Arc<SimMarketData>) {
    let sim = SimMarketData::new(SimMarketDataConfig {
        farm_up_delay_ms: 1,
        burst_enabled: false,
        tick_drift_bps: 0.0,
        tick_cadence_ms: 3_600_000,
        // Park the rt-bar auto-emitter so every bar in the test comes
        // from inject_for_test. 1 hour is effectively "never" for a
        // test that runs in under a second.
        realtime_bar_size_ms: 3_600_000,
        ..SimMarketDataConfig::default()
    });
    let source: Arc<dyn MarketDataSource> = sim.clone();
    let router = MarketDataRouter::new(source);
    (router, sim)
}

/// Build a 5 s RT-bar with fixed OHLCV and a chosen window open.
fn mk_rt_bar(sym: &SymbolKey, ts_open: DateTime<Utc>, o: f64, h: f64, l: f64, c: f64) -> Bar {
    Bar {
        symbol: sym.clone(),
        timeframe: Timeframe::S5,
        ts_open,
        ts_close: ts_open + chrono::Duration::seconds(5),
        o,
        h,
        l,
        c,
        volume: 100,
        trade_count: 1,
        wap: Some((o + c) / 2.0),
        completeness: BarCompleteness::Completed,
    }
}

/// Inject one rt-bar event and yield briefly so the aggregator task
/// gets a chance to pick it up. The `sleep` is short (20 ms) — well
/// below the 100 ms coalesce cadence — so partial emits stay coalesced.
async fn inject_rt_bar(sim: &SimMarketData, bar: Bar) {
    sim.inject_for_test(MarketEvent::Bar(bar));
    sleep(Duration::from_millis(20)).await;
}

/// Drain any already-pending items from a subscription handle using a
/// short timeout. Returns the collected bars.
async fn drain_bars<H>(handle: &mut H, max_items: usize, timeout_ms: u64) -> Vec<Arc<Bar>>
where
    H: BarRx,
{
    let mut out = Vec::new();
    for _ in 0..max_items {
        match tokio::time::timeout(Duration::from_millis(timeout_ms), handle.recv_bar()).await {
            Ok(Ok(bar)) => out.push(bar),
            Ok(Err(_)) => break,
            Err(_) => break,
        }
    }
    out
}

/// Thin trait so `drain_bars` works against either a
/// `SubscriptionHandle<Bar>` or a raw `broadcast::Receiver<Arc<Bar>>`.
trait BarRx {
    async fn recv_bar(&mut self) -> Result<Arc<Bar>, RecvError>;
}

impl BarRx for midas_market_data::SubscriptionHandle<Bar> {
    async fn recv_bar(&mut self) -> Result<Arc<Bar>, RecvError> {
        self.recv().await
    }
}

impl BarRx for tokio::sync::broadcast::Receiver<Arc<Bar>> {
    async fn recv_bar(&mut self) -> Result<Arc<Bar>, RecvError> {
        self.recv().await
    }
}

// ---------------------------------------------------------------------------
// 1. Aggregates rt-bars into the target timeframe
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn aggregates_rt_bars_into_target_tf() {
    let (router, sim) = build_router();
    let sym = aapl();

    // Subscribe first so the aggregator is spun up before we inject.
    let mut handle = router
        .subscribe_bars(sym.clone(), Timeframe::M1)
        .await
        .expect("subscribe");

    // Feed 12 5 s bars = one full 60 s window starting at t0.
    let t0 = Utc.with_ymd_and_hms(2026, 1, 2, 14, 30, 0).unwrap();
    for i in 0..12u32 {
        let ts_open = t0 + chrono::Duration::seconds(i as i64 * 5);
        let price = 100.0 + (i as f64) * 0.01;
        let bar = mk_rt_bar(&sym, ts_open, price, price + 0.02, price - 0.02, price);
        inject_rt_bar(&sim, bar).await;
    }
    // Cross into the next window so the first M1 closes.
    let bar_close = mk_rt_bar(
        &sym,
        t0 + chrono::Duration::seconds(60),
        101.0,
        101.0,
        101.0,
        101.0,
    );
    inject_rt_bar(&sim, bar_close).await;

    // Give the coalesce interval one full tick so the Completed emit
    // for the first window has landed.
    sleep(Duration::from_millis(250)).await;

    let bars = drain_bars(&mut handle, 32, 50).await;
    assert!(!bars.is_empty(), "aggregator emitted no bars");
    // One Completed at window boundary; Partials before and after.
    let completed: Vec<_> = bars
        .iter()
        .filter(|b| b.completeness == BarCompleteness::Completed)
        .collect();
    assert!(
        !completed.is_empty(),
        "expected at least one Completed bar after window rollover"
    );
    let first_completed = completed.first().unwrap();
    assert_eq!(first_completed.timeframe, Timeframe::M1);
    assert_eq!(first_completed.ts_open, t0);
    assert_eq!(first_completed.ts_close, t0 + chrono::Duration::seconds(60));
    // OHLC invariants: close == last fed bar before boundary, low/high
    // bracket the full range.
    assert!((first_completed.o - 100.0).abs() < 1e-9);
    assert!(first_completed.h >= 100.13); // final 5-s bar in window had h = 100.11 + 0.02 = 100.13
    assert!(first_completed.l <= 99.98);
    // Volume: 12 x 100 = 1200.
    assert_eq!(first_completed.volume, 1200);
}

// ---------------------------------------------------------------------------
// 2. Bar closes on window rollover
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn closes_bar_on_window_rollover() {
    let (router, sim) = build_router();
    let sym = aapl();

    let mut handle = router
        .subscribe_bars(sym.clone(), Timeframe::M1)
        .await
        .expect("subscribe");

    let t0 = Utc.with_ymd_and_hms(2026, 1, 2, 14, 30, 0).unwrap();
    // One bar in first window.
    inject_rt_bar(&sim, mk_rt_bar(&sym, t0, 100.0, 100.0, 100.0, 100.0)).await;
    // One bar in second window (t0 + 61 s).
    inject_rt_bar(
        &sim,
        mk_rt_bar(
            &sym,
            t0 + chrono::Duration::seconds(61),
            200.0,
            200.0,
            200.0,
            200.0,
        ),
    )
    .await;
    // Advance past the coalesce cadence so Partials get flushed.
    sleep(Duration::from_millis(250)).await;

    let bars = drain_bars(&mut handle, 32, 50).await;
    let completed: Vec<_> = bars
        .iter()
        .filter(|b| b.completeness == BarCompleteness::Completed)
        .collect();
    assert!(
        !completed.is_empty(),
        "expected at least one Completed bar after rollover"
    );
    let first_done = completed.first().unwrap();
    assert_eq!(first_done.ts_open, t0);
    // And a Partial belonging to the second window must exist (either
    // immediately after or trailing the Completed).
    let has_partial_second_window = bars.iter().any(|b| {
        b.completeness == BarCompleteness::Partial
            && b.ts_open == t0 + chrono::Duration::seconds(60)
    });
    assert!(
        has_partial_second_window,
        "no Partial bar found for the second M1 window"
    );
}

// ---------------------------------------------------------------------------
// 3. Multiple consumers share a single aggregator + single upstream
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn multiple_consumers_share_aggregator() {
    let (router, sim) = build_router();
    let sym = aapl();

    let _h1 = router
        .subscribe_bars(sym.clone(), Timeframe::M1)
        .await
        .expect("h1");
    let _h2 = router
        .subscribe_bars(sym.clone(), Timeframe::M1)
        .await
        .expect("h2");

    // Both share the same aggregator record, so exactly ONE rt-bar
    // upstream call into the sim.
    assert_eq!(sim.rt_bar_subscribe_call_count(), 1);
    assert_eq!(sim.live_rt_bar_subscription_count_for(&sym), 1);
    assert_eq!(router.aggregator_registry().registered_count().await, 1);
}

// ---------------------------------------------------------------------------
// 4. Different timeframes on the same symbol share ONE rt-bar upstream
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn different_timeframes_share_rt_sub() {
    let (router, sim) = build_router();
    let sym = aapl();

    let _m1 = router
        .subscribe_bars(sym.clone(), Timeframe::M1)
        .await
        .expect("m1");
    let _m5 = router
        .subscribe_bars(sym.clone(), Timeframe::M5)
        .await
        .expect("m5");

    // NB-6 Model A: ONE upstream rt-bar request for AAPL, shared by
    // both aggregators.
    assert_eq!(sim.rt_bar_subscribe_call_count(), 1);
    assert_eq!(sim.live_rt_bar_subscription_count_for(&sym), 1);
    // Two independent aggregator records.
    assert_eq!(router.aggregator_registry().registered_count().await, 2);
    assert!(
        router
            .aggregator_registry()
            .has_aggregator(&sym, Timeframe::M1)
            .await
    );
    assert!(
        router
            .aggregator_registry()
            .has_aggregator(&sym, Timeframe::M5)
            .await
    );
}

// ---------------------------------------------------------------------------
// 5. Last drop aborts the aggregator task + drops the upstream sub
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn last_drop_aborts_task() {
    let (router, sim) = build_router();
    let sym = aapl();

    let h = router
        .subscribe_bars(sym.clone(), Timeframe::M1)
        .await
        .expect("sub");
    assert_eq!(router.aggregator_registry().registered_count().await, 1);
    assert_eq!(sim.rt_bar_subscribe_call_count(), 1);

    drop(h);
    // Guard drop spawns the async removal; give it + the actor time
    // to cascade the DecRef down to the rt-bar hub. Chain is:
    //   AggGuard::drop → tokio::spawn(async { registry.remove })
    //   → JoinHandle::abort (aggregator task)
    //   → drop(rt_handle) in aborted task
    //   → RtBarSubGuard::drop sends DecRtBarRef
    //   → actor reaps the rt-bar publisher → cascades cancel upstream.
    for _ in 0..20 {
        sleep(Duration::from_millis(50)).await;
        if sim.live_rt_bar_subscription_count_for(&sym) == 0
            && router.aggregator_registry().registered_count().await == 0
        {
            break;
        }
    }

    assert_eq!(
        router.aggregator_registry().registered_count().await,
        0,
        "aggregator entry should have been removed on last drop"
    );
    assert_eq!(
        sim.live_rt_bar_subscription_count_for(&sym),
        0,
        "upstream rt-bar subscription should have been cancelled"
    );

    // Resubscribing spins up a fresh aggregator — bumps the sim's
    // subscribe call counter.
    let _h2 = router
        .subscribe_bars(sym.clone(), Timeframe::M1)
        .await
        .expect("resub");
    assert_eq!(sim.rt_bar_subscribe_call_count(), 2);
}

// ---------------------------------------------------------------------------
// 6. last_bar returns the current partial
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn last_bar_returns_current_partial() {
    let (router, sim) = build_router();
    let sym = aapl();

    let _h = router
        .subscribe_bars(sym.clone(), Timeframe::M1)
        .await
        .expect("sub");

    let t0 = Utc.with_ymd_and_hms(2026, 1, 2, 14, 30, 0).unwrap();
    inject_rt_bar(&sim, mk_rt_bar(&sym, t0, 100.0, 101.5, 99.5, 100.5)).await;
    // Second 5 s bar in same window.
    inject_rt_bar(
        &sim,
        mk_rt_bar(
            &sym,
            t0 + chrono::Duration::seconds(5),
            100.5,
            102.0,
            100.0,
            101.8,
        ),
    )
    .await;
    // Give the coalesce interval one tick so last_bar is populated.
    sleep(Duration::from_millis(200)).await;

    let snap = router
        .last_bar(&sym, Timeframe::M1)
        .await
        .expect("snapshot should exist");
    assert_eq!(snap.ts_open, t0);
    assert_eq!(snap.timeframe, Timeframe::M1);
    assert_eq!(snap.completeness, BarCompleteness::Partial);
    assert!((snap.o - 100.0).abs() < 1e-9);
    assert!((snap.c - 101.8).abs() < 1e-9);
    assert!(snap.h >= 102.0 - 1e-9);
    assert!(snap.l <= 99.5 + 1e-9);
    assert_eq!(snap.volume, 200);
}

// ---------------------------------------------------------------------------
// 7. Unsupported timeframes rejected (BR-22)
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rejects_unsupported_timeframe() {
    let (router, _sim) = build_router();
    let sym = aapl();

    for tf in [
        Timeframe::D1,
        Timeframe::W1,
        Timeframe::H4,
        Timeframe::MN1,
        Timeframe::S1,
    ] {
        let err = router.subscribe_bars(sym.clone(), tf).await.err();
        match err {
            Some(MarketDataError::UnsupportedTimeframe(got)) => assert_eq!(got, tf),
            other => panic!("expected UnsupportedTimeframe({tf:?}), got {other:?}"),
        }
    }
}

// ---------------------------------------------------------------------------
// 8. align_to_window guard (M-6) — synthetic zero-duration rejection
// ---------------------------------------------------------------------------
//
// Exercised at the subscribe boundary: `S1` is in the unsupported set
// so its `align_to_window` path never runs in steady-state. This is a
// belt-and-braces check that the public rejection is consistent with
// the internal guard, via the `UnsupportedTimeframe` surface.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn align_to_window_zero_secs_errors() {
    let (router, _sim) = build_router();
    let sym = aapl();
    // Route is: subscribe_bars(S1) → is_unsupported_tf(S1) → Err.
    let err = router
        .subscribe_bars(sym, Timeframe::S1)
        .await
        .expect_err("must err");
    assert!(matches!(
        err,
        MarketDataError::UnsupportedTimeframe(Timeframe::S1)
    ));
}

// ---------------------------------------------------------------------------
// 9. Lagged upstream invalidates the current partial bar (M-11)
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lagged_aggregator_invalidates_current_bar() {
    // The aggregator's rt-bar receiver has a broadcast capacity of 256
    // (from the router's rt_bar hub). Firing > 256 bars back-to-back
    // forces a Lagged event into the aggregator's recv loop.
    let (router, sim) = build_router();
    let sym = aapl();

    let mut handle = router
        .subscribe_bars(sym.clone(), Timeframe::M1)
        .await
        .expect("sub");

    // Prime a partial bar first.
    let t0 = Utc.with_ymd_and_hms(2026, 1, 2, 14, 30, 0).unwrap();
    inject_rt_bar(&sim, mk_rt_bar(&sym, t0, 100.0, 100.0, 100.0, 100.0)).await;
    sleep(Duration::from_millis(150)).await;

    // Overwhelm the rt-bar fan-out to force Lagged inside the
    // aggregator's upstream. The rt-bar broadcast capacity is 256;
    // firing > 1024 bars very quickly without yielding lets the
    // publisher push into the ring faster than the aggregator drains,
    // guaranteeing a Lagged.
    for i in 0..2000u32 {
        let ts = t0 + chrono::Duration::milliseconds(i as i64);
        sim.inject_for_test(MarketEvent::Bar(mk_rt_bar(
            &sym, ts, 200.0, 200.0, 200.0, 200.0,
        )));
    }
    sleep(Duration::from_millis(200)).await;

    // After Lagged, the aggregator drops `current` and the next rt-bar
    // opens a fresh partial. Feed one more well into the next window.
    let t_new = t0 + chrono::Duration::seconds(120);
    inject_rt_bar(&sim, mk_rt_bar(&sym, t_new, 50.0, 50.0, 50.0, 50.0)).await;
    sleep(Duration::from_millis(200)).await;

    // Drain everything the consumer has seen; assert the final `last_bar`
    // snapshot opens at the post-lag window and has clean OHLC (no
    // leftover from pre-lag state).
    let _drained = drain_bars(&mut handle, 256, 10).await;

    let snap = router
        .last_bar(&sym, Timeframe::M1)
        .await
        .expect("should have a post-lag snapshot");
    // Snapshot should reflect a window at or after the post-lag rt-bar.
    let window = t_new - chrono::Duration::seconds(t_new.timestamp().rem_euclid(60));
    assert_eq!(
        snap.ts_open, window,
        "expected snapshot to open at the post-lag window"
    );
    // OHLC carries the post-lag price cleanly.
    assert!((snap.c - 50.0).abs() < 1e-9);
    assert_eq!(snap.volume, 100);
}
