//! S9 crypto-M1 vertical-slice integration test.
//!
//! Builds a synthetic `MarketDataSource` that emits 10 minute bars of
//! historical BTC-USD data, then drips two more live bars on the
//! realtime channel. Subscribes via `build_history_then_live` and
//! asserts:
//!
//! - All historical candles carry `CRYPTO` calendar and
//!   `SessionKind::Regular`.
//! - Historical timestamps are monotonically increasing.
//! - After the seam, the first live candle's `window.open` sits
//!   strictly after the historical `last_ts`.
//! - No duplicate at the seam (HistoryThenLive dedup drops the live
//!   overlap).

use std::sync::{Arc, Mutex, OnceLock};

use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use midas_bars_adapter::{build_history_then_live, HeuristicSymbolResolver};
use midas_broker_core::market_data::{
    Bar, BarCompleteness, ConnectionState, ContractDetails, FarmStatus, GenericTicks, IbDuration,
    MarketDataError, ReqId, SecurityType, SymbolKey, Tick, TickByTickKind, Timeframe, WhatToShow,
};
use midas_broker_core::provider::{
    HistoricalBarsResult, HistoricalStream, MarketDataSource, RealtimeBarStream, TickStream,
};
use midas_calendar::{BarPeriod, SessionKind, CRYPTO_SPOT_ID};
use midas_stream::BarStream;
use tokio::sync::{broadcast, watch};

// ---------------------------------------------------------------------------
// Mock MarketDataSource
// ---------------------------------------------------------------------------

struct MockSource {
    historical: HistoricalBarsResult,
    // Live channel used to feed RealtimeBarAdapter via the broker-core
    // RealtimeBarStream handle. We stash the sender in a Mutex so the
    // test can `push_live()` after `subscribe_realtime_bars` is called.
    live_tx: Mutex<Option<broadcast::Sender<Arc<Bar>>>>,
    // Watch channel for connection_state — never read by this test but
    // needs a Sender to keep the Receiver alive.
    _conn_tx: watch::Sender<ConnectionState>,
    conn_rx: watch::Receiver<ConnectionState>,
    farm_tx: broadcast::Sender<FarmStatus>,
}

impl MockSource {
    fn new(historical: HistoricalBarsResult) -> Arc<Self> {
        let (conn_tx, conn_rx) = watch::channel(ConnectionState::Ready);
        let (farm_tx, _) = broadcast::channel(16);
        Arc::new(Self {
            historical,
            live_tx: Mutex::new(None),
            _conn_tx: conn_tx,
            conn_rx,
            farm_tx,
        })
    }

    fn push_live(&self, bar: Bar) {
        let guard = self.live_tx.lock().unwrap();
        if let Some(tx) = guard.as_ref() {
            let _ = tx.send(Arc::new(bar));
        }
    }
}

#[async_trait]
impl MarketDataSource for MockSource {
    async fn subscribe_ticks(
        &self,
        _symbol: &SymbolKey,
        _con_id: i32,
        _generic_ticks: GenericTicks,
    ) -> Result<TickStream, MarketDataError> {
        let (_tx, rx) = broadcast::channel::<Arc<Tick>>(1);
        let last_error = Arc::new(OnceLock::new());
        Ok(TickStream::new(ReqId(1), rx, last_error, Box::new(|| {})))
    }

    async fn subscribe_tick_by_tick(
        &self,
        _symbol: &SymbolKey,
        _con_id: i32,
        _kind: TickByTickKind,
    ) -> Result<TickStream, MarketDataError> {
        let (_tx, rx) = broadcast::channel::<Arc<Tick>>(1);
        let last_error = Arc::new(OnceLock::new());
        Ok(TickStream::new(ReqId(2), rx, last_error, Box::new(|| {})))
    }

    async fn subscribe_realtime_bars(
        &self,
        _symbol: &SymbolKey,
        _con_id: i32,
        _what_to_show: WhatToShow,
    ) -> Result<RealtimeBarStream, MarketDataError> {
        let (tx, rx) = broadcast::channel::<Arc<Bar>>(32);
        *self.live_tx.lock().unwrap() = Some(tx);
        let last_error = Arc::new(OnceLock::new());
        Ok(RealtimeBarStream::new(
            ReqId(3),
            rx,
            last_error,
            Box::new(|| {}),
        ))
    }

    async fn historical_bars(
        &self,
        _symbol: &SymbolKey,
        _con_id: i32,
        _end: chrono::DateTime<Utc>,
        _duration: IbDuration,
        _bar_size: Timeframe,
        _what_to_show: WhatToShow,
        _use_rth: bool,
    ) -> Result<HistoricalBarsResult, MarketDataError> {
        Ok(self.historical.clone())
    }

    async fn historical_stream(
        &self,
        _symbol: &SymbolKey,
        _con_id: i32,
        _duration: IbDuration,
        _bar_size: Timeframe,
        _what_to_show: WhatToShow,
        _use_rth: bool,
    ) -> Result<HistoricalStream, MarketDataError> {
        // Not exercised by this test — return a closed channel.
        let (_tx, rx) = tokio::sync::mpsc::channel(1);
        Ok(HistoricalStream::new(ReqId(4), rx, Box::new(|| {})))
    }

    async fn resolve_contract(
        &self,
        _symbol: &SymbolKey,
        _sec_type: SecurityType,
        _exchange: &str,
    ) -> Result<ContractDetails, MarketDataError> {
        Err(MarketDataError::Unsupported)
    }

    fn farm_status(&self) -> broadcast::Receiver<FarmStatus> {
        self.farm_tx.subscribe()
    }

    fn connection_state(&self) -> watch::Receiver<ConnectionState> {
        self.conn_rx.clone()
    }

    fn name(&self) -> &str {
        "mock"
    }
}

// ---------------------------------------------------------------------------
// Synthetic bar builders
// ---------------------------------------------------------------------------

fn btc_bar(minute_offset: i64) -> Bar {
    // 2024-08-17 00:00 UTC as the base timestamp. Crypto is Regular at
    // every UTC minute under CRYPTO_SPOT.
    let base = Utc.with_ymd_and_hms(2024, 8, 17, 0, 0, 0).unwrap();
    let open = base + chrono::Duration::minutes(minute_offset);
    Bar {
        symbol: SymbolKey {
            contract_id: 0, // Ignored by MockSource; adapter overrides.
            symbol: "BTC-USD".into(),
        },
        timeframe: Timeframe::M1,
        ts_open: open,
        ts_close: open + chrono::Duration::minutes(1),
        o: 50_000.0 + (minute_offset as f64) * 1.0,
        h: 50_010.0 + (minute_offset as f64) * 1.0,
        l: 49_990.0 + (minute_offset as f64) * 1.0,
        c: 50_005.0 + (minute_offset as f64) * 1.0,
        volume: 100 + minute_offset as u64,
        trade_count: 10,
        wap: Some(50_000.0 + (minute_offset as f64) * 1.0),
        completeness: BarCompleteness::Completed,
    }
}

// ---------------------------------------------------------------------------
// The pipeline test
// ---------------------------------------------------------------------------

#[tokio::test]
async fn crypto_m1_history_then_live_pipeline() {
    // 10 minutes of historical bars: 00:00..00:10.
    let hist_bars: Vec<Bar> = (0..10).map(btc_bar).collect();
    let first_ts = hist_bars.first().unwrap().ts_open;
    let last_ts = hist_bars.last().unwrap().ts_open;
    let historical = HistoricalBarsResult {
        bars: hist_bars,
        first_ts,
        last_ts,
    };

    let source = MockSource::new(historical);
    let resolver = HeuristicSymbolResolver::new();
    let source_arc: Arc<dyn MarketDataSource> = source.clone();

    let mut stream = build_history_then_live(
        source_arc,
        &resolver,
        "BTC-USD",
        BarPeriod::m1(),
        Utc.with_ymd_and_hms(2024, 8, 17, 1, 0, 0).unwrap(),
        IbDuration::Seconds(600),
        true,
        None,
    )
    .await
    .expect("history-then-live builds");

    // Calendar sanity.
    assert_eq!(stream.meta().calendar.id(), CRYPTO_SPOT_ID);
    assert_eq!(stream.meta().period, BarPeriod::m1());

    // Drain 10 historical candles, asserting the invariants.
    let mut collected = Vec::new();
    for _ in 0..10 {
        let c = stream.next().await.expect("historical candle");
        collected.push(c);
    }
    assert_eq!(collected.len(), 10);
    for c in &collected {
        assert_eq!(c.calendar, CRYPTO_SPOT_ID);
        assert_eq!(c.session.kind(), SessionKind::Regular);
    }
    // Monotonic increasing timestamps.
    for w in collected.windows(2) {
        assert!(
            w[1].window.open > w[0].window.open,
            "historical not monotonic: {} !> {}",
            w[1].window.open,
            w[0].window.open,
        );
    }
    let historical_last = collected.last().unwrap().window.open;
    assert_eq!(historical_last, last_ts);

    // Now push two live bars: one that overlaps the seam (minute_offset
    // = 9, same as last historical) and one strictly after
    // (minute_offset = 10).
    //
    // Bug-hunt H3 (plan/session-aware-charts/99-diagnostic-findings-r2.md):
    // prior behaviour dropped the seam bar with `<=`, which also dropped
    // Partial refreshes of the seam. The fix uses strict `<`: same-open
    // live bars are FORWARDED to downstream consumers. Downstream
    // `CandleSeries::apply` overwrites the row by open-ts, so the storage
    // never sees a duplicate — but the STREAM forwards both.
    //
    // So the stream now emits minute_offset=9 (seam refresh) THEN
    // minute_offset=10.
    source.push_live(btc_bar(9));
    source.push_live(btc_bar(10));

    let seam_refresh = stream.next().await.expect("seam refresh candle");
    assert_eq!(
        seam_refresh.window.open, historical_last,
        "seam refresh carries the same window.open as the last historical bar"
    );
    let live = stream.next().await.expect("live candle after seam");
    assert!(
        live.window.open > historical_last,
        "live {} !> historical_last {}",
        live.window.open,
        historical_last,
    );
    let expected_live_open = Utc.with_ymd_and_hms(2024, 8, 17, 0, 10, 0).unwrap();
    assert_eq!(live.window.open, expected_live_open);
    assert_eq!(live.calendar, CRYPTO_SPOT_ID);
    assert_eq!(live.session.kind(), SessionKind::Regular);
}
