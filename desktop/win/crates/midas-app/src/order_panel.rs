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
mod tests {
    use super::*;

    #[test]
    fn resolve_absolute_returns_value() {
        let price = resolve_price(PriceInputMode::Absolute, 192.0, 185.0, OrderSide::Buy, true);
        assert!((price - 192.0).abs() < f64::EPSILON);
    }

    #[test]
    fn resolve_offset_buy_tp() {
        let price = resolve_price(PriceInputMode::Offset, 6.50, 185.0, OrderSide::Buy, true);
        assert!((price - 191.50).abs() < 0.01);
    }

    #[test]
    fn resolve_offset_buy_sl() {
        let price = resolve_price(PriceInputMode::Offset, 3.0, 185.0, OrderSide::Buy, false);
        assert!((price - 182.0).abs() < 0.01);
    }

    #[test]
    fn resolve_percent_buy_tp() {
        let price = resolve_price(PriceInputMode::Percent, 3.5, 185.0, OrderSide::Buy, true);
        assert!((price - 191.475).abs() < 0.01);
    }

    #[test]
    fn resolve_percent_buy_sl() {
        let price = resolve_price(PriceInputMode::Percent, 1.9, 185.0, OrderSide::Buy, false);
        assert!((price - 181.485).abs() < 0.01);
    }

    #[test]
    fn resolve_offset_sell_tp() {
        let price = resolve_price(PriceInputMode::Offset, 6.50, 185.0, OrderSide::Sell, true);
        assert!((price - 178.50).abs() < 0.01);
    }

    #[test]
    fn resolve_percent_sell_sl() {
        let price = resolve_price(PriceInputMode::Percent, 1.9, 185.0, OrderSide::Sell, false);
        assert!((price - 188.515).abs() < 0.01);
    }

    #[test]
    fn risk_reward_full() {
        let rr = calculate_risk_reward(185.0, Some(192.0), Some(182.0), 100.0).unwrap();
        assert!((rr.risk_per_share - 3.0).abs() < 0.01);
        assert!((rr.reward_per_share - 7.0).abs() < 0.01);
        assert!((rr.total_risk - 300.0).abs() < 0.01);
        assert!((rr.total_reward - 700.0).abs() < 0.01);
        assert!((rr.ratio - 2.333).abs() < 0.01);
    }

    #[test]
    fn risk_reward_no_sl() {
        assert!(calculate_risk_reward(185.0, Some(192.0), None, 100.0).is_none());
    }

    #[test]
    fn risk_reward_zero_risk() {
        assert!(calculate_risk_reward(185.0, Some(192.0), Some(185.0), 100.0).is_none());
    }

    #[test]
    fn validate_missing_symbol() {
        let state = OrderPanelState::default();
        let errors = validate_panel(&state);
        assert!(errors.iter().any(|(f, _)| f == "symbol"));
    }

    #[test]
    fn validate_buy_tp_below_price_rejected() {
        let mut state = OrderPanelState::default();
        state.symbol = "AAPL".to_string();
        state.last_price = Some(185.0);
        state.tp_enabled = true;
        state.tp_value = "180.0".to_string();
        state.sl_enabled = false;
        let errors = validate_panel(&state);
        assert!(errors.iter().any(|(f, _)| f == "tp"));
    }

    #[test]
    fn validate_valid_bracket() {
        let mut state = OrderPanelState::default();
        state.symbol = "AAPL".to_string();
        state.last_price = Some(185.0);
        state.quantity = "100".to_string();
        state.tp_enabled = true;
        state.tp_value = "192.0".to_string();
        state.sl_enabled = true;
        state.sl_value = "182.0".to_string();
        let errors = validate_panel(&state);
        assert!(errors.is_empty(), "expected no errors, got: {errors:?}");
    }

    // -- create_bracket_annotation tests --

    #[test]
    fn create_bracket_long_with_tp_sl() {
        use midas_chart::widget::order_bracket::*;

        let bracket = create_bracket_annotation(
            BracketSide::Long,
            185.0,
            Some(192.0),
            Some(182.0),
            100.0,
        );
        assert_eq!(bracket.side, BracketSide::Long);
        assert_eq!(bracket.status, BracketStatus::Pending);
        assert!((bracket.entry.price - 185.0).abs() < f64::EPSILON);
        assert!(bracket.take_profit.is_some());
        assert!((bracket.take_profit.unwrap().price - 192.0).abs() < f64::EPSILON);
        assert!(bracket.stop_loss.is_some());
        assert!((bracket.stop_loss.unwrap().price - 182.0).abs() < f64::EPSILON);
        assert_eq!(bracket.quantity, Some(100.0));
    }

    #[test]
    fn create_bracket_short_no_tp() {
        use midas_chart::widget::order_bracket::*;

        let bracket = create_bracket_annotation(
            BracketSide::Short,
            185.0,
            None,
            Some(188.0),
            50.0,
        );
        assert_eq!(bracket.side, BracketSide::Short);
        assert_eq!(bracket.status, BracketStatus::Pending);
        assert!(bracket.take_profit.is_none());
        assert!(bracket.stop_loss.is_some());
        assert_eq!(bracket.quantity, Some(50.0));
    }

    #[test]
    fn create_bracket_no_legs() {
        use midas_chart::widget::order_bracket::*;

        let bracket = create_bracket_annotation(
            BracketSide::Long,
            185.0,
            None,
            None,
            200.0,
        );
        assert!(bracket.take_profit.is_none());
        assert!(bracket.stop_loss.is_none());
        assert!((bracket.entry.price - 185.0).abs() < f64::EPSILON);
    }

    // -- map_lifecycle_to_chart_status tests --

    #[test]
    fn lifecycle_submitted_maps_to_pending() {
        use midas_chart::widget::order_bracket::BracketStatus;
        assert_eq!(map_lifecycle_to_chart_status("Submitted"), BracketStatus::Pending);
    }

    #[test]
    fn lifecycle_entry_filled_maps_to_active() {
        use midas_chart::widget::order_bracket::BracketStatus;
        assert_eq!(map_lifecycle_to_chart_status("EntryFilled"), BracketStatus::Active);
    }

    #[test]
    fn lifecycle_take_profit_hit_maps_to_closed() {
        use midas_chart::widget::order_bracket::BracketStatus;
        assert_eq!(map_lifecycle_to_chart_status("TakeProfitHit"), BracketStatus::Closed);
    }

    #[test]
    fn lifecycle_stop_loss_hit_maps_to_closed() {
        use midas_chart::widget::order_bracket::BracketStatus;
        assert_eq!(map_lifecycle_to_chart_status("StopLossHit"), BracketStatus::Closed);
    }

    #[test]
    fn lifecycle_cancelled_maps_to_cancelled() {
        use midas_chart::widget::order_bracket::BracketStatus;
        assert_eq!(map_lifecycle_to_chart_status("Cancelled"), BracketStatus::Cancelled);
    }

    #[test]
    fn lifecycle_rejected_maps_to_cancelled() {
        use midas_chart::widget::order_bracket::BracketStatus;
        assert_eq!(map_lifecycle_to_chart_status("Rejected"), BracketStatus::Cancelled);
    }

    #[test]
    fn lifecycle_unknown_defaults_to_pending() {
        use midas_chart::widget::order_bracket::BracketStatus;
        assert_eq!(map_lifecycle_to_chart_status("SomethingNew"), BracketStatus::Pending);
    }

    #[test]
    fn validate_non_numeric_quantity() {
        let mut state = OrderPanelState::default();
        state.symbol = "AAPL".to_string();
        state.last_price = Some(185.0);
        state.quantity = "abc".to_string();
        let errors = validate_panel(&state);
        assert!(
            errors.iter().any(|(f, msg)| f == "quantity" && msg.contains("not a number")),
            "expected 'not a number' error for quantity, got: {errors:?}"
        );
    }

    #[test]
    fn validate_non_numeric_tp_value() {
        let mut state = OrderPanelState::default();
        state.symbol = "AAPL".to_string();
        state.last_price = Some(185.0);
        state.quantity = "100".to_string();
        state.tp_enabled = true;
        state.tp_value = "xyz".to_string();
        state.sl_enabled = false;
        let errors = validate_panel(&state);
        assert!(
            errors.iter().any(|(f, msg)| f == "tp" && msg.contains("not a number")),
            "expected 'not a number' error for TP, got: {errors:?}"
        );
        // Should not double-report with generic "Invalid TP price"
        let tp_errors: Vec<_> = errors.iter().filter(|(f, _)| f == "tp").collect();
        assert_eq!(tp_errors.len(), 1, "TP should have exactly one error, got: {tp_errors:?}");
    }

    #[test]
    fn validate_non_numeric_sl_value() {
        let mut state = OrderPanelState::default();
        state.symbol = "AAPL".to_string();
        state.last_price = Some(185.0);
        state.quantity = "100".to_string();
        state.tp_enabled = false;
        state.sl_enabled = true;
        state.sl_value = "???".to_string();
        let errors = validate_panel(&state);
        assert!(
            errors.iter().any(|(f, msg)| f == "sl" && msg.contains("not a number")),
            "expected 'not a number' error for SL, got: {errors:?}"
        );
        let sl_errors: Vec<_> = errors.iter().filter(|(f, _)| f == "sl").collect();
        assert_eq!(sl_errors.len(), 1, "SL should have exactly one error, got: {sl_errors:?}");
    }

    // -- OrderPanel (dockable) tests --

    #[test]
    fn order_panel_new_sets_symbol() {
        let id = OrderPanelId::new(1);
        let panel = OrderPanel::new(id, "AAPL".to_string());
        assert_eq!(panel.id, id);
        assert_eq!(panel.state.symbol, "AAPL");
        assert!(panel.state.visible);
        assert_eq!(panel.state.side, OrderSide::Buy);
        assert_eq!(panel.state.quantity, "100");
    }

    #[test]
    fn order_panel_to_config_roundtrip() {
        let id = OrderPanelId::new(5);
        let mut panel = OrderPanel::new(id, "MSFT".to_string());
        panel.state.side = OrderSide::Sell;
        panel.state.quantity = "250".to_string();
        panel.symbol_link = LinkMode::ListenAll;

        let config = panel.to_config();
        assert_eq!(config.symbol, "MSFT");
        assert_eq!(config.side, "SELL");
        assert_eq!(config.quantity, "250");
        assert_eq!(config.symbol_link, LinkMode::ListenAll);

        let restored = OrderPanel::from_config(id, &config);
        assert_eq!(restored.id, id);
        assert_eq!(restored.state.symbol, "MSFT");
        assert_eq!(restored.state.side, OrderSide::Sell);
        assert_eq!(restored.state.quantity, "250");
        assert_eq!(restored.symbol_link, LinkMode::ListenAll);
        assert!(restored.state.visible);
    }

    #[test]
    fn order_panel_from_config_defaults() {
        let config = midas_core::config::OrderPanelConfig::default();
        let panel = OrderPanel::from_config(OrderPanelId::new(1), &config);
        assert_eq!(panel.state.side, OrderSide::Buy);
        assert_eq!(panel.state.quantity, "100");
        assert!(panel.state.symbol.is_empty());
    }
}
