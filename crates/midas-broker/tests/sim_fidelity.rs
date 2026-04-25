//! Slice 3 fidelity tests for the sim backend.
//!
//! Every timing-sensitive test uses `#[tokio::test(start_paused = true)]`
//! plus `tokio::time::advance` (BR-20). Wall-clock `sleep` would race
//! with CI's variable timing; paused time is deterministic.

use std::time::Duration;

use midas_broker::sim::{SimMarketData, SimMarketDataConfig, SimOrderClient, SimOrderConfig};
use midas_broker::stream::HistoricalStreamEvent;
use midas_broker::OrderAction;
use midas_broker::{MarketDataSource, OrderClient, OrderEvent};
use midas_broker::{OrderSpec, OrderType, Tif};
use midas_broker_core::market_data::{
    ConnectionState, FarmCode, GenericTicks, IbDuration, SymbolKey, TickKind, TickType, Timeframe,
    WhatToShow,
};

fn aapl() -> SymbolKey {
    SymbolKey {
        contract_id: 265_598,
        symbol: "AAPL".to_string(),
    }
}

fn fast_config() -> SimMarketDataConfig {
    SimMarketDataConfig {
        // Keep cadences small so tests run quickly under paused time.
        tick_cadence_ms: 250,
        farm_up_delay_ms: 100,
        burst_delay_ms: 50,
        late_tick_window_ms: 200,
        realtime_bar_size_ms: 5_000,
        rng_seed: Some(0xCAFEBABE),
        ..Default::default()
    }
}

async fn advance(d: Duration) {
    tokio::time::advance(d).await;
    // Yield so spawned tasks settle before the next check.
    tokio::task::yield_now().await;
    tokio::task::yield_now().await;
}

/// Advance `total` in `step`-sized chunks, yielding between each step
/// so `tokio::time::interval` fires at each cadence boundary rather
/// than bursting all accumulated ticks onto one poll.
async fn advance_stepped(total: Duration, step: Duration) {
    let mut elapsed = Duration::from_millis(0);
    while elapsed < total {
        let chunk = step.min(total - elapsed);
        tokio::time::advance(chunk).await;
        tokio::task::yield_now().await;
        tokio::task::yield_now().await;
        elapsed += chunk;
    }
}

// ────────────────────────────────────────────────────────────────────────
// 1. Initial burst emits all tick types
// ────────────────────────────────────────────────────────────────────────
#[tokio::test(start_paused = true)]
async fn initial_burst_emits_all_tick_types() {
    let sim = SimMarketData::new(fast_config());
    let mut stream = sim
        .subscribe_ticks(&aapl(), 1, GenericTicks::new())
        .await
        .unwrap();

    // Advance past the burst delay.
    advance(Duration::from_millis(200)).await;

    let mut seen_bid = false;
    let mut seen_ask = false;
    let mut seen_last = false;
    let mut seen_bid_size = false;
    let mut seen_ask_size = false;
    let mut seen_last_size = false;
    let mut seen_volume = false;
    let mut seen_params = false;

    // Drain whatever's ready.
    for _ in 0..32 {
        match stream.next().await {
            Ok(tick) => match (tick.kind, tick.tick_type) {
                (TickKind::Params, _) => seen_params = true,
                (_, TickType::Bid) if matches!(tick.kind, TickKind::Price) => seen_bid = true,
                (_, TickType::Ask) if matches!(tick.kind, TickKind::Price) => seen_ask = true,
                (_, TickType::Last) if matches!(tick.kind, TickKind::Price) => seen_last = true,
                (_, TickType::BidSize) => seen_bid_size = true,
                (_, TickType::AskSize) => seen_ask_size = true,
                (_, TickType::LastSize) => seen_last_size = true,
                (_, TickType::Volume) => seen_volume = true,
                _ => {}
            },
            Err(_) => break,
        }
        // Poll only what's immediately available; continue the loop by
        // quick-advancing a negligible amount.
        if seen_bid
            && seen_ask
            && seen_last
            && seen_bid_size
            && seen_ask_size
            && seen_last_size
            && seen_volume
            && seen_params
        {
            break;
        }
    }

    assert!(seen_bid, "missing Bid");
    assert!(seen_ask, "missing Ask");
    assert!(seen_last, "missing Last");
    assert!(seen_bid_size, "missing BidSize");
    assert!(seen_ask_size, "missing AskSize");
    assert!(seen_last_size, "missing LastSize");
    assert!(seen_volume, "missing Volume");
    assert!(seen_params, "missing TickParams");
}

// ────────────────────────────────────────────────────────────────────────
// 1c. Chart-load smoke: historical's last close ≈ burst's Last tick
//
// Mirrors the actual user-facing symptom: chart loads → calls
// historical_bars(end=now) → subscribes for live ticks → first Quote.last
// gets folded into the last historical bar via update_last_price. If
// historical and live disagree (as they did before the seed_price_for
// re-anchor), the gap shows as a runaway candle. This test pins the
// invariant: |historical_last_close - burst_last_tick| stays small.
// ────────────────────────────────────────────────────────────────────────
#[tokio::test(start_paused = true)]
async fn chart_load_historical_and_burst_agree_on_price() {
    use midas_broker_core::market_data::{TickValue, WhatToShow};

    let sim = SimMarketData::new(fast_config());

    // Let the sim's tick emitter run for a while so market_price
    // could (under the old behaviour) drift far from the historical
    // price level. After this advance, a bug-resurrected sim would
    // report a market_price several percent off the historical close.
    advance_stepped(Duration::from_secs(60), Duration::from_millis(250)).await;

    // The chart's actual call sequence: historical first, then subscribe.
    let hist = sim
        .historical_bars(
            &aapl(),
            1,
            chrono::Utc::now(),
            IbDuration::Days(1),
            Timeframe::M1,
            WhatToShow::Trades,
            true,
        )
        .await
        .unwrap();
    let historical_last_close = hist
        .bars
        .last()
        .expect("historical fetch returned no bars")
        .c;

    let mut stream = sim
        .subscribe_ticks(&aapl(), 1, GenericTicks::new())
        .await
        .unwrap();
    advance(Duration::from_millis(200)).await;

    // First Last tick from the burst must agree with historical's
    // last close — that's the value `update_last_price` will fold
    // into the chart's in-progress candle.
    let mut burst_last: Option<f64> = None;
    for _ in 0..32 {
        match stream.next().await {
            Ok(t) if matches!(t.kind, TickKind::Price) && matches!(t.tick_type, TickType::Last) => {
                if let TickValue::Price(p) = t.value {
                    burst_last = Some(p);
                    break;
                }
            }
            Ok(_) => continue,
            Err(_) => break,
        }
    }
    let burst_last = burst_last.expect("burst missing Last tick");

    // Tolerance: within 1% of the historical close. The re-anchor
    // pulls market_price to the same TestDataProvider call the
    // historical query used, so the only slack is the (tiny) clock
    // skew between the two `Utc::now()` reads in the sim. A bug
    // resurrection — drift from base_price unmoored from
    // TestDataProvider — produces multi-percent gaps.
    let gap_pct = ((burst_last - historical_last_close) / historical_last_close).abs() * 100.0;
    assert!(
        gap_pct < 1.0,
        "historical last close ({historical_last_close}) and burst Last ({burst_last}) disagree by \
         {gap_pct:.2}% — chart will show a runaway candle"
    );
}

// ────────────────────────────────────────────────────────────────────────
// 2. Multiple subs fan out independently
// ────────────────────────────────────────────────────────────────────────
#[tokio::test(start_paused = true)]
async fn multiple_subs_fan_out() {
    let sim = SimMarketData::new(fast_config());
    let a = sim
        .subscribe_ticks(&aapl(), 1, GenericTicks::new())
        .await
        .unwrap();
    let b = sim
        .subscribe_ticks(&aapl(), 1, GenericTicks::new())
        .await
        .unwrap();
    // Snapshot receivers BEFORE advancing; broadcast channels deliver
    // from each receiver's subscribe-point onward.
    let mut rx_a = a.resubscribe();
    let mut rx_b = b.resubscribe();

    // Advance past bursts + ~5s of emission (stepped so the interval
    // actually fires every 250 ms instead of once at the end).
    advance(Duration::from_millis(200)).await;
    advance_stepped(Duration::from_secs(5), Duration::from_millis(250)).await;

    let mut count_a = 0usize;
    let mut count_b = 0usize;
    while rx_a.try_recv().is_ok() {
        count_a += 1;
    }
    while rx_b.try_recv().is_ok() {
        count_b += 1;
    }

    assert!(count_a > 0, "sub A got no ticks");
    assert!(count_b > 0, "sub B got no ticks");
    let ratio = count_a.max(count_b) as f64 / count_a.min(count_b) as f64;
    assert!(
        ratio <= 2.0,
        "subs should receive comparable tick counts: a={count_a}, b={count_b}"
    );
    drop(a);
    drop(b);
}

// ────────────────────────────────────────────────────────────────────────
// 3. Cancel does not immediately silence (M-24 drain)
// ────────────────────────────────────────────────────────────────────────
#[tokio::test(start_paused = true)]
async fn cancel_does_not_immediately_silence() {
    let sim = SimMarketData::new(fast_config());
    let stream = sim
        .subscribe_ticks(&aapl(), 1, GenericTicks::new())
        .await
        .unwrap();
    advance(Duration::from_millis(200)).await;
    // Advance a full cadence to ensure the loop is active.
    advance(Duration::from_millis(300)).await;

    // Drop the handle. The sim keeps the sub in "draining" for
    // `late_tick_window_ms` (200ms by default in fast_config()).
    let rx = stream.resubscribe();
    drop(stream);

    // During the drain window a late tick may still arrive.
    advance(Duration::from_millis(100)).await;
    // After the drain window the GC sweep removes the sub; the `tx`
    // is dropped with the map entry so receivers observe Closed.
    advance(Duration::from_millis(400)).await;
    advance(Duration::from_millis(400)).await;

    // Drain the resubscribed receiver until Closed.
    let mut rx = rx;
    let mut closed = false;
    for _ in 0..32 {
        match rx.try_recv() {
            Ok(_) => continue,
            Err(tokio::sync::broadcast::error::TryRecvError::Closed) => {
                closed = true;
                break;
            }
            Err(tokio::sync::broadcast::error::TryRecvError::Empty) => {
                // Need to yield + advance more to let drain sweep fire.
                advance(Duration::from_millis(300)).await;
            }
            Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_)) => continue,
        }
    }
    assert!(closed, "stream did not close after drop + drain window");
}

// ────────────────────────────────────────────────────────────────────────
// 4. Tick cadence in range
// ────────────────────────────────────────────────────────────────────────
#[tokio::test(start_paused = true)]
async fn tick_cadence_is_in_range() {
    let sim = SimMarketData::new(fast_config());
    let stream = sim
        .subscribe_ticks(&aapl(), 1, GenericTicks::new())
        .await
        .unwrap();
    // Burn off the initial burst.
    advance(Duration::from_millis(200)).await;
    // Drain it so the tick count below only measures the emitter loop.
    let mut rx = stream.resubscribe();
    while rx.try_recv().is_ok() {}

    // Advance in 250 ms chunks so the interval's MissedTickBehavior
    // doesn't coalesce a 5 s jump into a single wake-up.
    advance_stepped(Duration::from_secs(5), Duration::from_millis(250)).await;

    // Count Last ticks (exactly one per cadence window).
    let mut last_count = 0usize;
    while let Ok(t) = rx.try_recv() {
        if t.tick_type == TickType::Last {
            last_count += 1;
        }
    }
    // Drop the stream to prevent keeping the sub alive past the test.
    drop(stream);
    // 5 s / 250 ms = 20 windows. Allow [10, 40] to cover boundary
    // races with the initial burst + a drift in stepped-advance alignment.
    assert!(
        (10..=40).contains(&last_count),
        "expected 10-40 Last events in 5s, got {last_count}"
    );
}

// ────────────────────────────────────────────────────────────────────────
// 4b. Tick drift — random-walk behaviour
// (ports the `auto_tick_with_*` tests retired with `TestBroker` in 10f).
// ────────────────────────────────────────────────────────────────────────

/// Collect every `Last` price emitted on `stream` while advancing paused
/// time in `step`-sized chunks for `total`. Mirrors the legacy
/// `poll_for` + `tick_last_values` pair but against the router-era
/// broadcast channel.
async fn collect_last_prices(
    rx: &mut tokio::sync::broadcast::Receiver<std::sync::Arc<midas_broker_core::market_data::Tick>>,
    total: Duration,
    step: Duration,
) -> Vec<f64> {
    use midas_broker_core::market_data::{TickType, TickValue};
    // Drain whatever was buffered before starting the window (init burst).
    while rx.try_recv().is_ok() {}
    advance_stepped(total, step).await;
    let mut out = Vec::new();
    while let Ok(tick) = rx.try_recv() {
        if tick.tick_type == TickType::Last {
            if let TickValue::PriceSize { price, .. } = tick.value {
                out.push(price);
            }
        }
    }
    out
}

#[tokio::test(start_paused = true)]
async fn tick_drift_moves_price_when_positive() {
    // Non-trivial drift plus a deterministic seed → consecutive `Last`
    // ticks should walk away from the initial price.
    let mut cfg = fast_config();
    cfg.tick_drift_bps = 100.0; // 1% peak drift so the walk is visible
    let sim = SimMarketData::new(cfg);
    // Burn off Ready + initial burst.
    advance(Duration::from_millis(300)).await;
    let stream = sim
        .subscribe_ticks(&aapl(), 1, GenericTicks::new())
        .await
        .unwrap();
    let mut rx = stream.resubscribe();

    let lasts =
        collect_last_prices(&mut rx, Duration::from_secs(2), Duration::from_millis(250)).await;
    drop(stream);
    assert!(
        lasts.len() >= 2,
        "expected ≥2 Last ticks over 2 s at 250 ms cadence, got {}",
        lasts.len()
    );
    let any_differ = lasts
        .iter()
        .any(|&a| lasts.iter().any(|&b| (a - b).abs() > f64::EPSILON));
    assert!(
        any_differ,
        "tick_drift_bps > 0 should produce distinct Last prices, got {lasts:?}"
    );
}

#[tokio::test(start_paused = true)]
async fn tick_drift_zero_keeps_price_constant() {
    let mut cfg = fast_config();
    cfg.tick_drift_bps = 0.0;
    let sim = SimMarketData::new(cfg);
    advance(Duration::from_millis(300)).await;
    let stream = sim
        .subscribe_ticks(&aapl(), 1, GenericTicks::new())
        .await
        .unwrap();
    let mut rx = stream.resubscribe();

    let lasts =
        collect_last_prices(&mut rx, Duration::from_secs(2), Duration::from_millis(250)).await;
    drop(stream);
    assert!(
        !lasts.is_empty(),
        "expected ≥1 Last tick even with zero drift"
    );
    let first = lasts[0];
    for (i, v) in lasts.iter().enumerate() {
        assert!(
            (*v - first).abs() < f64::EPSILON,
            "tick {i} last={v} should equal first tick last={first} when drift is zero"
        );
    }
}

// ────────────────────────────────────────────────────────────────────────
// 5. historical_stream transitions
// ────────────────────────────────────────────────────────────────────────
#[tokio::test(start_paused = true)]
async fn historical_stream_transitions() {
    let sim = SimMarketData::new(fast_config());
    let mut stream = sim
        .historical_stream(
            &aapl(),
            1,
            IbDuration::Days(2),
            Timeframe::M1,
            WhatToShow::Trades,
            true,
        )
        .await
        .unwrap();

    // Historical + End arrive synchronously.
    let first = stream.next().await.expect("historical");
    assert!(matches!(first, HistoricalStreamEvent::Historical(_)));
    let second = stream.next().await.expect("end");
    assert!(matches!(second, HistoricalStreamEvent::End { .. }));

    // Update follows at the bar-size cadence.
    advance(Duration::from_secs(120)).await;
    let third = stream.next().await.expect("update");
    assert!(matches!(third, HistoricalStreamEvent::Update(_)));
}

// ────────────────────────────────────────────────────────────────────────
// 6. historical_bars returns without streaming
// ────────────────────────────────────────────────────────────────────────
#[tokio::test(start_paused = true)]
async fn historical_bars_returns_immediately() {
    let sim = SimMarketData::new(fast_config());
    let result = sim
        .historical_bars(
            &aapl(),
            1,
            chrono::Utc::now(),
            IbDuration::Days(1),
            Timeframe::M5,
            WhatToShow::Trades,
            true,
        )
        .await
        .unwrap();
    assert!(!result.bars.is_empty());
    assert!(result.first_ts <= result.last_ts);
}

// ────────────────────────────────────────────────────────────────────────
// 7. Reconnect drops all subscriptions
// ────────────────────────────────────────────────────────────────────────
#[tokio::test(start_paused = true)]
async fn reconnect_drops_all_subscriptions() {
    let sim = SimMarketData::new(fast_config());
    let stream = sim
        .subscribe_ticks(&aapl(), 1, GenericTicks::new())
        .await
        .unwrap();
    advance(Duration::from_millis(200)).await;

    // Drain burst so we're in steady state.
    let mut rx = stream.resubscribe();
    while rx.try_recv().is_ok() {}

    sim.simulate_connection_lost(FarmCode::ConnectionRestoredDataLost);
    advance(Duration::from_millis(50)).await;

    // Drain until the original stream reports Closed.
    let mut closed = false;
    for _ in 0..64 {
        match rx.try_recv() {
            Ok(_) => continue,
            Err(tokio::sync::broadcast::error::TryRecvError::Closed) => {
                closed = true;
                break;
            }
            Err(tokio::sync::broadcast::error::TryRecvError::Empty) => {
                advance(Duration::from_millis(50)).await;
            }
            Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_)) => continue,
        }
    }
    assert!(closed, "old handle did not close after farm drop");

    // Reconnect and fresh subscribe works.
    sim.simulate_reconnect();
    advance(Duration::from_millis(50)).await;
    let fresh = sim
        .subscribe_ticks(&aapl(), 1, GenericTicks::new())
        .await
        .unwrap();
    advance(Duration::from_millis(200)).await;
    let mut fresh_rx = fresh.resubscribe();
    let mut got_any = false;
    for _ in 0..32 {
        if fresh_rx.try_recv().is_ok() {
            got_any = true;
            break;
        }
        advance(Duration::from_millis(50)).await;
    }
    assert!(got_any, "fresh subscription produced no ticks");
    drop(stream);
    drop(fresh);
}

// ────────────────────────────────────────────────────────────────────────
// 8. Farm status sequence on connect
// ────────────────────────────────────────────────────────────────────────
#[tokio::test(start_paused = true)]
async fn farm_status_sequence_on_connect() {
    let sim = SimMarketData::new(fast_config());
    let mut farm = sim.farm_status();
    let mut conn = sim.connection_state();
    let mut ord = sim.ordering_ready();

    // Advance past the farm-up delay.
    advance(Duration::from_millis(200)).await;

    let a = farm.recv().await.expect("mkt farm");
    let b = farm.recv().await.expect("hist farm");
    let c = farm.recv().await.expect("secdef farm");
    assert_eq!(a.code, FarmCode::MarketDataFarmOk);
    assert_eq!(b.code, FarmCode::HistoricalDataFarmOk);
    assert_eq!(c.code, FarmCode::SecDefFarmOk);

    // OrderingReady fires on its own channel (M-14: NOT a FarmCode).
    let _ord = ord.recv().await.expect("ordering ready");

    // ConnectionState eventually reaches Ready.
    for _ in 0..8 {
        if matches!(*conn.borrow_and_update(), ConnectionState::Ready) {
            return;
        }
        if conn.changed().await.is_err() {
            break;
        }
    }
    panic!("connection state never reached Ready");
}

// ────────────────────────────────────────────────────────────────────────
// 9. Order place + fill flow
// ────────────────────────────────────────────────────────────────────────
#[tokio::test(start_paused = true)]
async fn order_place_and_fill_flow() {
    let sim = SimMarketData::new(fast_config());
    let orders = SimOrderClient::new(SimOrderConfig::default(), Some(sim.clone()));

    // Wait for Ready so next_order_id returns.
    advance(Duration::from_millis(250)).await;
    let id = orders.next_order_id().await.unwrap();

    let mut events = orders.order_events();
    let spec = OrderSpec {
        ib_order_id: id,
        symbol: aapl(),
        con_id: 1,
        action: OrderAction::Buy,
        order_type: OrderType::Market,
        quantity: 100.0,
        limit_price: None,
        stop_price: None,
        parent_id: None,
        transmit: true,
        tif: Tif::Day,
        outside_rth: false,
        oca_group: None,
        oca_type: None,
        conditions: vec![],
        algo_strategy: None,
        algo_params: vec![],
        good_after_time: None,
        good_till_date: None,
        display_size: None,
        hidden: false,
        trigger_method: midas_broker::TriggerMethod::Default,
        discretionary_amt: None,
        sweep_to_fill: false,
    };
    orders.place_order(spec).await.unwrap();

    let mut seen_submitted = false;
    let mut seen_status_submitted = false;
    let mut seen_execution = false;
    let mut seen_commission = false;
    let mut seen_filled = false;

    for _ in 0..16 {
        match events.recv().await {
            Ok(OrderEvent::Submitted { .. }) => seen_submitted = true,
            Ok(OrderEvent::StatusChanged {
                status: midas_broker::OrderStatus::Submitted,
                ..
            }) => seen_status_submitted = true,
            Ok(OrderEvent::ExecutionDetails { .. }) => seen_execution = true,
            Ok(OrderEvent::Commission { .. }) => seen_commission = true,
            Ok(OrderEvent::StatusChanged {
                status: midas_broker::OrderStatus::Filled,
                ..
            }) => {
                seen_filled = true;
                break;
            }
            Ok(_) => {}
            Err(_) => break,
        }
    }
    assert!(seen_submitted, "missing Submitted");
    assert!(seen_status_submitted, "missing StatusChanged(Submitted)");
    assert!(seen_execution, "missing ExecutionDetails");
    assert!(seen_commission, "missing Commission");
    assert!(seen_filled, "missing StatusChanged(Filled)");
}

// ────────────────────────────────────────────────────────────────────────
// 10. Data-kept reconnect keeps subscriptions
// ────────────────────────────────────────────────────────────────────────
#[tokio::test(start_paused = true)]
async fn data_kept_reconnect_keeps_subscriptions() {
    let sim = SimMarketData::new(fast_config());
    let stream = sim
        .subscribe_ticks(&aapl(), 1, GenericTicks::new())
        .await
        .unwrap();
    advance(Duration::from_millis(200)).await;
    let mut rx = stream.resubscribe();
    while rx.try_recv().is_ok() {}

    sim.simulate_connection_lost(FarmCode::ConnectionRestoredDataKept);
    advance_stepped(Duration::from_secs(1), Duration::from_millis(250)).await;

    // Stream should still deliver ticks.
    let mut got_tick = false;
    for _ in 0..32 {
        if rx.try_recv().is_ok() {
            got_tick = true;
            break;
        }
        advance(Duration::from_millis(50)).await;
    }
    assert!(got_tick, "1102 dropped subscription unexpectedly");
    drop(stream);
}

// ────────────────────────────────────────────────────────────────────────
// 11. Deterministic historical seam (BR-21)
// ────────────────────────────────────────────────────────────────────────
#[tokio::test(start_paused = true)]
async fn deterministic_historical_seam() {
    let t0 = chrono::Utc::now();
    let cfg = SimMarketDataConfig {
        historical_last_ts: Some(t0),
        ..fast_config()
    };
    let sim = SimMarketData::new(cfg);
    let result = sim
        .historical_bars(
            &aapl(),
            1,
            chrono::Utc::now(),
            IbDuration::Days(1),
            Timeframe::M5,
            WhatToShow::Trades,
            true,
        )
        .await
        .unwrap();
    assert_eq!(result.last_ts, t0, "historical_last_ts override ignored");
}
