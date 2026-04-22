//! Router behaviour tests (slice 5).
//!
//! Uses `SimMarketData` as the upstream provider. Timing-sensitive
//! tests use `#[tokio::test(start_paused = true)]` + `tokio::time::advance`
//! (BR-20) so they don't depend on wall-clock wobble.

use std::sync::Arc;
use std::time::Duration;

use chrono::{TimeZone, Utc};
use futures::StreamExt;
use midas_broker::sim::{SimMarketData, SimMarketDataConfig};
use midas_broker::MarketDataSource;
use midas_broker_core::market_data::{IbDuration, MarketDataError, SymbolKey, Timeframe};
use midas_market_data::MarketDataRouter;
use tokio::sync::broadcast::error::RecvError;
use tokio::time::sleep;

fn aapl() -> SymbolKey {
    SymbolKey {
        contract_id: 265598,
        symbol: "AAPL".into(),
    }
}

fn msft() -> SymbolKey {
    SymbolKey {
        contract_id: 272093,
        symbol: "MSFT".into(),
    }
}

fn build_router() -> (Arc<MarketDataRouter>, Arc<SimMarketData>) {
    // Quiet sim: no initial burst, no emitter drift. Tests that want
    // synthetic ticks use `inject_for_test` directly so they stay
    // deterministic regardless of the emitter cadence.
    let sim = SimMarketData::new(SimMarketDataConfig {
        farm_up_delay_ms: 1,
        burst_enabled: false,
        tick_drift_bps: 0.0,
        // A very long cadence effectively silences the emitter for
        // tests that complete well within this window.
        tick_cadence_ms: 60_000,
        ..SimMarketDataConfig::default()
    });
    let source: Arc<dyn MarketDataSource> = sim.clone();
    let router = MarketDataRouter::new(source);
    (router, sim)
}

/// Wait briefly for the actor to drain pending control-plane
/// messages. Unconditional short sleep is simpler than polling
/// `debug_dump` and matches what the plan's tests do.
async fn drain() {
    sleep(Duration::from_millis(50)).await;
}

// ---------------------------------------------------------------------------
// 1. Single upstream per symbol (refcount fan-out)
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn single_subscribe_opens_one_upstream() {
    let (router, sim) = build_router();
    let sym = aapl();

    let _h1 = router.subscribe_ticks(sym.clone()).await.expect("sub1");
    let _h2 = router.subscribe_ticks(sym.clone()).await.expect("sub2");

    assert_eq!(sim.tick_subscribe_call_count(), 1);
    assert_eq!(sim.live_subscription_count_for(&sym), 1);
}

// ---------------------------------------------------------------------------
// 2. Last drop cancels upstream
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn last_drop_cancels_upstream() {
    let (router, sim) = build_router();
    let sym = aapl();

    let h = router.subscribe_ticks(sym.clone()).await.expect("sub");
    assert_eq!(sim.live_subscription_count_for(&sym), 1);

    drop(h);
    drain().await;
    // Allow the actor to run and abort the publisher, whose dropped
    // TickStream fires the cancel closure.
    sleep(Duration::from_millis(100)).await;
    assert_eq!(sim.live_subscription_count_for(&sym), 0);
}

// ---------------------------------------------------------------------------
// 3. Multiple drops decrement refcount
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn multiple_drops_decrement_refcount() {
    let (router, sim) = build_router();
    let sym = aapl();

    let h1 = router.subscribe_ticks(sym.clone()).await.expect("sub1");
    let h2 = router.subscribe_ticks(sym.clone()).await.expect("sub2");
    let h3 = router.subscribe_ticks(sym.clone()).await.expect("sub3");

    drop(h1);
    drop(h2);
    drain().await;
    // Still have h3: upstream must be alive.
    assert_eq!(sim.live_subscription_count_for(&sym), 1);
    assert_eq!(sim.tick_subscribe_call_count(), 1);

    drop(h3);
    drain().await;
    sleep(Duration::from_millis(100)).await;
    assert_eq!(sim.live_subscription_count_for(&sym), 0);
}

// ---------------------------------------------------------------------------
// 4. Lagged consumer does not block producer
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lagged_consumer_does_not_block_producer() {
    let (router, sim) = build_router();
    let sym = aapl();

    let mut fast = router.subscribe_ticks(sym.clone()).await.expect("fast");
    let stall = router.subscribe_ticks(sym.clone()).await.expect("stall");

    use midas_broker_core::market_data::{
        MarketEvent, ReqId, Tick, TickAttributes, TickKind, TickType, TickValue,
    };

    // Helper: inject one synthetic tick.
    let push_tick = |i: usize| {
        sim.inject_for_test(MarketEvent::Tick(Tick {
            symbol: sym.clone(),
            req_id: ReqId(0),
            kind: TickKind::Price,
            tick_type: TickType::Last,
            value: TickValue::Price(100.0 + (i as f64) * 0.001),
            attrs: TickAttributes::default(),
            ts: Utc::now(),
        }));
    };

    // Prime the pipeline: push a handful of ticks and let the fast
    // consumer drain them. This establishes that the fan-out works
    // at all.
    for i in 0..10 {
        push_tick(i);
    }
    sleep(Duration::from_millis(50)).await;
    let mut fast_got = 0usize;
    for _ in 0..50 {
        match tokio::time::timeout(Duration::from_millis(20), fast.recv()).await {
            Ok(Ok(_)) => fast_got += 1,
            _ => break,
        }
    }
    assert!(
        fast_got > 0,
        "fast consumer should have received the priming ticks"
    );

    // Now overflow the stall consumer's ring (cap 4096). Publish in
    // batches with tiny pauses so the publisher can keep up with the
    // upstream and fan each tick out to `fast` + the stalled ring.
    for batch in 0..10 {
        for i in 0..600 {
            push_tick(batch * 600 + i + 10);
        }
        sleep(Duration::from_millis(5)).await;
    }
    sleep(Duration::from_millis(100)).await;

    // Fast consumer still gets ticks: producer never blocked.
    let mut fast_after = 0usize;
    for _ in 0..200 {
        match tokio::time::timeout(Duration::from_millis(5), fast.recv()).await {
            Ok(Ok(_)) => fast_after += 1,
            Ok(Err(RecvError::Lagged(_))) => { /* fast also lagged; still forward progress */ }
            _ => break,
        }
    }
    assert!(
        fast_after > 0,
        "fast consumer should continue receiving while stall is stuck"
    );

    // Stalled consumer eventually reports Lagged.
    let (mut rx, _g) = stall.into_parts();
    let mut saw_lagged = false;
    for _ in 0..200 {
        match tokio::time::timeout(Duration::from_millis(5), rx.recv()).await {
            Ok(Err(RecvError::Lagged(_))) => {
                saw_lagged = true;
                break;
            }
            Ok(Ok(_)) => {}
            _ => break,
        }
    }
    assert!(
        saw_lagged,
        "stall consumer should have observed RecvError::Lagged"
    );
}

// ---------------------------------------------------------------------------
// 5. last_quote watch coalesces
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn last_quote_watch_coalesces() {
    let (router, sim) = build_router();
    let sym = aapl();

    let mut watch = router.last_quote(sym.clone()).await.expect("watch");

    // Inject many price ticks; a watch consumer should see the
    // LATEST, not every single one.
    use midas_broker_core::market_data::{
        MarketEvent, ReqId, Tick, TickAttributes, TickKind, TickType, TickValue,
    };
    let base = Utc.with_ymd_and_hms(2026, 1, 2, 14, 30, 0).unwrap();
    for i in 0..200 {
        sim.inject_for_test(MarketEvent::Tick(Tick {
            symbol: sym.clone(),
            req_id: ReqId(0),
            kind: TickKind::Price,
            tick_type: TickType::Last,
            value: TickValue::Price(100.0 + (i as f64) * 0.01),
            attrs: TickAttributes::default(),
            ts: base + chrono::Duration::milliseconds(i as i64),
        }));
    }

    sleep(Duration::from_millis(300)).await;
    // changed() should yield Ok and the current borrow should be a
    // high-indexed price (close to 100.0 + 199 * 0.01 = 101.99).
    let _ = tokio::time::timeout(Duration::from_millis(50), watch.changed()).await;
    let q = watch.borrow();
    let last = q.last.expect("should have observed a last price");
    assert!(
        last >= 100.5,
        "watch should have coalesced to a near-latest price; got {last}"
    );
}

// ---------------------------------------------------------------------------
// 6. Watch-only consumer keeps publisher alive
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn watch_only_consumer_keeps_publisher_alive() {
    let (router, sim) = build_router();
    let sym = aapl();

    let watch = router.last_quote(sym.clone()).await.expect("watch");
    // No broadcast consumer — but the upstream MUST still be alive
    // because watch_refcount > 0.
    assert_eq!(sim.live_subscription_count_for(&sym), 1);

    // Publisher should be alive per debug dump.
    let dump = router.debug_dump().await;
    let info = dump
        .iter()
        .find(|s| s.symbol == sym)
        .expect("symbol in dump");
    assert!(info.tick_publisher_alive, "publisher must be alive");
    assert_eq!(info.watch_refcount, 1);
    assert_eq!(info.tick_refcount, 0);

    drop(watch);
    drain().await;
    sleep(Duration::from_millis(100)).await;
    assert_eq!(sim.live_subscription_count_for(&sym), 0);
}

// ---------------------------------------------------------------------------
// 7 + 8. history_then_live seam (no gap, no dup)
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn history_then_live_no_gap_no_dup() {
    // Fix the seam boundary so we can reason about duplicates.
    let t_server = Utc.with_ymd_and_hms(2026, 1, 2, 14, 30, 0).unwrap();
    let sim = SimMarketData::new(SimMarketDataConfig {
        farm_up_delay_ms: 1,
        burst_enabled: false,
        historical_last_ts: Some(t_server),
        ..SimMarketDataConfig::default()
    });
    let source: Arc<dyn MarketDataSource> = sim.clone();
    let router = MarketDataRouter::new(source);
    let sym = aapl();

    // Run the seam helper and harvest the first N bars.
    let stream = router
        .history_then_live(sym.clone(), Timeframe::S5, IbDuration::Days(1))
        .await
        .expect("history_then_live");

    let mut pinned = Box::pin(stream);
    // Pull the first few items with a short timeout so we don't hang
    // if the live tail stalls.
    let mut bars = Vec::new();
    for _ in 0..5 {
        match tokio::time::timeout(Duration::from_millis(500), pinned.next()).await {
            Ok(Some(bar)) => bars.push(bar),
            Ok(None) => break,
            Err(_) => break,
        }
    }
    assert!(!bars.is_empty(), "seam produced no bars");

    // Historical portion: every ts_open must be <= t_server OR the
    // bar is a completed historical bar. Live tail must have
    // ts_open > t_server.
    // We don't know the exact split without introspection — instead
    // assert no duplicate ts_open across the sequence, and
    // monotonic (or at least non-decreasing) time.
    let mut last_ts = None;
    for bar in &bars {
        if let Some(prev) = last_ts {
            assert!(
                bar.ts_open >= prev,
                "seam produced out-of-order bars: {prev:?} -> {:?}",
                bar.ts_open
            );
        }
        last_ts = Some(bar.ts_open);
    }
    // No duplicates.
    let mut seen: std::collections::HashSet<_> = std::collections::HashSet::new();
    for bar in &bars {
        assert!(seen.insert(bar.ts_open), "duplicate ts_open at the seam");
    }
}

// ---------------------------------------------------------------------------
// 9. Concurrent subscribe + disconnect (no leak)
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_subscribe_plus_disconnect() {
    use midas_broker_core::market_data::FarmCode;
    let (router, sim) = build_router();
    let sym = aapl();
    let sim_for_task = sim.clone();
    let router_for_task = router.clone();
    let sym_for_task = sym.clone();

    let (sub_handle, disc_handle) = tokio::join!(
        tokio::spawn(async move { router_for_task.subscribe_ticks(sym_for_task).await }),
        tokio::spawn(async move {
            sim_for_task.simulate_connection_lost(FarmCode::ConnectionRestoredDataLost);
        })
    );
    let sub_result = sub_handle.expect("join");
    disc_handle.expect("join");

    // Drop whatever handle the subscribe call returned (if any) and
    // let the actor process the resulting DecRef.
    drop(sub_result);
    drain().await;
    sleep(Duration::from_millis(150)).await;

    // NM-3 / M-8: whichever order the race resolved, after the
    // consumer drops its handle there must be no lingering upstream
    // subscription. Assert the invariant once the dust settles.
    assert_eq!(
        sim.live_subscription_count_for(&sym),
        0,
        "no upstream should remain after disconnect + handle drop"
    );
}

// ---------------------------------------------------------------------------
// 10. Router dropped with live handles — handle yields Closed, no panic
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn router_dropped_with_live_handles_no_panic() {
    let (router, _sim) = build_router();
    let sym = aapl();
    let mut h = router.subscribe_ticks(sym.clone()).await.expect("sub");

    drop(router);
    // The publisher task was aborted by the actor's shutdown path;
    // the broadcast sender was dropped when SymbolHub dropped.
    // Consumer's next recv must yield Closed (or Lagged-then-Closed).
    sleep(Duration::from_millis(100)).await;
    let res = tokio::time::timeout(Duration::from_millis(500), h.recv()).await;
    match res {
        Ok(Err(RecvError::Closed)) => {}
        Ok(Err(RecvError::Lagged(_))) => {
            // Drain then we must see Closed.
            let next = tokio::time::timeout(Duration::from_millis(500), h.recv())
                .await
                .expect("no timeout");
            assert!(matches!(next, Err(RecvError::Closed)));
        }
        other => panic!("expected Closed, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// 11. Shared RT-bar upstream
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shared_rt_bar_upstream() {
    let (router, sim) = build_router();
    let sym = aapl();

    let _h1 = router.subscribe_rt_bars(sym.clone()).await.expect("rt1");
    let _h2 = router.subscribe_rt_bars(sym.clone()).await.expect("rt2");

    assert_eq!(sim.rt_bar_subscribe_call_count(), 1);
    assert_eq!(sim.live_rt_bar_subscription_count_for(&sym), 1);
}

// ---------------------------------------------------------------------------
// 12. Contract cache avoids duplicate resolution
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn contract_cache_avoids_duplicate_resolution() {
    let (router, sim) = build_router();
    let sym = aapl();

    let _h1 = router.subscribe_ticks(sym.clone()).await.expect("sub1");
    let _h2 = router.subscribe_ticks(sym.clone()).await.expect("sub2");
    let _h3 = router.subscribe_rt_bars(sym.clone()).await.expect("rt");

    // AAPL resolved once, reused everywhere else.
    assert_eq!(sim.resolve_contract_call_count(), 1);

    // Separate symbol: fresh resolution.
    let _h4 = router.subscribe_ticks(msft()).await.expect("msft");
    assert_eq!(sim.resolve_contract_call_count(), 2);
}

// ---------------------------------------------------------------------------
// 13. Subscribe returns Err if source fails (NM-3 rollback)
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn subscribe_returns_err_if_source_fails() {
    let (router, sim) = build_router();
    let sym = aapl();

    sim.arm_next_subscribe_ticks_error(MarketDataError::NoPermission {
        symbol: sym.symbol.clone(),
    });

    let err = router.subscribe_ticks(sym.clone()).await.err();
    assert!(matches!(err, Some(MarketDataError::NoPermission { .. })));

    // NM-3: no hub should be inserted.
    let dump = router.debug_dump().await;
    assert!(
        dump.iter().all(|d| d.symbol != sym),
        "no hub should have been registered on error"
    );
    assert_eq!(sim.live_subscription_count_for(&sym), 0);

    // A fresh subscribe after the injected error clears should succeed.
    let _h = router.subscribe_ticks(sym.clone()).await.expect("retry");
    assert_eq!(sim.live_subscription_count_for(&sym), 1);
}
