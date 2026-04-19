//! Read-only view of engine state for scenario-expression evaluation.
//!
//! The engine implements [`ScenarioQuery`] by projecting its live state into
//! the snapshot types declared here. Tests use [`super::super::mock_engine`]'s
//! implementation which serves canned data.
//!
//! Keep this trait narrow. Expression evaluation runs on every `when:` tick
//! and must never allocate if avoidable. If a new domain type needs access,
//! add a single method here, don't broaden existing return types.

use std::fmt;
use std::time::Duration;

/// Minimal projection of an order for expression access.
///
/// Field names are intentionally plain (no `Option<T>`): absent values are
/// filled by the producing engine/mock (e.g. `limit_price = NaN` when the
/// order has no limit). The interpreter's path-lookup falls through to
/// `Value::Null` for unknown fields so scenarios stay readable.
#[derive(Clone, Debug, PartialEq)]
pub struct OrderSnapshot {
    pub order_ref: String,
    pub symbol: String,
    pub side: String, // "buy" | "sell"
    pub quantity: f64,
    pub filled_qty: f64,
    pub remaining_qty: f64,
    pub status: OrderStatusName,
    pub limit_price: Option<f64>,
    pub stop_price: Option<f64>,
    pub avg_fill_price: Option<f64>,
    pub parent_ref: Option<String>,
}

/// Enum for order status — uses the same names as [`crate::engine::types::OrderStatusCode`]
/// but is decoupled from it so expression evaluation stays stable across
/// engine refactors. The parser treats `Filled`, `Cancelled`, etc. as bare
/// identifiers; the interpreter matches them stringly against this name.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum OrderStatusName {
    ApiPending,
    PendingSubmit,
    PreSubmitted,
    Submitted,
    Filled,
    PartiallyFilled,
    Cancelled,
    ApiCancelled,
    Inactive,
}

impl OrderStatusName {
    pub fn as_str(&self) -> &'static str {
        match self {
            OrderStatusName::ApiPending => "ApiPending",
            OrderStatusName::PendingSubmit => "PendingSubmit",
            OrderStatusName::PreSubmitted => "PreSubmitted",
            OrderStatusName::Submitted => "Submitted",
            OrderStatusName::Filled => "Filled",
            OrderStatusName::PartiallyFilled => "PartiallyFilled",
            OrderStatusName::Cancelled => "Cancelled",
            OrderStatusName::ApiCancelled => "ApiCancelled",
            OrderStatusName::Inactive => "Inactive",
        }
    }

    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            OrderStatusName::Filled | OrderStatusName::Cancelled | OrderStatusName::ApiCancelled
        )
    }
}

impl fmt::Display for OrderStatusName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Minimal projection of a position.
#[derive(Clone, Debug, PartialEq)]
pub struct PositionSnapshot {
    pub symbol: String,
    pub quantity: f64,
    pub avg_cost: f64,
    pub realized_pnl: f64,
    pub unrealized_pnl: f64,
}

/// Per-session metrics — just enough for pacing-style asserts.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SessionMetrics {
    pub msg_count: u64,
    pub msg_count_last_5s: u64,
    pub tick_count: u64,
    pub connected: bool,
}

/// Read-only view of the engine surface the expression interpreter sees.
///
/// Impls:
/// - [`crate::scenario::mock_engine::MockEngine`] — canned state for tests.
/// - Real engine — Stage 04/05 Wave 2 (produces snapshots on demand).
pub trait ScenarioQuery: Send + Sync {
    /// All known orders, in stable order (insertion order is fine). Empty
    /// collection is valid.
    fn orders(&self) -> Vec<OrderSnapshot>;

    /// Look up an order by `order_ref`. Returns `None` if absent.
    fn order_by_ref(&self, order_ref: &str) -> Option<OrderSnapshot> {
        self.orders().into_iter().find(|o| o.order_ref == order_ref)
    }

    /// Order by index (matches `orders[0]` path).
    fn order_by_index(&self, idx: usize) -> Option<OrderSnapshot> {
        self.orders().into_iter().nth(idx)
    }

    /// Positions keyed by bare-ticker symbol (e.g. `"AAPL"`).
    fn position_for(&self, symbol: &str) -> Option<PositionSnapshot>;

    /// All positions (may allocate).
    fn positions(&self) -> Vec<PositionSnapshot>;

    /// Metrics for `session[0]`, `session[1]`, …
    fn session_metrics(&self, session_id: u64) -> Option<SessionMetrics>;

    /// How long the current scenario has been running in virtual time.
    fn session_duration(&self) -> Duration;
}
