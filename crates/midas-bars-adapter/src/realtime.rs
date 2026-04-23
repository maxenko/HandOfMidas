//! `RealtimeBarStream` → `BarStream<Candle>` adapter.
//!
//! Wraps a broker-core [`RealtimeBarStream`] handle (which yields
//! `Result<Arc<Bar>, broadcast::error::RecvError>`) as a
//! [`midas_stream::BarStream`]. The adapter:
//!
//! - Captures `(Symbol, calendar, period)` at construction (meta is
//!   immutable for the lifetime of the stream).
//! - Converts every received `Bar` into a `Candle` via
//!   [`crate::bar_to_candle`].
//! - Treats `RecvError::Lagged` as a transient warning-class signal:
//!   skip the skipped-count marker and continue — the application has
//!   back-pressure tolerances and losing a bar is strictly better than
//!   closing the stream. `RecvError::Closed` surfaces as `None` from
//!   `next`.
//! - Reports `StreamError::NotSeekable` from `snapshot()` — live
//!   broadcast streams cannot rewind. Historical queries go through
//!   the composite builder.

use async_trait::async_trait;
use midas_bars::{Candle, Symbol};
use midas_broker_core::provider::RealtimeBarStream;
use midas_calendar::{BarPeriod, ExchangeCalendar};
use midas_stream::{BarStream, BarStreamMeta, StreamError, TimeRange};
use tokio::sync::broadcast::error::RecvError;

use crate::candle::bar_to_candle;

/// Live-tail bar stream built over a broker-core broadcast handle.
pub struct RealtimeBarAdapter {
    meta: BarStreamMeta,
    inner: RealtimeBarStream,
    period: BarPeriod,
}

impl std::fmt::Debug for RealtimeBarAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RealtimeBarAdapter")
            .field("meta", &self.meta)
            .field("period", &self.period)
            .finish()
    }
}

impl RealtimeBarAdapter {
    /// Build an adapter. `period` is the `BarPeriod` this live stream is
    /// pinned to (it will tag every emitted `Candle`). For IB sources
    /// the period is typically `BarPeriod::Clock(Seconds(5))` matching
    /// the 5s realtime-bar wire shape; aggregators synthesise larger
    /// periods upstream.
    pub fn new(
        inner: RealtimeBarStream,
        symbol: Symbol,
        calendar: &'static dyn ExchangeCalendar,
        period: BarPeriod,
    ) -> Self {
        Self {
            meta: BarStreamMeta::new(symbol, calendar, period),
            inner,
            period,
        }
    }

    /// Borrow the inner handle (diagnostics only).
    pub fn inner_req_id(&self) -> midas_broker_core::market_data::ReqId {
        self.inner.req_id()
    }
}

#[async_trait]
impl BarStream for RealtimeBarAdapter {
    fn meta(&self) -> &BarStreamMeta {
        &self.meta
    }

    async fn next(&mut self) -> Option<Candle> {
        // Loop over transient Lagged markers; surface Closed as None.
        loop {
            match self.inner.next().await {
                Ok(bar_arc) => {
                    let candle =
                        bar_to_candle(&bar_arc, self.meta.symbol, self.meta.calendar, self.period);
                    return Some(candle);
                }
                Err(RecvError::Lagged(_)) => {
                    // Skip the marker and try again; the broadcast
                    // channel has advanced past our cursor, but the
                    // next `recv()` will yield the newest available
                    // bar. Losing a bar is strictly preferable to
                    // dropping the whole stream.
                    continue;
                }
                Err(RecvError::Closed) => return None,
            }
        }
    }

    async fn snapshot(&mut self, _range: TimeRange) -> Result<Vec<Candle>, StreamError> {
        // Live broadcast streams have no backlog; historical queries
        // must go through a separate seekable source (FixtureBarStream
        // or a cold historical fetch) composed via HistoryThenLive.
        Err(StreamError::NotSeekable)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, OnceLock};

    use chrono::{TimeZone, Timelike};
    use midas_broker_core::market_data::{Bar, BarCompleteness, SymbolKey, Timeframe};
    use midas_broker_core::market_data::{MarketDataError, ReqId};
    use midas_calendar::{xnys, BarPeriod, SessionKind};
    use tokio::sync::broadcast;

    use super::*;

    fn make_adapter(period: BarPeriod) -> (broadcast::Sender<Arc<Bar>>, RealtimeBarAdapter) {
        let (tx, rx) = broadcast::channel::<Arc<Bar>>(16);
        let last_error: Arc<OnceLock<MarketDataError>> = Arc::new(OnceLock::new());
        let inner = RealtimeBarStream::new(ReqId(1), rx, last_error, Box::new(|| {}));
        let cal = xnys();
        let sym = midas_bars::Symbol::new("AAPL", cal.id());
        let adapter = RealtimeBarAdapter::new(inner, sym, cal, period);
        (tx, adapter)
    }

    fn sample_bar(ts_h: u32, ts_m: u32) -> Bar {
        let open = chrono::Utc
            .with_ymd_and_hms(2024, 1, 17, ts_h, ts_m, 0)
            .unwrap();
        Bar {
            symbol: SymbolKey {
                contract_id: 265598,
                symbol: "AAPL".into(),
            },
            timeframe: Timeframe::M1,
            ts_open: open,
            ts_close: open + chrono::Duration::minutes(1),
            o: 100.0,
            h: 101.0,
            l: 99.5,
            c: 100.5,
            volume: 1_000,
            trade_count: 10,
            wap: None,
            completeness: BarCompleteness::Completed,
        }
    }

    #[tokio::test]
    async fn next_maps_bar_to_candle() {
        let (tx, mut adapter) = make_adapter(BarPeriod::m1());
        // 2024-01-17 15:00 UTC = 10:00 ET = RTH.
        let bar = sample_bar(15, 0);
        tx.send(Arc::new(bar.clone())).unwrap();
        let c = adapter.next().await.expect("candle");
        assert_eq!(c.session.kind(), SessionKind::Regular);
        assert_eq!(c.window.open, bar.ts_open);
        assert_eq!(c.o, 100.0);
        assert_eq!(c.period, BarPeriod::m1());
    }

    #[tokio::test]
    async fn next_returns_none_when_channel_closed() {
        let (tx, mut adapter) = make_adapter(BarPeriod::m1());
        drop(tx);
        assert!(adapter.next().await.is_none());
    }

    #[tokio::test]
    async fn next_skips_lagged_marker_and_recovers() {
        // Small buffer to force a Lagged error.
        let (tx, rx) = broadcast::channel::<Arc<Bar>>(2);
        let last_error: Arc<OnceLock<MarketDataError>> = Arc::new(OnceLock::new());
        let inner = RealtimeBarStream::new(ReqId(1), rx, last_error, Box::new(|| {}));
        let cal = xnys();
        let sym = midas_bars::Symbol::new("AAPL", cal.id());
        let mut adapter = RealtimeBarAdapter::new(inner, sym, cal, BarPeriod::m1());

        // Overflow the channel so subsequent recv sees Lagged. With a
        // buffer of 2, the oldest retained slot after overflow is the
        // second-to-last push.
        for m in 0..10 {
            let bar = sample_bar(15, m);
            let _ = tx.send(Arc::new(bar));
        }
        // After Lagged, broadcast::recv yields the oldest still-buffered
        // item. The adapter skips the Lagged marker and returns that
        // candle. We only assert it's a valid, in-range minute rather
        // than pinning the exact value — tokio's broadcast buffer keeps
        // `capacity` slots but the "oldest after lag" index isn't a
        // public API guarantee.
        let c = adapter.next().await.expect("candle after Lagged");
        let minute = c.window.open.minute();
        assert!(minute < 10, "minute {minute} in range [0, 10)");
    }

    #[tokio::test]
    async fn snapshot_returns_not_seekable() {
        let (_tx, mut adapter) = make_adapter(BarPeriod::m1());
        let from = chrono::Utc.with_ymd_and_hms(2024, 1, 17, 0, 0, 0).unwrap();
        let to = chrono::Utc.with_ymd_and_hms(2024, 1, 18, 0, 0, 0).unwrap();
        let range = TimeRange::new(from, to).unwrap();
        let err = adapter.snapshot(range).await.unwrap_err();
        assert_eq!(err, StreamError::NotSeekable);
    }

    #[tokio::test]
    async fn meta_is_pinned_at_construction() {
        let (_tx, adapter) = make_adapter(BarPeriod::m1());
        let m = adapter.meta();
        assert_eq!(m.symbol.ticker(), "AAPL");
        assert_eq!(m.calendar.id(), xnys().id());
        assert_eq!(m.period, BarPeriod::m1());
    }
}
