//! Order panel widget for Market Order bracket entry.
//!
//! A floating/dockable widget that lets the user create market order
//! brackets with optional Take Profit and Stop Loss legs.
//! Follows TradingView's bracket order model (1 TP + 1 SL per bracket).

use midas_chart::widget::order_bracket::EntryType;
use midas_chart::widget::AnnotationId;
use midas_core::link::LinkMode;
use midas_core::ChartId;
use midas_core::OrderPanelId;
use serde::{Deserialize, Serialize};

// ===========================================================================
// State
// ===========================================================================

/// State for the floating order entry panel.
#[derive(Debug, Clone)]
pub struct OrderPanelState {
    /// Whether the panel is visible.
    #[allow(dead_code)] // part of planned API
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
    /// Entry order type (Market, Limit, Stop, StopLimit).
    pub entry_type: EntryType,
    /// Limit price input (for Limit and StopLimit types).
    pub limit_price: String,
    /// Stop price input (for Stop and StopLimit types).
    pub stop_price: String,
    /// Validation errors to display inline.
    pub errors: Vec<(String, String)>,
    /// Symbol (from active chart).
    pub symbol: String,
    /// Last known price (from chart candle data).
    pub last_price: Option<f64>,
    /// Which chart this panel is attached to.
    #[allow(dead_code)] // part of planned API
    pub source_chart: Option<ChartId>,
    /// Whether the confirmation dialog is showing.
    pub showing_confirmation: bool,
    /// Bracket chart toggle: `None` = [X] active (no bracket shown),
    /// `Some(Buy/Sell)` = bracket active on chart.
    pub bracket_active: Option<OrderSide>,
    /// Annotation ID of the live Draft bracket in `AnnotationStore`.
    /// Links this panel to its bracket for mutations and re-linking.
    pub bracket_annotation_id: Option<AnnotationId>,
    /// Whether the user has typed/clicked into this panel since the
    /// last successful hydration, submit, or cancel.
    ///
    /// Set to `true` on any user edit (keyboard/mouse) that mutates a
    /// price, side, entry type, quantity, or SL/TP toggle. Reset on
    /// successful submit, explicit cancel, successful hydration from a
    /// *different* ticker, or an explicit reset button.
    ///
    /// Consulted by [`OrderPanelState::hydrate_from_intent`]: when a
    /// `ChartActivated` fires for the *same* symbol the user is
    /// currently editing, the hydration is a no-op so we do not clobber
    /// in-progress edits.
    pub dirty: bool,
}

impl Default for OrderPanelState {
    fn default() -> Self {
        Self {
            visible: false,
            side: OrderSide::Buy,
            quantity: "100".to_string(),
            tp_enabled: false,
            tp_mode: PriceInputMode::Absolute,
            tp_value: String::new(),
            sl_enabled: false,
            sl_mode: PriceInputMode::Absolute,
            sl_value: String::new(),
            sl_type: StopLossType::Stop,
            sl_limit_value: String::new(),
            entry_type: EntryType::Market,
            limit_price: String::new(),
            stop_price: String::new(),
            errors: Vec::new(),
            symbol: String::new(),
            last_price: None,
            source_chart: None,
            showing_confirmation: false,
            bracket_active: None,
            bracket_annotation_id: None,
            dirty: false,
        }
    }
}

impl OrderPanelState {
    /// Hydrate this panel from a per-ticker [`crate::ticker_state::TickerState`].
    ///
    /// Uses `state.last_side()` and `state.last_entry_type()` as the
    /// compound key to look up the last-used
    /// [`crate::ticker_state::EntryMemory`] bucket and copies
    /// every field into the panel. If that compound key has never been
    /// touched (no entry in `state.entries()`), falls back to
    /// `EntryMemory::default()` — which sets `sl_enabled = true` per
    /// the "SL on by default per compound" rule.
    ///
    /// # Dirty guard
    ///
    /// If the panel is currently marked `dirty` **and** the incoming
    /// state is for the *same* symbol the panel is editing, this is a
    /// no-op — we do not clobber in-progress user edits. A hydration
    /// from a *different* ticker proceeds unconditionally and clears
    /// the dirty flag.
    pub fn hydrate_from_intent(
        &mut self,
        state: &crate::ticker_state::TickerState,
        last_price: Option<f64>,
    ) {
        if self.dirty && self.symbol == state.symbol().as_str() {
            return;
        }
        let memory = state
            .entries()
            .get(&(state.last_side(), state.last_entry_type()))
            .cloned()
            .unwrap_or_default();
        self.symbol = state.symbol().as_str().to_string();
        self.side = state.last_side();
        self.entry_type = state.last_entry_type();
        self.apply_entry_memory(&memory);
        self.last_price = last_price;
        self.bracket_annotation_id = state.live_annotation_id();
        self.dirty = false;
    }

    /// Soft re-hydrate the panel when the user toggles side or entry
    /// type *within* the same ticker.
    ///
    /// Re-reads the bucket at `(new_side, new_type)` from `state.entries()`
    /// (or falls back to `EntryMemory::default()` — including the
    /// default-true `sl_enabled`). Unlike
    /// [`Self::hydrate_from_intent`], this does **not** bump or clear
    /// the dirty flag: the side/type toggle is itself a user action,
    /// not a typed value, so downstream code may still consider the
    /// panel in-progress.
    pub fn rehydrate_for_compound(
        &mut self,
        state: &crate::ticker_state::TickerState,
        new_side: OrderSide,
        new_type: EntryType,
    ) {
        let memory = state
            .entries()
            .get(&(new_side, new_type))
            .cloned()
            .unwrap_or_default();
        self.side = new_side;
        self.entry_type = new_type;
        self.apply_entry_memory(&memory);
    }

    /// Copy every price / toggle field from an [`EntryMemory`] bucket
    /// into the panel. Shared by
    /// [`Self::hydrate_from_intent`] and [`Self::rehydrate_for_compound`].
    fn apply_entry_memory(&mut self, memory: &crate::ticker_state::EntryMemory) {
        self.quantity = memory
            .quantity
            .map(|q| format!("{}", q))
            .unwrap_or_else(|| "100".to_string());
        self.tp_enabled = memory.tp_enabled;
        self.tp_mode = memory.tp_mode;
        self.tp_value = memory.tp_value.clone();
        self.sl_enabled = memory.sl_enabled;
        self.sl_mode = memory.sl_mode;
        self.sl_value = memory.sl_value.clone();
        self.sl_type = memory.sl_type;
        self.sl_limit_value = memory.sl_limit_value.clone();
        // `entry_price_or_offset` lands in limit_price / stop_price
        // depending on entry_type. For Market it is unused.
        let entry_str = memory
            .entry_price_or_offset
            .map(|p| format!("{:.2}", p))
            .unwrap_or_default();
        // Clear both, then populate the relevant one.
        self.limit_price.clear();
        self.stop_price.clear();
        match self.entry_type {
            EntryType::Market => {}
            EntryType::Limit => {
                self.limit_price = entry_str;
            }
            EntryType::Stop => {
                self.stop_price = entry_str;
            }
            EntryType::StopLimit => {
                // With only a single stored price per compound we
                // round-trip it into the limit field. The stop trigger
                // will be re-entered by the user if needed.
                self.limit_price = entry_str;
            }
        }
    }
}

// ===========================================================================
// Supporting enums
// ===========================================================================

/// Buy or sell direction for the order panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum OrderSide {
    Buy,
    Sell,
}

/// How the user specifies TP/SL price.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PriceInputMode {
    /// Absolute price level (e.g., 192.00).
    Absolute,
    /// Dollar offset from last price (e.g., +6.50).
    #[allow(dead_code)] // part of planned UI
    Offset,
    /// Percentage from last price (e.g., +3.5%).
    #[allow(dead_code)] // part of planned UI
    Percent,
}

/// Stop loss order type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum StopLossType {
    /// Becomes market order when stop price is hit.
    Stop,
    /// Becomes limit order when stop price is hit.
    #[allow(dead_code)] // part of planned UI
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
    #[allow(dead_code)] // part of planned UI
    pub risk_per_share: f64,
    #[allow(dead_code)] // part of planned UI
    pub reward_per_share: f64,
    pub total_risk: f64,
    pub total_reward: f64,
    #[allow(dead_code)] // part of planned UI
    pub risk_pct: f64,
    #[allow(dead_code)] // part of planned UI
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

    let reward_per_share = tp_price.map(|tp| (tp - entry_price).abs()).unwrap_or(0.0);

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

    let _qty: f64 = match state.quantity.parse() {
        Ok(q) if q > 0.0 => q,
        Ok(_) => {
            errors.push((
                "quantity".to_string(),
                "Quantity must be positive".to_string(),
            ));
            0.0
        }
        Err(_) => {
            errors.push((
                "quantity".to_string(),
                "Invalid quantity (not a number)".to_string(),
            ));
            0.0
        }
    };

    if let Some(last_price) = state.last_price {
        if state.tp_enabled {
            let tp_val: f64 = match state.tp_value.parse() {
                Ok(v) => v,
                Err(_) => {
                    errors.push((
                        "tp".to_string(),
                        "Invalid TP value (not a number)".to_string(),
                    ));
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
                        errors.push((
                            "tp".to_string(),
                            "TP must be above current price for BUY".to_string(),
                        ));
                    }
                    OrderSide::Sell if tp_price >= last_price => {
                        errors.push((
                            "tp".to_string(),
                            "TP must be below current price for SELL".to_string(),
                        ));
                    }
                    _ => {}
                }
            }
        }

        if state.sl_enabled {
            let sl_val: f64 = match state.sl_value.parse() {
                Ok(v) => v,
                Err(_) => {
                    errors.push((
                        "sl".to_string(),
                        "Invalid SL value (not a number)".to_string(),
                    ));
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
                        errors.push((
                            "sl".to_string(),
                            "SL must be below current price for BUY".to_string(),
                        ));
                    }
                    OrderSide::Sell if sl_price <= last_price => {
                        errors.push((
                            "sl".to_string(),
                            "SL must be above current price for SELL".to_string(),
                        ));
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

/// Validate bracket annotation data for submission.
///
/// Unlike `validate_panel()` which validates form string inputs,
/// this validates `OrderBracket` f64 data directly for the
/// chart-driven bracket flow.
pub fn validate_bracket(
    bracket: &midas_chart::widget::order_bracket::OrderBracket,
    quantity: f64,
) -> Vec<(String, String)> {
    use midas_chart::widget::order_bracket::{BracketSide, EntryType};
    let mut errors = Vec::new();

    if quantity <= 0.0 {
        errors.push(("quantity".into(), "Quantity must be positive".into()));
    }

    // Entry type-specific validation.
    match bracket.entry_type {
        EntryType::Market if bracket.entry.line.price <= 0.0 => {
            errors.push(("entry".into(), "No market price available".into()));
        }
        EntryType::Limit if bracket.entry.line.price <= 0.0 => {
            errors.push(("entry".into(), "Limit price must be positive".into()));
        }
        EntryType::Stop if bracket.entry.line.price <= 0.0 => {
            errors.push(("entry".into(), "Stop price must be positive".into()));
        }
        EntryType::StopLimit => {
            if bracket.entry.line.price <= 0.0 {
                errors.push(("entry".into(), "Limit price must be positive".into()));
            }
            match bracket.entry_stop_price {
                None => {
                    errors.push((
                        "entry".into(),
                        "Stop-Limit requires a stop trigger price".into(),
                    ));
                }
                Some(sp) if sp <= 0.0 => {
                    errors.push(("entry".into(), "Stop trigger price must be positive".into()));
                }
                Some(sp) => {
                    // BUY: limit must be ≤ stop; SELL: limit must be ≥ stop.
                    match bracket.side {
                        BracketSide::Long if bracket.entry.line.price > sp => {
                            errors.push((
                                "entry".into(),
                                "Limit price must be at or below stop for BUY".into(),
                            ));
                        }
                        BracketSide::Short if bracket.entry.line.price < sp => {
                            errors.push((
                                "entry".into(),
                                "Limit price must be at or above stop for SELL".into(),
                            ));
                        }
                        _ => {}
                    }
                }
            }
        }
        _ => {}
    }

    if let Some(ref sl) = bracket.stop_loss {
        match bracket.side {
            BracketSide::Long if sl.line.price >= bracket.entry.line.price => {
                errors.push(("sl".into(), "Stop loss must be below entry for BUY".into()));
            }
            BracketSide::Short if sl.line.price <= bracket.entry.line.price => {
                errors.push(("sl".into(), "Stop loss must be above entry for SELL".into()));
            }
            _ => {}
        }
    }

    errors
}

// ===========================================================================
// Instant bracket helpers
// ===========================================================================

/// Compute default bracket prices for an instant bracket.
///
/// Returns `(entry, tp, sl)` where TP/SL are `None` when disabled.
///
/// `price_per_pixel` is the camera's price-per-pixel ratio
/// (`(price_high - price_low) / viewport_height`). When provided, the
/// default offsets are clamped so that TP and SL are always at least
/// `MIN_LEG_PX` pixels from the entry line — guaranteeing the legs are
/// visually distinct and grabbable regardless of zoom level. Without
/// camera info, falls back to percentage-only defaults.
#[allow(dead_code)] // used by legacy paths; may be removed in Slice 4
pub fn default_bracket_prices(
    last_price: f64,
    side: OrderSide,
    tp_enabled: bool,
    sl_enabled: bool,
    price_per_pixel: Option<f64>,
) -> (f64, Option<f64>, Option<f64>) {
    /// Minimum screen-space separation between a leg and entry (pixels).
    const MIN_LEG_PX: f64 = 30.0;

    // Percentage-based defaults.
    let pct_tp_offset = last_price * 0.01; // 1%
    let pct_sl_offset = last_price * 0.005; // 0.5%

    // Screen-space minimum offset (if camera info available).
    let px_min = price_per_pixel
        .map(|ppp| (ppp * MIN_LEG_PX).max(0.01))
        .unwrap_or(0.0);

    let tp_offset = pct_tp_offset.max(px_min);
    let sl_offset = pct_sl_offset.max(px_min);

    let tp = if tp_enabled {
        Some(match side {
            OrderSide::Buy => last_price + tp_offset,
            OrderSide::Sell => last_price - tp_offset,
        })
    } else {
        None
    };
    let sl = if sl_enabled {
        Some(match side {
            OrderSide::Buy => last_price - sl_offset,
            OrderSide::Sell => last_price + sl_offset,
        })
    } else {
        None
    };
    (last_price, tp, sl)
}

/// Populate panel string inputs from a bracket's f64 prices.
///
/// Called after bracket creation and after chart drag to keep the
/// panel in sync with the annotation store (single source of truth).
pub fn sync_panel_from_bracket(
    state: &mut OrderPanelState,
    bracket: &midas_chart::widget::order_bracket::OrderBracket,
) {
    // Sync entry type so the panel dropdown matches the bracket.
    state.entry_type = bracket.entry_type;

    // Sync side.
    state.side = match bracket.side {
        midas_chart::widget::order_bracket::BracketSide::Long => OrderSide::Buy,
        midas_chart::widget::order_bracket::BracketSide::Short => OrderSide::Sell,
    };

    // Sync quantity.
    if let Some(qty) = bracket.quantity {
        state.quantity = format!("{}", qty);
    }

    // Entry price → limit_price / stop_price depending on entry_type.
    let entry_str = format!("{:.2}", bracket.entry.line.price);
    match bracket.entry_type {
        midas_chart::widget::order_bracket::EntryType::Market => {
            // Market entry tracks last_price; no panel input to sync.
        }
        midas_chart::widget::order_bracket::EntryType::Limit => {
            state.limit_price = entry_str;
        }
        midas_chart::widget::order_bracket::EntryType::Stop => {
            state.stop_price = entry_str;
        }
        midas_chart::widget::order_bracket::EntryType::StopLimit => {
            state.limit_price = entry_str;
            if let Some(sp) = bracket.entry_stop_price {
                state.stop_price = format!("{:.2}", sp);
            }
        }
    }

    // TP: sync enabled flag and price value.
    state.tp_enabled = bracket.take_profit.is_some();
    if let Some(ref tp) = bracket.take_profit {
        state.tp_value = format!("{:.2}", tp.line.price);
    } else {
        state.tp_value.clear();
    }

    // SL: sync enabled flag and price value.
    state.sl_enabled = bracket.stop_loss.is_some();
    if let Some(ref sl) = bracket.stop_loss {
        state.sl_value = format!("{:.2}", sl.line.price);
    } else {
        state.sl_value.clear();
    }
}

// ===========================================================================
// Bracket normalization
// ===========================================================================

/// Normalize a bracket so its data matches its `entry_type` rules.
///
/// Called after loading from disk or recalling a saved bracket to ensure
/// stale data from old code versions doesn't produce incorrect chart lines.
/// This is the single source of truth for what a bracket of each type
/// should contain.
///
/// **Entry lines by type:**
/// - Market: entry at last price, NOT draggable. No stop trigger.
/// - Limit: entry at limit price, draggable. No stop trigger.
/// - Stop: entry at stop price, draggable. No stop trigger.
/// - StopLimit: entry at limit price + stop trigger line, both draggable.
///
/// **TP/SL legs:** Preserved as-is (user-controlled via panel toggles).
/// Only cleared if they have degenerate values (price <= 0).
///
/// **Side constraints:** TP/SL on wrong side of entry are mirrored
/// to the correct side (preserves offset distance from entry).
pub fn normalize_bracket(bracket: &mut midas_chart::widget::order_bracket::OrderBracket) {
    use midas_chart::widget::order_bracket::{BracketSide, EntryType};

    // ── Entry stop_price by type ───────────────────────────────────
    match bracket.entry_type {
        EntryType::Market | EntryType::Limit | EntryType::Stop => {
            bracket.entry_stop_price = None;
        }
        EntryType::StopLimit => {
            if bracket.entry_stop_price.is_none() {
                bracket.entry_stop_price = Some(bracket.entry.line.price);
            }
        }
    }

    // ── Validate TP leg ────────────────────────────────────────────
    // Long TP must be above entry; Short TP must be below entry.
    // If on wrong side (e.g., after a side flip), mirror it.
    if let Some(ref mut tp) = bracket.take_profit {
        if tp.line.price <= 0.0 {
            bracket.take_profit = None;
        } else {
            let offset = (tp.line.price - bracket.entry.line.price).abs();
            tp.line.price = match bracket.side {
                BracketSide::Long => bracket.entry.line.price + offset,
                BracketSide::Short => bracket.entry.line.price - offset,
            };
        }
    }

    // ── Validate SL leg ────────────────────────────────────────────
    // Long SL must be below entry; Short SL must be above entry.
    // If on wrong side (e.g., after a side flip), mirror it.
    if let Some(ref mut sl) = bracket.stop_loss {
        if sl.line.price <= 0.0 {
            bracket.stop_loss = None;
        } else {
            let offset = (sl.line.price - bracket.entry.line.price).abs();
            sl.line.price = match bracket.side {
                BracketSide::Long => bracket.entry.line.price - offset,
                BracketSide::Short => bracket.entry.line.price + offset,
            };
        }
    }

    // ── Validate entry price ───────────────────────────────────────
    if bracket.entry.line.price <= 0.0 {
        // Degenerate bracket — mark as cancelled so it doesn't render
        // interactively but is still visible as a dim line.
        bracket.status = midas_chart::widget::order_bracket::BracketStatus::Cancelled;
    }
}

// ===========================================================================
// Hide / recall helpers
// ===========================================================================

/// Whether a bracket's entry price has drifted far enough from the
/// current price to warrant repositioning on recall.
///
/// Returns `true` when `|entry - current| > gatr_abs`. Falls back
/// to 5% of current price when G.ATR is unavailable.
pub fn should_reposition(entry_price: f64, current_price: f64, gatr_abs: Option<f64>) -> bool {
    let threshold = gatr_abs.unwrap_or_else(|| (current_price.abs() * 0.05).max(0.01));
    (entry_price - current_price).abs() > threshold
}

/// Shift all bracket legs by a delta to center the entry near the
/// current price. Preserves R:R shape (TP and SL offsets unchanged).
pub fn reposition_bracket(
    bracket: &mut midas_chart::widget::order_bracket::OrderBracket,
    current_price: f64,
) {
    let delta = current_price - bracket.entry.line.price;
    bracket.entry.line.price += delta;
    if let Some(ref mut sp) = bracket.entry_stop_price {
        *sp += delta;
    }
    if let Some(ref mut tp) = bracket.take_profit {
        tp.line.price += delta;
    }
    if let Some(ref mut sl) = bracket.stop_loss {
        sl.line.price += delta;
    }
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
        line: midas_chart::widget::PriceLine {
            price,
            extent: midas_chart::widget::LineExtent::FullWidth,
            stroke: midas_chart::widget::LineStroke {
                color: [0.0, 0.0, 0.0, 1.0],
                width: 1.5,
                style: LineStyle::Solid,
            },
        },
        role: LegRole::Entry,
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
        saved: false,
        filled_qty: None,
        entry_type: EntryType::Market,
        entry_stop_price: None,
        wrong_side_warning: false,
    }
}

/// Map a broker lifecycle status string to a chart `BracketStatus`.
///
/// The broker engine uses `BracketLifecycleStatus` which is a separate
/// type in `midas-broker`. This function bridges the string
/// representation to the chart-side enum without creating a hard
/// dependency between `midas-app` and `midas-broker`.
#[allow(dead_code)] // part of planned API
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
    #[allow(dead_code)] // part of planned API
    pub id: OrderPanelId,
    /// Form state (side, quantity, TP/SL, validation, confirmation).
    pub state: OrderPanelState,
    /// Symbol link group for cross-panel symbol propagation.
    pub symbol_link: LinkMode,
    /// Bound symbol key resolved from the symbol-link color group.
    ///
    /// Set by [`crate::app::MidasApp::bind_panel_to_symbol`]. `None`
    /// means the panel is unbound. Persisted in config for restart.
    pub bound_symbol: Option<crate::annotation_store::SymbolKey>,
}

impl OrderPanel {
    /// Create a new dockable order panel with the given symbol.
    pub fn new(id: OrderPanelId, symbol: String) -> Self {
        let bound = if symbol.is_empty() {
            None
        } else {
            Some(crate::annotation_store::SymbolKey::new(&symbol))
        };
        let state = OrderPanelState {
            symbol,
            visible: true, // always visible in docked mode
            ..Default::default()
        };
        Self {
            id,
            state,
            symbol_link: LinkMode::default(),
            bound_symbol: bound,
        }
    }

    /// Serialize this panel's state to a config struct for persistence.
    pub fn to_config(&self) -> midas_core::config::OrderPanelConfig {
        let bracket_active = self.state.bracket_active.map(|side| match side {
            OrderSide::Buy => "BUY".to_string(),
            OrderSide::Sell => "SELL".to_string(),
        });
        midas_core::config::OrderPanelConfig {
            symbol: self.state.symbol.clone(),
            side: match self.state.side {
                OrderSide::Buy => "BUY".to_string(),
                OrderSide::Sell => "SELL".to_string(),
            },
            quantity: self.state.quantity.clone(),
            symbol_link: self.symbol_link,
            bracket_active,
            bound_symbol: self.bound_symbol.as_ref().map(|k| k.as_str().to_string()),
        }
    }

    /// Restore a panel from a saved config.
    /// Restore a panel from a saved config.
    pub fn from_config(id: OrderPanelId, config: &midas_core::config::OrderPanelConfig) -> Self {
        let bracket_active = config.bracket_active.as_deref().and_then(|s| match s {
            "BUY" => Some(OrderSide::Buy),
            "SELL" => Some(OrderSide::Sell),
            _ => None,
        });
        // Restore bound_symbol from config, falling back to the legacy `symbol` field.
        let bound_symbol = config
            .bound_symbol
            .as_deref()
            .or(Some(config.symbol.as_str()))
            .filter(|s| !s.is_empty())
            .map(crate::annotation_store::SymbolKey::new);
        let state = OrderPanelState {
            symbol: config.symbol.clone(),
            side: if config.side == "SELL" {
                OrderSide::Sell
            } else {
                OrderSide::Buy
            },
            quantity: config.quantity.clone(),
            visible: true,
            bracket_active,
            ..Default::default()
        };
        Self {
            id,
            state,
            symbol_link: config.symbol_link,
            bound_symbol,
        }
    }

    /// Re-link this panel to a hidden saved Draft bracket in the annotation store.
    ///
    /// `annotations` must be the slice for **this panel's symbol** (i.e.,
    /// from `annotation_store.get(&self.state.symbol)`). The first hidden
    /// saved Draft bracket found is claimed. Only the first panel for a
    /// given symbol should call this (ownership semantics).
    #[allow(dead_code)] // called from app init path (not yet wired)
    pub fn relink_hidden_bracket(&mut self, annotations: &[midas_chart::widget::Annotation]) {
        use midas_chart::widget::order_bracket::BracketStatus;
        use midas_chart::widget::{AnnotationKind, Presence};

        if self.state.bracket_annotation_id.is_some() {
            return; // Already linked
        }

        let found = annotations.iter().find(|a| {
            a.presence == Presence::Hidden
                && matches!(&a.kind, AnnotationKind::OrderBracket(b)
                    if b.status == BracketStatus::Draft && b.saved)
        });

        if let Some(ann) = found {
            self.state.bracket_annotation_id = Some(ann.id);
        }
    }
}

/// Actions for a specific order panel instance.
#[derive(Debug, Clone)]
pub enum OrderPanelAction {
    /// Set the order side (Buy/Sell) without changing bracket mode.
    #[allow(dead_code)] // retained for non-bracket side changes
    SetSide(OrderSide),
    /// Update the quantity input text.
    SetQuantity(String),
    /// Toggle Take Profit enabled.
    ToggleTp(bool),
    /// Set TP price input mode.
    #[allow(dead_code)] // part of planned UI
    SetTpMode(PriceInputMode),
    /// Update TP value input text.
    SetTpValue(String),
    /// Toggle Stop Loss enabled.
    ToggleSl(bool),
    /// Set SL price input mode.
    #[allow(dead_code)] // part of planned UI
    SetSlMode(PriceInputMode),
    /// Update SL value input text.
    SetSlValue(String),
    /// Set SL type (Stop vs StopLimit).
    #[allow(dead_code)] // part of planned UI
    SetSlType(StopLossType),
    /// Update SL limit price input text.
    #[allow(dead_code)] // part of planned UI
    SetSlLimit(String),
    /// Submit the order (triggers confirmation dialog).
    Submit,
    /// User confirmed the order in the confirmation dialog.
    ConfirmYes,
    /// User cancelled the confirmation dialog.
    ConfirmNo,
    /// Dismiss the order panel (close confirmation or clear errors).
    #[allow(dead_code)] // part of planned UI
    Dismiss,
    /// Set the bracket chart toggle mode.
    /// `Some(Buy/Sell)` activates a bracket on the chart.
    /// `None` deactivates (clears unsaved bracket, caches state).
    SetBracketMode(Option<OrderSide>),
    /// Set the entry order type (Market, Limit, Stop, StopLimit).
    SetEntryType(EntryType),
    /// Update the limit price input text.
    SetLimitPrice(String),
    /// Update the stop price input text.
    SetStopPrice(String),
    /// Increment/decrement a price field by a delta (from mouse wheel).
    StepPrice {
        /// Which price field to adjust.
        field: PriceField,
        /// Amount to add (positive = up, negative = down).
        delta: f64,
    },
}

/// Which price field a `StepPrice` action targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PriceField {
    /// Take profit price.
    Tp,
    /// Stop loss price.
    Sl,
    /// Limit entry price.
    LimitPrice,
    /// Stop entry price.
    StopPrice,
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests;
