//! Order panel widget for Market Order bracket entry.
//!
//! A floating/dockable widget that lets the user create market order
//! brackets with optional Take Profit and Stop Loss legs.
//! Follows TradingView's bracket order model (1 TP + 1 SL per bracket).

use midas_core::ChartId;
use midas_core::OrderPanelId;
use midas_core::link::LinkMode;

// ===========================================================================
// State
// ===========================================================================

/// State for the floating order entry panel.
#[derive(Debug, Clone)]
pub struct OrderPanelState {
    /// Whether the panel is visible.
    pub visible: bool,
    /// Current side selection.
    pub side: OrderSide,
    /// Quantity input value (string for text input; parsed on submit).
    pub quantity: String,
    /// Take profit enabled.
    pub tp_enabled: bool,
    /// Take profit input mode.
    pub tp_mode: PriceInputMode,
    /// Take profit input value (meaning depends on tp_mode).
    pub tp_value: String,
    /// Stop loss enabled.
    pub sl_enabled: bool,
    /// Stop loss input mode.
    pub sl_mode: PriceInputMode,
    /// Stop loss input value.
    pub sl_value: String,
    /// Stop loss type.
    pub sl_type: StopLossType,
    /// Stop limit price (only when sl_type == StopLimit).
    pub sl_limit_value: String,
    /// Validation errors to display inline.
    pub errors: Vec<(String, String)>,
    /// Symbol (from active chart).
    pub symbol: String,
    /// Last known price (from chart candle data).
    pub last_price: Option<f64>,
    /// Which chart this panel is attached to.
    pub source_chart: Option<ChartId>,
    /// Whether the confirmation dialog is showing.
    pub showing_confirmation: bool,
}

impl Default for OrderPanelState {
    fn default() -> Self {
        Self {
            visible: false,
            side: OrderSide::Buy,
            quantity: "100".to_string(),
            tp_enabled: true,
            tp_mode: PriceInputMode::Absolute,
            tp_value: String::new(),
            sl_enabled: true,
            sl_mode: PriceInputMode::Absolute,
            sl_value: String::new(),
            sl_type: StopLossType::Stop,
            sl_limit_value: String::new(),
            errors: Vec::new(),
            symbol: String::new(),
            last_price: None,
            source_chart: None,
            showing_confirmation: false,
        }
    }
}

// ===========================================================================
// Supporting enums
// ===========================================================================

/// Buy or sell direction for the order panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderSide {
    Buy,
    Sell,
}

/// How the user specifies TP/SL price.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PriceInputMode {
    /// Absolute price level (e.g., 192.00).
    Absolute,
    /// Dollar offset from last price (e.g., +6.50).
    Offset,
    /// Percentage from last price (e.g., +3.5%).
    Percent,
}

/// Stop loss order type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopLossType {
    /// Becomes market order when stop price is hit.
    Stop,
    /// Becomes limit order when stop price is hit.
    StopLimit,
}

// ===========================================================================
// Price resolution
// ===========================================================================

/// Resolve a price input to an absolute price.
pub fn resolve_price(
    mode: PriceInputMode,
    value: f64,
    last_price: f64,
    side: OrderSide,
    is_tp: bool,
) -> f64 {
    match mode {
        PriceInputMode::Absolute => value,
        PriceInputMode::Offset => {
            if (side == OrderSide::Buy) == is_tp {
                last_price + value.abs()
            } else {
                last_price - value.abs()
            }
        }
        PriceInputMode::Percent => {
            let factor = value.abs() / 100.0;
            if (side == OrderSide::Buy) == is_tp {
                last_price * (1.0 + factor)
            } else {
                last_price * (1.0 - factor)
            }
        }
    }
}

// ===========================================================================
// Risk/Reward calculation
// ===========================================================================

/// Real-time risk/reward calculation.
#[derive(Debug, Clone)]
pub struct RiskReward {
    pub risk_per_share: f64,
    pub reward_per_share: f64,
    pub total_risk: f64,
    pub total_reward: f64,
    pub risk_pct: f64,
    pub reward_pct: f64,
    pub ratio: f64,
}

/// Calculate risk/reward for the current panel inputs.
pub fn calculate_risk_reward(
    entry_price: f64,
    tp_price: Option<f64>,
    sl_price: Option<f64>,
    quantity: f64,
) -> Option<RiskReward> {
    let sl = sl_price?;
    let risk_per_share = (entry_price - sl).abs();
    if risk_per_share < f64::EPSILON {
        return None;
    }

    let reward_per_share = tp_price
        .map(|tp| (tp - entry_price).abs())
        .unwrap_or(0.0);

    Some(RiskReward {
        risk_per_share,
        reward_per_share,
        total_risk: risk_per_share * quantity,
        total_reward: reward_per_share * quantity,
        risk_pct: risk_per_share / entry_price * 100.0,
        reward_pct: reward_per_share / entry_price * 100.0,
        ratio: if risk_per_share > 0.0 {
            reward_per_share / risk_per_share
        } else {
            0.0
        },
    })
}

// ===========================================================================
// Validation
// ===========================================================================

/// Validate order panel inputs before submission.
pub fn validate_panel(state: &OrderPanelState) -> Vec<(String, String)> {
    let mut errors = Vec::new();

    if state.symbol.is_empty() {
        errors.push(("symbol".to_string(), "No symbol selected".to_string()));
    }

    let qty: f64 = match state.quantity.parse() {
        Ok(q) if q > 0.0 => q,
        Ok(_) => {
            errors.push(("quantity".to_string(), "Quantity must be positive".to_string()));
            0.0
        }
        Err(_) => {
            errors.push(("quantity".to_string(), "Invalid quantity (not a number)".to_string()));
            0.0
        }
    };

    if let Some(last_price) = state.last_price {
        if state.tp_enabled {
            let tp_val: f64 = match state.tp_value.parse() {
                Ok(v) => v,
                Err(_) => {
                    errors.push(("tp".to_string(), "Invalid TP value (not a number)".to_string()));
                    0.0
                }
            };
            let tp_price = resolve_price(state.tp_mode, tp_val, last_price, state.side, true);
            let tp_has_error = errors.iter().any(|(k, _)| k == "tp");
            if tp_price <= 0.0 && !tp_has_error {
                errors.push(("tp".to_string(), "Invalid TP price".to_string()));
            }
            // Direction check (skip if value already invalid)
            if !tp_has_error {
                match state.side {
                    OrderSide::Buy if tp_price <= last_price => {
                        errors.push(("tp".to_string(), "TP must be above current price for BUY".to_string()));
                    }
                    OrderSide::Sell if tp_price >= last_price => {
                        errors.push(("tp".to_string(), "TP must be below current price for SELL".to_string()));
                    }
                    _ => {}
                }
            }
        }

        if state.sl_enabled {
            let sl_val: f64 = match state.sl_value.parse() {
                Ok(v) => v,
                Err(_) => {
                    errors.push(("sl".to_string(), "Invalid SL value (not a number)".to_string()));
                    0.0
                }
            };
            let sl_price = resolve_price(state.sl_mode, sl_val, last_price, state.side, false);
            let sl_has_error = errors.iter().any(|(k, _)| k == "sl");
            if sl_price <= 0.0 && !sl_has_error {
                errors.push(("sl".to_string(), "Invalid SL price".to_string()));
            }
            // Direction check (skip if value already invalid)
            if !sl_has_error {
                match state.side {
                    OrderSide::Buy if sl_price >= last_price => {
                        errors.push(("sl".to_string(), "SL must be below current price for BUY".to_string()));
                    }
                    OrderSide::Sell if sl_price <= last_price => {
                        errors.push(("sl".to_string(), "SL must be above current price for SELL".to_string()));
                    }
                    _ => {}
                }
            }
        }
    } else {
        errors.push(("price".to_string(), "No market price available".to_string()));
    }

    errors
}

// ===========================================================================
// OrderAnnotationLink
// ===========================================================================

/// Maps a chart OrderBracket annotation to its broker order legs.
/// Stored in midas-app's annotation manager.
#[derive(Debug, Clone)]
pub struct OrderAnnotationLink {
    /// Annotation ID in the chart's AnnotationStore.
    pub annotation_id: u64,
    /// Broker order UUID of the parent (entry) order.
    pub parent_order_id: uuid::Uuid,
    /// Broker order UUID of the TP child (if any).
    pub tp_order_id: Option<uuid::Uuid>,
    /// Broker order UUID of the SL child (if any).
    pub sl_order_id: Option<uuid::Uuid>,
    /// Symbol (for quick lookup without loading orders).
    pub symbol: String,
    /// Side of the bracket (Long/Short), cached at creation time for reconciliation.
    pub side: midas_chart::widget::order_bracket::BracketSide,
    /// Quantity submitted, cached at creation time for reconciliation.
    pub quantity: f64,
    /// When this link was created, for FIFO ordering during reconciliation.
    pub created_at: std::time::Instant,
}

// ===========================================================================
// Bracket annotation bridge
// ===========================================================================

/// Create an `OrderBracket` annotation from broker event data.
///
/// Builds the chart-side `OrderBracket` struct with the given prices and
/// sets the initial status to `Pending`. The returned value is ready to
/// wrap in an `AnnotationKind::OrderBracket` and add to the annotation store.
pub fn create_bracket_annotation(
    side: midas_chart::widget::order_bracket::BracketSide,
    entry_price: f64,
    tp_price: Option<f64>,
    sl_price: Option<f64>,
    quantity: f64,
) -> midas_chart::widget::order_bracket::OrderBracket {
    use midas_chart::widget::level::LineStyle;
    use midas_chart::widget::order_bracket::*;

    let make_leg = |price: f64| BracketLeg {
        price,
        timestamp: None,
        color: None,
        style: LineStyle::Solid,
        line_width: 1.5,
        label: None,
        projected_pnl: None,
        projected_pnl_pct: None,
    };

    OrderBracket {
        entry: make_leg(entry_price),
        take_profit: tp_price.map(&make_leg),
        stop_loss: sl_price.map(&make_leg),
        side,
        status: BracketStatus::Pending,
        quantity: Some(quantity),
    }
}

/// Map a broker lifecycle status string to a chart `BracketStatus`.
///
/// The broker engine uses `BracketLifecycleStatus` which is a separate
/// type in `midas-broker`. This function bridges the string
/// representation to the chart-side enum without creating a hard
/// dependency between `midas-app` and `midas-broker`.
pub fn map_lifecycle_to_chart_status(
    status: &str,
) -> midas_chart::widget::order_bracket::BracketStatus {
    use midas_chart::widget::order_bracket::BracketStatus;
    match status {
        "Submitted" => BracketStatus::Pending,
        "PartialFill" | "PartiallyFilled" => BracketStatus::PartialFill,
        "EntryFilled" => BracketStatus::Active,
        "TakeProfitHit" | "StopLossHit" | "Closed" => BracketStatus::Closed,
        "Cancelled" | "Rejected" | "Error" => BracketStatus::Cancelled,
        _ => BracketStatus::Pending,
    }
}

// ===========================================================================
// Dockable order panel (first-class pane)
// ===========================================================================

/// Dockable order entry panel (first-class pane like Chart/Watchlist).
#[derive(Debug, Clone)]
pub struct OrderPanel {
    /// Unique identifier within the workspace.
    pub id: OrderPanelId,
    /// Form state (side, quantity, TP/SL, validation, confirmation).
    pub state: OrderPanelState,
    /// Symbol link group for cross-panel symbol propagation.
    pub symbol_link: LinkMode,
}

impl OrderPanel {
    /// Create a new dockable order panel with the given symbol.
    pub fn new(id: OrderPanelId, symbol: String) -> Self {
        let mut state = OrderPanelState::default();
        state.symbol = symbol;
        state.visible = true; // always visible in docked mode
        Self {
            id,
            state,
            symbol_link: LinkMode::default(),
        }
    }

    /// Serialize this panel's state to a config struct for persistence.
    pub fn to_config(&self) -> midas_core::config::OrderPanelConfig {
        midas_core::config::OrderPanelConfig {
            symbol: self.state.symbol.clone(),
            side: match self.state.side {
                OrderSide::Buy => "BUY".to_string(),
                OrderSide::Sell => "SELL".to_string(),
            },
            quantity: self.state.quantity.clone(),
            symbol_link: self.symbol_link,
        }
    }

    /// Restore a panel from a saved config.
    pub fn from_config(id: OrderPanelId, config: &midas_core::config::OrderPanelConfig) -> Self {
        let mut state = OrderPanelState::default();
        state.symbol = config.symbol.clone();
        state.side = if config.side == "SELL" {
            OrderSide::Sell
        } else {
            OrderSide::Buy
        };
        state.quantity = config.quantity.clone();
        state.visible = true;
        Self {
            id,
            state,
            symbol_link: config.symbol_link,
        }
    }
}

/// Actions for a specific order panel instance.
#[derive(Debug, Clone)]
pub enum OrderPanelAction {
    /// Set the order side (Buy/Sell).
    SetSide(OrderSide),
    /// Update the quantity input text.
    SetQuantity(String),
    /// Toggle Take Profit enabled.
    ToggleTp(bool),
    /// Set TP price input mode.
    SetTpMode(PriceInputMode),
    /// Update TP value input text.
    SetTpValue(String),
    /// Toggle Stop Loss enabled.
    ToggleSl(bool),
    /// Set SL price input mode.
    SetSlMode(PriceInputMode),
    /// Update SL value input text.
    SetSlValue(String),
    /// Set SL type (Stop vs StopLimit).
    SetSlType(StopLossType),
    /// Update SL limit price input text.
    SetSlLimit(String),
    /// Submit the order (triggers confirmation dialog).
    Submit,
    /// User confirmed the order in the confirmation dialog.
    ConfirmYes,
    /// User cancelled the confirmation dialog.
    ConfirmNo,
    /// Dismiss the order panel (close confirmation or clear errors).
    Dismiss,
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests;
