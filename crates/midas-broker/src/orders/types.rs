use std::fmt;
use std::str::FromStr;

use chrono::{DateTime, Utc};
use midas_core::SecurityType;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::state::OrderStatus;

// ===========================================================================
// OrderAction
// ===========================================================================

/// Direction of an order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum OrderAction {
    Buy,
    Sell,
}

impl fmt::Display for OrderAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Buy => f.write_str("BUY"),
            Self::Sell => f.write_str("SELL"),
        }
    }
}

impl FromStr for OrderAction {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "BUY" => Ok(Self::Buy),
            "SELL" => Ok(Self::Sell),
            other => Err(format!("unknown OrderAction: {other}")),
        }
    }
}

// ===========================================================================
// OrderKind
// ===========================================================================

/// IB order type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum OrderKind {
    Market,
    Limit,
    Stop,
    StopLimit,
    TrailingStop,
}

impl fmt::Display for OrderKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::Market => "MKT",
            Self::Limit => "LMT",
            Self::Stop => "STP",
            Self::StopLimit => "STP LMT",
            Self::TrailingStop => "TRAIL",
        };
        f.write_str(s)
    }
}

impl FromStr for OrderKind {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "MKT" => Ok(Self::Market),
            "LMT" => Ok(Self::Limit),
            "STP" => Ok(Self::Stop),
            "STP LMT" => Ok(Self::StopLimit),
            "TRAIL" => Ok(Self::TrailingStop),
            other => Err(format!("unknown OrderKind: {other}")),
        }
    }
}

// ===========================================================================
// TimeInForce
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

impl fmt::Display for TimeInForce {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::Day => "DAY",
            Self::Gtc => "GTC",
            Self::Ioc => "IOC",
            Self::Gtd => "GTD",
            Self::Opg => "OPG",
        };
        f.write_str(s)
    }
}

impl FromStr for TimeInForce {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "DAY" => Ok(Self::Day),
            "GTC" => Ok(Self::Gtc),
            "IOC" => Ok(Self::Ioc),
            "GTD" => Ok(Self::Gtd),
            "OPG" => Ok(Self::Opg),
            other => Err(format!("unknown TimeInForce: {other}")),
        }
    }
}

// ===========================================================================
// FillInfo
// ===========================================================================

/// Details of a single execution report from IB.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FillInfo {
    /// IB execution ID (unique per partial fill).
    pub ib_exec_id: String,
    /// Timestamp of the execution.
    pub timestamp: DateTime<Utc>,
    /// Number of shares/contracts in this fill.
    pub shares: f64,
    /// Price at which this tranche was filled.
    pub price: f64,
    /// Commission charged for this fill (may be reported later).
    pub commission: Option<f64>,
    /// Exchange where the fill occurred.
    pub exchange: Option<String>,
    /// IB side string, e.g. "BOT", "SLD".
    pub side: String,
}

// ===========================================================================
// LocalOrder
// ===========================================================================

/// The broker's local representation of an order throughout its lifecycle.
///
/// This struct owns all state.  It is persisted to SQLite and kept in an
/// in-memory map keyed by `id`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalOrder {
    // -- Identity --
    /// Local UUID (v7, time-sortable).
    pub id: Uuid,
    /// IB order ID assigned when the order is placed.
    pub ib_order_id: Option<i32>,
    /// IB permanent order ID (stable across reconnections).
    pub ib_perm_id: Option<i64>,

    // -- Contract --
    /// Trading symbol (e.g. "AAPL", "ESZ4").
    pub symbol: String,
    /// IB contract ID (resolved after contract lookup).
    pub con_id: Option<i32>,
    /// Security type (Stock, Option, Future, Forex).
    pub sec_type: SecurityType,
    /// Destination exchange.
    pub exchange: String,
    /// Currency code.
    pub currency: String,

    // -- Order parameters --
    pub action: OrderAction,
    pub order_type: OrderKind,
    pub quantity: f64,
    pub limit_price: Option<f64>,
    pub stop_price: Option<f64>,
    pub trail_amount: Option<f64>,
    pub trail_percent: Option<f64>,
    pub tif: TimeInForce,

    // -- State & grouping --
    pub status: OrderStatus,
    /// Parent order UUID for bracket legs.
    pub parent_id: Option<Uuid>,
    /// OCA (one-cancels-all) group name.
    pub oca_group: Option<String>,
    /// Role within a bracket: "PARENT", "PROFIT", "STOP".
    pub bracket_role: Option<String>,
    /// Strategy label, e.g. "momentum_scalp".
    pub strategy: Option<String>,
    /// Arbitrary tags for filtering/grouping.
    pub tags: Vec<String>,

    // -- Algo --
    /// IB algo strategy name, e.g. "Adaptive", "TWAP".
    pub algo_strategy: Option<String>,
    /// Algo-specific parameters as JSON.
    pub algo_params: Option<serde_json::Value>,

    // -- Execution flags --
    pub outside_rth: bool,

    // -- Fill tracking --
    pub filled_qty: f64,
    pub remaining_qty: f64,
    pub avg_fill_price: Option<f64>,
    pub last_fill_price: Option<f64>,
    pub commission: Option<f64>,

    // -- Activation tracking --
    /// How many times this order has been activated.
    pub activation_count: i32,
    pub last_activated_at: Option<DateTime<Utc>>,
    pub last_deactivated_at: Option<DateTime<Utc>>,

    // -- Timestamps --
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl LocalOrder {
    /// Create a new draft order with sensible defaults.
    ///
    /// The order is assigned a UUIDv7 and the `created_at`/`updated_at`
    /// timestamps are set to the current UTC time.  All other fields are
    /// defaulted (no IB IDs, no prices, DAY TIF, STK/SMART/USD).
    pub fn new_draft(
        symbol: impl Into<String>,
        action: OrderAction,
        order_type: OrderKind,
        quantity: f64,
    ) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::now_v7(),
            ib_order_id: None,
            ib_perm_id: None,

            symbol: symbol.into(),
            con_id: None,
            sec_type: SecurityType::Stock,
            exchange: "SMART".to_string(),
            currency: "USD".to_string(),

            action,
            order_type,
            quantity,
            limit_price: None,
            stop_price: None,
            trail_amount: None,
            trail_percent: None,
            tif: TimeInForce::Day,

            status: OrderStatus::Draft,
            parent_id: None,
            oca_group: None,
            bracket_role: None,
            strategy: None,
            tags: Vec::new(),

            algo_strategy: None,
            algo_params: None,

            outside_rth: false,

            filled_qty: 0.0,
            remaining_qty: quantity,
            avg_fill_price: None,
            last_fill_price: None,
            commission: None,

            activation_count: 0,
            last_activated_at: None,
            last_deactivated_at: None,

            created_at: now,
            updated_at: now,
        }
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // OrderAction
    // -----------------------------------------------------------------------

    #[test]
    fn order_action_display_fromstr() {
        assert_eq!(OrderAction::Buy.to_string(), "BUY");
        assert_eq!(OrderAction::Sell.to_string(), "SELL");
        assert_eq!("BUY".parse::<OrderAction>().unwrap(), OrderAction::Buy);
        assert_eq!("SELL".parse::<OrderAction>().unwrap(), OrderAction::Sell);
        assert!("buy".parse::<OrderAction>().is_err());
    }

    // -----------------------------------------------------------------------
    // OrderKind
    // -----------------------------------------------------------------------

    #[test]
    fn order_kind_display_fromstr() {
        let cases = [
            (OrderKind::Market, "MKT"),
            (OrderKind::Limit, "LMT"),
            (OrderKind::Stop, "STP"),
            (OrderKind::StopLimit, "STP LMT"),
            (OrderKind::TrailingStop, "TRAIL"),
        ];
        for (kind, s) in cases {
            assert_eq!(kind.to_string(), s);
            assert_eq!(s.parse::<OrderKind>().unwrap(), kind);
        }
        assert!("MARKET".parse::<OrderKind>().is_err());
    }

    // -----------------------------------------------------------------------
    // TimeInForce
    // -----------------------------------------------------------------------

    #[test]
    fn time_in_force_display_fromstr() {
        let cases = [
            (TimeInForce::Day, "DAY"),
            (TimeInForce::Gtc, "GTC"),
            (TimeInForce::Ioc, "IOC"),
            (TimeInForce::Gtd, "GTD"),
            (TimeInForce::Opg, "OPG"),
        ];
        for (tif, s) in cases {
            assert_eq!(tif.to_string(), s);
            assert_eq!(s.parse::<TimeInForce>().unwrap(), tif);
        }
        assert!("gtc".parse::<TimeInForce>().is_err());
    }

    // -----------------------------------------------------------------------
    // LocalOrder::new_draft
    // -----------------------------------------------------------------------

    #[test]
    fn new_draft_defaults() {
        let order = LocalOrder::new_draft("AAPL", OrderAction::Buy, OrderKind::Limit, 100.0);

        assert_eq!(order.symbol, "AAPL");
        assert_eq!(order.action, OrderAction::Buy);
        assert_eq!(order.order_type, OrderKind::Limit);
        assert_eq!(order.quantity, 100.0);
        assert_eq!(order.remaining_qty, 100.0);
        assert_eq!(order.filled_qty, 0.0);
        assert_eq!(order.status, OrderStatus::Draft);
        assert_eq!(order.sec_type, SecurityType::Stock);
        assert_eq!(order.exchange, "SMART");
        assert_eq!(order.currency, "USD");
        assert_eq!(order.tif, TimeInForce::Day);
        assert!(!order.outside_rth);
        assert!(order.ib_order_id.is_none());
        assert!(order.limit_price.is_none());
        assert_eq!(order.activation_count, 0);
        assert!(order.tags.is_empty());
        // UUIDv7 should have version nibble = 7.
        assert_eq!(order.id.get_version_num(), 7);
    }

    #[test]
    fn new_draft_timestamps_are_recent() {
        let before = Utc::now();
        let order = LocalOrder::new_draft("SPY", OrderAction::Sell, OrderKind::Market, 50.0);
        let after = Utc::now();

        assert!(order.created_at >= before && order.created_at <= after);
        assert_eq!(order.created_at, order.updated_at);
    }

    #[test]
    fn local_order_serde_round_trip() {
        let order = LocalOrder::new_draft("TSLA", OrderAction::Buy, OrderKind::StopLimit, 10.0);
        let json = serde_json::to_string(&order).unwrap();
        let restored: LocalOrder = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.id, order.id);
        assert_eq!(restored.symbol, "TSLA");
        assert_eq!(restored.order_type, OrderKind::StopLimit);
        assert_eq!(restored.status, OrderStatus::Draft);
    }
}
