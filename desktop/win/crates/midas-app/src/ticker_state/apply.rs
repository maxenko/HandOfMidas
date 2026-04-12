//! The `apply()` method and its supporting enums.
//!
//! `TickerState::apply()` is the sole mutation entry point. It takes a
//! [`TickerMsg`], mutates `self`, and returns a `Vec<TickerEffect>` that
//! the caller in `MidasApp::update()` interprets mechanically.
//!
//! # Stub status
//!
//! Slice 0 lands every variant as a stub returning `vec![]`. Slice 1
//! fills in bracket handlers; Slice 2 fills in broker/GATR/levels.

use midas_chart::widget::order_bracket::{EntryType, LegRole, OrderBracket};
use midas_chart::widget::AnnotationId;

use crate::app::ToastAction;
use crate::level_store::StoredLevel;
use crate::order_panel::OrderSide;

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

// ── apply() implementation ──────────────────────────────────────────

impl TickerState {
    /// Apply a message to this ticker state, returning any side-effects.
    ///
    /// This is the sole mutation entry point. Every variant is currently
    /// a stub returning `vec![]` (Slice 0). Slice 1 fills in bracket
    /// handlers; Slice 2 fills in broker/GATR/levels.
    #[allow(unused_variables)] // stubs do not use the variant fields yet
    pub fn apply(&mut self, msg: TickerMsg) -> Vec<TickerEffect> {
        match msg {
            // ── Bracket lifecycle ────────────────────────────────
            TickerMsg::EnsureDraftBracket { side, entry_type } => vec![],
            TickerMsg::CancelBracket => vec![],
            TickerMsg::SaveBracket => vec![],
            TickerMsg::DeleteBracket => vec![],
            TickerMsg::RecallBracket => vec![],

            // ── Bracket field mutations ──────────────────────────
            TickerMsg::SetLegPrice { role, price } => vec![],
            TickerMsg::SetTpEnabled(_) => vec![],
            TickerMsg::SetSlEnabled(_) => vec![],
            TickerMsg::SetQuantity(_) => vec![],
            TickerMsg::SetSide(_) => vec![],
            TickerMsg::SetEntryType(_) => vec![],
            TickerMsg::DragLeg { role, new_price } => vec![],

            // ── Text editing focus lock ──────────────────────────
            TickerMsg::BeginEdit(_) => vec![],
            TickerMsg::UpdateEditValue(_) => vec![],
            TickerMsg::CommitEdit { field, value } => vec![],
            TickerMsg::CancelEdit => vec![],

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
            TickerMsg::SubmitOrder => vec![],
            TickerMsg::OrderPending { order_id } => vec![],
            TickerMsg::OrderFilled {
                filled_qty,
                avg_price,
            } => vec![],
            TickerMsg::OrderPartialFill { filled_qty } => vec![],
            TickerMsg::OrderRejected { reason } => vec![],
            TickerMsg::OrderCancelled => vec![],

            // ── Persistence ──────────────────────────────────────
            TickerMsg::Hydrated(_) => vec![],
        }
    }
}
