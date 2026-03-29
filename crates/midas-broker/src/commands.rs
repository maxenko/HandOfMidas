use midas_core::SecurityType;
use uuid::Uuid;

/// Commands sent from the UI to the broker engine via mpsc channel.
///
/// The engine receives these on `mpsc::Receiver<BrokerCommand>` and
/// dispatches to the appropriate handler. Commands are fire-and-forget;
/// responses come back as [`BrokerEvent`](crate::events::BrokerEvent)s on
/// the broadcast channels.
#[derive(Debug)]
pub enum BrokerCommand {
    // ── Connection ──────────────────────────────────────────────────────────
    /// Initiate connection to TWS / IB Gateway.
    Connect,
    /// Gracefully disconnect from TWS / IB Gateway.
    Disconnect,
    /// Force a reconnection cycle.
    Reconnect,

    // ── Orders ──────────────────────────────────────────────────────────────
    /// Create a new order in local state (not yet submitted to IB).
    CreateOrder(CreateOrderParams),
    /// Submit a locally-created order to IB.
    ActivateOrder { order_id: Uuid },
    /// Pull an active order back from IB (cancel + return to local staging).
    DeactivateOrder { order_id: Uuid },
    /// Cancel an order at IB.
    CancelOrder { order_id: Uuid },
    /// Modify price or quantity of an existing order.
    ModifyOrder {
        order_id: Uuid,
        new_price: Option<f64>,
        new_qty: Option<f64>,
    },

    // ── Brackets ────────────────────────────────────────────────────────────
    /// Create a bracket order (entry + take-profit + stop-loss) as a single unit.
    CreateBracketOrder {
        entry: CreateOrderParams,
        take_profit_price: f64,
        stop_loss_price: f64,
    },

    // ── Market Data ─────────────────────────────────────────────────────────
    /// Subscribe to streaming L1 market data.
    SubscribeMarketData { symbol: String, con_id: i32 },
    /// Unsubscribe from streaming market data.
    UnsubscribeMarketData { symbol: String },
    /// Request historical OHLCV bars (one-shot).
    RequestHistoricalData {
        symbol: String,
        con_id: i32,
        duration: String,
        bar_size: String,
        request_id: u64,
    },

    // ── Account ─────────────────────────────────────────────────────────────
    /// Request current positions from IB.
    RequestPositions,
    /// Request account summary (balances, margin, etc.).
    RequestAccountSummary,

    // ── State Recovery ──────────────────────────────────────────────────────
    /// Request a snapshot of all tracked orders for UI synchronization.
    RequestOrderSnapshot,

    // ── System ──────────────────────────────────────────────────────────────
    /// Shut down the broker engine gracefully.
    Shutdown,
}

/// Parameters for creating a new order. Passed inside
/// [`BrokerCommand::CreateOrder`] and [`BrokerCommand::CreateBracketOrder`].
#[derive(Debug, Clone)]
pub struct CreateOrderParams {
    /// IB symbol string, e.g. "AAPL".
    pub symbol: String,
    /// IB contract ID. `None` means the engine will resolve it.
    pub con_id: Option<i32>,
    /// Security type (Stock, Option, Future, Forex).
    pub sec_type: SecurityType,
    /// Exchange routing, e.g. "SMART".
    pub exchange: String,
    /// Currency code, e.g. "USD".
    pub currency: String,
    /// Trade direction: "BUY" or "SELL".
    pub action: String,
    /// IB order type string: "MKT", "LMT", "STP", "TRAIL", etc.
    pub order_type: String,
    /// Number of shares/contracts.
    pub quantity: f64,
    /// Limit price (required for LMT orders).
    pub limit_price: Option<f64>,
    /// Stop trigger price (required for STP orders).
    pub stop_price: Option<f64>,
    /// Trailing amount in absolute price units.
    pub trail_amount: Option<f64>,
    /// Trailing amount as a percentage.
    pub trail_percent: Option<f64>,
    /// Time-in-force: "DAY", "GTC", "IOC", etc.
    pub tif: String,
    /// Allow fills outside regular trading hours.
    pub outside_rth: bool,
    /// IB algo strategy name, e.g. "Adaptive".
    pub algo_strategy: Option<String>,
    /// Algo-specific parameters as JSON.
    pub algo_params: Option<serde_json::Value>,
    /// User-defined tag for grouping / filtering.
    pub tag: Option<String>,
    /// Strategy name that originated this order.
    pub strategy: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_params() -> CreateOrderParams {
        CreateOrderParams {
            symbol: "AAPL".to_string(),
            con_id: Some(265598),
            sec_type: SecurityType::Stock,
            exchange: "SMART".to_string(),
            currency: "USD".to_string(),
            action: "BUY".to_string(),
            order_type: "LMT".to_string(),
            quantity: 100.0,
            limit_price: Some(175.00),
            stop_price: None,
            trail_amount: None,
            trail_percent: None,
            tif: "DAY".to_string(),
            outside_rth: false,
            algo_strategy: None,
            algo_params: None,
            tag: Some("test".to_string()),
            strategy: None,
        }
    }

    #[test]
    fn create_order_params_debug() {
        let params = sample_params();
        let dbg = format!("{params:?}");
        assert!(dbg.contains("AAPL"));
        assert!(dbg.contains("LMT"));
    }

    #[test]
    fn create_order_params_clone() {
        let params = sample_params();
        let cloned = params.clone();
        assert_eq!(cloned.symbol, "AAPL");
        assert_eq!(cloned.quantity, 100.0);
    }

    #[test]
    fn broker_command_debug() {
        let cmd = BrokerCommand::Shutdown;
        assert_eq!(format!("{cmd:?}"), "Shutdown");

        let cmd = BrokerCommand::CancelOrder {
            order_id: uuid::Uuid::nil(),
        };
        let dbg = format!("{cmd:?}");
        assert!(dbg.contains("CancelOrder"));
    }

    #[test]
    fn bracket_order_command() {
        let cmd = BrokerCommand::CreateBracketOrder {
            entry: sample_params(),
            take_profit_price: 200.0,
            stop_loss_price: 160.0,
        };
        match cmd {
            BrokerCommand::CreateBracketOrder {
                take_profit_price,
                stop_loss_price,
                ..
            } => {
                assert!((take_profit_price - 200.0).abs() < f64::EPSILON);
                assert!((stop_loss_price - 160.0).abs() < f64::EPSILON);
            }
            _ => panic!("expected CreateBracketOrder"),
        }
    }
}
