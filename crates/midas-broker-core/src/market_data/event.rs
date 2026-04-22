//! [`MarketEvent`] — the canonical unified event enum.
//!
//! The router's internal plumbing speaks `MarketEvent`. Public consumer
//! APIs expose narrower handle types (`SubscriptionHandle<Tick>`,
//! `watch::Receiver<Quote>`, …), but everything that crosses the
//! upstream-decode → router boundary threads through one of these
//! variants.
//!
//! Per M-16, historical data is delivered as three distinct variants:
//! [`MarketEvent::Historical`] for the initial bulk payload,
//! [`MarketEvent::HistoricalDataEnd`] as the seam marker, and
//! [`MarketEvent::HistoricalUpdate`] for each trailing live bar while
//! `keep_up_to_date` is in effect.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::SymbolKey;

use super::bar::{Bar, Timeframe};
use super::connection::ConnectionState;
use super::error::ErrorCode;
use super::farm::FarmStatus;
use super::req_id::ReqId;
use super::tick::Tick;

/// Every event the router can observe or emit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum MarketEvent {
    /// A single tick (any [`TickKind`]).
    ///
    /// [`TickKind`]: super::tick::TickKind
    Tick(Tick),
    /// A finalised (or current) [`Bar`] from the aggregator or RT-bar
    /// fan-out.
    Bar(Bar),
    /// Farm-state transition (see [`FarmStatus`]).
    FarmStatus(FarmStatus),
    /// Connection-state transition.
    ConnectionState(ConnectionState),
    /// IB emitted `nextValidId` — order placement is now permitted
    /// (M-14).
    OrderingReady {
        /// Next valid IB order id.
        next_order_id: i32,
    },
    /// Router accepted a new subscription.
    SubscriptionAccepted {
        /// Wire request id.
        req_id: ReqId,
        /// Symbol that was subscribed.
        symbol: SymbolKey,
        /// Stream flavour.
        kind: StreamKind,
    },
    /// Subscription ended for some reason.
    SubscriptionEnded {
        /// Wire request id that ended.
        req_id: ReqId,
        /// Why it ended.
        reason: EndReason,
    },
    /// One-shot historical payload (the bulk bars).
    Historical(Vec<Bar>),
    /// Historical data end marker (the `t_server` seam).
    HistoricalDataEnd {
        /// Wire request id for the historical fetch.
        req_id: ReqId,
        /// First bar timestamp in the payload.
        first_ts: DateTime<Utc>,
        /// Last bar timestamp in the payload — the
        /// `history ≤ t_server < live` seam boundary.
        last_ts: DateTime<Utc>,
    },
    /// A trailing live bar while `keep_up_to_date = true` (rust-ibapi
    /// 2.10 "update" event).
    HistoricalUpdate(Bar),
    /// An error associated with this request (or connection-wide).
    Error {
        /// Originating request id, if any.
        req_id: Option<ReqId>,
        /// Classified code.
        code: ErrorCode,
        /// Raw IB message text, preserved for logs.
        message: String,
    },
}

/// Kind of stream referenced by subscription lifecycle events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum StreamKind {
    /// `reqMktData` tick stream.
    Tick,
    /// `reqTickByTickData` tick stream.
    TickByTick,
    /// `reqRealTimeBars` 5-second bars.
    RealtimeBar,
    /// Aggregator-produced bar stream at the given timeframe.
    Bar(Timeframe),
    /// `historical_data` or `historical_data_streaming`.
    Historical,
}

/// Why a subscription ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EndReason {
    /// Consumer dropped the handle; router issued a cancel.
    Cancelled,
    /// Underlying connection was lost.
    Disconnected,
    /// IB farm hosting this sub went down.
    FarmDropped,
    /// Upstream error forced termination.
    Error,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::market_data::bar::BarCompleteness;
    use crate::market_data::tick::{TickAttributes, TickKind, TickType, TickValue};
    use chrono::TimeZone;

    fn sample_tick() -> Tick {
        Tick {
            symbol: SymbolKey {
                contract_id: 265598,
                symbol: "AAPL".into(),
            },
            req_id: ReqId(1),
            kind: TickKind::Price,
            tick_type: TickType::Last,
            value: TickValue::Price(100.0),
            attrs: TickAttributes::default(),
            ts: Utc.with_ymd_and_hms(2026, 1, 2, 14, 30, 0).unwrap(),
        }
    }

    fn sample_bar() -> Bar {
        Bar {
            symbol: SymbolKey {
                contract_id: 265598,
                symbol: "AAPL".into(),
            },
            timeframe: Timeframe::M1,
            ts_open: Utc.with_ymd_and_hms(2026, 1, 2, 14, 30, 0).unwrap(),
            ts_close: Utc.with_ymd_and_hms(2026, 1, 2, 14, 31, 0).unwrap(),
            o: 100.0,
            h: 101.0,
            l: 99.5,
            c: 100.5,
            volume: 10_000,
            trade_count: 20,
            wap: Some(100.3),
            completeness: BarCompleteness::Completed,
        }
    }

    #[test]
    fn market_event_tick_serde_roundtrip() {
        let e = MarketEvent::Tick(sample_tick());
        let json = serde_json::to_string(&e).unwrap();
        let back: MarketEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(e, back);
    }

    #[test]
    fn market_event_bar_serde_roundtrip() {
        let e = MarketEvent::Bar(sample_bar());
        let json = serde_json::to_string(&e).unwrap();
        let back: MarketEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(e, back);
    }

    #[test]
    fn market_event_historical_end_serde_roundtrip() {
        let e = MarketEvent::HistoricalDataEnd {
            req_id: ReqId(2),
            first_ts: Utc.with_ymd_and_hms(2026, 1, 2, 9, 30, 0).unwrap(),
            last_ts: Utc.with_ymd_and_hms(2026, 1, 2, 16, 0, 0).unwrap(),
        };
        let json = serde_json::to_string(&e).unwrap();
        let back: MarketEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(e, back);
    }

    #[test]
    fn market_event_error_serde_roundtrip() {
        let e = MarketEvent::Error {
            req_id: Some(ReqId(3)),
            code: ErrorCode::NoMarketDataPermission,
            message: "No market data permissions for ISLAND STK".into(),
        };
        let json = serde_json::to_string(&e).unwrap();
        let back: MarketEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(e, back);
    }

    #[test]
    fn stream_kind_variants_roundtrip() {
        for kind in [
            StreamKind::Tick,
            StreamKind::TickByTick,
            StreamKind::RealtimeBar,
            StreamKind::Bar(Timeframe::M5),
            StreamKind::Historical,
        ] {
            let json = serde_json::to_string(&kind).unwrap();
            let back: StreamKind = serde_json::from_str(&json).unwrap();
            assert_eq!(kind, back);
        }
    }

    #[test]
    fn end_reason_variants_roundtrip() {
        for r in [
            EndReason::Cancelled,
            EndReason::Disconnected,
            EndReason::FarmDropped,
            EndReason::Error,
        ] {
            let json = serde_json::to_string(&r).unwrap();
            let back: EndReason = serde_json::from_str(&json).unwrap();
            assert_eq!(r, back);
        }
    }

    #[test]
    fn market_event_debug_does_not_panic_on_nan() {
        let mut tick = sample_tick();
        tick.value = TickValue::Price(f64::NAN);
        let e = MarketEvent::Tick(tick);
        let _ = format!("{e:?}");
    }
}
