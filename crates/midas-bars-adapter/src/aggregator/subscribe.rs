//! `subscribe_aggregated_bars` — convenience wiring that takes a
//! [`MarketDataSource`], a [`SymbolResolver`], and a target period, and
//! returns a ready-to-drain [`SessionedBarStream`].
//!
//! Wires up:
//! 1. Resolver lookup → `(Symbol, calendar, contract_id)`.
//! 2. `source.subscribe_ticks(...)` → broker-core [`TickStream`].
//! 3. A spawned pump task that forwards `TickStream::recv()` output into
//!    a local `mpsc::Sender<Arc<Tick>>`. The pump absorbs
//!    [`broadcast::error::RecvError::Lagged`] by logging-and-continuing
//!    (same policy as [`RealtimeBarAdapter`](crate::realtime::RealtimeBarAdapter)).
//! 4. A fresh [`SessionedBarAggregator`] + [`SessionedBarStream`].
//!
//! The spawned pump task lives for as long as the `TickStream` handle
//! exists. When the consumer drops the `SessionedBarStream`, the mpsc
//! closes → the pump's next `send` fails → pump exits → `TickStream`
//! drops → broker-core cancels upstream (BR-2 RAII).

use std::sync::Arc;
use std::time::Duration;

use midas_broker_core::market_data::{GenericTicks, SymbolKey, Tick};
use midas_broker_core::provider::MarketDataSource;
use midas_calendar::BarPeriod;
use midas_clock::Clock;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::mpsc;

use crate::error::AdapterError;
use crate::resolver::SymbolResolver;
use crate::timeout::BROKER_CALL_TIMEOUT;

use super::config::AggregatorConfig;
use super::core::SessionedBarAggregator;
use super::stream::SessionedBarStream;

/// Buffer depth for the internal tick pump → aggregator channel. 1024
/// absorbs short stalls (e.g. a few hundred milliseconds of GC / scheduler
/// lag) without back-pressuring the broker-core broadcast receiver.
const PUMP_BUFFER: usize = 1024;

/// Subscribe to a live-aggregated bar stream for `ticker` at `period`.
pub async fn subscribe_aggregated_bars(
    source: Arc<dyn MarketDataSource>,
    resolver: &dyn SymbolResolver,
    ticker: &str,
    period: BarPeriod,
    clock: Arc<dyn Clock>,
) -> Result<SessionedBarStream, AdapterError> {
    subscribe_aggregated_bars_with_timeout(
        source,
        resolver,
        ticker,
        period,
        clock,
        BROKER_CALL_TIMEOUT,
    )
    .await
}

/// Internal entry-point exposed for tests that need to pin a short
/// deadline on the broker call — the public
/// [`subscribe_aggregated_bars`] uses the adapter-wide default of
/// [`BROKER_CALL_TIMEOUT`]. Production callers should go through the
/// wrapper above.
pub async fn subscribe_aggregated_bars_with_timeout(
    source: Arc<dyn MarketDataSource>,
    resolver: &dyn SymbolResolver,
    ticker: &str,
    period: BarPeriod,
    clock: Arc<dyn Clock>,
    timeout: Duration,
) -> Result<SessionedBarStream, AdapterError> {
    let resolved = resolver.resolve(ticker)?;

    // Validate period against the resolved calendar BEFORE issuing an
    // upstream subscribe — we'd rather reject the config up front than
    // leak a TickStream handle that will never produce usable bars.
    resolved
        .calendar
        .validate_period(period)
        .map_err(|e| AdapterError::Stream(midas_stream::StreamError::Upstream(e.to_string())))?;

    let symbol_key = SymbolKey {
        contract_id: resolved.contract_id,
        symbol: ticker.to_string(),
    };

    // Subscribe ticks, bounded by the adapter timeout. `GenericTicks::
    // new()` leaves the list empty — we only need Last/PriceSize,
    // which IB emits by default with reqMktData.
    let subscribe_fut =
        source.subscribe_ticks(&symbol_key, resolved.contract_id, GenericTicks::new());
    let mut tick_handle = match tokio::time::timeout(timeout, subscribe_fut).await {
        Ok(res) => res?,
        Err(_) => {
            tracing::warn!(
                ticker,
                secs = timeout.as_secs(),
                "subscribe_aggregated_bars: subscribe_ticks timed out",
            );
            return Err(AdapterError::Timeout {
                op: "subscribe_ticks",
                secs: timeout.as_secs(),
            });
        }
    };

    let (tx, rx) = mpsc::channel::<Arc<Tick>>(PUMP_BUFFER);

    // Pump task: read from the broker-core broadcast handle, forward to
    // the mpsc. Owns `tick_handle`; when `tx.send` fails (mpsc closed
    // by consumer drop) the task exits and drops the handle, cascading
    // the upstream cancel.
    tokio::spawn(async move {
        loop {
            match tick_handle.next().await {
                Ok(t) => {
                    if tx.send(t).await.is_err() {
                        // Consumer dropped the stream; exit.
                        return;
                    }
                }
                Err(RecvError::Lagged(n)) => {
                    // Broker-core broadcast overflow. Log-and-continue;
                    // the aggregator's OHLC state tolerates skipped
                    // ticks (degraded volume/trade_count, same OHLC) and
                    // the next tick drives the stream forward.
                    tracing::warn!(lagged = n, "subscribe_aggregated_bars: tick pump lagged");
                }
                Err(RecvError::Closed) => {
                    return;
                }
            }
        }
    });

    let cfg = AggregatorConfig::new(resolved.symbol, resolved.calendar, period, clock);
    let agg = SessionedBarAggregator::new(cfg).map_err(|e| match e {
        super::core::AggregatorError::InvalidPeriod(err) => {
            AdapterError::Stream(midas_stream::StreamError::Upstream(err.to_string()))
        }
    })?;

    Ok(SessionedBarStream::new(agg, rx))
}

#[cfg(test)]
mod tests {
    use super::*;

    use async_trait::async_trait;
    use chrono::{DateTime, Utc};
    use midas_broker_core::market_data::{
        ConnectionState, ContractDetails, FarmStatus, GenericTicks, IbDuration, MarketDataError,
        SecurityType, SymbolKey, TickByTickKind, Timeframe, WhatToShow,
    };
    use midas_broker_core::provider::{
        HistoricalBarsResult, HistoricalStream, MarketDataSource, RealtimeBarStream, TickStream,
    };
    use midas_clock::SystemClock;
    use tokio::sync::{broadcast, watch};

    use crate::resolver::StaticSymbolResolver;

    /// Mock provider whose `subscribe_ticks` future never resolves.
    /// Used to exercise the timeout path.
    struct StallingSource {
        _conn_tx: watch::Sender<ConnectionState>,
        conn_rx: watch::Receiver<ConnectionState>,
        farm_tx: broadcast::Sender<FarmStatus>,
    }

    impl StallingSource {
        fn new() -> Arc<Self> {
            let (conn_tx, conn_rx) = watch::channel(ConnectionState::Ready);
            let (farm_tx, _) = broadcast::channel(4);
            Arc::new(Self {
                _conn_tx: conn_tx,
                conn_rx,
                farm_tx,
            })
        }
    }

    #[async_trait]
    impl MarketDataSource for StallingSource {
        async fn subscribe_ticks(
            &self,
            _symbol: &SymbolKey,
            _con_id: i32,
            _generic_ticks: GenericTicks,
        ) -> Result<TickStream, MarketDataError> {
            std::future::pending::<()>().await;
            unreachable!("pending future by construction");
        }

        async fn subscribe_tick_by_tick(
            &self,
            _symbol: &SymbolKey,
            _con_id: i32,
            _kind: TickByTickKind,
        ) -> Result<TickStream, MarketDataError> {
            Err(MarketDataError::Unsupported)
        }

        async fn subscribe_realtime_bars(
            &self,
            _symbol: &SymbolKey,
            _con_id: i32,
            _what: WhatToShow,
        ) -> Result<RealtimeBarStream, MarketDataError> {
            Err(MarketDataError::Unsupported)
        }

        async fn historical_bars(
            &self,
            _symbol: &SymbolKey,
            _con_id: i32,
            _end: DateTime<Utc>,
            _duration: IbDuration,
            _bar_size: Timeframe,
            _what_to_show: WhatToShow,
            _use_rth: bool,
        ) -> Result<HistoricalBarsResult, MarketDataError> {
            Err(MarketDataError::Unsupported)
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
            Err(MarketDataError::Unsupported)
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
            "stalling-test-source"
        }
    }

    /// Regression: app-harden M1. `subscribe_ticks` stalls forever;
    /// wrapper must bail with `AdapterError::Timeout` instead of
    /// hanging the caller.
    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn subscribe_aggregated_bars_times_out_on_stalled_provider() {
        let source: Arc<dyn MarketDataSource> = StallingSource::new();
        let resolver = StaticSymbolResolver::new();
        let clock: Arc<dyn Clock> = Arc::new(SystemClock);
        let result = subscribe_aggregated_bars_with_timeout(
            source,
            &resolver,
            "BTC-USD",
            BarPeriod::m1(),
            clock,
            Duration::from_millis(100),
        )
        .await;
        match result {
            Err(AdapterError::Timeout { op, secs: _ }) => {
                assert_eq!(op, "subscribe_ticks");
            }
            other => panic!("expected AdapterError::Timeout, got {other:?}"),
        }
    }
}
