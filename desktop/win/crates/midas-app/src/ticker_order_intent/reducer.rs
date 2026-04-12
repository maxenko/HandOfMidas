//! Top-level reducer dispatcher for [`OrderIntentAppMsg`].
//!
//! The enum locked in Slice 1a is preserved verbatim. Slice 3 fills in
//! the first four match arms (`UpdateFromPanel`, `UpdateFromBracketDrag`,
//! `CancelLiveBracket`, `RemoveLiveBracket`) with real handler bodies.
//! Slice 4 fills in the remaining GATR / pin / undo arms.
//!
//! # Design
//!
//! The handler bodies are deliberately split into two layers:
//!
//! - [`apply_order_intent_msg`] — the public dispatcher. Takes
//!   `&mut MidasApp` so `app.rs::update()` can route with a one-line
//!   call. Extracts the mutable references the handlers need and
//!   forwards them into the pure helpers.
//! - [`apply_update_from_surface`], [`apply_cancel_live_bracket`],
//!   [`apply_remove_live_bracket`] — the actual reducer logic, taking
//!   bare `&mut AnnotationStore`, `&impl TickerIntentAccess`, and a
//!   `&mut HashMap<OrderPanelId, OrderPanel>` so unit tests can drive
//!   them without constructing a full `MidasApp`.
//!
//! This mirrors the Slice 2 `bootstrap::bootstrap_from_annotations`
//! pattern: trait-shaped inputs → a function you can test in isolation.
//!
//! # Feedback-loop guard (D3 source-tagged refresh)
//!
//! Every write carries an `IntentSource`. After the reducer applies the
//! write, it refreshes only the *opposite* surface: a `Chart`-sourced
//! update re-syncs the panel; a `Panel`-sourced update re-syncs the
//! chart annotation. The originating surface already has the new state,
//! so touching it a second time would echo right back into the reducer.
//!
//! The actor's cache-equality check is the second line of defense: if
//! an echo does sneak through, the store returns `NoOpReason::IdenticalToCache`
//! and the reducer early-returns with `Task::none()`.

use std::collections::HashMap;

use iced::Task;
use midas_chart::widget::order_bracket::{BracketSide, OrderBracket};
use midas_chart::widget::{AnnotationId, AnnotationKind};

use crate::annotation_store::{AnnotationStore, SymbolKey};
use crate::app::{MidasApp, Message};
use crate::order_panel::{OrderPanel, OrderSide};
use midas_core::OrderPanelId;

use super::handle::TickerIntentAccess;
use super::{
    EntryMemory, IntentSource, OrderIntentMsg, TickerOrderIntent, UpsertOutcome,
};

/// Top-level reducer message for ticker-intent updates. Wrapped by
/// `Message::OrderIntent` in [`crate::app::Message`] and routed into
/// [`apply_order_intent_msg`] from `MidasApp::update`.
///
/// Locked at Slice 1a: Slices 3 and 4 fill in handler bodies but do
/// not add or remove variants.
#[derive(Debug, Clone)]
#[allow(dead_code)] // Slice 4 consumes the GATR / pin / undo variants.
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

/// Top-level reducer entry point.
///
/// Called from `MidasApp::update()` to dispatch an
/// [`OrderIntentAppMsg`] into the matching handler. The Slice 4 GATR /
/// pin / undo arms remain `Task::none()` stubs.
pub fn apply_order_intent_msg(app: &mut MidasApp, msg: OrderIntentAppMsg) -> Task<Message> {
    match msg {
        OrderIntentAppMsg::UpdateFromPanel {
            symbol,
            snapshot,
            source,
        }
        | OrderIntentAppMsg::UpdateFromBracketDrag {
            symbol,
            snapshot,
            source,
        } => {
            let active_symbol = app
                .active_chart_symbol()
                .map(|s| SymbolKey::new(&s));
            apply_update_from_surface(
                &mut app.annotation_store,
                &app.order_intent_handle,
                &mut app.order_panels,
                active_symbol.as_ref(),
                symbol,
                *snapshot,
                source,
            );
            Task::none()
        }
        OrderIntentAppMsg::CancelLiveBracket { symbol } => {
            apply_cancel_live_bracket(
                &mut app.annotation_store,
                &app.order_intent_handle,
                &mut app.order_panels,
                &symbol,
            );
            Task::none()
        }
        OrderIntentAppMsg::RemoveLiveBracket {
            symbol,
            annotation_id,
        } => {
            apply_remove_live_bracket(
                &app.order_intent_handle,
                &mut app.order_panels,
                &symbol,
                annotation_id,
            );
            Task::none()
        }
        // Slice 4 fills in the GATR / pin / undo handler bodies.
        OrderIntentAppMsg::MaybeSnapToGatr { .. } => Task::none(),
        OrderIntentAppMsg::TogglePin { .. } => Task::none(),
        OrderIntentAppMsg::UndoSnap { .. } => Task::none(),
    }
}

/// Apply an `UpdateFromPanel` / `UpdateFromBracketDrag` message against
/// the given dependencies.
///
/// Implements the Slice 3 reducer flow:
///
/// 1. Symbol-consistency guard. A `Chart`-sourced drag message whose
///    `symbol` does not match the active chart's symbol is dropped with
///    a `warn!` log — the user switched tickers mid-drag. Panel-sourced
///    messages are exempt because the panel can target any symbol.
/// 2. Cache-equality short-circuit. If the inbound snapshot matches the
///    cached intent byte-for-byte, drop as NoOp.
/// 3. Compound-key merge. Extract `(snapshot.last_side,
///    snapshot.last_entry_type)` from the snapshot and write the
///    matching `EntryMemory` bucket into the cached intent. All other
///    buckets are preserved — SL-off-per-compound is sticky.
/// 4. Upsert through the `TickerIntentAccess` handle.
/// 5. If `intent.live_annotation_id.is_some()`, propagate the new leg
///    prices into the annotation via `AnnotationStore::update`.
/// 6. Refresh-skip-matching-source: on a `Chart` source, re-hydrate the
///    linked order panel; on a `Panel` source, the annotation is the
///    refresh target and was already touched in step 5. This is the
///    feedback-loop guard.
pub(crate) fn apply_update_from_surface(
    annotation_store: &mut AnnotationStore,
    handle: &impl TickerIntentAccess,
    panels: &mut HashMap<OrderPanelId, OrderPanel>,
    active_chart_symbol: Option<&SymbolKey>,
    symbol: SymbolKey,
    snapshot: TickerOrderIntent,
    source: IntentSource,
) {
    // ── (1) Symbol-consistency guard (Chart-sourced only). ────────────
    if source == IntentSource::Chart {
        match active_chart_symbol {
            Some(active) if *active == symbol => {}
            other => {
                tracing::warn!(
                    target: "ticker_intent::reducer",
                    "dropping mid-drag bracket update: snapshot symbol={} active={:?}",
                    symbol.as_str(),
                    other.map(|s| s.as_str())
                );
                return;
            }
        }
    }

    // ── (2) Cache-equality short-circuit. ─────────────────────────────
    let cached = handle.snapshot(&symbol);
    if let Some(ref existing) = cached {
        if **existing == snapshot {
            return;
        }
    }

    // ── (3) Compound-key merge. ───────────────────────────────────────
    // Preserve every bucket from the cached intent; overwrite only the
    // bucket the inbound snapshot advertises as its `last_*` key.
    let side = snapshot.last_side;
    let entry_type = snapshot.last_entry_type;
    let inbound_memory = snapshot
        .entries
        .get(&(side, entry_type))
        .cloned()
        .unwrap_or_else(EntryMemory::default);

    let mut merged = match cached.as_ref() {
        Some(arc) => (**arc).clone(),
        None => TickerOrderIntent::new(symbol.clone()),
    };
    merged.symbol = symbol.clone();
    merged.last_side = side;
    merged.last_entry_type = entry_type;
    merged.entries.insert((side, entry_type), inbound_memory);
    merged.live_annotation_id = snapshot.live_annotation_id;
    merged.updated_at = snapshot.updated_at;

    // ── (4) Upsert. ───────────────────────────────────────────────────
    let outcome = handle.upsert(OrderIntentMsg::Upsert {
        symbol: symbol.clone(),
        intent: Box::new(merged.clone()),
        source,
    });
    if matches!(outcome, UpsertOutcome::NoOp { .. }) {
        return;
    }

    // ── (5) Propagate into the linked annotation, if any. ─────────────
    if let Some(ann_id) = merged.live_annotation_id {
        let leg_prices = extract_leg_prices(&snapshot, side, entry_type);
        annotation_store.update(symbol.as_str(), ann_id, |ann| {
            if let AnnotationKind::OrderBracket(ref mut bracket) = ann.kind {
                apply_leg_prices(bracket, side, &leg_prices);
            }
        });
    }

    // ── (6) Refresh-skip-matching-source. ─────────────────────────────
    // `Chart` source  → re-hydrate the linked panel (chart already current).
    // `Panel` source  → annotation already updated in step 5; skip panel.
    // Other sources (Hydration / Bootstrap / GatrSnap) refresh the panel
    // as a conservative default so startup-seeded memory reaches the UI.
    if !matches!(source, IntentSource::Panel) {
        for panel in panels.values_mut() {
            if panel.state.symbol.eq_ignore_ascii_case(symbol.as_str()) {
                panel.state.hydrate_from_intent(&merged, panel.state.last_price);
            }
        }
    }
}

/// Apply a `CancelLiveBracket` message.
///
/// - Removes the linked annotation from the store (if any).
/// - Clears `intent.live_annotation_id`, preserving every
///   `EntryMemory` bucket and the `last_side` / `last_entry_type`
///   compound key. "Forget this particular bracket, not the user's
///   preferences."
/// - Resets the `dirty` flag on every panel linked to either the
///   symbol or the old annotation id. This is the non-negotiable
///   edit-then-undo rule: once the live bracket is gone, Slice 2's
///   hydration guard would otherwise refuse to re-hydrate on the next
///   `ChartActivated` and leave the panel stuck on stale input.
pub(crate) fn apply_cancel_live_bracket(
    annotation_store: &mut AnnotationStore,
    handle: &impl TickerIntentAccess,
    panels: &mut HashMap<OrderPanelId, OrderPanel>,
    symbol: &SymbolKey,
) {
    let Some(arc) = handle.snapshot(symbol) else {
        return;
    };
    let mut cleared = (*arc).clone();
    let old_ann = cleared.live_annotation_id.take();
    cleared.updated_at = chrono::Utc::now();

    if let Some(ann_id) = old_ann {
        annotation_store.remove(symbol.as_str(), ann_id);
    }

    // Dirty-reset rule.
    for panel in panels.values_mut() {
        let matches_symbol = panel.state.symbol.eq_ignore_ascii_case(symbol.as_str());
        let matches_ann = old_ann.is_some()
            && panel.state.bracket_annotation_id == old_ann;
        if matches_symbol || matches_ann {
            panel.state.dirty = false;
            panel.state.bracket_annotation_id = None;
        }
    }

    let _ = handle.upsert(OrderIntentMsg::Upsert {
        symbol: symbol.clone(),
        intent: Box::new(cleared),
        source: IntentSource::Panel,
    });
}

/// Apply a `RemoveLiveBracket` message — the inverse hook for external
/// removals (undo, hotkey, drag-off-chart).
///
/// Only acts if `intent.live_annotation_id == Some(annotation_id)`.
/// The annotation itself is **not** removed here — the caller already
/// removed it (that is what "external removal" means). The reducer just
/// reconciles the intent's back-link and resets the panel's dirty flag.
pub(crate) fn apply_remove_live_bracket(
    handle: &impl TickerIntentAccess,
    panels: &mut HashMap<OrderPanelId, OrderPanel>,
    symbol: &SymbolKey,
    annotation_id: AnnotationId,
) {
    let Some(arc) = handle.snapshot(symbol) else {
        return;
    };
    if arc.live_annotation_id != Some(annotation_id) {
        return;
    }
    let mut cleared = (*arc).clone();
    cleared.live_annotation_id = None;
    cleared.updated_at = chrono::Utc::now();

    for panel in panels.values_mut() {
        let matches_symbol = panel.state.symbol.eq_ignore_ascii_case(symbol.as_str());
        let matches_ann = panel.state.bracket_annotation_id == Some(annotation_id);
        if matches_symbol || matches_ann {
            panel.state.dirty = false;
            panel.state.bracket_annotation_id = None;
        }
    }

    let _ = handle.upsert(OrderIntentMsg::Upsert {
        symbol: symbol.clone(),
        intent: Box::new(cleared),
        source: IntentSource::Panel,
    });
}

// ── Leg-price projection helpers ─────────────────────────────────────

/// The four scalar prices a bracket snapshot carries into the
/// annotation. All are optional because TP / SL can be absent.
#[derive(Clone, Copy, Debug, Default)]
struct LegPrices {
    entry: Option<f64>,
    tp: Option<f64>,
    sl: Option<f64>,
}

/// Extract the leg prices from a `TickerOrderIntent` snapshot for the
/// given compound bucket. Returns default (all `None`) when the bucket
/// is absent — the annotation is left untouched in that case.
fn extract_leg_prices(
    intent: &TickerOrderIntent,
    side: OrderSide,
    entry_type: midas_chart::widget::order_bracket::EntryType,
) -> LegPrices {
    let Some(memory) = intent.entries.get(&(side, entry_type)) else {
        return LegPrices::default();
    };
    LegPrices {
        entry: memory.entry_price_or_offset,
        tp: memory.tp_value.parse::<f64>().ok(),
        sl: memory.sl_value.parse::<f64>().ok(),
    }
}

/// Apply a set of leg prices to a live bracket annotation. Leaves any
/// leg whose price is `None` untouched. Also syncs the `side` field so
/// the chart reflects the panel's latest toggle.
fn apply_leg_prices(bracket: &mut OrderBracket, side: OrderSide, prices: &LegPrices) {
    bracket.side = match side {
        OrderSide::Buy => BracketSide::Long,
        OrderSide::Sell => BracketSide::Short,
    };
    if let Some(entry) = prices.entry {
        bracket.entry.line.price = entry;
    }
    if let (Some(tp_price), Some(ref mut tp)) = (prices.tp, bracket.take_profit.as_mut()) {
        tp.line.price = tp_price;
    }
    if let (Some(sl_price), Some(ref mut sl)) = (prices.sl, bracket.stop_loss.as_mut()) {
        sl.line.price = sl_price;
    }
}

#[cfg(test)]
mod tests;
