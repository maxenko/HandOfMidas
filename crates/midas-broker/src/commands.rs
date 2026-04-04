use uuid::Uuid;

use crate::orders::bracket::MarketBracketParams;

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

    // ── Orders ──────────────────────────────────────────────────────────────
    /// Cancel an order at IB.
    CancelOrder { order_id: Uuid },
    /// Modify price or quantity of an existing order.
    ModifyOrder {
        order_id: Uuid,
        new_price: Option<f64>,
        new_qty: Option<f64>,
    },

    // ── Brackets ────────────────────────────────────────────────────────────
    /// Create and immediately submit a market order bracket.
    /// Builds the bracket, persists all legs, and submits to the broker.
    CreateMarketBracket(MarketBracketParams),
    /// Cancel an entire bracket (parent + all children) as a unit.
    CancelBracket { parent_id: Uuid },
    /// Modify a bracket leg's price without affecting other legs.
    ModifyBracketLeg { order_id: Uuid, new_price: f64 },

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

#[cfg(test)]
mod tests {
    use super::*;

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
    fn market_bracket_command() {
        use crate::orders::bracket::MarketBracketParams;
        let cmd = BrokerCommand::CreateMarketBracket(MarketBracketParams {
            symbol: "AAPL".to_string(),
            con_id: None,
            sec_type: midas_core::SecurityType::Stock,
            exchange: "SMART".to_string(),
            currency: "USD".to_string(),
            action: crate::orders::types::OrderAction::Buy,
            quantity: 100.0,
            outside_rth: false,
            take_profit: None,
            stop_loss: None,
            reference_price: None,
            strategy: None,
            tags: Vec::new(),
        });
        let dbg = format!("{cmd:?}");
        assert!(dbg.contains("CreateMarketBracket"));
    }
}
