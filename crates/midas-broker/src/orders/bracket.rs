//! Market bracket types: params, grouping, lifecycle status, and derivation.
//!
//! A market bracket consists of a parent market order with optional
//! take-profit (limit) and stop-loss (stop/stop-limit) children.

use std::fmt;
use std::str::FromStr;

use midas_core::SecurityType;
use serde::{Deserialize, Serialize};

use super::state::OrderStatus;
use super::types::{LocalOrder, OrderAction, TimeInForce};

// ===========================================================================
// MarketBracketParams
// ===========================================================================

/// Parameters for creating a Market Order bracket.
///
/// A market bracket consists of:
/// - Parent: Market order (BUY or SELL)
/// - Take Profit: Limit order on the opposite side (optional)
/// - Stop Loss: Stop or StopLimit order on the opposite side (optional)
///
/// At least one of `take_profit` or `stop_loss` should be present for
/// the bracket to be meaningful, though a naked market order is valid.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketBracketParams {
    // -- Contract --
    pub symbol: String,
    pub con_id: Option<i32>,
    pub sec_type: SecurityType,
    pub exchange: String,
    pub currency: String,

    // -- Entry --
    pub action: OrderAction,
    pub quantity: f64,
    pub outside_rth: bool,

    // -- Take Profit --
    pub take_profit: Option<TakeProfitParams>,

    // -- Stop Loss --
    pub stop_loss: Option<StopLossParams>,

    // -- Risk Guard --
    /// Last traded price at submission time. Used by the engine-level
    /// order size guard for notional value calculation. Populated by the
    /// order panel from chart candle data.
    pub reference_price: Option<f64>,

    // -- Metadata --
    pub strategy: Option<String>,
    pub tags: Vec<String>,
}

/// Take profit configuration for a bracket.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TakeProfitParams {
    /// Limit price for the take profit order.
    pub price: f64,
    /// Time-in-force for the TP leg. Defaults to GTC.
    pub tif: Option<TimeInForce>,
}

/// Stop loss configuration for a bracket.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StopLossParams {
    /// Trigger price for the stop.
    pub stop_price: f64,
    /// If set, creates a StopLimit instead of a Stop. This is the limit
    /// price that the order converts to after the stop triggers.
    pub limit_price: Option<f64>,
    /// Time-in-force for the SL leg. Defaults to GTC.
    pub tif: Option<TimeInForce>,
}

// ===========================================================================
// BracketGroup
// ===========================================================================

/// A complete bracket: parent + optional TP + optional SL.
/// Loaded from the database by querying children of a parent_id.
#[derive(Debug, Clone)]
pub struct BracketGroup {
    pub parent: LocalOrder,
    pub take_profit: Option<LocalOrder>,
    pub stop_loss: Option<LocalOrder>,
}

impl BracketGroup {
    /// All orders in activation order (parent first, SL last for transmit=true).
    pub fn legs(&self) -> Vec<&LocalOrder> {
        let mut legs = vec![&self.parent];
        if let Some(ref tp) = self.take_profit {
            legs.push(tp);
        }
        if let Some(ref sl) = self.stop_loss {
            legs.push(sl);
        }
        legs
    }

    /// True if all legs can be activated (Inactive or Error).
    pub fn can_activate(&self) -> bool {
        self.legs().iter().all(|o| o.status.can_activate())
    }

    /// True if the parent has filled (TP/SL should be live or terminal).
    pub fn is_active(&self) -> bool {
        self.parent.status == OrderStatus::Filled
    }

    /// True if the bracket is fully resolved (all legs terminal).
    pub fn is_closed(&self) -> bool {
        self.legs().iter().all(|o| o.status.is_terminal())
    }
}

// ===========================================================================
// BracketLifecycleStatus
// ===========================================================================

/// Bracket lifecycle status. Derived from individual order statuses, not stored.
/// Lives in midas-broker (NOT midas-chart). The app layer maps this to the
/// chart-layer BracketStatus enum (which is a coarser visual enum).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BracketLifecycleStatus {
    /// Parent is PendingSubmit/Submitted, children are PreSubmitted.
    Submitted,
    /// Parent filled, TP/SL are live at exchange.
    EntryFilled,
    /// Take profit child filled, stop loss auto-cancelled.
    TakeProfitHit,
    /// Stop loss child filled, take profit auto-cancelled.
    StopLossHit,
    /// All legs cancelled (parent cancelled before fill, or user cancelled).
    Cancelled,
    /// Parent rejected by IB.
    Rejected,
    /// Local/internal error during submission.
    Error,
    /// All legs terminal but doesn't fit above categories.
    Closed,
}

impl BracketLifecycleStatus {
    /// Returns true if this bracket has reached a final state and will never change again.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::TakeProfitHit
                | Self::StopLossHit
                | Self::Cancelled
                | Self::Rejected
                | Self::Error
                | Self::Closed
        )
    }
}

impl fmt::Display for BracketLifecycleStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::Submitted => "Submitted",
            Self::EntryFilled => "EntryFilled",
            Self::TakeProfitHit => "TakeProfitHit",
            Self::StopLossHit => "StopLossHit",
            Self::Cancelled => "Cancelled",
            Self::Rejected => "Rejected",
            Self::Error => "Error",
            Self::Closed => "Closed",
        };
        f.write_str(s)
    }
}

impl FromStr for BracketLifecycleStatus {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Submitted" => Ok(Self::Submitted),
            "EntryFilled" => Ok(Self::EntryFilled),
            "TakeProfitHit" => Ok(Self::TakeProfitHit),
            "StopLossHit" => Ok(Self::StopLossHit),
            "Cancelled" => Ok(Self::Cancelled),
            "Rejected" => Ok(Self::Rejected),
            "Error" => Ok(Self::Error),
            "Closed" => Ok(Self::Closed),
            other => Err(format!("unknown BracketLifecycleStatus: {other}")),
        }
    }
}

// ===========================================================================
// Validation
// ===========================================================================

/// Validation errors for market bracket params.
#[derive(Debug, Clone, PartialEq)]
pub enum ValidationError {
    MissingSymbol,
    MissingExchange,
    MissingCurrency,
    InvalidQuantity,
    InvalidPrice(&'static str),
}

impl fmt::Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingSymbol => f.write_str("symbol is required"),
            Self::MissingExchange => f.write_str("exchange is required"),
            Self::MissingCurrency => f.write_str("currency is required"),
            Self::InvalidQuantity => f.write_str("quantity must be positive"),
            Self::InvalidPrice(field) => write!(f, "{field} price must be positive"),
        }
    }
}

/// Validate market bracket params. Returns errors if any fields are invalid.
pub fn validate_market_bracket(
    params: &MarketBracketParams,
) -> Result<(), Vec<ValidationError>> {
    let mut errors = Vec::new();

    if params.symbol.is_empty() {
        errors.push(ValidationError::MissingSymbol);
    }

    if params.exchange.is_empty() {
        errors.push(ValidationError::MissingExchange);
    }

    if params.currency.is_empty() {
        errors.push(ValidationError::MissingCurrency);
    }

    if !(params.quantity.is_finite() && params.quantity > 0.0) {
        errors.push(ValidationError::InvalidQuantity);
    }

    if let Some(ref tp) = params.take_profit {
        if !(tp.price.is_finite() && tp.price > 0.0) {
            errors.push(ValidationError::InvalidPrice("take_profit"));
        }
    }

    if let Some(ref sl) = params.stop_loss {
        if !(sl.stop_price.is_finite() && sl.stop_price > 0.0) {
            errors.push(ValidationError::InvalidPrice("stop_loss"));
        }
        if let Some(limit) = sl.limit_price {
            if !(limit.is_finite() && limit > 0.0) {
                errors.push(ValidationError::InvalidPrice("stop_loss_limit"));
            }
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

// ===========================================================================
// Directional Validation
// ===========================================================================

/// Directional warnings (not hard errors for market orders).
#[derive(Debug, Clone, PartialEq)]
pub enum DirectionWarning {
    TpBelowReference { tp: f64, reference_price: f64 },
    TpAboveReference { tp: f64, reference_price: f64 },
    SlAboveReference { sl: f64, reference_price: f64 },
    SlBelowReference { sl: f64, reference_price: f64 },
}

/// Check that TP/SL are on the correct side relative to a reference price.
/// For market orders, these are warnings (logged, not rejected).
pub fn check_bracket_direction(
    action: OrderAction,
    reference_price: f64,
    tp_price: Option<f64>,
    sl_price: Option<f64>,
) -> Vec<DirectionWarning> {
    let mut warnings = Vec::new();
    match action {
        OrderAction::Buy => {
            if let Some(tp) = tp_price {
                if tp <= reference_price {
                    warnings.push(DirectionWarning::TpBelowReference { tp, reference_price });
                }
            }
            if let Some(sl) = sl_price {
                if sl >= reference_price {
                    warnings.push(DirectionWarning::SlAboveReference { sl, reference_price });
                }
            }
        }
        OrderAction::Sell => {
            if let Some(tp) = tp_price {
                if tp >= reference_price {
                    warnings.push(DirectionWarning::TpAboveReference { tp, reference_price });
                }
            }
            if let Some(sl) = sl_price {
                if sl <= reference_price {
                    warnings.push(DirectionWarning::SlBelowReference { sl, reference_price });
                }
            }
        }
    }
    warnings
}

// ===========================================================================
// Status Derivation
// ===========================================================================

/// Derive bracket lifecycle status from individual order statuses.
///
/// SAFETY: This function must NEVER panic. It runs on every order status
/// callback in the trading engine. An unexpected state logs a warning
/// and returns Closed as a safe fallback.
pub fn derive_bracket_status(group: &BracketGroup) -> BracketLifecycleStatus {
    let parent = &group.parent;

    // Parent cancelled
    if parent.status == OrderStatus::Cancelled {
        return BracketLifecycleStatus::Cancelled;
    }

    // Parent rejected
    if parent.status == OrderStatus::Rejected {
        return BracketLifecycleStatus::Rejected;
    }

    // Error check must precede is_terminal() check below.
    if parent.status == OrderStatus::Error {
        return BracketLifecycleStatus::Error;
    }

    // Parent still working (not terminal — and not Error, handled above)
    if !parent.status.is_terminal() {
        return BracketLifecycleStatus::Submitted;
    }

    // Parent is terminal — should be Filled at this point.
    if parent.status != OrderStatus::Filled {
        tracing::warn!(
            "Unexpected: parent {} is terminal ({}) but not Filled — returning Closed as fallback",
            group.parent.id,
            parent.status,
        );
        return BracketLifecycleStatus::Closed;
    }

    // Check if any child is in Error state.
    let child_error = [&group.take_profit, &group.stop_loss]
        .iter()
        .filter_map(|c| c.as_ref())
        .any(|c| c.status == OrderStatus::Error);
    if child_error {
        return BracketLifecycleStatus::Error;
    }

    // Check if TP hit
    if let Some(ref tp) = group.take_profit {
        if tp.status == OrderStatus::Filled {
            return BracketLifecycleStatus::TakeProfitHit;
        }
    }

    // Check if SL hit
    if let Some(ref sl) = group.stop_loss {
        if sl.status == OrderStatus::Filled {
            return BracketLifecycleStatus::StopLossHit;
        }
    }

    // Children still live at exchange
    let any_live = group.legs().iter().any(|o| o.status.is_live_at_ib());
    if any_live {
        return BracketLifecycleStatus::EntryFilled;
    }

    // All terminal but not caught above — edge case
    BracketLifecycleStatus::Closed
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orders::types::{OrderAction, OrderKind};

    // -- helpers ----------------------------------------------------------

    fn make_order(status: OrderStatus) -> LocalOrder {
        let mut o = LocalOrder::new_draft("AAPL", OrderAction::Buy, OrderKind::Market, 100.0);
        o.status = status;
        o
    }

    fn make_bracket(
        parent_status: OrderStatus,
        tp_status: Option<OrderStatus>,
        sl_status: Option<OrderStatus>,
    ) -> BracketGroup {
        let parent = make_order(parent_status);
        let parent_id = parent.id;
        BracketGroup {
            parent,
            take_profit: tp_status.map(|s| {
                let mut o =
                    LocalOrder::new_draft("AAPL", OrderAction::Sell, OrderKind::Limit, 100.0);
                o.status = s;
                o.parent_id = Some(parent_id);
                o
            }),
            stop_loss: sl_status.map(|s| {
                let mut o =
                    LocalOrder::new_draft("AAPL", OrderAction::Sell, OrderKind::Stop, 100.0);
                o.status = s;
                o.parent_id = Some(parent_id);
                o
            }),
        }
    }

    fn sample_params() -> MarketBracketParams {
        MarketBracketParams {
            symbol: "AAPL".to_string(),
            con_id: Some(265598),
            sec_type: SecurityType::Stock,
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
        }
    }

    // -- BracketLifecycleStatus Display/FromStr ---------------------------

    #[test]
    fn lifecycle_status_display_fromstr_round_trip() {
        let all = [
            BracketLifecycleStatus::Submitted,
            BracketLifecycleStatus::EntryFilled,
            BracketLifecycleStatus::TakeProfitHit,
            BracketLifecycleStatus::StopLossHit,
            BracketLifecycleStatus::Cancelled,
            BracketLifecycleStatus::Rejected,
            BracketLifecycleStatus::Error,
            BracketLifecycleStatus::Closed,
        ];
        for status in all {
            let s = status.to_string();
            let parsed: BracketLifecycleStatus = s.parse().unwrap();
            assert_eq!(parsed, status, "round-trip failed for {status}");
        }
    }

    #[test]
    fn lifecycle_status_parse_unknown_fails() {
        assert!("Bogus".parse::<BracketLifecycleStatus>().is_err());
    }

    #[test]
    fn lifecycle_status_serde_round_trip() {
        let status = BracketLifecycleStatus::TakeProfitHit;
        let json = serde_json::to_string(&status).unwrap();
        let restored: BracketLifecycleStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, status);
    }

    // -- Validation -------------------------------------------------------

    #[test]
    fn valid_full_bracket() {
        assert!(validate_market_bracket(&sample_params()).is_ok());
    }

    #[test]
    fn valid_tp_only() {
        let mut p = sample_params();
        p.stop_loss = None;
        assert!(validate_market_bracket(&p).is_ok());
    }

    #[test]
    fn valid_sl_only() {
        let mut p = sample_params();
        p.take_profit = None;
        assert!(validate_market_bracket(&p).is_ok());
    }

    #[test]
    fn valid_naked_market() {
        let mut p = sample_params();
        p.take_profit = None;
        p.stop_loss = None;
        assert!(validate_market_bracket(&p).is_ok());
    }

    #[test]
    fn reject_empty_symbol() {
        let mut p = sample_params();
        p.symbol = String::new();
        let errs = validate_market_bracket(&p).unwrap_err();
        assert!(errs.contains(&ValidationError::MissingSymbol));
    }

    #[test]
    fn reject_empty_exchange() {
        let mut p = sample_params();
        p.exchange = String::new();
        let errs = validate_market_bracket(&p).unwrap_err();
        assert!(errs.contains(&ValidationError::MissingExchange));
    }

    #[test]
    fn reject_empty_currency() {
        let mut p = sample_params();
        p.currency = String::new();
        let errs = validate_market_bracket(&p).unwrap_err();
        assert!(errs.contains(&ValidationError::MissingCurrency));
    }

    #[test]
    fn reject_zero_quantity() {
        let mut p = sample_params();
        p.quantity = 0.0;
        let errs = validate_market_bracket(&p).unwrap_err();
        assert!(errs.contains(&ValidationError::InvalidQuantity));
    }

    #[test]
    fn reject_negative_quantity() {
        let mut p = sample_params();
        p.quantity = -100.0;
        let errs = validate_market_bracket(&p).unwrap_err();
        assert!(errs.contains(&ValidationError::InvalidQuantity));
    }

    #[test]
    fn reject_negative_tp_price() {
        let mut p = sample_params();
        p.take_profit = Some(TakeProfitParams {
            price: -10.0,
            tif: None,
        });
        let errs = validate_market_bracket(&p).unwrap_err();
        assert!(errs.contains(&ValidationError::InvalidPrice("take_profit")));
    }

    #[test]
    fn reject_negative_sl_price() {
        let mut p = sample_params();
        p.stop_loss = Some(StopLossParams {
            stop_price: -10.0,
            limit_price: None,
            tif: None,
        });
        let errs = validate_market_bracket(&p).unwrap_err();
        assert!(errs.contains(&ValidationError::InvalidPrice("stop_loss")));
    }

    #[test]
    fn reject_nan_quantity() {
        let mut p = sample_params();
        p.quantity = f64::NAN;
        let errs = validate_market_bracket(&p).unwrap_err();
        assert!(errs.contains(&ValidationError::InvalidQuantity));
    }

    #[test]
    fn reject_infinity_quantity() {
        let mut p = sample_params();
        p.quantity = f64::INFINITY;
        let errs = validate_market_bracket(&p).unwrap_err();
        assert!(errs.contains(&ValidationError::InvalidQuantity));
    }

    #[test]
    fn reject_nan_tp_price() {
        let mut p = sample_params();
        p.take_profit = Some(TakeProfitParams {
            price: f64::NAN,
            tif: None,
        });
        let errs = validate_market_bracket(&p).unwrap_err();
        assert!(errs.contains(&ValidationError::InvalidPrice("take_profit")));
    }

    #[test]
    fn reject_infinity_sl_stop_price() {
        let mut p = sample_params();
        p.stop_loss = Some(StopLossParams {
            stop_price: f64::INFINITY,
            limit_price: None,
            tif: None,
        });
        let errs = validate_market_bracket(&p).unwrap_err();
        assert!(errs.contains(&ValidationError::InvalidPrice("stop_loss")));
    }

    #[test]
    fn reject_nan_sl_limit_price() {
        let mut p = sample_params();
        p.stop_loss = Some(StopLossParams {
            stop_price: 182.0,
            limit_price: Some(f64::NAN),
            tif: None,
        });
        let errs = validate_market_bracket(&p).unwrap_err();
        assert!(errs.contains(&ValidationError::InvalidPrice("stop_loss_limit")));
    }

    // -- Directional Validation -------------------------------------------

    #[test]
    fn buy_tp_above_entry() {
        let w = check_bracket_direction(OrderAction::Buy, 185.0, Some(192.0), None);
        assert!(w.is_empty());
    }

    #[test]
    fn buy_tp_below_entry_warns() {
        let w = check_bracket_direction(OrderAction::Buy, 185.0, Some(180.0), None);
        assert_eq!(w.len(), 1);
        assert!(matches!(w[0], DirectionWarning::TpBelowReference { .. }));
    }

    #[test]
    fn buy_sl_below_entry() {
        let w = check_bracket_direction(OrderAction::Buy, 185.0, None, Some(182.0));
        assert!(w.is_empty());
    }

    #[test]
    fn buy_sl_above_entry_warns() {
        let w = check_bracket_direction(OrderAction::Buy, 185.0, None, Some(190.0));
        assert_eq!(w.len(), 1);
        assert!(matches!(w[0], DirectionWarning::SlAboveReference { .. }));
    }

    #[test]
    fn sell_tp_below_entry() {
        let w = check_bracket_direction(OrderAction::Sell, 185.0, Some(178.0), None);
        assert!(w.is_empty());
    }

    #[test]
    fn sell_tp_above_entry_warns() {
        let w = check_bracket_direction(OrderAction::Sell, 185.0, Some(190.0), None);
        assert_eq!(w.len(), 1);
        assert!(matches!(w[0], DirectionWarning::TpAboveReference { .. }));
    }

    #[test]
    fn sell_sl_above_entry() {
        let w = check_bracket_direction(OrderAction::Sell, 185.0, None, Some(190.0));
        assert!(w.is_empty());
    }

    #[test]
    fn sell_sl_below_entry_warns() {
        let w = check_bracket_direction(OrderAction::Sell, 185.0, None, Some(180.0));
        assert_eq!(w.len(), 1);
        assert!(matches!(w[0], DirectionWarning::SlBelowReference { .. }));
    }

    // -- BracketGroup -----------------------------------------------------

    #[test]
    fn legs_count_full() {
        let g = make_bracket(
            OrderStatus::Inactive,
            Some(OrderStatus::Inactive),
            Some(OrderStatus::Inactive),
        );
        assert_eq!(g.legs().len(), 3);
    }

    #[test]
    fn legs_count_tp_only() {
        let g = make_bracket(OrderStatus::Inactive, Some(OrderStatus::Inactive), None);
        assert_eq!(g.legs().len(), 2);
    }

    #[test]
    fn legs_count_sl_only() {
        let g = make_bracket(OrderStatus::Inactive, None, Some(OrderStatus::Inactive));
        assert_eq!(g.legs().len(), 2);
    }

    #[test]
    fn can_activate_all_inactive() {
        let g = make_bracket(
            OrderStatus::Inactive,
            Some(OrderStatus::Inactive),
            Some(OrderStatus::Inactive),
        );
        assert!(g.can_activate());
    }

    #[test]
    fn can_activate_one_error() {
        let g = make_bracket(
            OrderStatus::Error,
            Some(OrderStatus::Inactive),
            Some(OrderStatus::Inactive),
        );
        assert!(g.can_activate());
    }

    #[test]
    fn cannot_activate_one_submitted() {
        let g = make_bracket(
            OrderStatus::Submitted,
            Some(OrderStatus::Inactive),
            Some(OrderStatus::Inactive),
        );
        assert!(!g.can_activate());
    }

    #[test]
    fn is_active_when_parent_filled() {
        let g = make_bracket(
            OrderStatus::Filled,
            Some(OrderStatus::Submitted),
            Some(OrderStatus::Submitted),
        );
        assert!(g.is_active());
    }

    #[test]
    fn is_closed_all_terminal() {
        let g = make_bracket(
            OrderStatus::Filled,
            Some(OrderStatus::Filled),
            Some(OrderStatus::Cancelled),
        );
        assert!(g.is_closed());
    }

    #[test]
    fn not_closed_child_live() {
        let g = make_bracket(
            OrderStatus::Filled,
            Some(OrderStatus::Submitted),
            Some(OrderStatus::Submitted),
        );
        assert!(!g.is_closed());
    }

    // -- derive_bracket_status --------------------------------------------

    #[test]
    fn derive_parent_pending() {
        let g = make_bracket(
            OrderStatus::PendingSubmit,
            Some(OrderStatus::PendingSubmit),
            Some(OrderStatus::PendingSubmit),
        );
        assert_eq!(derive_bracket_status(&g), BracketLifecycleStatus::Submitted);
    }

    #[test]
    fn derive_parent_submitted() {
        let g = make_bracket(
            OrderStatus::Submitted,
            Some(OrderStatus::PreSubmitted),
            Some(OrderStatus::PreSubmitted),
        );
        assert_eq!(derive_bracket_status(&g), BracketLifecycleStatus::Submitted);
    }

    #[test]
    fn derive_parent_filled_children_live() {
        let g = make_bracket(
            OrderStatus::Filled,
            Some(OrderStatus::Submitted),
            Some(OrderStatus::Submitted),
        );
        assert_eq!(
            derive_bracket_status(&g),
            BracketLifecycleStatus::EntryFilled
        );
    }

    #[test]
    fn derive_tp_filled() {
        let g = make_bracket(
            OrderStatus::Filled,
            Some(OrderStatus::Filled),
            Some(OrderStatus::Cancelled),
        );
        assert_eq!(
            derive_bracket_status(&g),
            BracketLifecycleStatus::TakeProfitHit
        );
    }

    #[test]
    fn derive_sl_filled() {
        let g = make_bracket(
            OrderStatus::Filled,
            Some(OrderStatus::Cancelled),
            Some(OrderStatus::Filled),
        );
        assert_eq!(
            derive_bracket_status(&g),
            BracketLifecycleStatus::StopLossHit
        );
    }

    #[test]
    fn derive_parent_cancelled() {
        let g = make_bracket(
            OrderStatus::Cancelled,
            Some(OrderStatus::Cancelled),
            Some(OrderStatus::Cancelled),
        );
        assert_eq!(
            derive_bracket_status(&g),
            BracketLifecycleStatus::Cancelled
        );
    }

    #[test]
    fn derive_parent_rejected() {
        let g = make_bracket(
            OrderStatus::Rejected,
            Some(OrderStatus::Rejected),
            Some(OrderStatus::Rejected),
        );
        assert_eq!(
            derive_bracket_status(&g),
            BracketLifecycleStatus::Rejected
        );
    }

    #[test]
    fn derive_parent_error() {
        let g = make_bracket(
            OrderStatus::Error,
            Some(OrderStatus::Inactive),
            Some(OrderStatus::Inactive),
        );
        assert_eq!(derive_bracket_status(&g), BracketLifecycleStatus::Error);
    }

    #[test]
    fn derive_child_error() {
        let g = make_bracket(
            OrderStatus::Filled,
            Some(OrderStatus::Error),
            Some(OrderStatus::Submitted),
        );
        assert_eq!(derive_bracket_status(&g), BracketLifecycleStatus::Error);
    }

    #[test]
    fn derive_all_closed() {
        let g = make_bracket(
            OrderStatus::Filled,
            Some(OrderStatus::Cancelled),
            Some(OrderStatus::Cancelled),
        );
        assert_eq!(derive_bracket_status(&g), BracketLifecycleStatus::Closed);
    }

    #[test]
    fn derive_unexpected_parent_status_does_not_panic() {
        let g = make_bracket(OrderStatus::Filled, None, None);
        assert_eq!(derive_bracket_status(&g), BracketLifecycleStatus::Closed);
    }
}
