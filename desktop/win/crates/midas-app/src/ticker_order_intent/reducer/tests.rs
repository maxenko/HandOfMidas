//! Slice 3 reducer unit tests.
//!
//! These drive [`super::apply_update_from_surface`],
//! [`super::apply_cancel_live_bracket`], and
//! [`super::apply_remove_live_bracket`] directly — bypassing
//! `apply_order_intent_msg` so we do not have to construct a full
//! `MidasApp`. Coverage matches the Slice 3 plan's testing matrix:
//!
//! - Drag round-trip lands in both panel and annotation.
//! - Panel round-trip lands in the annotation without echoing back.
//! - Cancel preserves memory, clears the live link, resets dirty.
//! - External removal clears the intent's back-link without error.
//! - Edit-then-undo regression for both Cancel and RemoveLiveBracket.
//! - Mid-drag ticker switch drops the message with a warn log.
//! - No-op suppression via the cache-equality check.
//! - Panel edit with no live bracket does not create a phantom annotation.
//! - Compound-key write isolation — other buckets are not stomped.
//! - SL-off persists per compound.

use std::collections::HashMap;
use std::sync::Arc;

use midas_chart::widget::level::LineStyle;
use midas_chart::widget::order_bracket::{
    BracketLeg, BracketSide, BracketStatus, EntryType, LegRole, OrderBracket,
};
use midas_chart::widget::{
    AnnotationId, AnnotationKind, LineExtent, LineStroke, PriceLine,
};
use midas_core::OrderPanelId;
use parking_lot::Mutex;

use crate::annotation_store::{AnnotationStore, SymbolKey};
use crate::order_panel::{OrderPanel, OrderSide, PriceInputMode, StopLossType};
use crate::ticker_order_intent::handle::TickerIntentAccess;
use crate::ticker_order_intent::store::UpsertOutcome;
use crate::ticker_order_intent::{
    EntryMemory, GatrAnchor, IntentSource, NoOpReason, OrderIntentMsg, TickerOrderIntent,
    CURRENT_VERSION,
};

use super::{
    apply_cancel_live_bracket, apply_remove_live_bracket, apply_snap_to_intent,
    apply_update_from_surface, UpdateSurfaceOutcome,
};

// ── Mock `TickerIntentAccess` ───────────────────────────────────────

/// In-process stand-in for `TickerOrderIntentHandle` that avoids the
/// `redb` + background-actor costs. Every call is synchronous and
/// observable from the test thread.
struct MockHandle {
    inner: Mutex<HashMap<SymbolKey, Arc<TickerOrderIntent>>>,
    upsert_count: Mutex<u32>,
}

impl MockHandle {
    fn new() -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
            upsert_count: Mutex::new(0),
        }
    }

    fn seed(&self, symbol: SymbolKey, intent: TickerOrderIntent) {
        self.inner.lock().insert(symbol, Arc::new(intent));
    }

    fn upsert_count(&self) -> u32 {
        *self.upsert_count.lock()
    }
}

impl TickerIntentAccess for MockHandle {
    fn snapshot(&self, symbol: &SymbolKey) -> Option<Arc<TickerOrderIntent>> {
        self.inner.lock().get(symbol).cloned()
    }

    fn upsert(&self, msg: OrderIntentMsg) -> UpsertOutcome {
        match msg {
            OrderIntentMsg::Upsert {
                symbol,
                intent,
                source: _,
            } => {
                let mut map = self.inner.lock();
                if let Some(existing) = map.get(&symbol) {
                    if **existing == *intent {
                        return UpsertOutcome::NoOp {
                            reason: NoOpReason::IdenticalToCache,
                        };
                    }
                }
                map.insert(symbol, Arc::new(*intent));
                *self.upsert_count.lock() += 1;
                UpsertOutcome::Applied {
                    generation: *self.upsert_count.lock() as u64,
                }
            }
            _ => UpsertOutcome::NoOp {
                reason: NoOpReason::StaleSource,
            },
        }
    }

    async fn flush_now(&self) {}

    async fn shutdown(self) {}
}

// ── Fixtures ────────────────────────────────────────────────────────

fn make_leg(price: f64) -> BracketLeg {
    BracketLeg {
        line: PriceLine {
            price,
            extent: LineExtent::FullWidth,
            stroke: LineStroke {
                color: [0.0, 0.0, 0.0, 1.0],
                width: 1.5,
                style: LineStyle::Solid,
            },
        },
        role: LegRole::Entry,
        projected_pnl: None,
        projected_pnl_pct: None,
    }
}

fn make_bracket(entry: f64, tp: Option<f64>, sl: Option<f64>) -> OrderBracket {
    OrderBracket {
        entry: make_leg(entry),
        take_profit: tp.map(make_leg),
        stop_loss: sl.map(make_leg),
        side: BracketSide::Long,
        status: BracketStatus::Draft,
        quantity: Some(10.0),
        saved: false,
        filled_qty: None,
        entry_type: EntryType::Limit,
        entry_stop_price: None,
        wrong_side_warning: false,
    }
}

fn make_memory(entry: f64, tp: Option<f64>, sl: Option<f64>, sl_enabled: bool) -> EntryMemory {
    EntryMemory {
        entry_price_or_offset: Some(entry),
        quantity: Some(10.0),
        tp_enabled: tp.is_some(),
        tp_value: tp.map(|p| format!("{:.2}", p)).unwrap_or_default(),
        tp_mode: PriceInputMode::Absolute,
        sl_enabled,
        sl_value: sl.map(|p| format!("{:.2}", p)).unwrap_or_default(),
        sl_mode: PriceInputMode::Absolute,
        sl_type: StopLossType::Stop,
        sl_limit_value: String::new(),
    }
}

fn make_intent(
    symbol: &str,
    side: OrderSide,
    entry_type: EntryType,
    entries: Vec<((OrderSide, EntryType), EntryMemory)>,
    live_id: Option<AnnotationId>,
) -> TickerOrderIntent {
    TickerOrderIntent {
        version: CURRENT_VERSION,
        symbol: SymbolKey::new(symbol),
        last_side: side,
        last_entry_type: entry_type,
        entries: entries.into_iter().collect(),
        gatr_anchor: GatrAnchor::default(),
        live_annotation_id: live_id,
        broker_order_id: None,
        pinned: false,
        updated_at: chrono::Utc::now(),
    }
}

fn make_panel(id: u32, symbol: &str, ann_id: Option<AnnotationId>) -> OrderPanel {
    let mut panel = OrderPanel::new(OrderPanelId::new(id), symbol.to_string());
    panel.state.symbol = symbol.to_string();
    panel.state.bracket_annotation_id = ann_id;
    panel
}

fn panels_map(panels: Vec<OrderPanel>) -> HashMap<OrderPanelId, OrderPanel> {
    panels.into_iter().map(|p| (p.id, p)).collect()
}

fn bracket_entry_price(store: &AnnotationStore, symbol: &str, id: AnnotationId) -> f64 {
    store
        .get_bracket(symbol, id)
        .map(|b| b.entry.line.price)
        .expect("bracket present")
}

// ── Tests ───────────────────────────────────────────────────────────

#[test]
fn drag_update_lands_in_annotation_and_panel() {
    let mut store = AnnotationStore::new();
    let ann_id = store.add(
        "AAPL",
        AnnotationKind::OrderBracket(Box::new(make_bracket(100.0, Some(102.0), Some(98.0)))),
    );

    let handle = MockHandle::new();
    handle.seed(
        SymbolKey::new("AAPL"),
        make_intent(
            "AAPL",
            OrderSide::Buy,
            EntryType::Limit,
            vec![(
                (OrderSide::Buy, EntryType::Limit),
                make_memory(100.0, Some(102.0), Some(98.0), true),
            )],
            Some(ann_id),
        ),
    );

    let mut panels = panels_map(vec![make_panel(1, "AAPL", Some(ann_id))]);

    // Drag moved entry to 105, TP to 107, SL to 103.
    let snapshot = make_intent(
        "AAPL",
        OrderSide::Buy,
        EntryType::Limit,
        vec![(
            (OrderSide::Buy, EntryType::Limit),
            make_memory(105.0, Some(107.0), Some(103.0), true),
        )],
        Some(ann_id),
    );

    let active = SymbolKey::new("AAPL");
    apply_update_from_surface(
        &mut store,
        &handle,
        &mut panels,
        Some(&active),
        SymbolKey::new("AAPL"),
        snapshot,
        IntentSource::Chart,
        None,
        None,
    );

    // Annotation moved.
    assert!((bracket_entry_price(&store, "AAPL", ann_id) - 105.0).abs() < 1e-9);
    // Intent updated.
    let intent = handle.snapshot(&SymbolKey::new("AAPL")).unwrap();
    let mem = intent
        .entries
        .get(&(OrderSide::Buy, EntryType::Limit))
        .unwrap();
    assert_eq!(mem.entry_price_or_offset, Some(105.0));
    // Panel hydrated from the new state (Chart-sourced → panel refresh fires).
    let panel = panels.values().next().unwrap();
    assert_eq!(panel.state.limit_price, "105.00");
}

#[test]
fn panel_update_updates_annotation_and_returns_noop_task() {
    let mut store = AnnotationStore::new();
    let ann_id = store.add(
        "AAPL",
        AnnotationKind::OrderBracket(Box::new(make_bracket(100.0, Some(102.0), Some(98.0)))),
    );

    let handle = MockHandle::new();
    handle.seed(
        SymbolKey::new("AAPL"),
        make_intent(
            "AAPL",
            OrderSide::Buy,
            EntryType::Limit,
            vec![(
                (OrderSide::Buy, EntryType::Limit),
                make_memory(100.0, Some(102.0), Some(98.0), true),
            )],
            Some(ann_id),
        ),
    );

    let mut panels = panels_map(vec![make_panel(1, "AAPL", Some(ann_id))]);

    // Panel edit: entry to 110.
    let snapshot = make_intent(
        "AAPL",
        OrderSide::Buy,
        EntryType::Limit,
        vec![(
            (OrderSide::Buy, EntryType::Limit),
            make_memory(110.0, Some(112.0), Some(107.0), true),
        )],
        Some(ann_id),
    );

    apply_update_from_surface(
        &mut store,
        &handle,
        &mut panels,
        None, // panel-sourced — active symbol is irrelevant
        SymbolKey::new("AAPL"),
        snapshot,
        IntentSource::Panel,
        None,
        None,
    );

    // Annotation moved.
    assert!((bracket_entry_price(&store, "AAPL", ann_id) - 110.0).abs() < 1e-9);
    // One upsert applied.
    assert_eq!(handle.upsert_count(), 1);
}

#[test]
fn cancel_preserves_memory_and_resets_dirty() {
    let mut store = AnnotationStore::new();
    let ann_id = store.add(
        "AAPL",
        AnnotationKind::OrderBracket(Box::new(make_bracket(100.0, Some(102.0), Some(98.0)))),
    );

    let handle = MockHandle::new();
    handle.seed(
        SymbolKey::new("AAPL"),
        make_intent(
            "AAPL",
            OrderSide::Buy,
            EntryType::Limit,
            vec![
                (
                    (OrderSide::Buy, EntryType::Limit),
                    make_memory(100.0, Some(102.0), Some(98.0), true),
                ),
                (
                    (OrderSide::Sell, EntryType::Stop),
                    make_memory(90.0, None, None, false),
                ),
            ],
            Some(ann_id),
        ),
    );

    let mut panel = make_panel(1, "AAPL", Some(ann_id));
    panel.state.dirty = true;
    let mut panels = panels_map(vec![panel]);

    apply_cancel_live_bracket(&mut store, &handle, &mut panels, &SymbolKey::new("AAPL"));

    // Annotation removed.
    assert!(store.get_bracket("AAPL", ann_id).is_none());

    // Memory preserved.
    let intent = handle.snapshot(&SymbolKey::new("AAPL")).unwrap();
    assert!(intent.live_annotation_id.is_none());
    assert_eq!(intent.last_side, OrderSide::Buy);
    assert_eq!(intent.last_entry_type, EntryType::Limit);
    assert_eq!(intent.entries.len(), 2);
    let mem = intent
        .entries
        .get(&(OrderSide::Buy, EntryType::Limit))
        .unwrap();
    assert_eq!(mem.entry_price_or_offset, Some(100.0));
    let sell_stop = intent
        .entries
        .get(&(OrderSide::Sell, EntryType::Stop))
        .unwrap();
    assert!(!sell_stop.sl_enabled);

    // Dirty reset on the linked panel.
    let panel = panels.values().next().unwrap();
    assert!(!panel.state.dirty);
    assert_eq!(panel.state.bracket_annotation_id, None);
}

#[test]
fn external_removal_clears_back_link() {
    let handle = MockHandle::new();
    let ann_id = AnnotationId(42);
    handle.seed(
        SymbolKey::new("AAPL"),
        make_intent(
            "AAPL",
            OrderSide::Buy,
            EntryType::Limit,
            vec![(
                (OrderSide::Buy, EntryType::Limit),
                make_memory(100.0, None, None, true),
            )],
            Some(ann_id),
        ),
    );
    let mut panels = panels_map(vec![make_panel(1, "AAPL", Some(ann_id))]);

    apply_remove_live_bracket(&handle, &mut panels, &SymbolKey::new("AAPL"), ann_id);

    let intent = handle.snapshot(&SymbolKey::new("AAPL")).unwrap();
    assert!(intent.live_annotation_id.is_none());
    // Memory preserved.
    assert!(intent
        .entries
        .contains_key(&(OrderSide::Buy, EntryType::Limit)));
    let panel = panels.values().next().unwrap();
    assert_eq!(panel.state.bracket_annotation_id, None);
}

#[test]
fn edit_then_remove_live_bracket_resets_dirty() {
    let handle = MockHandle::new();
    let ann_id = AnnotationId(7);
    handle.seed(
        SymbolKey::new("AAPL"),
        make_intent(
            "AAPL",
            OrderSide::Buy,
            EntryType::Limit,
            vec![(
                (OrderSide::Buy, EntryType::Limit),
                make_memory(100.0, None, None, true),
            )],
            Some(ann_id),
        ),
    );

    let mut panel = make_panel(1, "AAPL", Some(ann_id));
    panel.state.dirty = true;
    let mut panels = panels_map(vec![panel]);

    apply_remove_live_bracket(&handle, &mut panels, &SymbolKey::new("AAPL"), ann_id);

    let panel = panels.values().next().unwrap();
    assert!(!panel.state.dirty);

    // A follow-up hydration from the intent succeeds.
    let intent = handle.snapshot(&SymbolKey::new("AAPL")).unwrap();
    let mut fresh = make_panel(2, "AAPL", None);
    fresh.state.hydrate_from_intent(&intent, Some(100.0));
    assert_eq!(fresh.state.limit_price, "100.00");
}

#[test]
fn edit_then_cancel_resets_dirty() {
    let mut store = AnnotationStore::new();
    let ann_id = store.add(
        "AAPL",
        AnnotationKind::OrderBracket(Box::new(make_bracket(100.0, None, None))),
    );

    let handle = MockHandle::new();
    handle.seed(
        SymbolKey::new("AAPL"),
        make_intent(
            "AAPL",
            OrderSide::Buy,
            EntryType::Limit,
            vec![(
                (OrderSide::Buy, EntryType::Limit),
                make_memory(100.0, None, None, true),
            )],
            Some(ann_id),
        ),
    );

    let mut panel = make_panel(1, "AAPL", Some(ann_id));
    panel.state.dirty = true;
    let mut panels = panels_map(vec![panel]);

    apply_cancel_live_bracket(&mut store, &handle, &mut panels, &SymbolKey::new("AAPL"));

    let panel = panels.values().next().unwrap();
    assert!(!panel.state.dirty);

    // Hydration from the memory still works.
    let intent = handle.snapshot(&SymbolKey::new("AAPL")).unwrap();
    let mut fresh = make_panel(2, "AAPL", None);
    fresh.state.hydrate_from_intent(&intent, Some(100.0));
    assert_eq!(fresh.state.limit_price, "100.00");
}

#[test]
fn mid_drag_ticker_switch_drops_message() {
    let mut store = AnnotationStore::new();
    let ann_id = store.add(
        "AAPL",
        AnnotationKind::OrderBracket(Box::new(make_bracket(100.0, None, None))),
    );
    let handle = MockHandle::new();
    handle.seed(
        SymbolKey::new("AAPL"),
        make_intent(
            "AAPL",
            OrderSide::Buy,
            EntryType::Limit,
            vec![(
                (OrderSide::Buy, EntryType::Limit),
                make_memory(100.0, None, None, true),
            )],
            Some(ann_id),
        ),
    );
    let mut panels = panels_map(vec![make_panel(1, "AAPL", Some(ann_id))]);

    // Active chart now showing MSFT; drag message claims AAPL.
    let active = SymbolKey::new("MSFT");
    let snapshot = make_intent(
        "AAPL",
        OrderSide::Buy,
        EntryType::Limit,
        vec![(
            (OrderSide::Buy, EntryType::Limit),
            make_memory(999.0, None, None, true),
        )],
        Some(ann_id),
    );

    apply_update_from_surface(
        &mut store,
        &handle,
        &mut panels,
        Some(&active),
        SymbolKey::new("AAPL"),
        snapshot,
        IntentSource::Chart,
        None,
        None,
    );

    // Dropped: annotation still at 100.
    assert!((bracket_entry_price(&store, "AAPL", ann_id) - 100.0).abs() < 1e-9);
    // No writes.
    assert_eq!(handle.upsert_count(), 0);
}

#[test]
fn noop_suppression_on_identical_second_update() {
    let mut store = AnnotationStore::new();
    let ann_id = store.add(
        "AAPL",
        AnnotationKind::OrderBracket(Box::new(make_bracket(100.0, None, None))),
    );
    let handle = MockHandle::new();
    // Pre-seed an intent whose (Buy, Limit) bucket already matches the
    // incoming snapshot exactly. The store's cache-equality check
    // returns NoOp on the first call.
    let entries = vec![(
        (OrderSide::Buy, EntryType::Limit),
        make_memory(105.0, None, None, true),
    )];
    let seeded = make_intent(
        "AAPL",
        OrderSide::Buy,
        EntryType::Limit,
        entries.clone(),
        Some(ann_id),
    );
    // Snapshot first so we can reuse the same timestamp in the seeded
    // intent — otherwise `updated_at` differs and the equality check
    // sees a change.
    let mut snapshot = seeded.clone();
    snapshot.updated_at = seeded.updated_at;

    handle.seed(SymbolKey::new("AAPL"), seeded);
    let mut panels = panels_map(vec![make_panel(1, "AAPL", Some(ann_id))]);

    apply_update_from_surface(
        &mut store,
        &handle,
        &mut panels,
        None,
        SymbolKey::new("AAPL"),
        snapshot,
        IntentSource::Panel,
        None,
        None,
    );

    assert_eq!(handle.upsert_count(), 0);
}

#[test]
fn panel_edit_without_live_bracket_does_not_create_phantom() {
    let mut store = AnnotationStore::new();
    let handle = MockHandle::new();
    handle.seed(
        SymbolKey::new("AAPL"),
        make_intent(
            "AAPL",
            OrderSide::Buy,
            EntryType::Limit,
            vec![(
                (OrderSide::Buy, EntryType::Limit),
                make_memory(100.0, None, None, true),
            )],
            None, // no live bracket
        ),
    );
    let mut panels = panels_map(vec![make_panel(1, "AAPL", None)]);

    let snapshot = make_intent(
        "AAPL",
        OrderSide::Buy,
        EntryType::Limit,
        vec![(
            (OrderSide::Buy, EntryType::Limit),
            make_memory(110.0, None, None, true),
        )],
        None,
    );

    apply_update_from_surface(
        &mut store,
        &handle,
        &mut panels,
        None,
        SymbolKey::new("AAPL"),
        snapshot,
        IntentSource::Panel,
        None,
        None,
    );

    // No annotations exist for AAPL — nothing was created.
    assert!(store.get("AAPL").is_empty());
    // Intent was updated.
    let intent = handle.snapshot(&SymbolKey::new("AAPL")).unwrap();
    assert_eq!(
        intent
            .entries
            .get(&(OrderSide::Buy, EntryType::Limit))
            .unwrap()
            .entry_price_or_offset,
        Some(110.0)
    );
}

#[test]
fn compound_key_write_isolation() {
    let mut store = AnnotationStore::new();
    let handle = MockHandle::new();
    // Seed (Buy, Stop) with sl_enabled = false — the user explicitly
    // turned SL off for stop-entries earlier in the session.
    handle.seed(
        SymbolKey::new("AAPL"),
        make_intent(
            "AAPL",
            OrderSide::Buy,
            EntryType::Stop,
            vec![(
                (OrderSide::Buy, EntryType::Stop),
                make_memory(95.0, None, None, false),
            )],
            None,
        ),
    );

    let mut panels = panels_map(vec![make_panel(1, "AAPL", None)]);

    // Now the user switches to (Buy, Limit) and turns SL on.
    let snapshot = make_intent(
        "AAPL",
        OrderSide::Buy,
        EntryType::Limit,
        vec![(
            (OrderSide::Buy, EntryType::Limit),
            make_memory(100.0, None, None, true),
        )],
        None,
    );

    apply_update_from_surface(
        &mut store,
        &handle,
        &mut panels,
        None,
        SymbolKey::new("AAPL"),
        snapshot,
        IntentSource::Panel,
        None,
        None,
    );

    let intent = handle.snapshot(&SymbolKey::new("AAPL")).unwrap();
    // New bucket landed.
    let buy_limit = intent
        .entries
        .get(&(OrderSide::Buy, EntryType::Limit))
        .expect("(Buy, Limit) bucket written");
    assert!(buy_limit.sl_enabled);
    assert_eq!(buy_limit.entry_price_or_offset, Some(100.0));
    // Original bucket untouched.
    let buy_stop = intent
        .entries
        .get(&(OrderSide::Buy, EntryType::Stop))
        .expect("(Buy, Stop) bucket preserved");
    assert!(!buy_stop.sl_enabled);
    assert_eq!(buy_stop.entry_price_or_offset, Some(95.0));
    // last_* updated to the new bucket.
    assert_eq!(intent.last_side, OrderSide::Buy);
    assert_eq!(intent.last_entry_type, EntryType::Limit);
}

#[test]
fn sl_off_persists_per_compound() {
    let mut store = AnnotationStore::new();
    let handle = MockHandle::new();
    handle.seed(
        SymbolKey::new("AAPL"),
        make_intent(
            "AAPL",
            OrderSide::Buy,
            EntryType::Stop,
            vec![],
            None,
        ),
    );
    let mut panels = panels_map(vec![make_panel(1, "AAPL", None)]);

    // Turn SL off in (Buy, Stop).
    apply_update_from_surface(
        &mut store,
        &handle,
        &mut panels,
        None,
        SymbolKey::new("AAPL"),
        make_intent(
            "AAPL",
            OrderSide::Buy,
            EntryType::Stop,
            vec![(
                (OrderSide::Buy, EntryType::Stop),
                make_memory(100.0, None, None, false),
            )],
            None,
        ),
        IntentSource::Panel,
        None,
        None,
    );

    // Turn SL off in (Sell, Stop).
    apply_update_from_surface(
        &mut store,
        &handle,
        &mut panels,
        None,
        SymbolKey::new("AAPL"),
        make_intent(
            "AAPL",
            OrderSide::Sell,
            EntryType::Stop,
            vec![(
                (OrderSide::Sell, EntryType::Stop),
                make_memory(100.0, None, None, false),
            )],
            None,
        ),
        IntentSource::Panel,
        None,
        None,
    );

    let intent = handle.snapshot(&SymbolKey::new("AAPL")).unwrap();
    // Both stop buckets sticky-off.
    assert!(!intent
        .entries
        .get(&(OrderSide::Buy, EntryType::Stop))
        .unwrap()
        .sl_enabled);
    assert!(!intent
        .entries
        .get(&(OrderSide::Sell, EntryType::Stop))
        .unwrap()
        .sl_enabled);
    // Limit buckets are absent — a fresh hydration into those would
    // fall back to `EntryMemory::default()` which has `sl_enabled = true`.
    assert!(!intent
        .entries
        .contains_key(&(OrderSide::Buy, EntryType::Limit)));
    assert!(!intent
        .entries
        .contains_key(&(OrderSide::Sell, EntryType::Limit)));
}

// ── Slice 4: anchor lifecycle + discoverability-toast tests ──────────

/// The anchor-seeding rule — `Upsert { source: Bootstrap }` leaves
/// `anchor_price == None`, and the very first subsequent
/// `Upsert { source: Panel }` (with a valid `current_price`) sets it.
#[test]
fn anchor_lifecycle_bootstrap_then_panel_seeds_anchor() {
    let mut store = AnnotationStore::new();
    let ann_id = store.add(
        "AAPL",
        AnnotationKind::OrderBracket(Box::new(make_bracket(100.0, Some(102.0), Some(98.0)))),
    );
    let handle = MockHandle::new();
    // Seed as if a Bootstrap pass ran: entries populated, but
    // gatr_anchor is the default None/None pair.
    handle.seed(
        SymbolKey::new("AAPL"),
        make_intent(
            "AAPL",
            OrderSide::Buy,
            EntryType::Limit,
            vec![(
                (OrderSide::Buy, EntryType::Limit),
                make_memory(100.0, Some(102.0), Some(98.0), true),
            )],
            Some(ann_id),
        ),
    );
    assert!(handle
        .snapshot(&SymbolKey::new("AAPL"))
        .unwrap()
        .gatr_anchor
        .anchor_price
        .is_none());

    let mut panels = panels_map(vec![make_panel(1, "AAPL", Some(ann_id))]);
    let snapshot = make_intent(
        "AAPL",
        OrderSide::Buy,
        EntryType::Limit,
        vec![(
            (OrderSide::Buy, EntryType::Limit),
            make_memory(101.0, Some(103.0), Some(99.0), true),
        )],
        Some(ann_id),
    );
    let outcome = apply_update_from_surface(
        &mut store,
        &handle,
        &mut panels,
        None,
        SymbolKey::new("AAPL"),
        snapshot,
        IntentSource::Panel,
        Some(150.0),
        Some(2.5),
    );
    assert_eq!(outcome, UpdateSurfaceOutcome::AppliedAnchorSeeded);
    let intent = handle.snapshot(&SymbolKey::new("AAPL")).unwrap();
    assert_eq!(intent.gatr_anchor.anchor_price, Some(150.0));
    assert_eq!(intent.gatr_anchor.anchor_gatr, Some(2.5));
}

/// After an anchor is seeded, a subsequent `Panel`-sourced upsert
/// bumps `updated_at` but does **not** overwrite the anchor. The
/// outcome should be `Applied`, not `AppliedAnchorSeeded`.
#[test]
fn anchor_lifecycle_second_panel_upsert_keeps_anchor() {
    let mut store = AnnotationStore::new();
    let ann_id = store.add(
        "AAPL",
        AnnotationKind::OrderBracket(Box::new(make_bracket(100.0, Some(102.0), Some(98.0)))),
    );
    let handle = MockHandle::new();
    let mut initial = make_intent(
        "AAPL",
        OrderSide::Buy,
        EntryType::Limit,
        vec![(
            (OrderSide::Buy, EntryType::Limit),
            make_memory(100.0, Some(102.0), Some(98.0), true),
        )],
        Some(ann_id),
    );
    initial.gatr_anchor = GatrAnchor {
        anchor_price: Some(150.0),
        anchor_gatr: Some(2.5),
    };
    handle.seed(SymbolKey::new("AAPL"), initial);

    let mut panels = panels_map(vec![make_panel(1, "AAPL", Some(ann_id))]);
    let snapshot = make_intent(
        "AAPL",
        OrderSide::Buy,
        EntryType::Limit,
        vec![(
            (OrderSide::Buy, EntryType::Limit),
            make_memory(105.0, Some(107.0), Some(103.0), true),
        )],
        Some(ann_id),
    );
    let outcome = apply_update_from_surface(
        &mut store,
        &handle,
        &mut panels,
        None,
        SymbolKey::new("AAPL"),
        snapshot,
        IntentSource::Panel,
        Some(200.0),
        Some(3.0),
    );
    assert_eq!(outcome, UpdateSurfaceOutcome::Applied);
    let intent = handle.snapshot(&SymbolKey::new("AAPL")).unwrap();
    // Anchor unchanged.
    assert_eq!(intent.gatr_anchor.anchor_price, Some(150.0));
    assert_eq!(intent.gatr_anchor.anchor_gatr, Some(2.5));
}

/// A `Bootstrap`-sourced upsert never seeds the anchor, even when a
/// valid `current_price` is available.
#[test]
fn anchor_lifecycle_bootstrap_source_never_seeds() {
    let mut store = AnnotationStore::new();
    let ann_id = store.add(
        "AAPL",
        AnnotationKind::OrderBracket(Box::new(make_bracket(100.0, Some(102.0), Some(98.0)))),
    );
    let handle = MockHandle::new();
    let mut panels = panels_map(vec![make_panel(1, "AAPL", Some(ann_id))]);
    let snapshot = make_intent(
        "AAPL",
        OrderSide::Buy,
        EntryType::Limit,
        vec![(
            (OrderSide::Buy, EntryType::Limit),
            make_memory(100.0, Some(102.0), Some(98.0), true),
        )],
        Some(ann_id),
    );
    let outcome = apply_update_from_surface(
        &mut store,
        &handle,
        &mut panels,
        None,
        SymbolKey::new("AAPL"),
        snapshot,
        IntentSource::Bootstrap,
        Some(150.0),
        Some(2.5),
    );
    // The upsert landed, but the anchor is still None.
    assert_eq!(outcome, UpdateSurfaceOutcome::Applied);
    let intent = handle.snapshot(&SymbolKey::new("AAPL")).unwrap();
    assert!(intent.gatr_anchor.anchor_price.is_none());
    assert!(intent.gatr_anchor.anchor_gatr.is_none());
}

/// A `Chart`-sourced upsert also seeds the anchor (same rule as Panel).
#[test]
fn anchor_lifecycle_chart_source_seeds_anchor() {
    let mut store = AnnotationStore::new();
    let ann_id = store.add(
        "AAPL",
        AnnotationKind::OrderBracket(Box::new(make_bracket(100.0, Some(102.0), Some(98.0)))),
    );
    let handle = MockHandle::new();
    handle.seed(
        SymbolKey::new("AAPL"),
        make_intent(
            "AAPL",
            OrderSide::Buy,
            EntryType::Limit,
            vec![(
                (OrderSide::Buy, EntryType::Limit),
                make_memory(100.0, Some(102.0), Some(98.0), true),
            )],
            Some(ann_id),
        ),
    );
    let mut panels = panels_map(vec![make_panel(1, "AAPL", Some(ann_id))]);
    let snapshot = make_intent(
        "AAPL",
        OrderSide::Buy,
        EntryType::Limit,
        vec![(
            (OrderSide::Buy, EntryType::Limit),
            make_memory(105.0, Some(107.0), Some(103.0), true),
        )],
        Some(ann_id),
    );
    let active = SymbolKey::new("AAPL");
    let outcome = apply_update_from_surface(
        &mut store,
        &handle,
        &mut panels,
        Some(&active),
        SymbolKey::new("AAPL"),
        snapshot,
        IntentSource::Chart,
        Some(120.0),
        Some(1.5),
    );
    assert_eq!(outcome, UpdateSurfaceOutcome::AppliedAnchorSeeded);
    let intent = handle.snapshot(&SymbolKey::new("AAPL")).unwrap();
    assert_eq!(intent.gatr_anchor.anchor_price, Some(120.0));
    assert_eq!(intent.gatr_anchor.anchor_gatr, Some(1.5));
}

// ── Single-source-of-truth snap tests ────────────────────────────
//
// These drive [`super::apply_snap_to_intent`] — the pure-function
// half of `MaybeSnapToGatr`. Previously, panel-input snapping lived
// in `order_panel::snap_panel_to_current_price`. The refactor moves
// every snap decision into the reducer so the panel and the chart
// bracket are both views of the same intent.

/// Build an intent with a stale GATR anchor and a populated
/// `(Buy, EntryType)` bucket. `anchor_price` is the "where the
/// bracket was last endorsed" marker used by the recency guard.
fn snap_intent(
    symbol: &str,
    entry_type: EntryType,
    anchor_price: f64,
    bucket_entry: Option<f64>,
    bucket_tp: Option<f64>,
    bucket_sl: Option<f64>,
    live_id: Option<AnnotationId>,
) -> TickerOrderIntent {
    let memory = EntryMemory {
        entry_price_or_offset: bucket_entry,
        quantity: Some(100.0),
        tp_enabled: bucket_tp.is_some(),
        tp_value: bucket_tp.map(|p| format!("{:.2}", p)).unwrap_or_default(),
        tp_mode: PriceInputMode::Absolute,
        sl_enabled: bucket_sl.is_some(),
        sl_value: bucket_sl.map(|p| format!("{:.2}", p)).unwrap_or_default(),
        sl_mode: PriceInputMode::Absolute,
        sl_type: StopLossType::Stop,
        sl_limit_value: String::new(),
    };
    TickerOrderIntent {
        version: CURRENT_VERSION,
        symbol: SymbolKey::new(symbol),
        last_side: OrderSide::Buy,
        last_entry_type: entry_type,
        entries: [((OrderSide::Buy, entry_type), memory)].into_iter().collect(),
        gatr_anchor: GatrAnchor {
            anchor_price: Some(anchor_price),
            anchor_gatr: Some(0.40),
        },
        live_annotation_id: live_id,
        broker_order_id: None,
        pinned: false,
        // Beyond the recency guard so the snap can fire.
        updated_at: chrono::Utc::now() - chrono::Duration::try_hours(2).unwrap(),
    }
}

/// Panel-only PLTR regression: no chart bracket, stored
/// `(Buy, Limit).entry_price_or_offset = 112.66`, market at 14.45.
/// After `apply_snap_to_intent`, the EntryMemory is shifted to
/// ~14.45 and the hydrated panel shows the corrected limit_price.
#[test]
fn snap_panel_only_pltr_regression() {
    let mut store = AnnotationStore::new();
    let handle = MockHandle::new();
    let symbol = SymbolKey::new("PLTR");

    handle.seed(
        symbol.clone(),
        snap_intent(
            "PLTR",
            EntryType::Limit,
            112.66, // stale anchor from the ACME era
            Some(112.66),
            None,
            None,
            None, // no live bracket — panel-only
        ),
    );

    let mut panels = panels_map(vec![make_panel(1, "PLTR", None)]);

    let applied = apply_snap_to_intent(&mut store, &handle, &mut panels, &symbol, 14.45, Some(0.40))
        .expect("snap should fire");
    assert!((applied.delta + 98.21).abs() < 1e-6, "delta = 14.45 - 112.66");

    // Intent EntryMemory shifted.
    let intent = handle.snapshot(&symbol).unwrap();
    let mem = intent
        .entries
        .get(&(OrderSide::Buy, EntryType::Limit))
        .unwrap();
    let new_entry = mem.entry_price_or_offset.unwrap();
    assert!(
        (new_entry - 14.45).abs() < 1e-6,
        "EntryMemory.entry_price_or_offset shifted from 112.66 to ~14.45, got {new_entry}"
    );
    // Anchor refreshed to current price.
    assert_eq!(intent.gatr_anchor.anchor_price, Some(14.45));

    // Hydrated panel shows the corrected value.
    let panel = panels.values().next().unwrap();
    assert_eq!(panel.state.limit_price, "14.45");
}

/// Both surfaces lockstep: an intent with a stale `(Buy, Limit)`
/// bucket AND a stale live bracket annotation. After
/// `apply_snap_to_intent`, both are shifted by the same delta.
#[test]
fn snap_lockstep_shifts_entry_memory_and_annotation() {
    let mut store = AnnotationStore::new();
    // Live bracket at the stale price era.
    let ann_id = store.add(
        "PLTR",
        AnnotationKind::OrderBracket(Box::new(make_bracket(112.66, Some(115.0), Some(110.0)))),
    );

    let handle = MockHandle::new();
    handle.seed(
        SymbolKey::new("PLTR"),
        snap_intent(
            "PLTR",
            EntryType::Limit,
            112.66,
            Some(112.66),
            Some(115.0),
            Some(110.0),
            Some(ann_id),
        ),
    );

    let mut panels = panels_map(vec![make_panel(1, "PLTR", Some(ann_id))]);
    let symbol = SymbolKey::new("PLTR");

    let applied = apply_snap_to_intent(&mut store, &handle, &mut panels, &symbol, 14.45, Some(0.40))
        .expect("snap should fire");
    let delta = applied.delta;

    // Intent shifted.
    let intent = handle.snapshot(&symbol).unwrap();
    let mem = intent
        .entries
        .get(&(OrderSide::Buy, EntryType::Limit))
        .unwrap();
    assert!((mem.entry_price_or_offset.unwrap() - (112.66 + delta)).abs() < 1e-6);
    let new_tp: f64 = mem.tp_value.parse().unwrap();
    assert!((new_tp - (115.0 + delta)).abs() < 1e-6);
    let new_sl: f64 = mem.sl_value.parse().unwrap();
    assert!((new_sl - (110.0 + delta)).abs() < 1e-6);

    // Chart bracket shifted by the same delta.
    let bracket = store.get_bracket("PLTR", ann_id).unwrap();
    assert!((bracket.entry.line.price - (112.66 + delta)).abs() < 1e-6);

    // Undo slot seeded with both the prev intent and the bracket clone.
    assert_eq!(applied.pre_snap.annotation_id, Some(ann_id));
    assert!(applied.pre_snap.bracket.is_some());
    let prev_entry = applied
        .pre_snap
        .prev_intent
        .entries
        .get(&(OrderSide::Buy, EntryType::Limit))
        .unwrap()
        .entry_price_or_offset;
    assert_eq!(prev_entry, Some(112.66));
}

/// Panel-only snap has no bracket to reposition → `pre_snap.bracket`
/// is `None` and `pre_snap.annotation_id` is `None`.
#[test]
fn snap_panel_only_pre_snap_has_no_bracket() {
    let mut store = AnnotationStore::new();
    let handle = MockHandle::new();
    let symbol = SymbolKey::new("PLTR");
    handle.seed(
        symbol.clone(),
        snap_intent("PLTR", EntryType::Limit, 112.66, Some(112.66), None, None, None),
    );
    let mut panels = panels_map(vec![make_panel(1, "PLTR", None)]);

    let applied =
        apply_snap_to_intent(&mut store, &handle, &mut panels, &symbol, 14.45, Some(0.40)).unwrap();
    assert!(applied.pre_snap.annotation_id.is_none());
    assert!(applied.pre_snap.bracket.is_none());
}

/// The snap skips TP/SL fields that are in Offset / Percent mode
/// (they are relative to entry, not absolute).
#[test]
fn snap_skips_offset_and_percent_mode_fields() {
    let mut store = AnnotationStore::new();
    let handle = MockHandle::new();
    let symbol = SymbolKey::new("PLTR");

    let memory = EntryMemory {
        entry_price_or_offset: Some(112.66),
        quantity: Some(100.0),
        tp_enabled: true,
        tp_value: "1.00".to_string(), // offset: +1.00 off entry
        tp_mode: PriceInputMode::Offset,
        sl_enabled: true,
        sl_value: "2.5".to_string(), // percent: 2.5% below entry
        sl_mode: PriceInputMode::Percent,
        sl_type: StopLossType::Stop,
        sl_limit_value: String::new(),
    };
    handle.seed(
        symbol.clone(),
        TickerOrderIntent {
            version: CURRENT_VERSION,
            symbol: symbol.clone(),
            last_side: OrderSide::Buy,
            last_entry_type: EntryType::Limit,
            entries: [((OrderSide::Buy, EntryType::Limit), memory)]
                .into_iter()
                .collect(),
            gatr_anchor: GatrAnchor {
                anchor_price: Some(112.66),
                anchor_gatr: Some(0.40),
            },
            live_annotation_id: None,
            broker_order_id: None,
            pinned: false,
            updated_at: chrono::Utc::now() - chrono::Duration::try_hours(2).unwrap(),
        },
    );
    let mut panels = panels_map(vec![make_panel(1, "PLTR", None)]);

    apply_snap_to_intent(&mut store, &handle, &mut panels, &symbol, 14.45, Some(0.40)).unwrap();

    let intent = handle.snapshot(&symbol).unwrap();
    let mem = intent
        .entries
        .get(&(OrderSide::Buy, EntryType::Limit))
        .unwrap();
    // Entry shifted.
    assert!((mem.entry_price_or_offset.unwrap() - 14.45).abs() < 1e-6);
    // TP / SL untouched — the string values round-trip verbatim.
    assert_eq!(mem.tp_value, "1.00");
    assert_eq!(mem.sl_value, "2.5");
}

/// Market entry type: `entry_price_or_offset` is not shifted (it is
/// always the live last_price, not a stored absolute), but absolute
/// TP / SL fields in that bucket still are.
#[test]
fn snap_market_bucket_skips_entry_but_shifts_absolute_tp_sl() {
    let mut store = AnnotationStore::new();
    let handle = MockHandle::new();
    let symbol = SymbolKey::new("PLTR");

    let memory = EntryMemory {
        entry_price_or_offset: Some(112.66), // stale; Market ignores it
        quantity: Some(100.0),
        tp_enabled: true,
        tp_value: "120.00".to_string(),
        tp_mode: PriceInputMode::Absolute,
        sl_enabled: true,
        sl_value: "110.00".to_string(),
        sl_mode: PriceInputMode::Absolute,
        sl_type: StopLossType::Stop,
        sl_limit_value: String::new(),
    };
    handle.seed(
        symbol.clone(),
        TickerOrderIntent {
            version: CURRENT_VERSION,
            symbol: symbol.clone(),
            last_side: OrderSide::Buy,
            last_entry_type: EntryType::Market,
            entries: [((OrderSide::Buy, EntryType::Market), memory)]
                .into_iter()
                .collect(),
            gatr_anchor: GatrAnchor {
                anchor_price: Some(112.66),
                anchor_gatr: Some(0.40),
            },
            live_annotation_id: None,
            broker_order_id: None,
            pinned: false,
            updated_at: chrono::Utc::now() - chrono::Duration::try_hours(2).unwrap(),
        },
    );
    let mut panels = panels_map(vec![make_panel(1, "PLTR", None)]);

    apply_snap_to_intent(&mut store, &handle, &mut panels, &symbol, 14.45, Some(0.40)).unwrap();

    let intent = handle.snapshot(&symbol).unwrap();
    let mem = intent
        .entries
        .get(&(OrderSide::Buy, EntryType::Market))
        .unwrap();
    // Market entry: stored entry_price_or_offset is NOT shifted.
    assert_eq!(mem.entry_price_or_offset, Some(112.66));
    // TP / SL shifted by the same delta.
    let delta = 14.45 - 112.66;
    let new_tp: f64 = mem.tp_value.parse().unwrap();
    assert!((new_tp - (120.0 + delta)).abs() < 1e-6);
    let new_sl: f64 = mem.sl_value.parse().unwrap();
    assert!((new_sl - (110.0 + delta)).abs() < 1e-6);
}

/// StopLimit SL type: `sl_limit_value` is an absolute dollar level
/// (the limit leg of the stop-limit order) and is shifted too.
#[test]
fn snap_shifts_stoplimit_sl_limit_value() {
    let mut store = AnnotationStore::new();
    let handle = MockHandle::new();
    let symbol = SymbolKey::new("PLTR");

    let memory = EntryMemory {
        entry_price_or_offset: Some(112.66),
        quantity: Some(100.0),
        tp_enabled: false,
        tp_value: String::new(),
        tp_mode: PriceInputMode::Absolute,
        sl_enabled: true,
        sl_value: "110.00".to_string(),
        sl_mode: PriceInputMode::Absolute,
        sl_type: StopLossType::StopLimit,
        sl_limit_value: "105.00".to_string(),
    };
    handle.seed(
        symbol.clone(),
        TickerOrderIntent {
            version: CURRENT_VERSION,
            symbol: symbol.clone(),
            last_side: OrderSide::Buy,
            last_entry_type: EntryType::Limit,
            entries: [((OrderSide::Buy, EntryType::Limit), memory)]
                .into_iter()
                .collect(),
            gatr_anchor: GatrAnchor {
                anchor_price: Some(112.66),
                anchor_gatr: Some(0.40),
            },
            live_annotation_id: None,
            broker_order_id: None,
            pinned: false,
            updated_at: chrono::Utc::now() - chrono::Duration::try_hours(2).unwrap(),
        },
    );
    let mut panels = panels_map(vec![make_panel(1, "PLTR", None)]);
    apply_snap_to_intent(&mut store, &handle, &mut panels, &symbol, 14.45, Some(0.40)).unwrap();

    let intent = handle.snapshot(&symbol).unwrap();
    let mem = intent
        .entries
        .get(&(OrderSide::Buy, EntryType::Limit))
        .unwrap();
    let delta = 14.45 - 112.66;
    let new_sl_limit: f64 = mem.sl_limit_value.parse().unwrap();
    assert!((new_sl_limit - (105.0 + delta)).abs() < 1e-6);
}

/// No intent → helper returns `None` without touching the store.
#[test]
fn snap_missing_intent_returns_none() {
    let mut store = AnnotationStore::new();
    let handle = MockHandle::new();
    let mut panels = panels_map(vec![]);
    let symbol = SymbolKey::new("MISSING");
    let applied =
        apply_snap_to_intent(&mut store, &handle, &mut panels, &symbol, 14.45, Some(0.40));
    assert!(applied.is_none());
}

/// Guards drop the snap → helper returns `None` and no mutations.
#[test]
fn snap_respects_pinned_guard() {
    let mut store = AnnotationStore::new();
    let handle = MockHandle::new();
    let symbol = SymbolKey::new("PLTR");

    let mut intent = snap_intent("PLTR", EntryType::Limit, 112.66, Some(112.66), None, None, None);
    intent.pinned = true;
    handle.seed(symbol.clone(), intent);

    let mut panels = panels_map(vec![make_panel(1, "PLTR", None)]);
    assert!(
        apply_snap_to_intent(&mut store, &handle, &mut panels, &symbol, 14.45, Some(0.40))
            .is_none()
    );
    // Intent unchanged.
    let intent = handle.snapshot(&symbol).unwrap();
    let mem = intent
        .entries
        .get(&(OrderSide::Buy, EntryType::Limit))
        .unwrap();
    assert_eq!(mem.entry_price_or_offset, Some(112.66));
}

/// With no `current_price` available, anchor-seeding is skipped.
/// The upsert still lands, but outcome is plain `Applied`.
#[test]
fn anchor_lifecycle_missing_price_does_not_seed() {
    let mut store = AnnotationStore::new();
    let ann_id = store.add(
        "AAPL",
        AnnotationKind::OrderBracket(Box::new(make_bracket(100.0, Some(102.0), Some(98.0)))),
    );
    let handle = MockHandle::new();
    handle.seed(
        SymbolKey::new("AAPL"),
        make_intent(
            "AAPL",
            OrderSide::Buy,
            EntryType::Limit,
            vec![(
                (OrderSide::Buy, EntryType::Limit),
                make_memory(100.0, Some(102.0), Some(98.0), true),
            )],
            Some(ann_id),
        ),
    );
    let mut panels = panels_map(vec![make_panel(1, "AAPL", Some(ann_id))]);
    let snapshot = make_intent(
        "AAPL",
        OrderSide::Buy,
        EntryType::Limit,
        vec![(
            (OrderSide::Buy, EntryType::Limit),
            make_memory(105.0, Some(107.0), Some(103.0), true),
        )],
        Some(ann_id),
    );
    let outcome = apply_update_from_surface(
        &mut store,
        &handle,
        &mut panels,
        None,
        SymbolKey::new("AAPL"),
        snapshot,
        IntentSource::Panel,
        None,
        None,
    );
    assert_eq!(outcome, UpdateSurfaceOutcome::Applied);
    let intent = handle.snapshot(&SymbolKey::new("AAPL")).unwrap();
    assert!(intent.gatr_anchor.anchor_price.is_none());
}
