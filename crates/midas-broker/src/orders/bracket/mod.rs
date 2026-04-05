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
pub fn validate_market_bracket(params: &MarketBracketParams) -> Result<(), Vec<ValidationError>> {
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
                    warnings.push(DirectionWarning::TpBelowReference {
                        tp,
                        reference_price,
                    });
                }
            }
            if let Some(sl) = sl_price {
                if sl >= reference_price {
                    warnings.push(DirectionWarning::SlAboveReference {
                        sl,
                        reference_price,
                    });
                }
            }
        }
        OrderAction::Sell => {
            if let Some(tp) = tp_price {
                if tp >= reference_price {
                    warnings.push(DirectionWarning::TpAboveReference {
                        tp,
                        reference_price,
                    });
                }
            }
            if let Some(sl) = sl_price {
                if sl <= reference_price {
                    warnings.push(DirectionWarning::SlBelowReference {
                        sl,
                        reference_price,
                    });
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

#[cfg(test)]
mod tests;
