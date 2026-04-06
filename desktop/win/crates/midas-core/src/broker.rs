//! Broker bridge types for the desktop workspace.
//!
//! These types mirror their counterparts in `crates/midas-broker/src/orders/`.
//! The desktop workspace cannot depend on midas-broker (which depends on ibapi).
//! Changes to either side must be kept in sync manually.

use serde::{Deserialize, Serialize};

// ===========================================================================
// MIRROR OF: crates/midas-broker/src/orders/types.rs::OrderAction
// ===========================================================================

/// Direction of an order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum OrderAction {
    Buy,
    Sell,
}

impl std::fmt::Display for OrderAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Buy => f.write_str("BUY"),
            Self::Sell => f.write_str("SELL"),
        }
    }
}

// ===========================================================================
// MIRROR OF: crates/midas-broker/src/orders/types.rs::TimeInForce
// ===========================================================================

/// Time-in-force qualifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TimeInForce {
    Day,
    Gtc,
    Ioc,
    Gtd,
    Opg,
}

// ===========================================================================
// MIRROR OF: crates/midas-broker/src/orders/types.rs::OrderKind
// ===========================================================================

/// Order type (desktop mirror of broker's `OrderKind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EntryKind {
    Market,
    Limit,
    Stop,
    StopLimit,
}

// ===========================================================================
// MIRROR OF: crates/midas-broker/src/orders/bracket.rs::BracketParams
// ===========================================================================

/// Parameters for creating an order bracket (desktop mirror).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BracketParams {
    pub symbol: String,
    pub con_id: Option<i32>,
    pub sec_type: crate::SecurityType,
    pub exchange: String,
    pub currency: String,
    pub action: OrderAction,
    pub quantity: f64,
    pub outside_rth: bool,
    pub take_profit: Option<TakeProfitParams>,
    pub stop_loss: Option<StopLossParams>,
    pub reference_price: Option<f64>,
    pub strategy: Option<String>,
    pub tags: Vec<String>,
    /// Entry order type.
    pub entry_kind: EntryKind,
    /// Entry limit price (for Limit and StopLimit). None for Market/Stop.
    pub entry_price: Option<f64>,
    /// Entry stop trigger price (for Stop and StopLimit). None for Market/Limit.
    pub entry_stop_price: Option<f64>,
}

/// Take profit configuration (desktop mirror).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TakeProfitParams {
    pub price: f64,
    pub tif: Option<TimeInForce>,
}

/// Stop loss configuration (desktop mirror).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StopLossParams {
    pub stop_price: f64,
    pub limit_price: Option<f64>,
    pub tif: Option<TimeInForce>,
}

// ===========================================================================
// MIRROR OF: crates/midas-broker/src/orders/bracket.rs::BracketLifecycleStatus
// ===========================================================================

/// Bracket lifecycle status (desktop mirror).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BracketLifecycleStatus {
    Submitted,
    EntryFilled,
    TakeProfitHit,
    StopLossHit,
    Cancelled,
    Rejected,
    Error,
    Closed,
}

// ===========================================================================
// MIRROR OF: crates/midas-broker/src/client.rs::PositionRecord
// ===========================================================================

/// A single position as reported by the broker (desktop mirror).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PositionRecord {
    pub symbol: String,
    pub quantity: f64,
    pub avg_cost: f64,
}

// ===========================================================================
// MIRROR OF: crates/midas-broker/src/client.rs::AccountSummary
// ===========================================================================

/// Snapshot of account values (desktop mirror).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AccountSummary {
    pub cash_balance: f64,
    pub unrealized_pnl: f64,
    pub realized_pnl: f64,
}

// ===========================================================================
// Broker Events (desktop-side)
// ===========================================================================

/// Bracket events received by the desktop app from the broker engine.
#[derive(Debug, Clone)]
pub enum BracketEvent {
    Created {
        parent_id: uuid::Uuid,
        take_profit_id: Option<uuid::Uuid>,
        stop_loss_id: Option<uuid::Uuid>,
        symbol: String,
        action: OrderAction,
        quantity: f64,
    },
    StatusChanged {
        parent_id: uuid::Uuid,
        status: BracketLifecycleStatus,
        entry_fill_price: Option<f64>,
    },
}

// ===========================================================================
// OrderBroker trait
// ===========================================================================

/// Trait for broker order operations. The desktop workspace defines the
/// interface; the root workspace provides the implementation.
pub trait OrderBroker: Send + Sync {
    /// Connection info
    fn name(&self) -> &str;
    fn is_connected(&self) -> bool;

    /// Create and submit a bracket order.
    fn create_bracket(&self, params: BracketParams) -> Result<(), String>;

    /// Cancel an entire bracket.
    fn cancel_bracket(&self, parent_id: uuid::Uuid) -> Result<(), String>;

    /// Modify a bracket leg's price.
    fn modify_bracket_leg(&self, order_id: uuid::Uuid, new_price: f64) -> Result<(), String>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn order_action_display() {
        assert_eq!(OrderAction::Buy.to_string(), "BUY");
        assert_eq!(OrderAction::Sell.to_string(), "SELL");
    }

    #[test]
    fn bracket_params_serde_round_trip() {
        let params = BracketParams {
            symbol: "AAPL".to_string(),
            con_id: Some(265598),
            sec_type: crate::SecurityType::Stock,
            exchange: "SMART".to_string(),
            currency: "USD".to_string(),
            action: OrderAction::Buy,
            quantity: 100.0,
            outside_rth: false,
            take_profit: Some(TakeProfitParams {
                price: 192.0,
                tif: None,
            }),
            stop_loss: Some(StopLossParams {
                stop_price: 182.0,
                limit_price: None,
                tif: None,
            }),
            reference_price: Some(185.50),
            strategy: None,
            tags: Vec::new(),
            entry_kind: EntryKind::Market,
            entry_price: None,
            entry_stop_price: None,
        };
        let json = serde_json::to_string(&params).unwrap();
        let decoded: BracketParams = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.symbol, "AAPL");
        assert_eq!(decoded.quantity, 100.0);
    }

    #[test]
    fn lifecycle_status_eq() {
        assert_eq!(
            BracketLifecycleStatus::Submitted,
            BracketLifecycleStatus::Submitted
        );
        assert_ne!(
            BracketLifecycleStatus::Submitted,
            BracketLifecycleStatus::Cancelled
        );
    }
}
