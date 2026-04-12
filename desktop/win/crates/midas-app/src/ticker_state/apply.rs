//! The `apply()` method and its supporting enums.
//!
//! `TickerState::apply()` is the sole mutation entry point. It takes a
//! [`TickerMsg`], mutates `self`, and returns a `Vec<TickerEffect>` that
//! the caller in `MidasApp::update()` interprets mechanically.
//!
//! # Slice 1 status
//!
//! All bracket lifecycle and field-mutation handlers are implemented.
//! Broker/GATR/levels remain stubs (Slice 2).

use midas_chart::widget::order_bracket::{
    BracketLeg, BracketSide, BracketStatus, EntryType, LegRole, OrderBracket,
};
use midas_chart::widget::AnnotationId;

use crate::app::ToastAction;
use crate::level_store::StoredLevel;
use crate::order_panel::OrderSide;
use crate::ticker_order_intent::price_defaults::default_initial_prices;

use super::{EditingField, TickerState};

// ── TickerMsg ───────────────────────────────────────────────────────

/// The complete mutation vocabulary for [`TickerState`].
///
/// Every UI interaction, broker event, or market data update that
/// touches per-symbol state is modeled as a variant of this enum.
/// `TickerState::apply()` pattern-matches on it exhaustively.
#[derive(Debug, Clone)]
pub enum TickerMsg {
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

    // ── Persistence ──────────────────────────────────────────────

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
    /// This is the sole mutation entry point. Bracket lifecycle and
    /// field-mutation handlers are fully implemented (Slice 1).
    /// Broker/GATR/levels remain stubs (Slice 2).
    #[allow(unused_variables)] // Slice 2 stubs do not use fields yet
    pub fn apply(&mut self, msg: TickerMsg) -> Vec<TickerEffect> {
        match msg {
            // ── Bracket lifecycle ────────────────────────────────

            TickerMsg::EnsureDraftBracket { side, entry_type } => {
                self.last_side = side;
                self.last_entry_type = entry_type;
                self.generation += 1;

                let bracket_side = to_bracket_side(side);
                let current_price = self.last_price.unwrap_or(0.0);

                // If a live bracket already exists, flip side + entry type.
                if let Some(ref mut b) = self.live_bracket {
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

            TickerMsg::CancelBracket => {
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

            TickerMsg::SaveBracket => {
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

            TickerMsg::DeleteBracket => {
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

            TickerMsg::RecallBracket => {
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
                        && crate::order_panel::should_reposition(
                            b.entry.line.price,
                            last,
                            self.gatr_abs,
                        )
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

            // ── Bracket field mutations ──────────────────────────

            TickerMsg::SetLegPrice { role, price } => {
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

            TickerMsg::SetTpEnabled(enabled) => {
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

            TickerMsg::SetSlEnabled(enabled) => {
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

            TickerMsg::SetQuantity(qty) => {
                if let Some(ref mut b) = self.live_bracket {
                    self.generation += 1;
                    b.quantity = Some(qty);
                    vec![TickerEffect::ProjectBracket(b.clone())]
                } else {
                    vec![]
                }
            }

            TickerMsg::SetSide(side) => {
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

            TickerMsg::SetEntryType(entry_type) => {
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

            TickerMsg::DragLeg { role, new_price } => {
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
                                    tp.projected_pnl =
                                        Some(sign * (new_price - entry_price) * qty);
                                    tp.projected_pnl_pct =
                                        if entry_price.abs() > f64::EPSILON {
                                            Some(
                                                sign * (new_price - entry_price) / entry_price
                                                    * 100.0,
                                            )
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
                                    sl.projected_pnl =
                                        Some(sign * (new_price - entry_price) * qty);
                                    sl.projected_pnl_pct =
                                        if entry_price.abs() > f64::EPSILON {
                                            Some(
                                                sign * (new_price - entry_price) / entry_price
                                                    * 100.0,
                                            )
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

            // ── Text editing focus lock ──────────────────────────

            TickerMsg::BeginEdit(field) => {
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

            TickerMsg::UpdateEditValue(text) => {
                self.editing_value = Some(text);
                vec![]
            }

            TickerMsg::CommitEdit { field, value } => {
                self.generation += 1;
                let effects = self.apply_commit(field, value);
                self.editing_field = None;
                self.editing_value = None;
                effects
            }

            TickerMsg::CancelEdit => {
                self.generation += 1;
                self.editing_field = None;
                self.editing_value = None;
                vec![]
            }

            // ── GATR ─────────────────────────────────────────────
            TickerMsg::MaybeSnap {
                current_price,
                gatr_abs,
            } => vec![],
            TickerMsg::TogglePin => vec![],
            TickerMsg::UndoSnap => vec![],

            // ── Market data ──────────────────────────────────────
            TickerMsg::UpdateMarketData {
                last_price,
                gatr_abs,
            } => vec![],

            // ── Levels ───────────────────────────────────────────
            TickerMsg::AddLevel(_) => vec![],
            TickerMsg::RemoveLevel(_) => vec![],
            TickerMsg::UpdateLevel { index, level } => vec![],
            TickerMsg::ToggleLevelLock(_) => vec![],

            // ── Broker events ────────────────────────────────────
            TickerMsg::SubmitOrder => {
                if let Some(ref mut b) = self.live_bracket {
                    self.generation += 1;
                    b.status = BracketStatus::Pending;
                    vec![
                        TickerEffect::ProjectBracket(b.clone()),
                        TickerEffect::PersistDirty,
                    ]
                } else {
                    vec![]
                }
            }
            TickerMsg::OrderPending { order_id } => vec![],
            TickerMsg::OrderFilled {
                filled_qty,
                avg_price,
            } => vec![],
            TickerMsg::OrderPartialFill { filled_qty } => vec![],
            TickerMsg::OrderRejected { reason } => {
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
            TickerMsg::OrderCancelled => vec![],

            // ── Persistence ──────────────────────────────────────
            TickerMsg::Hydrated(_) => vec![],
        }
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
}
