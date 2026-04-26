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
use midas_broker_core::market_data::{
    EndReason, IbDuration, MarketDataError, SymbolKey, Timeframe,
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
// Bug 6: update_last_quote only sends on actual price change
// ---------------------------------------------------------------------------
//
// The publisher previously called `update_last_quote` for every
// Size/LastSize tick AND refreshed `next.ts = tick.ts`
// unconditionally, guaranteeing `next != current` for every tick and
// waking all watch consumers on every update — even size-only ticks
// that carried no price information. Fix: refresh `ts` only inside
// the price-mutating match arms, and skip the `update_last_quote`
// call entirely on `Size` ticks.

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn size_only_ticks_do_not_wake_watch_consumers() {
    use midas_broker_core::market_data::{
        MarketEvent, ReqId, Tick, TickAttributes, TickKind, TickType, TickValue,
    };

    let (router, sim) = build_router();
    let sym = aapl();

    let mut watch = router.last_quote(sym.clone()).await.expect("watch");

    // Seed a baseline `last` price so the next identical price tick
    // won't be the first-write-wakeup.
    let base_ts = Utc.with_ymd_and_hms(2026, 1, 2, 14, 30, 0).unwrap();
    sim.inject_for_test(MarketEvent::Tick(Tick {
        symbol: sym.clone(),
        req_id: ReqId(0),
        kind: TickKind::Price,
        tick_type: TickType::Last,
        value: TickValue::Price(100.50),
        attrs: TickAttributes::default(),
        ts: base_ts,
    }));
    // Drain the first wakeup.
    let _ = tokio::time::timeout(Duration::from_millis(200), watch.changed()).await;
    assert_eq!(watch.borrow().last, Some(100.50));

    // Now emit a storm of size-only ticks with advancing
    // timestamps. NONE of them should wake the watch consumer —
    // `last` (and bid/ask) never change.
    for i in 1..=50 {
        sim.inject_for_test(MarketEvent::Tick(Tick {
            symbol: sym.clone(),
            req_id: ReqId(0),
            kind: TickKind::Size,
            tick_type: TickType::LastSize,
            value: TickValue::Size(100 + i as i64),
            attrs: TickAttributes::default(),
            ts: base_ts + chrono::Duration::milliseconds(i as i64 * 10),
        }));
    }
    sleep(Duration::from_millis(100)).await;

    // No change should have landed on the watch.
    assert!(
        !watch.has_changed().unwrap_or(true),
        "size-only ticks must not wake the watch"
    );

    // And a real price change DOES still wake the consumer — this
    // protects against the opposite regression of silencing all
    // updates.
    sim.inject_for_test(MarketEvent::Tick(Tick {
        symbol: sym.clone(),
        req_id: ReqId(0),
        kind: TickKind::Price,
        tick_type: TickType::Last,
        value: TickValue::Price(101.25),
        attrs: TickAttributes::default(),
        ts: base_ts + chrono::Duration::seconds(1),
    }));
    let changed = tokio::time::timeout(Duration::from_millis(500), watch.changed()).await;
    assert!(
        changed.is_ok(),
        "a genuine price change must still fire the watch"
    );
    assert_eq!(watch.borrow().last, Some(101.25));
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

// ---------------------------------------------------------------------------
// S8 §E: iced subscription teardown propagates the DecRef chain.
// ---------------------------------------------------------------------------

/// Simulates the iced-side teardown flow end-to-end in an in-process
/// harness:
///
/// 1. A spawned task subscribes via the router (like the chart
///    subscription stream builder does).
/// 2. The outer task is dropped (like iced dropping the `Subscription`
///    on a re-diff when the chart is gone).
/// 3. The `SubscriptionHandle` drops, the guard runs, the router sees
///    `DecRef`, and upstream gets cancelled.
///
/// Asserts that within 250 ms of the outer-task drop, the router's
/// `debug_dump()` reports zero consumers for the symbol — proving the
/// RAII chain works when the stream closure is dropped mid-recv.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn iced_subscription_teardown_propagates_dec_ref() {
    let (router, sim) = build_router();
    let sym = aapl();

    // Spawn the "iced-side" task: holds the handle, recvs forever.
    // When the JoinHandle is dropped the future is aborted and the
    // handle's Drop runs, sending `DecRef` to the router.
    let router_cl = router.clone();
    let sym_cl = sym.clone();
    let task = tokio::spawn(async move {
        let handle = router_cl
            .subscribe_ticks(sym_cl)
            .await
            .expect("subscribe in iced-like task");
        let mut rx = handle.resubscribe();
        // Park forever — the only way out is an abort from the
        // outer test scope, which is what iced teardown does.
        loop {
            let _ = rx.recv().await;
        }
    });

    // Let the subscribe land.
    sleep(Duration::from_millis(50)).await;
    assert_eq!(
        sim.live_subscription_count_for(&sym),
        1,
        "upstream should be subscribed while the task holds the handle"
    );
    let dump = router.debug_dump().await;
    let pre = dump.iter().find(|d| d.symbol == sym).expect("hub present");
    assert_eq!(pre.tick_refcount, 1, "one tick consumer via iced-like task");

    // Simulate iced dropping the subscription: abort the task.
    task.abort();
    let _ = task.await;

    // Poll debug_dump until the refcount is gone (or timeout).
    let deadline = tokio::time::Instant::now() + Duration::from_millis(250);
    loop {
        let dump = router.debug_dump().await;
        let still_live = dump.iter().any(|d| d.symbol == sym && d.tick_refcount > 0);
        if !still_live {
            break;
        }
        if tokio::time::Instant::now() >= deadline {
            panic!("teardown did not propagate DecRef within 250 ms: {dump:?}");
        }
        sleep(Duration::from_millis(20)).await;
    }
    // Upstream should also be cancelled.
    let deadline = tokio::time::Instant::now() + Duration::from_millis(250);
    while sim.live_subscription_count_for(&sym) != 0 {
        if tokio::time::Instant::now() >= deadline {
            panic!(
                "upstream sub count != 0 after teardown: {}",
                sim.live_subscription_count_for(&sym)
            );
        }
        sleep(Duration::from_millis(20)).await;
    }
}

// ---------------------------------------------------------------------------
// Bugs 3 + 4: Refcount leak on ensure_tick_publisher failure
// ---------------------------------------------------------------------------
//
// Regression coverage for the existing-hub paths of
// `handle_subscribe_ticks` and `handle_open_hub_for_watch`. If
// `ensure_tick_publisher.await?` returns Err AFTER the IncRef has
// landed, no `SubscriptionHandle` reaches the caller, so no
// `Tick/WatchSubGuard` exists to DecRef on drop — the refcount is
// orphaned. The fix moves the `fetch_add(1)` below the fallible
// `.await?`, matching the pattern already used correctly in
// `handle_subscribe_rt_bars`.

/// Open a hub via `subscribe_rt_bars` (no tick publisher spawned),
/// then call `subscribe_ticks` with the sim armed to fail. The
/// existing-hub branch runs `ensure_tick_publisher` → upstream fails
/// → `?` returns Err. The hub's `tick_refcount` must remain 0, not
/// leak to 1.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn subscribe_ticks_existing_hub_path_does_not_leak_refcount_on_upstream_error() {
    let (router, sim) = build_router();
    let sym = aapl();

    // Prime a hub with ONLY an rt-bar publisher.
    let _rt = router
        .subscribe_rt_bars(sym.clone())
        .await
        .expect("rt-bar subscribe");

    let dump = router.debug_dump().await;
    let pre = dump
        .iter()
        .find(|d| d.symbol == sym)
        .expect("rt-bar subscribe should register a hub");
    assert_eq!(pre.tick_refcount, 0);

    // Arm the next `source.subscribe_ticks` to fail — this is what
    // `ensure_tick_publisher` calls internally when it has to spawn
    // a tick publisher on a hub that doesn't have one yet.
    sim.arm_next_subscribe_ticks_error(MarketDataError::NoPermission {
        symbol: sym.symbol.clone(),
    });

    // The tick-subscribe call should return Err. Without the fix,
    // `tick_refcount` was bumped to 1 before the `?` unwind, and
    // stays there forever.
    let err = router.subscribe_ticks(sym.clone()).await.err();
    assert!(matches!(err, Some(MarketDataError::NoPermission { .. })));

    drain().await;

    let dump = router.debug_dump().await;
    let post = dump
        .iter()
        .find(|d| d.symbol == sym)
        .expect("hub still present (rt-bar handle alive)");
    assert_eq!(
        post.tick_refcount, 0,
        "tick_refcount must not leak on existing-hub upstream failure"
    );
    assert_eq!(
        post.rt_bar_refcount, 1,
        "rt-bar refcount must survive the failed tick subscribe"
    );
}

// ---------------------------------------------------------------------------
// Bug 5: GuardedStream closes on Lagged instead of silently skipping
// ---------------------------------------------------------------------------

/// `SubscriptionHandle::into_stream` used to `continue` on
/// `RecvError::Lagged`, which silently skipped dropped items and hid
/// a gap from consumers (history_then_live in particular stitched a
/// hole at the seam on a long history fetch). The fix closes the
/// stream on Lagged so the caller has to re-open — an explicit
/// signal instead of an invisible one.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn guarded_stream_closes_on_lagged_instead_of_skipping() {
    use futures::StreamExt;
    use midas_broker_core::market_data::{Bar as CoreBar, BarCompleteness, MarketEvent};

    let (router, sim) = build_router();
    let sym = aapl();

    let handle = router
        .subscribe_rt_bars(sym.clone())
        .await
        .expect("subscribe_rt_bars");
    let mut stream = handle.into_stream();

    // Flood more bars than the RT-bar broadcast ring holds (cap 256)
    // WITHOUT polling the stream. Next poll observes Lagged.
    let base = Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap();
    for i in 0i64..400 {
        let ts_open = base + chrono::Duration::seconds(i * 5);
        sim.inject_for_test(MarketEvent::Bar(CoreBar {
            symbol: sym.clone(),
            timeframe: Timeframe::S5,
            ts_open,
            ts_close: ts_open + chrono::Duration::seconds(5),
            o: 100.0,
            h: 101.0,
            l: 99.0,
            c: 100.5,
            volume: 1000,
            trade_count: 42,
            wap: Some(100.25),
            completeness: BarCompleteness::Completed,
        }));
    }
    // Let the publisher pump the ring full.
    sleep(Duration::from_millis(100)).await;

    // First poll: the BroadcastStream surfaces Lagged as the
    // first error item. Our GuardedStream turns that into a
    // clean `None`.
    let first = tokio::time::timeout(Duration::from_millis(500), stream.next())
        .await
        .expect("stream.next() didn't respond");
    assert!(
        first.is_none(),
        "GuardedStream must close on Lagged, not silently skip (got: {first:?})"
    );

    // Sticky-close: subsequent polls keep yielding None.
    let second = tokio::time::timeout(Duration::from_millis(50), stream.next())
        .await
        .expect("stream.next() didn't respond");
    assert!(second.is_none(), "GuardedStream stays closed after Lagged");
}

/// Same shape for `handle_open_hub_for_watch`: prime an rt-bar-only
/// hub, arm a subscribe_ticks error, open a watch, expect Err with
/// `watch_refcount` staying at 0.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn open_hub_for_watch_existing_hub_path_does_not_leak_refcount_on_upstream_error() {
    let (router, sim) = build_router();
    let sym = aapl();

    let _rt = router
        .subscribe_rt_bars(sym.clone())
        .await
        .expect("rt-bar subscribe");

    sim.arm_next_subscribe_ticks_error(MarketDataError::NoPermission {
        symbol: sym.symbol.clone(),
    });

    let err = router.last_quote(sym.clone()).await.err();
    assert!(matches!(err, Some(MarketDataError::NoPermission { .. })));

    drain().await;

    let dump = router.debug_dump().await;
    let post = dump
        .iter()
        .find(|d| d.symbol == sym)
        .expect("hub still present (rt-bar handle alive)");
    assert_eq!(
        post.watch_refcount, 0,
        "watch_refcount must not leak on existing-hub upstream failure"
    );
    assert_eq!(
        post.rt_bar_refcount, 1,
        "rt-bar refcount must survive the failed watch open"
    );
}

// ---------------------------------------------------------------------------
// P0 wedge guard: a hung upstream must not wedge the control actor.
// ---------------------------------------------------------------------------

/// Arm the sim so `subscribe_ticks` parks on a `Notify`. The router's
/// per-handler `ROUTER_ACTOR_OP_TIMEOUT` (10s) must close the handler
/// out with an `Err(..)` so the actor can drain the next control
/// message. Without the timeout the actor would block on the mpsc
/// forever and every subsequent call would pile up.
///
/// Uses `tokio::time::pause` + `advance` so the test doesn't actually
/// wait ten wall-clock seconds. A multi-thread flavour is required so
/// the parked sim task and the advancing driver task make forward
/// progress independently.
#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn hung_upstream_subscribe_ticks_is_broken_by_actor_timeout() {
    let (router, sim) = build_router();
    let sym = aapl();

    // Arm the hang — the first subscribe_ticks will park on the
    // returned Notify.
    let hang = sim.arm_next_subscribe_ticks_hang();

    // Kick off the subscribe in a background task so we can advance
    // the virtual clock past the deadline.
    let router_cl = router.clone();
    let sym_cl = sym.clone();
    let subscribe = tokio::spawn(async move { router_cl.subscribe_ticks(sym_cl).await });

    // Advance past the actor's 10s budget. The handler should close
    // out with `Err(Other("... timed out ..."))`.
    tokio::time::advance(Duration::from_secs(11)).await;

    let result = subscribe
        .await
        .expect("subscribe task didn't finish after deadline");
    let err = result.expect_err("should have timed out");
    let msg = format!("{err}");
    assert!(
        msg.contains("timed out"),
        "expected timeout error, got: {msg}"
    );

    // Release the stuck sim task so it doesn't leak into the runtime
    // teardown.
    hang.notify_one();

    // Follow-up subscribe should work (the actor is still alive).
    // Drain a quick tick so the previously-hung task finishes before
    // the next subscribe overtakes it.
    tokio::time::advance(Duration::from_millis(10)).await;
    let _h = router
        .subscribe_ticks(sym.clone())
        .await
        .expect("actor is not wedged; second subscribe should succeed");
}

// ---------------------------------------------------------------------------
// Backlog counter — sanity: healthy traffic drains to zero.
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn control_backlog_drains_under_healthy_traffic() {
    let (router, _sim) = build_router();
    let sym = aapl();
    // Subscribe + drop a few times.
    for _ in 0..10 {
        let h = router.subscribe_ticks(sym.clone()).await.expect("sub");
        drop(h);
    }
    drain().await;
    assert_eq!(
        router.control_backlog(),
        0,
        "backlog must drain to zero under healthy traffic"
    );
}

// ---------------------------------------------------------------------------
// Slice B2 — router disconnect policy: full upstream close emits
// `EndReason::Disconnected` on the per-hub end-reason watch, then the
// hub is dropped from `state.per_symbol` and a follow-up subscribe
// re-spawns a fresh hub.
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn upstream_disconnect_emits_end_reason_then_closes() {
    use midas_broker_core::market_data::FarmCode;

    let (router, sim) = build_router();
    let sym = aapl();

    // Two consumers on the same symbol — the disconnect policy must
    // fan out the end-reason to both.
    let h1 = router.subscribe_ticks(sym.clone()).await.expect("sub1");
    let h2 = router.subscribe_ticks(sym.clone()).await.expect("sub2");

    // Snapshot the end-reason watch BEFORE the disconnect — both
    // receivers should observe `Some(Disconnected)` after the actor
    // processes the publisher's `UpstreamClosed`.
    let mut end1 = h1.end_reason();
    let mut end2 = h2.end_reason();
    assert_eq!(*end1.borrow(), None, "watch starts at None pre-disconnect");
    assert_eq!(*end2.borrow(), None, "watch starts at None pre-disconnect");

    // Sanity: hub is in the map before the disconnect.
    let pre = router.debug_dump().await;
    assert!(
        pre.iter().any(|s| s.symbol == sym),
        "hub must be present pre-disconnect: {pre:?}"
    );

    // Drop the upstream stream by simulating a full connection loss.
    // `simulate_connection_lost(ConnectionLost)` synchronously closes
    // every active subscription's broadcast sender; the router's tick
    // publisher observes `RecvError::Closed` and notifies the actor
    // via `UpstreamClosed`.
    sim.simulate_connection_lost(FarmCode::ConnectionLost);

    // Let the publisher task observe the close and the actor process
    // the `UpstreamClosed` message. Under `start_paused = true` the
    // tasks still run; we only need to advance once their work has
    // hit a `tokio::time::*` boundary (none in this path), but a tiny
    // advance lets the queued tasks make progress before we poll.
    tokio::time::advance(Duration::from_millis(50)).await;

    // Each consumer's end-reason watch must flip to
    // `Some(EndReason::Disconnected)`. `changed()` returns once the
    // value moves off the initial `None`.
    let r1 = tokio::time::timeout(Duration::from_secs(1), end1.changed()).await;
    assert!(
        r1.is_ok() && r1.unwrap().is_ok(),
        "consumer 1 should observe a watch change for the disconnect"
    );
    assert_eq!(
        *end1.borrow(),
        Some(EndReason::Disconnected),
        "consumer 1 must see EndReason::Disconnected"
    );

    let r2 = tokio::time::timeout(Duration::from_secs(1), end2.changed()).await;
    assert!(
        r2.is_ok() && r2.unwrap().is_ok(),
        "consumer 2 should observe a watch change for the disconnect"
    );
    assert_eq!(
        *end2.borrow(),
        Some(EndReason::Disconnected),
        "consumer 2 must see EndReason::Disconnected"
    );

    // Subsequent broadcast `recv` on each handle must yield `Closed`
    // (the hub Arc has dropped, taking its broadcast senders with it).
    let mut h1 = h1;
    let mut h2 = h2;
    let recv1 = tokio::time::timeout(Duration::from_secs(1), h1.recv()).await;
    assert!(
        matches!(recv1, Ok(Err(RecvError::Closed))),
        "consumer 1 broadcast must close after disconnect; got {recv1:?}"
    );
    let recv2 = tokio::time::timeout(Duration::from_secs(1), h2.recv()).await;
    assert!(
        matches!(recv2, Ok(Err(RecvError::Closed))),
        "consumer 2 broadcast must close after disconnect; got {recv2:?}"
    );

    // `state.per_symbol` no longer contains the hub. The public
    // `debug_dump()` is the in-process projection of that map.
    let post = router.debug_dump().await;
    assert!(
        post.iter().all(|s| s.symbol != sym),
        "per_symbol must drop the hub after upstream close: {post:?}"
    );

    // Drop the now-defunct handles so any DecRef draining is balanced.
    drop(h1);
    drop(h2);
    tokio::time::advance(Duration::from_millis(50)).await;

    // Follow-up subscribe re-spawns a fresh hub. The sim's emitter is
    // silenced (`tick_cadence_ms: 60_000`) but the upstream call count
    // increments — proving the router went through the first-subscribe
    // path again rather than reusing a stale entry.
    let pre_calls = sim.tick_subscribe_call_count();
    let _h3 = router.subscribe_ticks(sym.clone()).await.expect("re-sub");
    tokio::time::advance(Duration::from_millis(20)).await;

    let dump = router.debug_dump().await;
    assert!(
        dump.iter().any(|s| s.symbol == sym),
        "re-subscribe must spawn a fresh hub: {dump:?}"
    );
    assert_eq!(
        sim.tick_subscribe_call_count(),
        pre_calls + 1,
        "re-subscribe must trigger a fresh upstream subscribe_ticks call"
    );
}

// ---------------------------------------------------------------------------
// 19. ETH-shading S4-router: `use_rth` propagation
// ---------------------------------------------------------------------------
//
// `MarketDataRouter::historical_bars` exposes a `use_rth: bool`
// parameter that maps to IB's `useRTH` flag (RTH-only vs.
// pre/post-market included). The router must forward whatever the
// caller passed to the underlying `MarketDataSource::historical_bars`
// call unaltered — the desktop chart's ETH knob only works if the
// value reaches IB.
//
// Sim ignores `use_rth` (S4-sim wires real ETH bar emission gated on
// it) but records the value via `last_historical_use_rth()` for
// observation here.

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn historical_bars_use_rth_true_propagates() {
    let (router, sim) = build_router();
    assert_eq!(sim.last_historical_use_rth(), None, "no calls yet");

    let _ = router
        .historical_bars(aapl(), Timeframe::D1, IbDuration::Days(5), true)
        .await
        .expect("historical_bars(use_rth = true)");

    assert_eq!(
        sim.last_historical_use_rth(),
        Some(true),
        "router must forward use_rth = true to the upstream source"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn historical_bars_use_rth_false_propagates() {
    let (router, sim) = build_router();

    let _ = router
        .historical_bars(aapl(), Timeframe::D1, IbDuration::Days(5), false)
        .await
        .expect("historical_bars(use_rth = false)");

    assert_eq!(
        sim.last_historical_use_rth(),
        Some(false),
        "router must forward use_rth = false (pre/post-market included) \
         to the upstream source"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn historical_bars_use_rth_flips_per_call() {
    let (router, sim) = build_router();

    let _ = router
        .historical_bars(aapl(), Timeframe::D1, IbDuration::Days(5), true)
        .await
        .expect("first call");
    assert_eq!(sim.last_historical_use_rth(), Some(true));

    let _ = router
        .historical_bars(msft(), Timeframe::D1, IbDuration::Days(5), false)
        .await
        .expect("second call with flipped flag");
    assert_eq!(
        sim.last_historical_use_rth(),
        Some(false),
        "second call must overwrite the first — no stale state"
    );
}
