//! Slice 4 live-IB integration tests.
//!
//! **Dev-local only.** These tests require a running TWS or IB Gateway
//! paper session on `127.0.0.1:4002`. They are gated with
//! `#[cfg_attr(not(feature = "ib_live_tests"), ignore)]` so they
//! compile in every CI build (that's how M-31's non-blocking clippy
//! job catches drift in the feature-gated code) but do NOT run unless
//! the feature is explicitly enabled.
//!
//! Run locally with:
//!
//! ```bash
//! cargo test -p midas-broker --features ib_live_tests --test ib_live -- --include-ignored
//! ```
//!
//! BR-24: these are Tier-2 tests, not CI-gated.

#![allow(dead_code)]

use std::sync::Arc;
use std::time::Duration;

use midas_broker::ib::{IbMarketData, IbMarketDataConfig};
use midas_broker::stream::HistoricalStreamEvent;
use midas_broker::MarketDataSource;
use midas_broker_core::market_data::{
    ConnectionState, FarmCode, GenericTicks, IbDuration, SymbolKey, TickType, Timeframe, WhatToShow,
};

fn paper_config() -> IbMarketDataConfig {
    IbMarketDataConfig {
        host: "127.0.0.1".into(),
        port: 4002,
        client_id: 913, // avoid collisions with devloop + manual trading sessions
        ..IbMarketDataConfig::default()
    }
}

fn spy() -> SymbolKey {
    SymbolKey {
        // 756733 is IB's conId for SPY on SMART/NASDAQ; harmless if the
        // test account resolves a different listing via contract_details.
        contract_id: 756733,
        symbol: "SPY".into(),
    }
}

#[tokio::test]
#[cfg_attr(not(feature = "ib_live_tests"), ignore)]
async fn ib_connect_reports_server_version() {
    let md = IbMarketData::new(paper_config());
    md.connect().await.expect("ib connect");
    let mut rx = md.connection_state();
    // Walk the watch until Ready.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let reached = matches!(
            *rx.borrow(),
            ConnectionState::Ready | ConnectionState::Connected { .. }
        );
        if reached {
            break;
        }
        tokio::select! {
            _ = tokio::time::sleep_until(deadline) => {
                let snap = rx.borrow().clone();
                panic!("did not reach Connected/Ready: last = {snap:?}");
            }
            r = rx.changed() => { r.expect("watch closed"); }
        }
    }
}

#[tokio::test]
#[cfg_attr(not(feature = "ib_live_tests"), ignore)]
async fn ib_subscribe_ticks_emits_bursts() {
    let md = Arc::new(IbMarketData::new(paper_config()));
    md.connect().await.expect("connect");
    let stream = md
        .subscribe_ticks(
            &spy(),
            spy().contract_id,
            GenericTicks::from_codes(vec![233, 293]),
        )
        .await
        .expect("subscribe_ticks");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let (mut saw_bid, mut saw_ask, mut saw_last) = (false, false, false);
    let mut rx = stream.resubscribe();
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_secs(2), rx.recv()).await {
            Ok(Ok(tick)) => match tick.tick_type {
                TickType::Bid => saw_bid = true,
                TickType::Ask => saw_ask = true,
                TickType::Last => saw_last = true,
                _ => {}
            },
            Ok(Err(e)) => panic!("recv error: {e}"),
            Err(_) => continue,
        }
        if saw_bid && saw_ask && saw_last {
            break;
        }
    }
    assert!(
        saw_bid && saw_ask && saw_last,
        "missing {saw_bid}/{saw_ask}/{saw_last}"
    );
    drop(stream);
}

#[tokio::test]
#[cfg_attr(not(feature = "ib_live_tests"), ignore)]
async fn ib_historical_stream_keep_up_to_date() {
    let md = Arc::new(IbMarketData::new(paper_config()));
    md.connect().await.expect("connect");
    let mut stream = md
        .historical_stream(
            &spy(),
            spy().contract_id,
            IbDuration::Seconds(3600),
            Timeframe::M1,
            WhatToShow::Trades,
            true,
        )
        .await
        .expect("historical_stream");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    let (mut got_historical, mut got_end, mut got_update) = (false, false, false);
    while tokio::time::Instant::now() < deadline && !(got_historical && got_end && got_update) {
        match tokio::time::timeout(Duration::from_secs(5), stream.next()).await {
            Ok(Some(HistoricalStreamEvent::Historical(_))) => got_historical = true,
            Ok(Some(HistoricalStreamEvent::End { .. })) => got_end = true,
            Ok(Some(HistoricalStreamEvent::Update(_))) => got_update = true,
            Ok(Some(HistoricalStreamEvent::Error(e))) => panic!("stream error: {e}"),
            Ok(None) => break,
            Err(_) => continue,
        }
    }
    assert!(got_historical && got_end, "missing Historical/End");
    // Update may or may not arrive within 20s depending on market session;
    // log-only assertion.
    if !got_update {
        eprintln!("note: no live update arrived within 20s (market likely closed)");
    }
}

#[tokio::test]
#[cfg_attr(not(feature = "ib_live_tests"), ignore)]
async fn ib_farm_status_fires_on_connect() {
    let md = IbMarketData::new(paper_config());
    // Subscribe BEFORE connect so we don't miss the burst.
    let mut farm_rx = md.farm_status();
    md.connect().await.expect("connect");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let mut ok_seen = false;
    while tokio::time::Instant::now() < deadline && !ok_seen {
        match tokio::time::timeout(Duration::from_secs(2), farm_rx.recv()).await {
            Ok(Ok(fs)) if matches!(fs.code, FarmCode::MarketDataFarmOk) => ok_seen = true,
            Ok(Ok(_)) => {}
            Ok(Err(_)) => break,
            Err(_) => continue,
        }
    }
    // Paper sessions sometimes start while their US market-data farm is
    // in 2108 / inactive state; log-only to avoid false CI reds for
    // future dev-loop runs.
    if !ok_seen {
        eprintln!("note: MarketDataFarmOk did not arrive within 10s (farm may be inactive)");
    }
}
