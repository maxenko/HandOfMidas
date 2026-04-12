//! The `apply()` method and its supporting enums.
//!
//! `TickerState::apply()` is the sole mutation entry point. It takes a
//! [`TickerMsg`], mutates `self`, and returns a `Vec<TickerEffect>` that
//! the caller in `MidasApp::update()` interprets mechanically.
//!
//! # Slice 2 status
//!
//! All handlers are implemented: bracket lifecycle, field mutations,
//! broker events, GATR snap/pin/undo, level CRUD, and market data.

use std::time::{Duration, Instant};

use chrono::Utc;
use midas_chart::widget::order_bracket::{
    BracketLeg, BracketSide, BracketStatus, EntryType, LegRole, OrderBracket,
};
use midas_chart::widget::AnnotationId;

use crate::app::ToastAction;
use crate::level_store::StoredLevel;
use crate::order_panel::OrderSide;

use super::price_defaults::default_initial_prices;

/// Minimum absolute GATR value before the snap rule is allowed to fire.
pub(crate) const MIN_GATR_ABS: f64 = 1e-9;

/// Minimum recency (in seconds) before the snap rule is allowed to fire.
pub(crate) const RECENCY_GUARD_SECS: i64 = 60 * 60;

use super::{EditingField, PreSnapState, TickerState};

/// Session-bounded TTL for the GATR snap undo slot.
const SNAP_UNDO_TTL: Duration = Duration::from_secs(30);

// ── TickerMsg ───────────────────────────────────────────────────────

/// The complete mutation vocabulary for [`TickerState`].
///
/// Every UI interaction, broker event, or market data update that
/// touches per-symbol state is modeled as a variant of this enum.
/// `TickerState::apply()` pattern-matches on it exhaustively.
#[derive(Debug, Clone)]
pub enum TickerMsg {
    // ── Bracket mode (BUY / X / SELL toggle) ─────────────────────
    /// Set the bracket mode. `Some(side)` activates brackets for the
    /// given side; `None` deactivates brackets (X button).
    SetBracketMode(Option<OrderSide>),

    // ── Bracket lifecycle ────────────────────────────────────────
    /// Ensure a draft bracket exists for the given side and entry type.
    /// Creates one if none exists; recalls the saved one if it does.
    EnsureDraftBracket {
        /// Trade direction.
        side: OrderSide,
        /// Entry order type.
        entry_type: EntryType,
    },
    /// Cancel the live bracket (hide without deleting saved state).
    CancelBracket,
    /// Save the live bracket (pin it so it survives toggle).
    SaveBracket,
    /// Delete the live bracket and its saved state.
    DeleteBracket,
    /// Recall a previously saved bracket.
    RecallBracket,

    // ── Bracket field mutations ──────────────────────────────────
    /// Set the price of a specific bracket leg.
    SetLegPrice {
        /// Which leg to modify.
        role: LegRole,
        /// New absolute price.
        price: f64,
    },
    /// Enable or disable the take-profit leg.
    SetTpEnabled(bool),
    /// Enable or disable the stop-loss leg.
    SetSlEnabled(bool),
    /// Set the bracket quantity.
    SetQuantity(f64),
    /// Change the trade direction.
    SetSide(OrderSide),
    /// Change the entry order type.
    SetEntryType(EntryType),
    /// Drag a bracket leg to a new price (from chart interaction).
    DragLeg {
        /// Which leg is being dragged.
        role: LegRole,
        /// New price after the drag delta.
        new_price: f64,
    },

    // ── Text editing focus lock ─────────────────────────────────
    /// Focus entered a text field. Sets the editing lock and clears
    /// the in-progress value.
    BeginEdit(EditingField),
    /// Keystroke in the locked field. Updates the in-progress text
    /// without triggering implicit commit.
    UpdateEditValue(String),
    /// Enter or blur: applies the final value and clears the lock.
    CommitEdit {
        /// Which field is being committed.
        field: EditingField,
        /// The final text value to apply.
        value: String,
    },
    /// Escape: reverts to pre-edit state and clears the lock.
    CancelEdit,

    // ── GATR ─────────────────────────────────────────────────────
    /// Evaluate the GATR snap rule for the current price.
    MaybeSnap {
        /// Current market price.
        current_price: f64,
        /// Current absolute GATR, if available.
        gatr_abs: Option<f64>,
    },
    /// Toggle the GATR pin state.
    TogglePin,
    /// Undo the last GATR snap (restore pre-snap state).
    UndoSnap,

    // ── Market data ──────────────────────────────────────────────
    /// Update cached market data for this symbol.
    UpdateMarketData {
        /// Latest price.
        last_price: f64,
        /// Latest absolute GATR.
        gatr_abs: Option<f64>,
    },

    // ── Levels ───────────────────────────────────────────────────
    /// Add a new price level.
    AddLevel(StoredLevel),
    /// Remove a price level by index.
    RemoveLevel(usize),
    /// Replace a price level at the given index.
    UpdateLevel {
        /// Index of the level to update.
        index: usize,
        /// New level data.
        level: StoredLevel,
    },
    /// Toggle the lock state of a level at the given index.
    ToggleLevelLock(usize),

    // ── Broker events ────────────────────────────────────────────
    /// Submit the live bracket to the broker.
    SubmitOrder,
    /// Broker acknowledged the order submission.
    OrderPending {
        /// Broker-assigned order identifier.
        #[allow(dead_code)]
        order_id: uuid::Uuid,
    },
    /// Order fully filled.
    OrderFilled {
        /// Total filled quantity.
        filled_qty: f64,
        /// Volume-weighted average fill price.
        avg_price: f64,
    },
    /// Order partially filled.
    OrderPartialFill {
        /// Quantity filled so far.
        filled_qty: f64,
    },
    /// Order rejected by the broker.
    OrderRejected {
        /// Human-readable rejection reason.
        reason: String,
    },
    /// Order cancelled (by user or broker).
    OrderCancelled,

    // ── Camera ──────────��────────────────────────────────────────
    /// Save the current camera viewport for this ticker.
    ///
    /// Fired once per user gesture (pan release, scroll-wheel tick)
    /// — not per frame.
    SaveCameraState {
        /// Visible time range start (epoch ms).
        time_start: f64,
        /// Visible time range end (epoch ms).
        time_end: f64,
        /// Visible price range bottom.
        price_low: f64,
        /// Visible price range top.
        price_high: f64,
        /// Whether the most recent candle was visible in the viewport.
        was_at_live_edge: bool,
    },

    // ── Persistence ���─────────────────────────────────────────────
    /// A persisted state was loaded from disk. Replaces the current
    /// in-memory state wholesale.
    Hydrated(Box<TickerState>),
}

// ── TickerEffect ────────────────────────────────────────────────────

/// Side-effects returned by [`TickerState::apply`].
///
/// The caller in `MidasApp::update()` interprets these mechanically.
/// `TickerState` never touches `AnnotationStore` or the broker bridge
/// directly — it only describes what should happen.
#[derive(Debug, Clone)]
pub enum TickerEffect {
    /// Project the bracket into `AnnotationStore`.
    ProjectBracket(OrderBracket),
    /// Remove a bracket annotation from `AnnotationStore`.
    RemoveBracket(AnnotationId),
    /// Project a level into `AnnotationStore`.
    ProjectLevel {
        /// Index of the level in `TickerState.levels`.
        index: usize,
        /// The level to project.
        level: StoredLevel,
    },
    /// Remove a level annotation from `AnnotationStore`.
    RemoveLevel {
        /// The annotation ID of the level to remove.
        annotation_id: AnnotationId,
    },
    /// Show a toast notification.
    Toast {
        /// Human-readable message.
        message: String,
        /// Optional action button (e.g. "Undo").
        action: Option<ToastAction>,
    },
    /// Mark this symbol's state as dirty for persistence.
    PersistDirty,
    /// Submit a bracket to the broker engine.
    SubmitToBroker {
        /// The bracket to submit.
        bracket: OrderBracket,
    },
}

// ── Helper: make a default bracket leg ─────────────────────────────

/// Create a `BracketLeg` with default stroke at the given price.
fn make_leg(price: f64, role: LegRole) -> BracketLeg {
    BracketLeg {
        line: midas_chart::widget::PriceLine {
            price,
            extent: midas_chart::widget::LineExtent::FullWidth,
            stroke: midas_chart::widget::LineStroke {
                color: [0.0, 0.0, 0.0, 1.0],
                width: 1.0,
                style: midas_chart::widget::LineStyle::Solid,
            },
        },
        role,
        projected_pnl: None,
        projected_pnl_pct: None,
    }
}

/// Convert an [`OrderSide`] to the chart-crate [`BracketSide`].
fn to_bracket_side(side: OrderSide) -> BracketSide {
    match side {
        OrderSide::Buy => BracketSide::Long,
        OrderSide::Sell => BracketSide::Short,
    }
}

// ── apply() implementation ──────────────────────────────────────────

impl TickerState {
    /// Apply a message to this ticker state, returning any side-effects.
    ///
    /// This is the sole mutation entry point. All handlers are fully
    /// implemented: bracket lifecycle, field mutations (Slice 1),
    /// broker events, GATR snap/pin/undo, level CRUD, and market data
    /// (Slice 2).
    ///
    /// Each category is dispatched to a private helper method.
    pub fn apply(&mut self, msg: TickerMsg) -> Vec<TickerEffect> {
        match msg {
            // ── Bracket mode ────────────────────────────────────
            TickerMsg::SetBracketMode(mode) => self.apply_set_bracket_mode(mode),

            // ── Bracket lifecycle ────────────────────────────────
            TickerMsg::EnsureDraftBracket { side, entry_type } => {
                self.apply_ensure_draft(side, entry_type)
            }
            TickerMsg::CancelBracket => self.apply_cancel_bracket(),
            TickerMsg::SaveBracket => self.apply_save_bracket(),
            TickerMsg::DeleteBracket => self.apply_delete_bracket(),
            TickerMsg::RecallBracket => self.apply_recall_bracket(),

            // ── Bracket field mutations ──────────────────────────
            TickerMsg::SetLegPrice { role, price } => self.apply_set_leg_price(role, price),
            TickerMsg::SetTpEnabled(enabled) => self.apply_set_tp_enabled(enabled),
            TickerMsg::SetSlEnabled(enabled) => self.apply_set_sl_enabled(enabled),
            TickerMsg::SetQuantity(qty) => self.apply_set_quantity(qty),
            TickerMsg::SetSide(side) => self.apply_set_side(side),
            TickerMsg::SetEntryType(entry_type) => self.apply_set_entry_type(entry_type),
            TickerMsg::DragLeg { role, new_price } => self.apply_drag_leg(role, new_price),

            // ── Text editing focus lock ──────────────────────────
            TickerMsg::BeginEdit(field) => self.apply_begin_edit(field),
            TickerMsg::UpdateEditValue(text) => self.apply_update_edit_value(text),
            TickerMsg::CommitEdit { field, value } => self.apply_commit_edit(field, value),
            TickerMsg::CancelEdit => self.apply_cancel_edit(),

            // ── GATR ─────────────────────────────────────────────
            TickerMsg::MaybeSnap {
                current_price,
                gatr_abs,
            } => self.apply_maybe_snap(current_price, gatr_abs),
            TickerMsg::TogglePin => self.apply_toggle_pin(),
            TickerMsg::UndoSnap => self.apply_undo_snap(),

            // ── Market data ──────────────────────────────────────
            TickerMsg::UpdateMarketData {
                last_price,
                gatr_abs,
            } => self.apply_market_data(last_price, gatr_abs),

            // ── Levels ───────────────────────────────────────────
            TickerMsg::AddLevel(level) => self.apply_add_level(level),
            TickerMsg::RemoveLevel(index) => self.apply_remove_level(index),
            TickerMsg::UpdateLevel { index, level } => self.apply_update_level(index, level),
            TickerMsg::ToggleLevelLock(index) => self.apply_toggle_level_lock(index),

            // ── Broker events ────────────────────────────────────
            TickerMsg::SubmitOrder => self.apply_submit_order(),
            TickerMsg::OrderPending { order_id } => self.apply_order_pending(order_id),
            TickerMsg::OrderFilled {
                filled_qty,
                avg_price,
            } => self.apply_order_filled(filled_qty, avg_price),
            TickerMsg::OrderPartialFill { filled_qty } => self.apply_order_partial_fill(filled_qty),
            TickerMsg::OrderRejected { reason } => self.apply_order_rejected(reason),
            TickerMsg::OrderCancelled => self.apply_order_cancelled(),

            // ── Camera ──────────────────────────────────────────────
            TickerMsg::SaveCameraState {
                time_start,
                time_end,
                price_low,
                price_high,
                was_at_live_edge,
            } => self.apply_save_camera(time_start, time_end, price_low, price_high, was_at_live_edge),

            // ── Persistence (one-liner, left inline) ─────────────
            TickerMsg::Hydrated(_) => vec![],
        }
    }

    // ── Bracket mode ─────────────────────────────────────────────────

    fn apply_set_bracket_mode(&mut self, mode: Option<OrderSide>) -> Vec<TickerEffect> {
        self.generation += 1;
        self.bracket_mode = mode;
        match mode {
            Some(side) => {
                self.last_side = side;
                // Delegate to ensure_draft to create/update the bracket.
                // bracket_mode is now Some, so the guard passes.
                let mut effects = self.apply_ensure_draft(side, self.last_entry_type);
                // Always persist regardless of whether ensure_draft
                // produced effects (it may have if bracket already existed).
                if !effects
                    .iter()
                    .any(|e| matches!(e, TickerEffect::PersistDirty))
                {
                    effects.push(TickerEffect::PersistDirty);
                }
                effects
            }
            None => {
                // Deactivate: cancel any existing bracket.
                let mut effects = self.apply_cancel_bracket();
                if !effects
                    .iter()
                    .any(|e| matches!(e, TickerEffect::PersistDirty))
                {
                    effects.push(TickerEffect::PersistDirty);
                }
                effects
            }
        }
    }

    // ── Bracket lifecycle ───────────────────────────────────────────

    fn apply_ensure_draft(&mut self, side: OrderSide, entry_type: EntryType) -> Vec<TickerEffect> {
        // Respect the BUY/X/SELL toggle: if bracket_mode is None (X),
        // no bracket should be created regardless of the trigger.
        if self.bracket_mode.is_none() {
            return vec![];
        }

        self.last_side = side;
        self.last_entry_type = entry_type;
        self.generation += 1;

        let bracket_side = to_bracket_side(side);
        let current_price = self.last_price.unwrap_or(0.0);

        // If a live bracket already exists, validate it. If its
        // prices are stale (0.0, NaN, or wildly off from current
        // price), replace it with fresh defaults. Otherwise just
        // flip side + entry type.
        if let Some(ref mut b) = self.live_bracket {
            let entry_price = b.entry.line.price;
            let stale = !entry_price.is_finite()
                || entry_price <= 0.0
                || (current_price > 0.0
                    && crate::order_panel::should_reposition(
                        entry_price,
                        current_price,
                        self.gatr_abs,
                    ));
            if stale && current_price > 0.0 {
                // Replace with fresh defaults at the current price.
                // Reset ALL fields to a clean Draft state — the old
                // bracket might have been Pending/Active/saved from a
                // prior session and those flags make it non-interactive.
                let prices = default_initial_prices(side, entry_type, current_price, self.gatr_abs);
                b.entry = make_leg(prices.entry, LegRole::Entry);
                b.take_profit = Some(make_leg(prices.take_profit, LegRole::TakeProfit));
                b.stop_loss = Some(make_leg(prices.stop_loss, LegRole::StopLoss));
                b.entry_stop_price = prices.stop_trigger;
                b.status = BracketStatus::Draft;
                b.saved = false;
                b.filled_qty = None;
                b.wrong_side_warning = false;
            }
            // Always reset to Draft — EnsureDraftBracket means
            // "ensure a DRAFT." A prior session might have left
            // the bracket as Pending/Active/Filled, which makes
            // the entry line non-interactive in the app-level
            // hit-test (chart_widget.rs only allows Draft entry
            // drags).
            b.status = BracketStatus::Draft;
            b.saved = false;
            b.side = bracket_side;
            b.entry_type = entry_type;
            crate::order_panel::normalize_bracket(b);
            return vec![
                TickerEffect::ProjectBracket(b.clone()),
                TickerEffect::PersistDirty,
            ];
        }

        // Build a fresh draft bracket from price_defaults.
        let prices = default_initial_prices(side, entry_type, current_price, self.gatr_abs);

        let tp_leg = Some(make_leg(prices.take_profit, LegRole::TakeProfit));
        let sl_leg = Some(make_leg(prices.stop_loss, LegRole::StopLoss));

        let bracket = OrderBracket {
            entry: make_leg(prices.entry, LegRole::Entry),
            take_profit: tp_leg,
            stop_loss: sl_leg,
            side: bracket_side,
            status: BracketStatus::Draft,
            quantity: None,
            saved: false,
            filled_qty: None,
            entry_type,
            entry_stop_price: prices.stop_trigger,
            wrong_side_warning: false,
        };

        self.live_bracket = Some(bracket.clone());
        vec![
            TickerEffect::ProjectBracket(bracket),
            TickerEffect::PersistDirty,
        ]
    }

    fn apply_cancel_bracket(&mut self) -> Vec<TickerEffect> {
        self.generation += 1;
        if let Some(ref b) = self.live_bracket {
            if b.saved {
                // Saved brackets: remove from chart but keep in
                // TickerState for future recall. Clear live state.
                let effects = if let Some(id) = self.live_annotation_id {
                    vec![TickerEffect::RemoveBracket(id), TickerEffect::PersistDirty]
                } else {
                    vec![TickerEffect::PersistDirty]
                };
                self.live_bracket = None;
                self.live_annotation_id = None;
                effects
            } else {
                // Unsaved brackets: delete entirely.
                let effects = if let Some(id) = self.live_annotation_id {
                    vec![TickerEffect::RemoveBracket(id), TickerEffect::PersistDirty]
                } else {
                    vec![TickerEffect::PersistDirty]
                };
                self.live_bracket = None;
                self.live_annotation_id = None;
                effects
            }
        } else {
            vec![]
        }
    }

    fn apply_save_bracket(&mut self) -> Vec<TickerEffect> {
        self.generation += 1;
        if let Some(ref mut b) = self.live_bracket {
            b.saved = true;
            vec![
                TickerEffect::ProjectBracket(b.clone()),
                TickerEffect::PersistDirty,
            ]
        } else {
            vec![]
        }
    }

    fn apply_delete_bracket(&mut self) -> Vec<TickerEffect> {
        self.generation += 1;
        let effects = if let Some(id) = self.live_annotation_id {
            vec![TickerEffect::RemoveBracket(id), TickerEffect::PersistDirty]
        } else {
            vec![TickerEffect::PersistDirty]
        };
        self.live_bracket = None;
        self.live_annotation_id = None;
        effects
    }

    fn apply_recall_bracket(&mut self) -> Vec<TickerEffect> {
        // Recall the saved bracket that was previously cancelled
        // (hidden). Currently live_bracket is None after CancelBracket
        // on a saved bracket. The caller must reconstruct the bracket
        // from annotation_store and set it back via EnsureDraftBracket.
        // This variant exists for parity; the actual recall is done
        // by the effect handler reading annotation_store.
        self.generation += 1;
        if let Some(ref mut b) = self.live_bracket {
            // If somehow live_bracket is present, just project it.
            let last = self.last_price.unwrap_or(0.0);
            if last > 0.0
                && crate::order_panel::should_reposition(b.entry.line.price, last, self.gatr_abs)
            {
                crate::order_panel::reposition_bracket(b, last);
            }
            crate::order_panel::normalize_bracket(b);
            vec![
                TickerEffect::ProjectBracket(b.clone()),
                TickerEffect::PersistDirty,
            ]
        } else {
            vec![]
        }
    }

    // ── Bracket field mutations ─────────────────────────────────────

    fn apply_set_leg_price(&mut self, role: LegRole, price: f64) -> Vec<TickerEffect> {
        if let Some(ref mut b) = self.live_bracket {
            self.generation += 1;
            match role {
                LegRole::Entry => b.entry.line.price = price,
                LegRole::TakeProfit => {
                    if let Some(ref mut tp) = b.take_profit {
                        tp.line.price = price;
                    }
                }
                LegRole::StopLoss => {
                    if let Some(ref mut sl) = b.stop_loss {
                        sl.line.price = price;
                    }
                }
                LegRole::StopTrigger => {
                    b.entry_stop_price = Some(price);
                }
            }
            vec![TickerEffect::ProjectBracket(b.clone())]
        } else {
            vec![]
        }
    }

    fn apply_set_tp_enabled(&mut self, enabled: bool) -> Vec<TickerEffect> {
        if let Some(ref mut b) = self.live_bracket {
            self.generation += 1;
            if enabled && b.take_profit.is_none() {
                let entry = b.entry.line.price;
                let offset = (entry * 0.01).max(0.01);
                let tp_price = match b.side {
                    BracketSide::Long => entry + offset,
                    BracketSide::Short => entry - offset,
                };
                b.take_profit = Some(make_leg(tp_price, LegRole::TakeProfit));
            } else if !enabled {
                b.take_profit = None;
            }
            vec![
                TickerEffect::ProjectBracket(b.clone()),
                TickerEffect::PersistDirty,
            ]
        } else {
            vec![]
        }
    }

    fn apply_set_sl_enabled(&mut self, enabled: bool) -> Vec<TickerEffect> {
        if let Some(ref mut b) = self.live_bracket {
            self.generation += 1;
            if enabled && b.stop_loss.is_none() {
                let entry = b.entry.line.price;
                let offset = (entry * 0.005).max(0.01);
                let sl_price = match b.side {
                    BracketSide::Long => entry - offset,
                    BracketSide::Short => entry + offset,
                };
                b.stop_loss = Some(make_leg(sl_price, LegRole::StopLoss));
            } else if !enabled {
                b.stop_loss = None;
            }
            vec![
                TickerEffect::ProjectBracket(b.clone()),
                TickerEffect::PersistDirty,
            ]
        } else {
            vec![]
        }
    }

    fn apply_set_quantity(&mut self, qty: f64) -> Vec<TickerEffect> {
        if let Some(ref mut b) = self.live_bracket {
            self.generation += 1;
            b.quantity = Some(qty);
            vec![TickerEffect::ProjectBracket(b.clone())]
        } else {
            vec![]
        }
    }

    fn apply_set_side(&mut self, side: OrderSide) -> Vec<TickerEffect> {
        self.last_side = side;
        self.generation += 1;
        if let Some(ref mut b) = self.live_bracket {
            b.side = to_bracket_side(side);
            crate::order_panel::normalize_bracket(b);
            vec![
                TickerEffect::ProjectBracket(b.clone()),
                TickerEffect::PersistDirty,
            ]
        } else {
            vec![TickerEffect::PersistDirty]
        }
    }

    fn apply_set_entry_type(&mut self, entry_type: EntryType) -> Vec<TickerEffect> {
        self.last_entry_type = entry_type;
        self.generation += 1;
        if let Some(ref mut b) = self.live_bracket {
            let last_price = self.last_price.unwrap_or(0.0);
            b.entry_type = entry_type;
            b.wrong_side_warning = false;
            match entry_type {
                EntryType::Market => {
                    b.entry.line.price = last_price;
                    b.entry_stop_price = None;
                }
                EntryType::Limit => {
                    // Keep current entry price if non-zero; else use
                    // last_price.
                    if b.entry.line.price.abs() < f64::EPSILON {
                        b.entry.line.price = last_price;
                    }
                    b.entry_stop_price = None;
                }
                EntryType::Stop => {
                    if b.entry.line.price.abs() < f64::EPSILON {
                        b.entry.line.price = last_price;
                    }
                    b.entry_stop_price = None;
                }
                EntryType::StopLimit => {
                    if b.entry.line.price.abs() < f64::EPSILON {
                        b.entry.line.price = last_price;
                    }
                    if b.entry_stop_price.is_none() {
                        b.entry_stop_price = Some(b.entry.line.price);
                    }
                }
            }
            vec![
                TickerEffect::ProjectBracket(b.clone()),
                TickerEffect::PersistDirty,
            ]
        } else {
            vec![TickerEffect::PersistDirty]
        }
    }

    fn apply_drag_leg(&mut self, role: LegRole, new_price: f64) -> Vec<TickerEffect> {
        if let Some(ref mut b) = self.live_bracket {
            self.generation += 1;
            let entry_price = b.entry.line.price;
            let qty = b.quantity.unwrap_or(0.0);
            let sign = match b.side {
                BracketSide::Long => 1.0,
                BracketSide::Short => -1.0,
            };
            match role {
                LegRole::Entry => {
                    b.entry.line.price = new_price;
                }
                LegRole::TakeProfit => {
                    if let Some(ref mut tp) = b.take_profit {
                        tp.line.price = new_price;
                        if b.status == BracketStatus::Active {
                            tp.projected_pnl = Some(sign * (new_price - entry_price) * qty);
                            tp.projected_pnl_pct = if entry_price.abs() > f64::EPSILON {
                                Some(sign * (new_price - entry_price) / entry_price * 100.0)
                            } else {
                                None
                            };
                        }
                    }
                }
                LegRole::StopLoss => {
                    if let Some(ref mut sl) = b.stop_loss {
                        sl.line.price = new_price;
                        if b.status == BracketStatus::Active {
                            sl.projected_pnl = Some(sign * (new_price - entry_price) * qty);
                            sl.projected_pnl_pct = if entry_price.abs() > f64::EPSILON {
                                Some(sign * (new_price - entry_price) / entry_price * 100.0)
                            } else {
                                None
                            };
                        }
                    }
                }
                LegRole::StopTrigger => {
                    b.entry_stop_price = Some(new_price);
                }
            }
            vec![TickerEffect::ProjectBracket(b.clone())]
        } else {
            vec![]
        }
    }

    // ── Text editing focus lock ─────────────────────────────────────

    fn apply_begin_edit(&mut self, field: EditingField) -> Vec<TickerEffect> {
        self.generation += 1;
        // Auto-commit the current field if switching to a different one.
        let mut effects = Vec::new();
        if let Some(current_field) = self.editing_field {
            if current_field != field {
                if let Some(ref value) = self.editing_value {
                    let commit_effects = self.apply_commit(current_field, value.clone());
                    effects.extend(commit_effects);
                }
            }
        }
        self.editing_field = Some(field);
        self.editing_value = None;
        effects
    }

    fn apply_update_edit_value(&mut self, text: String) -> Vec<TickerEffect> {
        self.editing_value = Some(text);
        vec![]
    }

    fn apply_commit_edit(&mut self, field: EditingField, value: String) -> Vec<TickerEffect> {
        self.generation += 1;
        let effects = self.apply_commit(field, value);
        self.editing_field = None;
        self.editing_value = None;
        effects
    }

    fn apply_cancel_edit(&mut self) -> Vec<TickerEffect> {
        self.generation += 1;
        self.editing_field = None;
        self.editing_value = None;
        vec![]
    }

    // ── GATR ────────────────────────────────────────────────────────

    fn apply_toggle_pin(&mut self) -> Vec<TickerEffect> {
        self.generation += 1;
        self.pinned = !self.pinned;
        vec![TickerEffect::PersistDirty]
    }

    fn apply_undo_snap(&mut self) -> Vec<TickerEffect> {
        if let Some((ref snap, instant)) = self.pre_snap {
            if instant.elapsed() <= SNAP_UNDO_TTL {
                let snap = snap.clone();
                self.generation += 1;
                self.entries = snap.entries;
                self.gatr_anchor = snap.gatr_anchor;
                if let Some(ref bracket) = snap.bracket {
                    self.live_bracket = Some(*bracket.clone());
                    self.pre_snap = None;
                    return vec![
                        TickerEffect::ProjectBracket(
                            self.live_bracket
                                .as_ref()
                                .expect("just set live_bracket")
                                .clone(),
                        ),
                        TickerEffect::PersistDirty,
                    ];
                }
                self.pre_snap = None;
                return vec![TickerEffect::PersistDirty];
            }
        }
        // TTL expired or no snap to undo.
        self.pre_snap = None;
        vec![]
    }

    // ── Market data ─────────────────────────────────────────────────

    fn apply_market_data(&mut self, last_price: f64, gatr_abs: Option<f64>) -> Vec<TickerEffect> {
        self.last_price = Some(last_price);
        self.gatr_abs = gatr_abs;
        // If the user is actively editing, skip auto-snap.
        if self.editing_field.is_some() {
            return vec![];
        }
        // Auto-trigger snap check with the fresh data.
        self.apply_maybe_snap(last_price, gatr_abs)
    }

    // ── Levels ──────────────────────────────────────────────────────

    fn apply_add_level(&mut self, level: StoredLevel) -> Vec<TickerEffect> {
        self.generation += 1;
        self.levels.push(level.clone());
        let index = self.levels.len() - 1;
        vec![
            TickerEffect::ProjectLevel { index, level },
            TickerEffect::PersistDirty,
        ]
    }

    fn apply_remove_level(&mut self, index: usize) -> Vec<TickerEffect> {
        if index < self.levels.len() {
            self.generation += 1;
            self.levels.remove(index);
            vec![TickerEffect::PersistDirty]
        } else {
            vec![]
        }
    }

    fn apply_update_level(&mut self, index: usize, level: StoredLevel) -> Vec<TickerEffect> {
        if index < self.levels.len() {
            self.generation += 1;
            self.levels[index] = level.clone();
            vec![
                TickerEffect::ProjectLevel { index, level },
                TickerEffect::PersistDirty,
            ]
        } else {
            vec![]
        }
    }

    fn apply_toggle_level_lock(&mut self, index: usize) -> Vec<TickerEffect> {
        if index < self.levels.len() {
            self.generation += 1;
            self.levels[index].locked = !self.levels[index].locked;
            vec![TickerEffect::PersistDirty]
        } else {
            vec![]
        }
    }

    // ── Broker events ───────────────────────────────────────────────

    fn apply_submit_order(&mut self) -> Vec<TickerEffect> {
        if let Some(ref mut b) = self.live_bracket {
            self.generation += 1;
            b.status = BracketStatus::Pending;
            let bracket = b.clone();
            vec![
                TickerEffect::SubmitToBroker {
                    bracket: bracket.clone(),
                },
                TickerEffect::ProjectBracket(bracket),
                TickerEffect::PersistDirty,
            ]
        } else {
            vec![]
        }
    }

    fn apply_order_pending(&mut self, _order_id: uuid::Uuid) -> Vec<TickerEffect> {
        // Bracket already in Pending state from SubmitOrder.
        // Record that the broker acknowledged it. Currently the
        // order_id is tracked in order_annotation_links, not on
        // TickerState, so this is a no-op for the state machine.
        // The caller wires the link externally.
        if let Some(ref b) = self.live_bracket {
            self.generation += 1;
            vec![TickerEffect::ProjectBracket(b.clone())]
        } else {
            vec![]
        }
    }

    fn apply_order_filled(&mut self, filled_qty: f64, avg_price: f64) -> Vec<TickerEffect> {
        if let Some(ref mut b) = self.live_bracket {
            self.generation += 1;
            b.status = BracketStatus::Active;
            b.filled_qty = Some(filled_qty);
            b.entry.line.price = avg_price;

            // Compute projected P&L on TP/SL legs.
            let sign = match b.side {
                BracketSide::Long => 1.0,
                BracketSide::Short => -1.0,
            };
            let qty = b.quantity.unwrap_or(filled_qty);
            if let Some(ref mut tp) = b.take_profit {
                tp.projected_pnl = Some(sign * (tp.line.price - avg_price) * qty);
                tp.projected_pnl_pct = if avg_price.abs() > f64::EPSILON {
                    Some(sign * (tp.line.price - avg_price) / avg_price * 100.0)
                } else {
                    None
                };
            }
            if let Some(ref mut sl) = b.stop_loss {
                sl.projected_pnl = Some(sign * (sl.line.price - avg_price) * qty);
                sl.projected_pnl_pct = if avg_price.abs() > f64::EPSILON {
                    Some(sign * (sl.line.price - avg_price) / avg_price * 100.0)
                } else {
                    None
                };
            }

            vec![
                TickerEffect::ProjectBracket(b.clone()),
                TickerEffect::Toast {
                    message: format!(
                        "{} filled {filled_qty} @ {avg_price:.2}",
                        self.symbol.as_str()
                    ),
                    action: None,
                },
                TickerEffect::PersistDirty,
            ]
        } else {
            vec![]
        }
    }

    fn apply_order_partial_fill(&mut self, filled_qty: f64) -> Vec<TickerEffect> {
        if let Some(ref mut b) = self.live_bracket {
            self.generation += 1;
            b.status = BracketStatus::PartialFill;
            b.filled_qty = Some(filled_qty);
            vec![
                TickerEffect::ProjectBracket(b.clone()),
                TickerEffect::PersistDirty,
            ]
        } else {
            vec![]
        }
    }

    fn apply_order_rejected(&mut self, reason: String) -> Vec<TickerEffect> {
        if let Some(ref mut b) = self.live_bracket {
            self.generation += 1;
            b.status = BracketStatus::Draft;
            vec![
                TickerEffect::ProjectBracket(b.clone()),
                TickerEffect::Toast {
                    message: format!("Order rejected: {reason}"),
                    action: None,
                },
            ]
        } else {
            vec![]
        }
    }

    fn apply_order_cancelled(&mut self) -> Vec<TickerEffect> {
        if let Some(ref mut b) = self.live_bracket {
            self.generation += 1;
            b.status = BracketStatus::Draft;
            vec![
                TickerEffect::ProjectBracket(b.clone()),
                TickerEffect::Toast {
                    message: format!("{} bracket cancelled", self.symbol.as_str()),
                    action: None,
                },
                TickerEffect::PersistDirty,
            ]
        } else {
            vec![]
        }
    }

    // ── Internal helpers ────────────────────────────────────────────

    /// Internal helper: evaluate the GATR snap rule and apply it if
    /// the guards pass.
    ///
    /// Reuses the guard logic from the old `gatr_snap` module
    /// and the repositioning helpers from [`crate::order_panel`].
    fn apply_maybe_snap(&mut self, current_price: f64, gatr_abs: Option<f64>) -> Vec<TickerEffect> {
        // Guard 1: pin.
        if self.pinned {
            return vec![];
        }
        // Guard 2: recency — fresh user edits are sacred.
        let now = Utc::now();
        if now.signed_duration_since(self.updated_at).num_seconds() < RECENCY_GUARD_SECS {
            return vec![];
        }
        // Guard 3: finite inputs + non-tiny GATR.
        if !current_price.is_finite() {
            return vec![];
        }
        let gatr = match gatr_abs {
            Some(g) if g.is_finite() && g > MIN_GATR_ABS => g,
            _ => return vec![],
        };
        // Guard 4: anchor must be seeded.
        let anchor_price = match self.gatr_anchor.anchor_price {
            Some(p) if p.is_finite() => p,
            _ => return vec![],
        };
        // Guard 5: no live bracket required — snap operates on entries too.
        // But we need either a bracket or entries to reposition.
        if self.live_bracket.is_none() && self.entries.is_empty() {
            return vec![];
        }
        // Threshold check using the existing helper.
        if !crate::order_panel::should_reposition(anchor_price, current_price, Some(gatr)) {
            return vec![];
        }

        // Snap fires. Stash pre-snap state for undo.
        let pre_snap = PreSnapState {
            bracket: self.live_bracket.as_ref().map(|b| Box::new(b.clone())),
            entries: self.entries.clone(),
            gatr_anchor: self.gatr_anchor,
        };
        self.pre_snap = Some((Box::new(pre_snap), Instant::now()));
        self.generation += 1;

        // Shift entry memory prices by delta.
        let delta = current_price - anchor_price;
        for mem in self.entries.values_mut() {
            if let Some(ref mut p) = mem.entry_price_or_offset {
                *p += delta;
            }
        }

        // Update anchor.
        self.gatr_anchor.anchor_price = Some(current_price);
        self.gatr_anchor.anchor_gatr = Some(gatr);

        // Reposition bracket if one exists.
        let mut effects = Vec::new();
        if let Some(ref mut b) = self.live_bracket {
            crate::order_panel::reposition_bracket(b, current_price);
            effects.push(TickerEffect::ProjectBracket(b.clone()));
        }

        effects.push(TickerEffect::Toast {
            message: format!("{} re-anchored", self.symbol.as_str()),
            action: Some(ToastAction {
                label: "Undo".to_string(),
                on_click: Box::new(crate::app::Message::Ticker(
                    self.symbol.clone(),
                    TickerMsg::UndoSnap,
                )),
            }),
        });
        effects.push(TickerEffect::PersistDirty);
        effects
    }

    /// Internal helper: apply a committed value to the appropriate bracket
    /// field. Returns the effects of the mutation.
    fn apply_commit(&mut self, field: EditingField, value: String) -> Vec<TickerEffect> {
        if let Some(ref mut b) = self.live_bracket {
            match field {
                EditingField::LimitPrice => {
                    if let Ok(price) = value.parse::<f64>() {
                        b.entry.line.price = price;
                    }
                }
                EditingField::StopPrice => {
                    if let Ok(price) = value.parse::<f64>() {
                        match b.entry_type {
                            EntryType::Stop => b.entry.line.price = price,
                            EntryType::StopLimit => b.entry_stop_price = Some(price),
                            _ => {}
                        }
                    }
                }
                EditingField::TpValue => {
                    if let Ok(price) = value.parse::<f64>() {
                        if let Some(ref mut tp) = b.take_profit {
                            tp.line.price = price;
                        }
                    }
                }
                EditingField::SlValue => {
                    if let Ok(price) = value.parse::<f64>() {
                        if let Some(ref mut sl) = b.stop_loss {
                            sl.line.price = price;
                        }
                    }
                }
                EditingField::SlLimitValue => {
                    // StopLimit SL limit price — not commonly used in V1
                    // but wired for completeness.
                }
                EditingField::Quantity => {
                    if let Ok(qty) = value.parse::<f64>() {
                        b.quantity = Some(qty);
                    }
                }
            }
            vec![
                TickerEffect::ProjectBracket(b.clone()),
                TickerEffect::PersistDirty,
            ]
        } else {
            vec![]
        }
    }

    // ── Camera ──────────────────────────────────────────────────────

    /// Store the current camera viewport for later restore.
    fn apply_save_camera(
        &mut self,
        time_start: f64,
        time_end: f64,
        price_low: f64,
        price_high: f64,
        was_at_live_edge: bool,
    ) -> Vec<TickerEffect> {
        self.camera_time_start = Some(time_start);
        self.camera_time_end = Some(time_end);
        self.camera_price_low = Some(price_low);
        self.camera_price_high = Some(price_high);
        self.camera_was_at_live_edge = was_at_live_edge;
        self.generation += 1;
        vec![TickerEffect::PersistDirty]
    }
}
