//! Top-level reducer dispatcher for [`OrderIntentAppMsg`].
//!
//! This file defines the locked-at-Slice-1a message enum, plus a
//! [`apply_order_intent_msg`] stub that returns [`iced::Task::none`]
//! for every variant. Slice 3 fills in the
//! panel / chart / cancel / remove arms; Slice 4 fills in the
//! GATR / pin / undo arms. The enum itself is *not* re-opened by
//! either downstream slice — new behaviour becomes a new handler
//! body inside an existing match arm.

use iced::Task;
use midas_chart::widget::AnnotationId;

use crate::annotation_store::SymbolKey;
use crate::app::{MidasApp, Message};

use super::{IntentSource, TickerOrderIntent};

/// Top-level reducer message for ticker-intent updates. Wrapped by
/// `Message::OrderIntent` in [`crate::app::Message`] and routed into
/// [`apply_order_intent_msg`] from `MidasApp::update`.
///
/// Locked at Slice 1a: Slices 3 and 4 fill in handler bodies but do
/// not add or remove variants.
#[derive(Debug, Clone)]
#[allow(dead_code)] // Slice 3/4 bodies land later; variants exist now.
pub enum OrderIntentAppMsg {
    /// The order panel was edited. Carries the full snapshot of the
    /// new intent so the reducer can compare it against the cached
    /// value and short-circuit identical writes.
    UpdateFromPanel {
        /// Symbol the panel is editing.
        symbol: SymbolKey,
        /// Full post-edit snapshot.
        snapshot: Box<TickerOrderIntent>,
        /// Source tag — always [`IntentSource::Panel`] in practice,
        /// but carried through so the feedback-loop guard is explicit.
        source: IntentSource,
    },
    /// The chart bracket was dragged. Same shape as `UpdateFromPanel`
    /// but with a different source tag so the refresh step can skip
    /// the chart widget.
    UpdateFromBracketDrag {
        /// Symbol the bracket belongs to — captured at drag-start so
        /// mid-drag ticker switches are detectable.
        symbol: SymbolKey,
        /// Full post-drag snapshot.
        snapshot: Box<TickerOrderIntent>,
        /// Source tag — always [`IntentSource::Chart`].
        source: IntentSource,
    },
    /// The live bracket for a symbol was cancelled (e.g. via panel X).
    CancelLiveBracket {
        /// Symbol whose bracket is being cancelled.
        symbol: SymbolKey,
    },
    /// The live bracket annotation for a symbol is being removed
    /// (e.g. because the user deleted it from the chart).
    RemoveLiveBracket {
        /// Symbol whose annotation is being removed.
        symbol: SymbolKey,
        /// ID of the annotation to drop.
        annotation_id: AnnotationId,
    },
    /// Fire the GATR snap rule for a symbol, if the guards pass.
    /// Slice 4 implements this.
    MaybeSnapToGatr {
        /// Symbol to evaluate.
        symbol: SymbolKey,
    },
    /// Toggle the pin state on a symbol's intent. Slice 4 implements.
    TogglePin {
        /// Symbol to toggle.
        symbol: SymbolKey,
    },
    /// Undo the most recent GATR snap for a symbol. Slice 4 implements.
    UndoSnap {
        /// Symbol whose snap is being undone.
        symbol: SymbolKey,
    },
}

/// Stub reducer for Slice 1a.
///
/// Every variant returns [`Task::none`]. Slice 3 replaces the first
/// four arms with real handlers; Slice 4 replaces the last three.
/// The signature is locked now so call sites can be written today.
pub fn apply_order_intent_msg(_app: &mut MidasApp, msg: OrderIntentAppMsg) -> Task<Message> {
    match msg {
        OrderIntentAppMsg::UpdateFromPanel { .. } => Task::none(),
        OrderIntentAppMsg::UpdateFromBracketDrag { .. } => Task::none(),
        OrderIntentAppMsg::CancelLiveBracket { .. } => Task::none(),
        OrderIntentAppMsg::RemoveLiveBracket { .. } => Task::none(),
        OrderIntentAppMsg::MaybeSnapToGatr { .. } => Task::none(),
        OrderIntentAppMsg::TogglePin { .. } => Task::none(),
        OrderIntentAppMsg::UndoSnap { .. } => Task::none(),
    }
}
