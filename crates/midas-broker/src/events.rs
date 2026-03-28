use chrono::{DateTime, Utc};
use midas_core::SymbolKey;
use uuid::Uuid;

/// Every event the broker engine can emit.
/// Sent over broadcast channels to consumers (UI, logger, strategy).
#[derive(Clone, Debug)]
pub enum BrokerEvent {
    // ── Connection ──────────────────────────────────────────────────────────
    Connected {
        server_version: i32,
    },
    Disconnected {
        reason: String,
    },
    Reconnecting {
        attempt: u32,
        next_retry_secs: u64,
    },
    Reconnected,

    // ── Orders ──────────────────────────────────────────────────────────────
    OrderCreated {
        order_id: Uuid,
    },
    OrderSubmitted {
        order_id: Uuid,
        ib_order_id: i32,
        ib_perm_id: i64,
    },
    OrderStatusChanged {
        order_id: Uuid,
        old_status: String,
        new_status: String,
        filled_qty: f64,
        remaining_qty: f64,
        avg_fill_price: f64,
    },
    OrderFilled {
        order_id: Uuid,
        ib_exec_id: String,
        shares: f64,
        price: f64,
        commission: Option<f64>,
    },
    OrderRejected {
        order_id: Uuid,
        reason: String,
    },
    OrderCancelled {
        order_id: Uuid,
        reason: String,
    },
    OrderError {
        order_id: Uuid,
        code: i32,
        message: String,
    },

    // ── Market Data: L1 ─────────────────────────────────────────────────────
    Tick {
        symbol: SymbolKey,
        bid: Option<f64>,
        ask: Option<f64>,
        last: Option<f64>,
        volume: Option<i64>,
        timestamp: DateTime<Utc>,
    },

    // ── Market Data: Bars ───────────────────────────────────────────────────
    RealtimeBar {
        symbol: SymbolKey,
        timestamp: i64,
        open: f64,
        high: f64,
        low: f64,
        close: f64,
        volume: i64,
    },
    BarClosed {
        symbol: SymbolKey,
        timestamp: i64,
        open: f64,
        high: f64,
        low: f64,
        close: f64,
        volume: i64,
    },
    BarUpdated {
        symbol: SymbolKey,
        timestamp: i64,
        open: f64,
        high: f64,
        low: f64,
        close: f64,
        volume: i64,
    },
    HistoricalDataComplete {
        request_id: u64,
        symbol: SymbolKey,
    },

    // ── Market Data: Depth ──────────────────────────────────────────────────
    DepthUpdate {
        symbol: SymbolKey,
        position: i32,
        side: DepthSide,
        price: f64,
        size: i64,
    },

    // ── Account ─────────────────────────────────────────────────────────────
    PositionUpdate {
        account: String,
        symbol: String,
        con_id: i32,
        quantity: f64,
        avg_cost: f64,
    },
    AccountValueUpdate {
        account: String,
        key: String,
        value: String,
        currency: String,
    },
    PnlUpdate {
        daily_pnl: f64,
        unrealized_pnl: f64,
        realized_pnl: f64,
    },

    // ── System ──────────────────────────────────────────────────────────────
    Warning {
        code: i32,
        message: String,
    },
    DataFarmStatus {
        farm: String,
        ok: bool,
    },
    Error {
        code: i32,
        message: String,
    },

    // ── Snapshot (for lag recovery) ─────────────────────────────────────────
    OrderSnapshot {
        orders: Vec<OrderSnapshotEntry>,
    },
}

/// Side of the order-book depth update.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DepthSide {
    Bid,
    Ask,
}

/// A point-in-time snapshot of one order, used for recovery after reconnect.
#[derive(Clone, Debug)]
pub struct OrderSnapshotEntry {
    pub order_id: Uuid,
    pub status: String,
    pub symbol: String,
    pub action: String,
    pub quantity: f64,
    pub filled_qty: f64,
    pub avg_fill_price: Option<f64>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[test]
    fn depth_side_eq() {
        assert_eq!(DepthSide::Bid, DepthSide::Bid);
        assert_ne!(DepthSide::Bid, DepthSide::Ask);
    }

    #[test]
    fn broker_event_clone() {
        let event = BrokerEvent::Connected { server_version: 176 };
        let cloned = event.clone();
        match cloned {
            BrokerEvent::Connected { server_version } => assert_eq!(server_version, 176),
            _ => panic!("expected Connected variant"),
        }
    }

    #[test]
    fn order_snapshot_entry_debug() {
        let entry = OrderSnapshotEntry {
            order_id: uuid::Uuid::nil(),
            status: "Filled".to_string(),
            symbol: "AAPL".to_string(),
            action: "BUY".to_string(),
            quantity: 100.0,
            filled_qty: 100.0,
            avg_fill_price: Some(175.50),
        };
        let dbg = format!("{entry:?}");
        assert!(dbg.contains("AAPL"));
        assert!(dbg.contains("Filled"));
    }

    #[test]
    fn tick_event_with_optional_fields() {
        let event = BrokerEvent::Tick {
            symbol: midas_core::SymbolKey {
                contract_id: 265598,
                symbol: "AAPL".to_string(),
            },
            bid: Some(175.00),
            ask: Some(175.05),
            last: None,
            volume: None,
            timestamp: Utc::now(),
        };
        match event {
            BrokerEvent::Tick { bid, ask, last, volume, .. } => {
                assert!(bid.is_some());
                assert!(ask.is_some());
                assert!(last.is_none());
                assert!(volume.is_none());
            }
            _ => panic!("expected Tick variant"),
        }
    }
}
