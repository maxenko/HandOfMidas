//! `build_history_then_live` — stitch a legacy
//! `MarketDataSource::historical_bars` response + a
//! `subscribe_realtime_bars` tail into a single
//! [`HistoryThenLive<FixtureBarStream, RealtimeBarAdapter>`].
//!
//! MVP restriction: only `BarPeriod::Clock` variants with a legacy
//! `Timeframe` counterpart are accepted. Session/Calendar periods need
//! the session-aware aggregator which lands in slice S7. Invalid
//! periods surface as [`AdapterError::NoTimeframeMapping`] without
//! touching the provider.

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use midas_broker_core::market_data::{IbDuration, WhatToShow};
use midas_broker_core::provider::MarketDataSource;
use midas_calendar::BarPeriod;
use midas_stream::{BarStreamMeta, FixtureBarStream, HistoryThenLive};

use crate::error::AdapterError;
use crate::historical::historical_bars_to_candles;
use crate::period::period_to_timeframe;
use crate::realtime::RealtimeBarAdapter;
use crate::resolver::SymbolResolver;
use crate::timeout::BROKER_CALL_TIMEOUT;

/// Concrete alias for the composite type returned by
/// [`build_history_then_live`]. Exposed so callers can hold one without
/// juggling the `<H, L>` generics themselves.
pub type HistoryThenLiveAdapter = HistoryThenLive<FixtureBarStream, RealtimeBarAdapter>;

/// Build a history-then-live composite `BarStream` for a ticker.
///
/// Steps:
/// 1. Resolve `ticker` via the supplied [`SymbolResolver`] — obtains
///    the `Symbol`, calendar, and provider `contract_id`.
/// 2. Map `period` → legacy `Timeframe` via
///    [`period_to_timeframe`]. MVP-restricted to clock-interval
///    periods; session/calendar periods return `NoTimeframeMapping`.
/// 3. Call `source.historical_bars` for the cold payload; convert to
///    `Vec<Candle>`; wrap in a [`FixtureBarStream`].
/// 4. Call `source.subscribe_realtime_bars` for the live tail; wrap in
///    a [`RealtimeBarAdapter`].
/// 5. Return a [`HistoryThenLive`] with the same `(symbol, calendar,
///    period)` metadata on both legs — the seam dedup will suppress
///    any live bar whose `window.open <= last_ts`.
///
/// `what_to_show` is an [`Option`]: `None` defaults to
/// [`WhatToShow::Trades`], matching IB's default for equities and
/// crypto spot.
#[allow(clippy::too_many_arguments)]
pub async fn build_history_then_live(
    source: Arc<dyn MarketDataSource>,
    resolver: &dyn SymbolResolver,
    ticker: &str,
    period: BarPeriod,
    history_end: DateTime<Utc>,
    history_duration: IbDuration,
    use_rth: bool,
    what_to_show: Option<WhatToShow>,
) -> Result<HistoryThenLiveAdapter, AdapterError> {
    build_history_then_live_with_timeout(
        source,
        resolver,
        ticker,
        period,
        history_end,
        history_duration,
        use_rth,
        what_to_show,
        BROKER_CALL_TIMEOUT,
    )
    .await
}

/// Timeout-parameterised variant of [`build_history_then_live`]. Tests
/// pass a short `Duration` to exercise the stall path without waiting
/// the production default.
#[allow(clippy::too_many_arguments)]
pub async fn build_history_then_live_with_timeout(
    source: Arc<dyn MarketDataSource>,
    resolver: &dyn SymbolResolver,
    ticker: &str,
    period: BarPeriod,
    history_end: DateTime<Utc>,
    history_duration: IbDuration,
    use_rth: bool,
    what_to_show: Option<WhatToShow>,
    timeout: Duration,
) -> Result<HistoryThenLiveAdapter, AdapterError> {
    let resolved = resolver.resolve(ticker)?;
    let timeframe = period_to_timeframe(period)?;
    let what = what_to_show.unwrap_or(WhatToShow::Trades);

    let symbol_key = midas_broker_core::market_data::SymbolKey {
        contract_id: resolved.contract_id,
        symbol: ticker.to_string(),
    };

    // 3. Cold payload — bounded by the adapter timeout so a stalled
    // provider cannot hang the UI.
    let hist_fut = source.historical_bars(
        &symbol_key,
        resolved.contract_id,
        history_end,
        history_duration,
        timeframe,
        what,
        use_rth,
    );
    let hist = match tokio::time::timeout(timeout, hist_fut).await {
        Ok(res) => res?,
        Err(_) => {
            tracing::warn!(
                ticker,
                secs = timeout.as_secs(),
                "build_history_then_live: historical_bars timed out",
            );
            return Err(AdapterError::Timeout {
                op: "historical_bars",
                secs: timeout.as_secs(),
            });
        }
    };

    let candles = historical_bars_to_candles(&hist, resolved.symbol, resolved.calendar, period);
    let meta = BarStreamMeta::new(resolved.symbol, resolved.calendar, period);
    let fixture = FixtureBarStream::new(meta.clone(), candles)?;

    // 4. Live tail — also bounded.
    let rt_fut = source.subscribe_realtime_bars(&symbol_key, resolved.contract_id, what);
    let rt = match tokio::time::timeout(timeout, rt_fut).await {
        Ok(res) => res?,
        Err(_) => {
            tracing::warn!(
                ticker,
                secs = timeout.as_secs(),
                "build_history_then_live: subscribe_realtime_bars timed out",
            );
            return Err(AdapterError::Timeout {
                op: "subscribe_realtime_bars",
                secs: timeout.as_secs(),
            });
        }
    };
    let live = RealtimeBarAdapter::new(rt, resolved.symbol, resolved.calendar, period);

    // 5. Composite.
    Ok(HistoryThenLive::new(meta, fixture, live))
}
