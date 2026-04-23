//! [`SessionedBarStream`] — `BarStream<Candle>` driven by a
//! [`SessionedBarAggregator`] plus an `mpsc::Receiver<Arc<Tick>>`.
//!
//! The stream owns both: ticks come in over the mpsc, get folded by the
//! aggregator, and candles drain out of `BarStream::next`. `Rollover`
//! outputs yield two candles in sequence via an internal `pending`
//! queue so the caller sees the closed-then-opened pair on consecutive
//! `next()` awaits.
//!
//! Live-only: `snapshot(range)` returns [`StreamError::NotSeekable`].
//! Historical is served via a separate seekable source composed with
//! [`HistoryThenLive`](midas_stream::HistoryThenLive).

use std::collections::VecDeque;
use std::sync::Arc;

use async_trait::async_trait;
use midas_bars::Candle;
use midas_broker_core::market_data::Tick;
use midas_stream::{BarStream, BarStreamMeta, StreamError, TimeRange};
use tokio::sync::mpsc;

use super::core::{AggregatorOutput, SessionedBarAggregator};

/// Live `BarStream<Candle>` driven by tick inputs through a
/// [`SessionedBarAggregator`].
pub struct SessionedBarStream {
    agg: SessionedBarAggregator,
    rx: mpsc::Receiver<Arc<Tick>>,
    pending: VecDeque<Candle>,
    meta: BarStreamMeta,
}

impl std::fmt::Debug for SessionedBarStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionedBarStream")
            .field("meta", &self.meta)
            .field("pending_len", &self.pending.len())
            .finish()
    }
}

impl SessionedBarStream {
    /// Build a stream over the supplied aggregator and tick receiver.
    /// The stream's [`BarStreamMeta`] is derived from the aggregator's
    /// config — one source of truth for `(symbol, calendar, period)`.
    pub fn new(agg: SessionedBarAggregator, rx: mpsc::Receiver<Arc<Tick>>) -> Self {
        let meta = BarStreamMeta::new(agg.symbol(), agg.calendar(), agg.period());
        Self {
            agg,
            rx,
            pending: VecDeque::new(),
            meta,
        }
    }

    /// Borrow the aggregator (diagnostics / integration tests only).
    pub fn aggregator(&self) -> &SessionedBarAggregator {
        &self.agg
    }

    /// Mutably borrow the aggregator — used by integration callers that
    /// want to call `flush_if_due` on a heartbeat outside the normal
    /// `next` loop.
    pub fn aggregator_mut(&mut self) -> &mut SessionedBarAggregator {
        &mut self.agg
    }
}

#[async_trait]
impl BarStream for SessionedBarStream {
    fn meta(&self) -> &BarStreamMeta {
        &self.meta
    }

    async fn next(&mut self) -> Option<Candle> {
        loop {
            if let Some(c) = self.pending.pop_front() {
                return Some(c);
            }
            let tick = self.rx.recv().await?;
            match self.agg.accept_tick(&tick) {
                AggregatorOutput::Ignored | AggregatorOutput::Folded => continue,
                AggregatorOutput::Opened(c)
                | AggregatorOutput::Partial(c)
                | AggregatorOutput::Closed(c) => return Some(c),
                AggregatorOutput::Rollover { closed, opened } => {
                    self.pending.push_back(*opened);
                    return Some(*closed);
                }
            }
        }
    }

    async fn snapshot(&mut self, _range: TimeRange) -> Result<Vec<Candle>, StreamError> {
        Err(StreamError::NotSeekable)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use midas_bars::Symbol;
    use midas_broker_core::market_data::{ReqId, TickAttributes, TickKind, TickType, TickValue};
    use midas_broker_core::SymbolKey;
    use midas_calendar::{xnys, BarPeriod};
    use midas_clock::SystemClock;

    use crate::aggregator::config::AggregatorConfig;

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

    fn build() -> (mpsc::Sender<Arc<Tick>>, SessionedBarStream) {
        let (tx, rx) = mpsc::channel::<Arc<Tick>>(16);
        let cal = xnys();
        let sym = Symbol::new("AAPL", cal.id());
        let cfg = AggregatorConfig::new(sym, cal, BarPeriod::m1(), Arc::new(SystemClock))
            .with_partial_emit_rate_hz(0);
        let agg = SessionedBarAggregator::new(cfg).unwrap();
        let stream = SessionedBarStream::new(agg, rx);
        (tx, stream)
    }

    #[tokio::test]
    async fn next_yields_opened_first() {
        let (tx, mut stream) = build();
        tx.send(Arc::new(ps_tick(100.0, 10, utc(2024, 1, 17, 15, 0, 0))))
            .await
            .unwrap();
        let c = stream.next().await.unwrap();
        assert_eq!(c.o, 100.0);
        assert_eq!(c.completeness, midas_bars::Completeness::Partial);
    }

    #[tokio::test]
    async fn next_yields_pending_opened_after_rollover_without_another_tick() {
        let (tx, mut stream) = build();
        tx.send(Arc::new(ps_tick(100.0, 10, utc(2024, 1, 17, 15, 0, 30))))
            .await
            .unwrap();
        tx.send(Arc::new(ps_tick(101.0, 5, utc(2024, 1, 17, 15, 1, 5))))
            .await
            .unwrap();
        // First drain: Opened.
        let a = stream.next().await.unwrap();
        assert_eq!(a.completeness, midas_bars::Completeness::Partial);
        // Second drain: Rollover.closed.
        let b = stream.next().await.unwrap();
        assert_eq!(b.completeness, midas_bars::Completeness::Completed);
        // Third drain: Rollover.opened — from the pending queue, no
        // further tick required.
        let c = stream.next().await.unwrap();
        assert_eq!(c.completeness, midas_bars::Completeness::Partial);
        assert_eq!(c.window.open, utc(2024, 1, 17, 15, 1, 0));
    }

    #[tokio::test]
    async fn next_returns_none_on_channel_close() {
        let (tx, mut stream) = build();
        drop(tx);
        assert!(stream.next().await.is_none());
    }

    #[tokio::test]
    async fn snapshot_returns_not_seekable() {
        let (_tx, mut stream) = build();
        let r = TimeRange::new(utc(2024, 1, 17, 0, 0, 0), utc(2024, 1, 18, 0, 0, 0)).unwrap();
        let err = stream.snapshot(r).await.unwrap_err();
        assert_eq!(err, StreamError::NotSeekable);
    }

    #[tokio::test]
    async fn next_skips_ignored_ticks() {
        let (tx, mut stream) = build();
        // Non-trade tick first; stream must not yield anything off it.
        let bid = Tick {
            symbol: SymbolKey {
                contract_id: 265598,
                symbol: "AAPL".into(),
            },
            req_id: ReqId(1),
            kind: TickKind::Price,
            tick_type: TickType::Bid,
            value: TickValue::Price(99.5),
            attrs: TickAttributes::default(),
            ts: utc(2024, 1, 17, 15, 0, 0),
        };
        tx.send(Arc::new(bid)).await.unwrap();
        // Then a trade tick.
        tx.send(Arc::new(ps_tick(100.0, 10, utc(2024, 1, 17, 15, 0, 5))))
            .await
            .unwrap();
        let c = stream.next().await.unwrap();
        assert_eq!(c.o, 100.0);
    }
}
