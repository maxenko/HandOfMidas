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
use std::time::Instant;

use iced::Task;
use midas_chart::widget::order_bracket::{BracketSide, OrderBracket};
use midas_chart::widget::{AnnotationId, AnnotationKind};

use crate::annotation_store::{AnnotationStore, SymbolKey};
use crate::app::{Message, MidasApp, ToastAction};
use crate::order_panel::{OrderPanel, OrderSide};
use midas_core::OrderPanelId;

use super::gatr_snap::{maybe_snap, SnapPlan};
use super::handle::TickerIntentAccess;
use super::{
    EntryMemory, GatrAnchor, IntentSource, OrderIntentMsg, TickerOrderIntent, UpsertOutcome,
};

/// Snapshot of the pre-snap state captured immediately before a GATR
/// snap fires, so the user can undo the repositioning within the TTL
/// window.
///
/// The full pre-snap [`TickerOrderIntent`] is stashed so undo restores
/// both surfaces (panel `EntryMemory` + `gatr_anchor`) in one step.
/// When a chart bracket existed at the time of the snap, the pre-snap
/// bracket clone is stashed alongside it; a panel-only snap leaves
/// `bracket = None`.
///
/// The 30-second session-bounded TTL is enforced by the
/// [`apply_undo_snap`] handler at drain time.
#[derive(Clone, Debug)]
pub struct PreSnapState {
    /// The annotation id the snap moved, when one existed.
    pub annotation_id: Option<AnnotationId>,
    /// A clone of the bracket before the snap was applied. `None`
    /// when the snap operated on a panel-only intent.
    pub bracket: Option<Box<OrderBracket>>,
    /// Full pre-snap intent. Undo upserts this verbatim so both the
    /// `EntryMemory` fields and the `gatr_anchor` are restored
    /// atomically.
    pub prev_intent: Box<TickerOrderIntent>,
    /// When the snap fired. Compared against [`UNDO_TTL`] on drain.
    pub stashed_at: Instant,
}

/// Session-bounded TTL for the GATR snap undo slot.
pub const UNDO_TTL: std::time::Duration = std::time::Duration::from_secs(30);

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
            let (current_price, gatr_abs) = app
                .market_cache
                .get(symbol.as_str())
                .map(|s| (s.last_price, s.gatr_abs))
                .unwrap_or((None, None));
            let outcome = apply_update_from_surface(
                &mut app.annotation_store,
                &app.order_intent_handle,
                &mut app.order_panels,
                active_symbol.as_ref(),
                symbol.clone(),
                *snapshot,
                source,
                current_price,
                gatr_abs,
            );
            // Discoverability toast: one-shot per (symbol, session) on
            // the very first write that seeds an anchor.
            if outcome == UpdateSurfaceOutcome::AppliedAnchorSeeded
                && !app.anchor_seed_toasts_shown.contains(&symbol)
            {
                app.anchor_seed_toasts_shown.insert(symbol.clone());
                let display = symbol.as_str().to_string();
                return Task::done(Message::ShowToast {
                    message: format!(
                        "{display}: bracket location recorded. \
                         Pin to lock against drift snap."
                    ),
                    action: None,
                });
            }
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
        OrderIntentAppMsg::MaybeSnapToGatr { symbol } => apply_maybe_snap(app, symbol),
        OrderIntentAppMsg::TogglePin { symbol } => apply_toggle_pin(app, symbol),
        OrderIntentAppMsg::UndoSnap { symbol } => apply_undo_snap(app, symbol),
    }
}

/// Outcome returned by [`apply_update_from_surface`] so the outer
/// dispatcher can decide whether to emit the Slice 4 discoverability
/// toast. None of these variants is an error — the `NoOp` / `Rejected`
/// paths are still valid "nothing further to do".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpdateSurfaceOutcome {
    /// The write did not land (NoOp, rejected by the symbol-consistency
    /// guard, etc.). No downstream side effects.
    Noop,
    /// The write landed but did not seed a fresh GATR anchor.
    Applied,
    /// The write landed **and** transitioned the intent's
    /// `gatr_anchor.anchor_price` from `None` to `Some(_)`. The outer
    /// dispatcher should emit the first-touch discoverability toast.
    AppliedAnchorSeeded,
}

/// Apply an `UpdateFromPanel` / `UpdateFromBracketDrag` message against
/// the given dependencies.
///
/// Implements the Slice 3 reducer flow, extended in Slice 4 with the
/// D4 anchor-seeding rule:
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
/// 4. **Slice 4 anchor-seeding rule**: if the cached intent had
///    `gatr_anchor.anchor_price == None` *before* the upsert AND the
///    inbound `source` is `Panel` or `Chart` (i.e. a real user touch),
///    seed `gatr_anchor` at `(current_price, gatr_abs)` from the
///    `current_price` / `gatr_abs` arguments the dispatcher resolved
///    from the market cache. Return [`UpdateSurfaceOutcome::AppliedAnchorSeeded`].
/// 5. Upsert through the `TickerIntentAccess` handle.
/// 6. If `intent.live_annotation_id.is_some()`, propagate the new leg
///    prices into the annotation via `AnnotationStore::update`.
/// 7. Refresh-skip-matching-source: on a `Chart` source, re-hydrate the
///    linked order panel; on a `Panel` source, the annotation is the
///    refresh target and was already touched in step 6. This is the
///    feedback-loop guard.
#[allow(clippy::too_many_arguments)]
pub(crate) fn apply_update_from_surface(
    annotation_store: &mut AnnotationStore,
    handle: &impl TickerIntentAccess,
    panels: &mut HashMap<OrderPanelId, OrderPanel>,
    active_chart_symbol: Option<&SymbolKey>,
    symbol: SymbolKey,
    snapshot: TickerOrderIntent,
    source: IntentSource,
    current_price: Option<f64>,
    gatr_abs: Option<f64>,
) -> UpdateSurfaceOutcome {
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
                return UpdateSurfaceOutcome::Noop;
            }
        }
    }

    // ── (2) Cache-equality short-circuit. ─────────────────────────────
    let cached = handle.snapshot(&symbol);
    if let Some(ref existing) = cached {
        if **existing == snapshot {
            return UpdateSurfaceOutcome::Noop;
        }
    }

    // Anchor-seed decision: only legal on a real user-initiated touch
    // (Panel or Chart), and only when no anchor has ever been written.
    // Any other source (Bootstrap / Hydration / GatrSnap) leaves the
    // anchor alone — that is the D4 "first-touch-endorses" rule.
    let is_user_touch = matches!(source, IntentSource::Panel | IntentSource::Chart);
    let previously_unseeded = cached
        .as_ref()
        .map(|arc| arc.gatr_anchor.anchor_price.is_none())
        .unwrap_or(true);
    let will_seed_anchor = is_user_touch
        && previously_unseeded
        && current_price.map(|p| p.is_finite()).unwrap_or(false);

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

    // ── (4) Anchor-seeding (Slice 4 / D4). ────────────────────────────
    if will_seed_anchor {
        if let Some(price) = current_price {
            merged.gatr_anchor = GatrAnchor {
                anchor_price: Some(price),
                anchor_gatr: gatr_abs.filter(|g| g.is_finite() && *g > 0.0),
            };
        }
    }

    // ── (5) Upsert. ───────────────────────────────────────────────────
    let outcome = handle.upsert(OrderIntentMsg::Upsert {
        symbol: symbol.clone(),
        intent: Box::new(merged.clone()),
        source,
    });
    if matches!(outcome, UpsertOutcome::NoOp { .. }) {
        return UpdateSurfaceOutcome::Noop;
    }

    // ── (6) Propagate into the linked annotation, if any. ─────────────
    if let Some(ann_id) = merged.live_annotation_id {
        let leg_prices = extract_leg_prices(&snapshot, side, entry_type);
        annotation_store.update(symbol.as_str(), ann_id, |ann| {
            if let AnnotationKind::OrderBracket(ref mut bracket) = ann.kind {
                apply_leg_prices(bracket, side, &leg_prices);
            }
        });
    }

    // ── (7) Refresh-skip-matching-source. ─────────────────────────────
    // `Chart` source  → re-hydrate the linked panel (chart already current).
    // `Panel` source  → annotation already updated in step 6; skip panel.
    // Other sources (Hydration / Bootstrap / GatrSnap) refresh the panel
    // as a conservative default so startup-seeded memory reaches the UI.
    if !matches!(source, IntentSource::Panel) {
        for panel in panels.values_mut() {
            if panel.state.symbol.eq_ignore_ascii_case(symbol.as_str()) {
                panel.state.hydrate_from_intent(&merged, panel.state.last_price);
            }
        }
    }

    if will_seed_anchor {
        UpdateSurfaceOutcome::AppliedAnchorSeeded
    } else {
        UpdateSurfaceOutcome::Applied
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

// ── Slice 4: GATR snap / pin / undo handlers ─────────────────────────

/// Outcome of [`apply_snap_to_intent`] — the pure-function half of
/// the `MaybeSnapToGatr` handler. Captures everything the outer
/// dispatcher needs to stash the undo slot and emit the toast
/// without re-borrowing `app`.
#[derive(Debug)]
pub(crate) struct SnapApplied {
    /// The delta applied to every absolute price (signed).
    pub delta: f64,
    /// The GATR used for the drift ratio shown in the toast.
    pub gatr_abs: Option<f64>,
    /// Pre-snap state to stash in `gatr_undo_slots`.
    pub pre_snap: PreSnapState,
}

/// Apply the GATR snap rule to a symbol's intent, updating both
/// surfaces (chart bracket annotation + `EntryMemory` panel memory)
/// in lockstep by the same `plan.delta`.
///
/// # Design
///
/// This is the canonical entry point for the "$112 stop on a $14
/// chart" bug. Both the chart bracket annotation and the order-panel
/// input fields are **views** of the same [`TickerOrderIntent`];
/// this function is the single place where the snap policy is
/// applied to that intent. Panels and annotations are refreshed as a
/// downstream side effect — neither makes an independent snap
/// decision.
///
/// # Flow
///
/// 1. Snapshot the intent. Early-return on missing intent.
/// 2. Call [`maybe_snap`]. Early-return on `None`.
/// 3. Clone the pre-snap intent (for undo). If the intent carries a
///    live annotation id, also clone the pre-snap bracket.
/// 4. Shift every absolute price in the active compound bucket
///    (`intent.entries[(last_side, last_entry_type)]`) by `plan.delta`.
///    Fields repositioned:
///      - `entry_price_or_offset` (all non-Market entry types)
///      - `tp_value` parsed, when `tp_enabled && tp_mode == Absolute`
///      - `sl_value` parsed, when `sl_enabled && sl_mode == Absolute`
///      - `sl_limit_value` parsed, when `sl_enabled && sl_type == StopLimit`
///
///    Offset / Percent mode fields are relative to entry and not touched.
/// 5. Write `plan.new_anchor` onto the intent and upsert through the
///    handle with `source = GatrSnap`.
/// 6. If `intent.live_annotation_id.is_some()`, reposition the live
///    annotation by the same delta via
///    [`crate::order_panel::reposition_bracket`].
/// 7. Re-hydrate every linked panel so the UI shows the corrected
///    `limit_price` / `stop_price` / `tp_value` / `sl_value`.
///
/// All pre-snap data is returned to the caller in [`SnapApplied`] so
/// the outer handler can stash the undo slot without re-borrowing.
pub(crate) fn apply_snap_to_intent(
    annotation_store: &mut AnnotationStore,
    handle: &impl TickerIntentAccess,
    panels: &mut HashMap<OrderPanelId, OrderPanel>,
    symbol: &SymbolKey,
    current_price: f64,
    gatr_abs: Option<f64>,
) -> Option<SnapApplied> {
    // (1) Snapshot.
    let intent = handle.snapshot(symbol)?;

    // (2) Consult the pure snap rule.
    let SnapPlan {
        delta,
        new_anchor,
        reason: _,
    } = maybe_snap(&intent, current_price, gatr_abs)?;

    // (3) Clone pre-snap state for the undo slot.
    let prev_intent = Box::new((*intent).clone());
    let mut pre_snap_bracket: Option<Box<OrderBracket>> = None;
    if let Some(ann_id) = intent.live_annotation_id {
        annotation_store.update(symbol.as_str(), ann_id, |ann| {
            if let AnnotationKind::OrderBracket(ref bracket) = ann.kind {
                pre_snap_bracket = Some(bracket.clone());
            }
        });
    }

    // (4) Shift every absolute price in the active compound bucket,
    // then apply the Fix 2 sanity fall-back so no field lands exactly
    // on the current price (or collapses Stop and Limit together).
    let mut updated = (*intent).clone();
    let key = (updated.last_side, updated.last_entry_type);
    let side = updated.last_side;
    let entry_type = updated.last_entry_type;
    let memory = updated.entries.entry(key).or_default();
    shift_entry_memory_prices(memory, entry_type, delta);
    sanitize_entry_memory_offsets(memory, side, entry_type, current_price, gatr_abs);

    // (5) Write the new anchor and upsert.
    updated.gatr_anchor = new_anchor;
    updated.updated_at = chrono::Utc::now();
    let _ = handle.upsert(OrderIntentMsg::Upsert {
        symbol: symbol.clone(),
        intent: Box::new(updated.clone()),
        source: IntentSource::GatrSnap,
    });

    // (6) Reposition the live annotation, if one exists.
    //
    // Use `apply_leg_prices` rather than `reposition_bracket` so the
    // Fix 2 sanitize step that ran on `EntryMemory` above is mirrored
    // onto the annotation, keeping both surfaces in lockstep. If we
    // called `reposition_bracket(current_price)` the annotation's
    // entry would land exactly on market while the panel's entry
    // sat one step away, creating a visible divergence.
    if let Some(ann_id) = intent.live_annotation_id {
        let leg_prices = extract_leg_prices(&updated, side, entry_type);
        annotation_store.update(symbol.as_str(), ann_id, |ann| {
            if let AnnotationKind::OrderBracket(ref mut bracket) = ann.kind {
                apply_leg_prices(bracket, side, &leg_prices);
            }
        });
    }

    // (7) Re-hydrate any linked panel so the UI reflects the new prices.
    for panel in panels.values_mut() {
        if panel.state.symbol.eq_ignore_ascii_case(symbol.as_str()) {
            // `hydrate_from_intent` is a no-op when the panel is
            // dirty on the same symbol — that's the Slice 2 in-flight
            // edit guard and is exactly the right behavior: a user
            // mid-edit is not re-anchored over their own typing.
            panel.state.hydrate_from_intent(&updated, panel.state.last_price);
        }
    }

    Some(SnapApplied {
        delta,
        gatr_abs,
        pre_snap: PreSnapState {
            annotation_id: intent.live_annotation_id,
            bracket: pre_snap_bracket,
            prev_intent,
            stashed_at: Instant::now(),
        },
    })
}

/// Shift every **absolute** price field in an [`EntryMemory`] bucket
/// by `delta`.
///
/// Skips:
/// - `entry_price_or_offset` for Market entries (the price is the
///   live last_price, not a stored absolute).
/// - TP / SL fields in `PriceInputMode::Offset` / `Percent` modes
///   (they are relative to entry; shifting entry already moved them).
/// - Unparseable string fields (a half-typed input is left alone).
fn shift_entry_memory_prices(
    memory: &mut EntryMemory,
    entry_type: midas_chart::widget::order_bracket::EntryType,
    delta: f64,
) {
    use crate::order_panel::PriceInputMode;
    use crate::order_panel::StopLossType;
    use midas_chart::widget::order_bracket::EntryType;

    // entry_price_or_offset — Limit / Stop / StopLimit only.
    if !matches!(entry_type, EntryType::Market) {
        if let Some(ref mut p) = memory.entry_price_or_offset {
            *p += delta;
        }
    }

    // tp_value — only when enabled and absolute.
    if memory.tp_enabled && memory.tp_mode == PriceInputMode::Absolute {
        if let Ok(parsed) = memory.tp_value.parse::<f64>() {
            memory.tp_value = format!("{:.2}", parsed + delta);
        }
    }

    // sl_value — only when enabled and absolute.
    if memory.sl_enabled && memory.sl_mode == PriceInputMode::Absolute {
        if let Ok(parsed) = memory.sl_value.parse::<f64>() {
            memory.sl_value = format!("{:.2}", parsed + delta);
        }
    }

    // sl_limit_value — only when SL is a stop-limit.
    if memory.sl_enabled && memory.sl_type == StopLossType::StopLimit {
        if let Ok(parsed) = memory.sl_limit_value.parse::<f64>() {
            memory.sl_limit_value = format!("{:.2}", parsed + delta);
        }
    }
}

/// Guarantee that every absolute price in an [`EntryMemory`] bucket
/// is visually distinct and not collapsed onto the current market
/// price. Run after [`shift_entry_memory_prices`] as a fall-back.
///
/// This is the Fix 2 safety net: the delta-shift path can legitimately
/// land a leg exactly on the current price (e.g. when the anchor was
/// set at the same price), and a StopLimit's entry/stop can collapse
/// onto each other. Replace any such field with the matching value
/// from [`price_defaults::default_initial_prices`] so the user can
/// still grab each line individually.
///
/// Fields left untouched:
/// - TP / SL stored in Offset / Percent mode (their on-screen positions
///   are derived from the entry, so they cannot collapse independently).
/// - Un-parseable `tp_value` / `sl_value` strings (mid-typing state).
fn sanitize_entry_memory_offsets(
    memory: &mut EntryMemory,
    side: OrderSide,
    entry_type: midas_chart::widget::order_bracket::EntryType,
    current_price: f64,
    gatr_abs: Option<f64>,
) {
    use crate::order_panel::PriceInputMode;
    use midas_chart::widget::order_bracket::EntryType;

    use super::price_defaults::{default_initial_prices, resolve_step, too_close};

    if !current_price.is_finite() {
        return;
    }
    let step = resolve_step(current_price, gatr_abs);
    let defaults = default_initial_prices(side, entry_type, current_price, gatr_abs);

    // Entry — Limit / Stop / StopLimit only.
    if !matches!(entry_type, EntryType::Market) {
        if let Some(ref mut p) = memory.entry_price_or_offset {
            if !p.is_finite() || *p < 0.001 || too_close(*p, current_price, step) {
                *p = defaults.entry;
            }
        }
    }

    // TP — only when enabled and absolute. Anything collapsing onto
    // the entry is pushed out to the default TP level.
    if memory.tp_enabled && memory.tp_mode == PriceInputMode::Absolute {
        if let Ok(parsed) = memory.tp_value.parse::<f64>() {
            let entry_anchor = memory
                .entry_price_or_offset
                .unwrap_or(defaults.entry);
            if too_close(parsed, entry_anchor, step) || too_close(parsed, current_price, step) {
                memory.tp_value = format!("{:.2}", defaults.take_profit);
            }
        }
    }

    // SL — same treatment.
    if memory.sl_enabled && memory.sl_mode == PriceInputMode::Absolute {
        if let Ok(parsed) = memory.sl_value.parse::<f64>() {
            let entry_anchor = memory
                .entry_price_or_offset
                .unwrap_or(defaults.entry);
            if too_close(parsed, entry_anchor, step) || too_close(parsed, current_price, step) {
                memory.sl_value = format!("{:.2}", defaults.stop_loss);
            }
        }
    }
}

/// Outcome of [`apply_ensure_draft_bracket`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EnsureDraftOutcome {
    /// A fresh Draft bracket annotation was created.
    Created,
    /// Skipped because the intent's entry type is Market.
    SkippedMarket,
    /// Skipped because a live bracket already exists in the store.
    SkippedLiveExists,
    /// Skipped because no intent exists for the symbol.
    SkippedNoIntent,
    /// Skipped because no reference price (stored or live) is available.
    SkippedNoPrice,
}

/// Ensure a `Draft` bracket annotation exists for `symbol` that
/// reflects the panel's current compound-key memory.
///
/// Fix 1: on `ActivateChart`, the user expects to see the bracket shape
/// for their currently-selected `(side, entry_type)` without having to
/// click Place Order. This function is the single authority for that
/// creation — called from `app.rs` after `apply_maybe_snap` and before
/// `hydrate_order_panel_for_chart`. The panel never creates annotations
/// on its own.
///
/// # Early-exit rules
///
/// 1. No intent for the symbol → return.
/// 2. `intent.last_entry_type == Market` → return. Market orders have
///    no user-set price to preview.
/// 3. `intent.live_annotation_id` already points to an annotation that
///    still exists in the store → return. Leave the live bracket alone.
///    (If the id is set but the annotation was removed externally, fall
///    through and create a fresh one.)
/// 4. The current compound's `entry_price_or_offset` is `None` AND the
///    market cache has no `current_price` → return. We cannot place a
///    draft without a reference price.
///
/// When none of the early exits fire, a fresh `BracketStatus::Draft`
/// `OrderBracket` is built from the active [`EntryMemory`] (falling
/// back to [`super::price_defaults::default_initial_prices`] for any
/// missing fields), inserted into `annotation_store`, and
/// `intent.live_annotation_id` is updated through the handle with
/// `IntentSource::Bootstrap` so the D4 first-touch anchor-seed rule
/// does not fire on a system-created bracket.
pub(crate) fn apply_ensure_draft_bracket(
    annotation_store: &mut AnnotationStore,
    handle: &impl TickerIntentAccess,
    symbol: &SymbolKey,
    current_price: Option<f64>,
    gatr_abs: Option<f64>,
) -> EnsureDraftOutcome {
    use midas_chart::widget::level::LineStyle;
    use midas_chart::widget::order_bracket::{
        BracketLeg, BracketStatus, EntryType, LegRole, OrderBracket,
    };
    use midas_chart::widget::{AnnotationKind, LineExtent, LineStroke, PriceLine};

    use super::price_defaults::default_initial_prices;

    // (1) Intent must exist.
    let Some(intent_arc) = handle.snapshot(symbol) else {
        return EnsureDraftOutcome::SkippedNoIntent;
    };
    let intent: TickerOrderIntent = (*intent_arc).clone();

    // (2) Market orders never get a draft bracket.
    if intent.last_entry_type == EntryType::Market {
        return EnsureDraftOutcome::SkippedMarket;
    }

    // (3) Already have a live bracket that exists in the store.
    if let Some(ann_id) = intent.live_annotation_id {
        if annotation_store
            .get_by_id(symbol.as_str(), ann_id)
            .is_some()
        {
            return EnsureDraftOutcome::SkippedLiveExists;
        }
        // Stale back-link — the annotation was removed externally.
        // Fall through and create a fresh draft.
    }

    let memory = intent
        .entries
        .get(&(intent.last_side, intent.last_entry_type))
        .cloned()
        .unwrap_or_default();

    // (4) Need a reference price. Prefer stored entry, else market.
    let reference_price = memory
        .entry_price_or_offset
        .filter(|p| p.is_finite() && *p > 0.0)
        .or_else(|| current_price.filter(|p| p.is_finite() && *p > 0.0));
    let Some(reference_price) = reference_price else {
        return EnsureDraftOutcome::SkippedNoPrice;
    };
    let effective_current = current_price
        .filter(|p| p.is_finite() && *p > 0.0)
        .unwrap_or(reference_price);
    let defaults = default_initial_prices(
        intent.last_side,
        intent.last_entry_type,
        effective_current,
        gatr_abs,
    );

    // Resolve each leg: stored memory wins, else defaults.
    let entry_price = memory
        .entry_price_or_offset
        .filter(|p| p.is_finite() && *p > 0.0)
        .unwrap_or(defaults.entry);

    let stop_trigger = match intent.last_entry_type {
        EntryType::StopLimit => defaults.stop_trigger,
        _ => None,
    };

    let tp_price = if memory.tp_enabled {
        memory
            .tp_value
            .parse::<f64>()
            .ok()
            .filter(|p| p.is_finite() && *p > 0.0)
            .or(Some(defaults.take_profit))
    } else {
        None
    };
    let sl_price = if memory.sl_enabled {
        memory
            .sl_value
            .parse::<f64>()
            .ok()
            .filter(|p| p.is_finite() && *p > 0.0)
            .or(Some(defaults.stop_loss))
    } else {
        None
    };

    let make_leg = |price: f64, role: LegRole| BracketLeg {
        line: PriceLine {
            price,
            extent: LineExtent::FullWidth,
            stroke: LineStroke {
                color: [0.0, 0.0, 0.0, 1.0],
                width: 1.5,
                style: LineStyle::Solid,
            },
        },
        role,
        projected_pnl: None,
        projected_pnl_pct: None,
    };

    let bracket = OrderBracket {
        entry: make_leg(entry_price, LegRole::Entry),
        take_profit: tp_price.map(|p| make_leg(p, LegRole::TakeProfit)),
        stop_loss: sl_price.map(|p| make_leg(p, LegRole::StopLoss)),
        side: match intent.last_side {
            OrderSide::Buy => BracketSide::Long,
            OrderSide::Sell => BracketSide::Short,
        },
        status: BracketStatus::Draft,
        quantity: memory.quantity,
        saved: false,
        filled_qty: None,
        entry_type: intent.last_entry_type,
        entry_stop_price: stop_trigger,
        wrong_side_warning: false,
    };

    let ann_id = annotation_store.add(
        symbol.as_str(),
        AnnotationKind::OrderBracket(Box::new(bracket)),
    );

    // Record the new annotation id on the intent. Source = Bootstrap
    // so the D4 first-touch rule does not treat this as a user touch.
    let mut updated = intent;
    updated.live_annotation_id = Some(ann_id);
    updated.updated_at = chrono::Utc::now();
    let _ = handle.upsert(OrderIntentMsg::Upsert {
        symbol: symbol.clone(),
        intent: Box::new(updated),
        source: IntentSource::Bootstrap,
    });

    EnsureDraftOutcome::Created
}

/// Apply a `MaybeSnapToGatr` message: route into the pure
/// [`apply_snap_to_intent`] helper, stash the undo slot, flush
/// durably, and emit the undo toast.
///
/// This is the dispatcher half — it resolves `current_price` /
/// `gatr_abs` from `app.market_cache` and hands the rest to the
/// pure helper so the policy can be unit-tested without a full
/// `MidasApp`.
pub(crate) fn apply_maybe_snap(app: &mut MidasApp, symbol: SymbolKey) -> Task<Message> {
    // Resolve market-cache inputs. `last_price` may not yet be
    // available on a freshly-loaded symbol; the guards in
    // `maybe_snap` will still drop the call when it isn't.
    let (current_price, gatr_abs) = match app.market_cache.get(symbol.as_str()) {
        Some(snap) => (snap.last_price, snap.gatr_abs),
        None => return Task::none(),
    };
    let current_price = match current_price {
        Some(p) => p,
        None => return Task::none(),
    };

    let applied = match apply_snap_to_intent(
        &mut app.annotation_store,
        &app.order_intent_handle,
        &mut app.order_panels,
        &symbol,
        current_price,
        gatr_abs,
    ) {
        Some(a) => a,
        None => return Task::none(),
    };

    let SnapApplied {
        delta,
        gatr_abs,
        pre_snap,
    } = applied;
    app.gatr_undo_slots.insert(symbol.clone(), pre_snap);

    // Force a durable flush so a crash between now and the next
    // debounce window does not leave the anchor un-persisted. Run on
    // the ambient tokio runtime — the flush is fire-and-forget from
    // the reducer's point of view.
    if let Ok(rt) = tokio::runtime::Handle::try_current() {
        let handle = app.order_intent_handle.clone();
        rt.spawn(async move {
            handle.flush_now().await;
        });
    }

    // Build the undo toast. The label + sign are handcrafted so the
    // delta is readable ("+2.00" / "-3.15").
    let sign = if delta >= 0.0 { "+" } else { "-" };
    let drift_ratio = gatr_abs
        .filter(|g| g.is_finite() && *g > 0.0)
        .map(|g| delta.abs() / g)
        .unwrap_or(0.0);
    let message = format!(
        "{sym}: bracket re-anchored {sign}{amt:.2} (price drifted {drift:.1}× GATR)",
        sym = symbol.as_str(),
        amt = delta.abs(),
        drift = drift_ratio,
    );
    Task::done(Message::ShowToast {
        message,
        action: Some(ToastAction {
            label: "Undo".to_string(),
            on_click: Box::new(Message::OrderIntent(
                crate::ticker_order_intent::OrderIntentAppMsg::UndoSnap { symbol },
            )),
        }),
    })
}

/// Apply a `TogglePin` message. Flips `TickerOrderIntent.pinned` and
/// writes the intent back through the handle. Subsequent
/// `MaybeSnapToGatr` evaluations read the new value on their next
/// guard-check pass.
pub(crate) fn apply_toggle_pin(app: &mut MidasApp, symbol: SymbolKey) -> Task<Message> {
    let intent = match app.order_intent_handle.snapshot(&symbol) {
        Some(arc) => arc,
        None => return Task::none(),
    };
    let mut updated = (*intent).clone();
    updated.pinned = !updated.pinned;
    updated.updated_at = chrono::Utc::now();
    let _ = app.order_intent_handle.upsert(OrderIntentMsg::Upsert {
        symbol: symbol.clone(),
        intent: Box::new(updated),
        source: IntentSource::Panel,
    });
    Task::none()
}

/// Apply an `UndoSnap` message. Restores both surfaces (panel
/// `EntryMemory` and live bracket annotation, when present) from
/// the stashed pre-snap state if it is still within [`UNDO_TTL`],
/// then forces a durable flush.
///
/// The intent is restored verbatim from `pre_snap.prev_intent`, so
/// both the `EntryMemory` shifts and the `gatr_anchor` are rolled
/// back in one upsert.
pub(crate) fn apply_undo_snap(app: &mut MidasApp, symbol: SymbolKey) -> Task<Message> {
    let stash = match app.gatr_undo_slots.remove(&symbol) {
        Some(s) => s,
        None => return Task::none(),
    };
    if stash.stashed_at.elapsed() > UNDO_TTL {
        return Task::none();
    }

    // Restore the live annotation, if the snap moved one.
    if let (Some(ann_id), Some(restore_box)) = (stash.annotation_id, stash.bracket.as_ref()) {
        let restore_clone = restore_box.clone();
        app.annotation_store
            .update(symbol.as_str(), ann_id, |ann| {
                if let AnnotationKind::OrderBracket(ref mut bracket) = ann.kind {
                    *bracket = restore_clone.clone();
                }
            });
    }

    // Restore the intent verbatim so the recency guard behaves as
    // if the snap never happened.
    let restored = *stash.prev_intent;
    let _ = app.order_intent_handle.upsert(OrderIntentMsg::Upsert {
        symbol: symbol.clone(),
        intent: Box::new(restored.clone()),
        source: IntentSource::GatrSnap,
    });

    // Refresh any linked panels.
    for panel in app.order_panels.values_mut() {
        if panel.state.symbol.eq_ignore_ascii_case(symbol.as_str()) {
            panel.state.hydrate_from_intent(&restored, panel.state.last_price);
        }
    }

    if let Ok(rt) = tokio::runtime::Handle::try_current() {
        let handle = app.order_intent_handle.clone();
        rt.spawn(async move {
            handle.flush_now().await;
        });
    }

    Task::none()
}

#[cfg(test)]
mod tests;
