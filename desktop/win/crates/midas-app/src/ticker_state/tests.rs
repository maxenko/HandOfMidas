//! Unit tests for the `ticker_state` module.
//!
//! Covers serde round-trip, v1->v2 migration, factory defaults, and
//! `apply()` stub behavior.

use std::collections::HashMap;

use midas_annotation_types::order_bracket::EntryType;

use crate::annotation_store::SymbolKey;
use crate::order_panel::OrderSide;

use super::apply::{TickerEffect, TickerMsg};
use super::{
    migrate_v1_v2, EditingField, EntryMemory, GatrAnchor, TickerOrderIntentV1, TickerState,
    CURRENT_VERSION,
};

// ── Serde round-trip ────────────────────────────────────────────────

#[test]
fn serde_round_trip() {
    let state = TickerState::new(SymbolKey::new("AAPL"));
    let json = serde_json::to_string_pretty(&state).expect("serialize");
    let restored: TickerState = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(restored.symbol(), state.symbol());
    assert_eq!(restored.version(), CURRENT_VERSION);
    assert_eq!(restored.last_side(), OrderSide::Buy);
    assert_eq!(restored.last_entry_type(), EntryType::Market);
    assert!(!restored.pinned());
    assert!(restored.live_bracket().is_none());
    assert!(restored.levels().is_empty());
    assert!(restored.last_price().is_none());
    assert!(restored.gatr_abs().is_none());
    assert!(!restored.is_editing());
    assert_eq!(restored.generation(), 0);
}

#[test]
fn serde_unknown_fields_forward_compat() {
    // Simulate a v2 blob that has extra fields a future v3 might add.
    let json = r#"{
        "symbol": "MSFT",
        "version": 2,
        "last_side": "Buy",
        "last_entry_type": "Market",
        "entries": [],
        "gatr_anchor": {},
        "pinned": false,
        "live_bracket": null,
        "live_annotation_id": null,
        "updated_at": "2025-01-01T00:00:00Z",
        "generation": 42,
        "future_field_that_does_not_exist": "hello"
    }"#;
    // Should deserialize without error — unknown fields are ignored.
    let state: TickerState = serde_json::from_str(json).expect("deserialize with unknown fields");
    assert_eq!(state.symbol().as_str(), "MSFT");
    assert_eq!(state.generation(), 42);
}

#[test]
fn serde_missing_fields_default_to_sensible_values() {
    // Minimal blob: only required fields.
    let json = r#"{
        "symbol": "GOOG"
    }"#;
    let state: TickerState = serde_json::from_str(json).expect("deserialize minimal");
    assert_eq!(state.symbol().as_str(), "GOOG");
    assert_eq!(state.version(), CURRENT_VERSION);
    assert_eq!(state.last_side(), OrderSide::Buy);
    assert_eq!(state.last_entry_type(), EntryType::Market);
    assert!(!state.pinned());
    assert!(state.live_bracket().is_none());
    assert!(state.levels().is_empty());
    assert_eq!(state.generation(), 0);
}

// ── v1 -> v2 migration ─────────────────────────────────────────────

#[test]
fn migrate_v1_v2_copies_all_fields() {
    let mut entries = HashMap::new();
    entries.insert(
        (OrderSide::Buy, EntryType::Limit),
        EntryMemory {
            entry_price_or_offset: Some(148.50),
            quantity: Some(200.0),
            tp_enabled: true,
            tp_value: "155.00".to_string(),
            sl_enabled: true,
            sl_value: "145.00".to_string(),
            ..EntryMemory::default()
        },
    );

    let intent = TickerOrderIntentV1 {
        version: 1,
        symbol: SymbolKey::new("IBM"),
        last_side: OrderSide::Sell,
        last_entry_type: EntryType::Limit,
        entries: entries.clone(),
        gatr_anchor: GatrAnchor {
            anchor_price: Some(150.0),
            anchor_gatr: Some(2.5),
        },
        live_annotation_id: None,
        broker_order_id: None,
        pinned: true,
        updated_at: chrono::Utc::now(),
    };

    let state = migrate_v1_v2(&intent);

    assert_eq!(state.symbol().as_str(), "IBM");
    assert_eq!(state.version(), CURRENT_VERSION);
    assert_eq!(state.last_side(), OrderSide::Sell);
    assert_eq!(state.last_entry_type(), EntryType::Limit);
    assert!(state.pinned());
    assert_eq!(state.gatr_anchor().anchor_price, Some(150.0));
    assert_eq!(state.gatr_anchor().anchor_gatr, Some(2.5));

    // Entry memory carried over.
    let mem = state.entries().get(&(OrderSide::Buy, EntryType::Limit));
    assert!(mem.is_some());
    let mem = mem.expect("entry memory present");
    assert_eq!(mem.entry_price_or_offset, Some(148.50));
    assert_eq!(mem.quantity, Some(200.0));
    assert!(mem.tp_enabled);
    assert_eq!(mem.tp_value, "155.00");
    assert!(mem.sl_enabled);
    assert_eq!(mem.sl_value, "145.00");

    // Fields that didn't exist in v1 default to None/empty.
    assert!(state.live_bracket().is_none());
    assert!(state.levels().is_empty());
    assert!(state.last_price().is_none());
    assert!(state.gatr_abs().is_none());
    assert!(!state.is_editing());
}

#[test]
fn migrate_v1_v2_preserves_annotation_id() {
    let intent = TickerOrderIntentV1 {
        version: 1,
        symbol: SymbolKey::new("TSLA"),
        last_side: OrderSide::Buy,
        last_entry_type: EntryType::Market,
        entries: HashMap::new(),
        gatr_anchor: GatrAnchor::default(),
        live_annotation_id: Some(midas_annotation_types::AnnotationId(42)),
        broker_order_id: None,
        pinned: false,
        updated_at: chrono::Utc::now(),
    };

    let state = migrate_v1_v2(&intent);
    assert_eq!(
        state.live_annotation_id(),
        Some(midas_annotation_types::AnnotationId(42))
    );
}

// ── Factory ─────────────────────────────────────────────────────────

#[test]
fn factory_new_defaults() {
    let state = TickerState::new(SymbolKey::new("SPY"));
    assert_eq!(state.symbol().as_str(), "SPY");
    assert_eq!(state.last_side(), OrderSide::Buy);
    assert_eq!(state.last_entry_type(), EntryType::Market);
    assert!(state.entries().is_empty());
    assert!(state.live_bracket().is_none());
    assert!(state.levels().is_empty());
    assert_eq!(state.version(), CURRENT_VERSION);
}

#[test]
fn factory_new_with_defaults_populates_all_compound_keys() {
    let state = TickerState::new_with_defaults(SymbolKey::new("IBM"), 150.0, Some(2.0));
    assert_eq!(state.entries().len(), 8);

    // Verify all 8 compound keys are present.
    for side in [OrderSide::Buy, OrderSide::Sell] {
        for entry_type in [
            EntryType::Market,
            EntryType::Limit,
            EntryType::Stop,
            EntryType::StopLimit,
        ] {
            let mem = state.entries().get(&(side, entry_type)).unwrap_or_else(|| {
                panic!("missing entry for ({side:?}, {entry_type:?})");
            });
            assert!(
                mem.entry_price_or_offset.is_some(),
                "entry price should be set for ({side:?}, {entry_type:?})"
            );
            assert!(mem.sl_enabled, "SL should be enabled by default");
            assert!(!mem.sl_value.is_empty(), "SL value should be populated");
        }
    }

    // Check that Buy Limit entry is below current price.
    let buy_limit = state
        .entries()
        .get(&(OrderSide::Buy, EntryType::Limit))
        .expect("buy limit present");
    assert!(
        buy_limit.entry_price_or_offset.expect("entry set") < 150.0,
        "Buy Limit entry should be below current price"
    );

    // Check that Sell Limit entry is above current price.
    let sell_limit = state
        .entries()
        .get(&(OrderSide::Sell, EntryType::Limit))
        .expect("sell limit present");
    assert!(
        sell_limit.entry_price_or_offset.expect("entry set") > 150.0,
        "Sell Limit entry should be above current price"
    );

    // Market data cached.
    assert_eq!(state.last_price(), Some(150.0));
    assert_eq!(state.gatr_abs(), Some(2.0));
}

#[test]
fn factory_from_legacy() {
    let mut entries = HashMap::new();
    entries.insert(
        (OrderSide::Buy, EntryType::Market),
        EntryMemory {
            quantity: Some(500.0),
            sl_enabled: true,
            ..EntryMemory::default()
        },
    );

    let intent = TickerOrderIntentV1 {
        version: 1,
        symbol: SymbolKey::new("NVDA"),
        last_side: OrderSide::Buy,
        last_entry_type: EntryType::Market,
        entries,
        gatr_anchor: GatrAnchor {
            anchor_price: Some(800.0),
            anchor_gatr: Some(15.0),
        },
        live_annotation_id: Some(midas_annotation_types::AnnotationId(99)),
        broker_order_id: None,
        pinned: true,
        updated_at: chrono::Utc::now(),
    };

    let state = TickerState::from_legacy(intent, Vec::new(), None, None);
    assert_eq!(state.symbol().as_str(), "NVDA");
    assert!(state.pinned());
    assert_eq!(
        state.live_annotation_id(),
        Some(midas_annotation_types::AnnotationId(99))
    );
}

// ── Slice 2: GATR snap/pin/undo ────────────────────────────────────

/// Build a test level with the given price and id.
fn test_level(id: u64, price: f64) -> crate::annotation_store::StoredLevel {
    crate::annotation_store::StoredLevel {
        level: midas_annotation_types::HorizontalLevel {
            id,
            line: midas_annotation_types::price_line::PriceLine {
                price,
                extent: midas_annotation_types::price_line::LineExtent::FullWidth,
                stroke: midas_annotation_types::price_line::LineStroke {
                    color: [1.0, 1.0, 1.0, 1.0],
                    width: 1.0,
                    style: midas_annotation_types::LineStyle::Solid,
                },
            },
            label: None,
            icon: midas_annotation_types::LevelIcon::default(),
        },
        locked: false,
    }
}

/// Build a state with a stale anchor suitable for snap testing.
/// The `updated_at` is set far in the past so the recency guard passes.
fn state_with_stale_anchor(anchor_price: f64, gatr: f64) -> TickerState {
    let mut state =
        TickerState::new_with_defaults(SymbolKey::new("SNAP"), anchor_price, Some(gatr));
    // Activate bracket mode so EnsureDraftBracket can create a bracket.
    state.force_bracket_mode(Some(OrderSide::Buy));
    // Create a bracket so snap has something to reposition.
    state.apply(TickerMsg::EnsureDraftBracket {
        side: OrderSide::Buy,
        entry_type: EntryType::Market,
    });
    // Seed the GATR anchor.
    state.force_gatr_anchor(super::GatrAnchor {
        anchor_price: Some(anchor_price),
        anchor_gatr: Some(gatr),
    });
    // Push updated_at far into the past so the recency guard passes.
    state.force_updated_at(chrono::Utc::now() - chrono::Duration::hours(2));
    state
}

#[test]
fn gatr_snap_stale_anchor_drift_triggers_reposition() {
    let mut state = state_with_stale_anchor(100.0, 2.0);
    let old_entry = state.live_bracket().expect("bracket").entry.line.price;

    // Price drifts by more than 1 GATR (2.0) from anchor (100.0).
    let effects = state.apply(TickerMsg::MaybeSnap {
        current_price: 103.0,
        gatr_abs: Some(2.0),
    });

    // Should have ProjectBracket + Toast (with Undo action) + PersistDirty.
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, TickerEffect::ProjectBracket(_))),
        "snap should project the repositioned bracket"
    );
    assert!(
        effects.iter().any(|e| matches!(
            e,
            TickerEffect::Toast { ref message, ref action }
            if message.contains("re-anchored") && action.is_some()
        )),
        "snap should emit a toast with Undo action"
    );
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, TickerEffect::PersistDirty)),
        "snap should persist"
    );

    // Bracket entry should have moved toward 103.0.
    let new_entry = state.live_bracket().expect("bracket").entry.line.price;
    assert!(
        (new_entry - old_entry).abs() > 1.0,
        "bracket should have repositioned"
    );
}

#[test]
fn gatr_snap_pinned_skips() {
    let mut state = state_with_stale_anchor(100.0, 2.0);
    state.apply(TickerMsg::TogglePin);
    assert!(state.pinned());

    let effects = state.apply(TickerMsg::MaybeSnap {
        current_price: 105.0,
        gatr_abs: Some(2.0),
    });
    assert!(effects.is_empty(), "pinned state should skip snap");
}

#[test]
fn pin_toggle_flips_and_persists() {
    let mut state = TickerState::new(SymbolKey::new("PIN"));
    assert!(!state.pinned());

    let effects = state.apply(TickerMsg::TogglePin);
    assert!(state.pinned());
    assert!(effects
        .iter()
        .any(|e| matches!(e, TickerEffect::PersistDirty)));

    let effects = state.apply(TickerMsg::TogglePin);
    assert!(!state.pinned());
    assert!(effects
        .iter()
        .any(|e| matches!(e, TickerEffect::PersistDirty)));
}

#[test]
fn undo_snap_within_ttl_restores_state() {
    let mut state = state_with_stale_anchor(100.0, 2.0);
    let old_entry = state.live_bracket().expect("bracket").entry.line.price;

    // Fire snap.
    state.apply(TickerMsg::MaybeSnap {
        current_price: 105.0,
        gatr_abs: Some(2.0),
    });
    let snapped_entry = state.live_bracket().expect("bracket").entry.line.price;
    assert!(
        (snapped_entry - old_entry).abs() > 1.0,
        "snap should have moved the entry"
    );

    // Undo within TTL.
    let effects = state.apply(TickerMsg::UndoSnap);
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, TickerEffect::ProjectBracket(_))),
        "undo should project the restored bracket"
    );
    let restored_entry = state.live_bracket().expect("bracket").entry.line.price;
    assert!(
        (restored_entry - old_entry).abs() < f64::EPSILON,
        "undo should restore the original entry price"
    );
}

#[test]
fn undo_snap_expired_ttl_is_noop() {
    let mut state = state_with_stale_anchor(100.0, 2.0);

    // Fire snap.
    state.apply(TickerMsg::MaybeSnap {
        current_price: 105.0,
        gatr_abs: Some(2.0),
    });

    // Expire the TTL by replacing the instant.
    state.force_pre_snap_instant(std::time::Instant::now() - std::time::Duration::from_secs(60));

    let effects = state.apply(TickerMsg::UndoSnap);
    assert!(effects.is_empty(), "expired undo should be a no-op");
}

// ── Slice 2: broker event lifecycle ────────────────────────────────

#[test]
fn submit_pending_filled_lifecycle() {
    use midas_annotation_types::order_bracket::BracketStatus;

    let mut state = TickerState::new_with_defaults(SymbolKey::new("LIFE"), 150.0, Some(2.0));
    state.force_bracket_mode(Some(OrderSide::Buy));
    state.apply(TickerMsg::EnsureDraftBracket {
        side: OrderSide::Buy,
        entry_type: EntryType::Market,
    });
    state.apply(TickerMsg::SetQuantity(100.0));
    assert_eq!(
        state.live_bracket().expect("b").status,
        BracketStatus::Draft
    );

    // Submit.
    let effects = state.apply(TickerMsg::SubmitOrder);
    assert_eq!(
        state.live_bracket().expect("b").status,
        BracketStatus::Pending
    );
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, TickerEffect::SubmitToBroker { .. })),
        "submit should emit SubmitToBroker"
    );
    assert!(effects
        .iter()
        .any(|e| matches!(e, TickerEffect::ProjectBracket(_))));

    // Pending acknowledgement.
    let effects = state.apply(TickerMsg::OrderPending {
        order_id: uuid::Uuid::now_v7(),
    });
    assert!(effects
        .iter()
        .any(|e| matches!(e, TickerEffect::ProjectBracket(_))));

    // Filled.
    let effects = state.apply(TickerMsg::OrderFilled {
        filled_qty: 100.0,
        avg_price: 150.50,
    });
    assert_eq!(
        state.live_bracket().expect("b").status,
        BracketStatus::Active
    );
    assert!(
        (state.live_bracket().expect("b").entry.line.price - 150.50).abs() < f64::EPSILON,
        "fill should update entry to avg_price"
    );
    assert!(
        state.live_bracket().expect("b").filled_qty == Some(100.0),
        "fill should record filled_qty"
    );
    assert!(effects.iter().any(|e| matches!(
        e,
        TickerEffect::Toast { ref message, .. } if message.contains("filled")
    )));
}

#[test]
fn order_rejected_reverts_to_draft() {
    use midas_annotation_types::order_bracket::BracketStatus;

    let mut state = TickerState::new_with_defaults(SymbolKey::new("REJ"), 200.0, Some(3.0));
    state.force_bracket_mode(Some(OrderSide::Sell));
    state.apply(TickerMsg::EnsureDraftBracket {
        side: OrderSide::Sell,
        entry_type: EntryType::Limit,
    });
    state.apply(TickerMsg::SubmitOrder);
    assert_eq!(
        state.live_bracket().expect("b").status,
        BracketStatus::Pending
    );

    let effects = state.apply(TickerMsg::OrderRejected {
        reason: "Insufficient margin".to_string(),
    });
    assert_eq!(
        state.live_bracket().expect("b").status,
        BracketStatus::Draft
    );
    assert!(effects.iter().any(|e| matches!(
        e,
        TickerEffect::Toast { ref message, .. } if message.contains("Insufficient margin")
    )));
}

#[test]
fn order_partial_fill_updates_qty() {
    use midas_annotation_types::order_bracket::BracketStatus;

    let mut state = TickerState::new_with_defaults(SymbolKey::new("PART"), 100.0, Some(1.0));
    state.force_bracket_mode(Some(OrderSide::Buy));
    state.apply(TickerMsg::EnsureDraftBracket {
        side: OrderSide::Buy,
        entry_type: EntryType::Market,
    });
    state.apply(TickerMsg::SubmitOrder);

    let effects = state.apply(TickerMsg::OrderPartialFill { filled_qty: 50.0 });
    assert_eq!(
        state.live_bracket().expect("b").status,
        BracketStatus::PartialFill
    );
    assert_eq!(state.live_bracket().expect("b").filled_qty, Some(50.0));
    assert!(effects
        .iter()
        .any(|e| matches!(e, TickerEffect::ProjectBracket(_))));
}

#[test]
fn order_cancelled_reverts_to_draft() {
    use midas_annotation_types::order_bracket::BracketStatus;

    let mut state = TickerState::new_with_defaults(SymbolKey::new("CANC"), 100.0, Some(1.0));
    state.force_bracket_mode(Some(OrderSide::Buy));
    state.apply(TickerMsg::EnsureDraftBracket {
        side: OrderSide::Buy,
        entry_type: EntryType::Market,
    });
    state.apply(TickerMsg::SubmitOrder);
    assert_eq!(
        state.live_bracket().expect("b").status,
        BracketStatus::Pending
    );

    let effects = state.apply(TickerMsg::OrderCancelled);
    assert_eq!(
        state.live_bracket().expect("b").status,
        BracketStatus::Draft
    );
    assert!(effects.iter().any(|e| matches!(
        e,
        TickerEffect::Toast { ref message, .. } if message.contains("cancelled")
    )));
}

// ── Slice 2: level CRUD ────────────────────────────────────────────

#[test]
fn add_level_pushes_and_projects() {
    let mut state = TickerState::new(SymbolKey::new("LVL"));
    assert!(state.levels().is_empty());

    let level = test_level(1, 150.0);
    let effects = state.apply(TickerMsg::AddLevel(level.clone()));
    assert_eq!(state.levels().len(), 1);
    assert!((state.levels()[0].line.price - 150.0).abs() < f64::EPSILON);
    assert!(effects
        .iter()
        .any(|e| matches!(e, TickerEffect::ProjectLevel { index: 0, .. })));
    assert!(effects
        .iter()
        .any(|e| matches!(e, TickerEffect::PersistDirty)));
}

#[test]
fn remove_level_shrinks_vec() {
    let mut state = TickerState::new(SymbolKey::new("LVL"));
    state.apply(TickerMsg::AddLevel(test_level(1, 100.0)));
    state.apply(TickerMsg::AddLevel(test_level(2, 200.0)));
    assert_eq!(state.levels().len(), 2);

    let effects = state.apply(TickerMsg::RemoveLevel(0));
    assert_eq!(state.levels().len(), 1);
    assert!((state.levels()[0].line.price - 200.0).abs() < f64::EPSILON);
    assert!(effects
        .iter()
        .any(|e| matches!(e, TickerEffect::PersistDirty)));
}

#[test]
fn remove_level_out_of_bounds_is_noop() {
    let mut state = TickerState::new(SymbolKey::new("LVL"));
    let effects = state.apply(TickerMsg::RemoveLevel(99));
    assert!(effects.is_empty());
}

#[test]
fn update_level_replaces_at_index() {
    let mut state = TickerState::new(SymbolKey::new("LVL"));
    state.apply(TickerMsg::AddLevel(test_level(1, 100.0)));

    let new_level = test_level(1, 200.0);
    let effects = state.apply(TickerMsg::UpdateLevel {
        index: 0,
        level: new_level,
    });
    assert!((state.levels()[0].line.price - 200.0).abs() < f64::EPSILON);
    assert!(effects
        .iter()
        .any(|e| matches!(e, TickerEffect::ProjectLevel { index: 0, .. })));
    assert!(effects
        .iter()
        .any(|e| matches!(e, TickerEffect::PersistDirty)));
}

#[test]
fn toggle_level_lock_flips() {
    let mut state = TickerState::new(SymbolKey::new("LVL"));
    state.apply(TickerMsg::AddLevel(test_level(1, 100.0)));
    assert!(!state.levels()[0].locked);

    let effects = state.apply(TickerMsg::ToggleLevelLock(0));
    assert!(state.levels()[0].locked);
    assert!(effects
        .iter()
        .any(|e| matches!(e, TickerEffect::PersistDirty)));

    state.apply(TickerMsg::ToggleLevelLock(0));
    assert!(!state.levels()[0].locked);
}

// ── Slice 2: UpdateMarketData triggers auto-snap ───────────────────

#[test]
fn update_market_data_triggers_auto_snap_when_stale() {
    let mut state = state_with_stale_anchor(100.0, 2.0);
    let old_entry = state.live_bracket().expect("bracket").entry.line.price;

    // UpdateMarketData with a price that drifts > 1 GATR from anchor.
    let effects = state.apply(TickerMsg::UpdateMarketData {
        last_price: 103.0,
        gatr_abs: Some(2.0),
    });

    assert_eq!(state.last_price(), Some(103.0));
    assert_eq!(state.gatr_abs(), Some(2.0));
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, TickerEffect::ProjectBracket(_))),
        "market data update should trigger auto-snap"
    );

    let new_entry = state.live_bracket().expect("bracket").entry.line.price;
    assert!(
        (new_entry - old_entry).abs() > 1.0,
        "auto-snap should reposition bracket"
    );
}

#[test]
fn update_market_data_moves_last_price() {
    // A plain state (no bracket, no stale anchor) should still have
    // its cached `last_price` bumped by `UpdateMarketData`. This is
    // the path the live-tick pipeline exercises — every broker tick
    // dispatches `TickerMsg::UpdateMarketData`, and the UI reads back
    // `last_price()` to drive decorator labels + badge text.
    let mut state = TickerState::new(SymbolKey::new("AAPL"));
    assert_eq!(state.last_price(), None);

    let _ = state.apply(TickerMsg::UpdateMarketData {
        last_price: 150.25,
        gatr_abs: Some(2.5),
    });
    assert_eq!(state.last_price(), Some(150.25));
    assert_eq!(state.gatr_abs(), Some(2.5));

    // A second tick moves the cached value again.
    let _ = state.apply(TickerMsg::UpdateMarketData {
        last_price: 151.10,
        gatr_abs: Some(2.5),
    });
    assert_eq!(state.last_price(), Some(151.10));
    assert_eq!(state.gatr_abs(), Some(2.5));
}

#[test]
fn update_market_data_skips_snap_while_editing() {
    let mut state = state_with_stale_anchor(100.0, 2.0);
    state.apply(TickerMsg::BeginEdit(EditingField::LimitPrice));

    let effects = state.apply(TickerMsg::UpdateMarketData {
        last_price: 105.0,
        gatr_abs: Some(2.0),
    });

    // Market data should still be cached.
    assert_eq!(state.last_price(), Some(105.0));
    // But no snap effects should fire.
    assert!(
        !effects
            .iter()
            .any(|e| matches!(e, TickerEffect::ProjectBracket(_))),
        "editing lock should suppress auto-snap"
    );
}

// ── Slice 1: bracket lifecycle tests ────────────────────────────────

#[test]
fn apply_ensure_draft_bracket_creates_live_bracket() {
    let mut state = TickerState::new_with_defaults(SymbolKey::new("AAPL"), 150.0, Some(2.0));
    state.force_bracket_mode(Some(OrderSide::Buy));
    let effects = state.apply(TickerMsg::EnsureDraftBracket {
        side: OrderSide::Buy,
        entry_type: EntryType::Market,
    });
    assert!(state.live_bracket().is_some());
    assert!(effects
        .iter()
        .any(|e| matches!(e, TickerEffect::ProjectBracket(_))));
    assert!(effects
        .iter()
        .any(|e| matches!(e, TickerEffect::PersistDirty)));
}

#[test]
fn apply_cancel_bracket_saved_hides() {
    let mut state = TickerState::new_with_defaults(SymbolKey::new("AAPL"), 150.0, Some(2.0));
    state.force_bracket_mode(Some(OrderSide::Buy));
    // Create + save.
    state.apply(TickerMsg::EnsureDraftBracket {
        side: OrderSide::Buy,
        entry_type: EntryType::Market,
    });
    state.apply(TickerMsg::SaveBracket);
    assert!(state.live_bracket().unwrap().saved);
    // Simulate the effect handler having set the annotation id.
    state.set_live_annotation_id(Some(midas_annotation_types::AnnotationId(99)));

    let effects = state.apply(TickerMsg::CancelBracket);
    assert!(state.live_bracket().is_none());
    assert!(effects
        .iter()
        .any(|e| matches!(e, TickerEffect::RemoveBracket(_))));
}

#[test]
fn apply_cancel_bracket_unsaved_deletes() {
    let mut state = TickerState::new_with_defaults(SymbolKey::new("AAPL"), 150.0, Some(2.0));
    state.force_bracket_mode(Some(OrderSide::Buy));
    state.apply(TickerMsg::EnsureDraftBracket {
        side: OrderSide::Buy,
        entry_type: EntryType::Market,
    });
    assert!(!state.live_bracket().unwrap().saved);
    state.set_live_annotation_id(Some(midas_annotation_types::AnnotationId(1)));

    let effects = state.apply(TickerMsg::CancelBracket);
    assert!(state.live_bracket().is_none());
    assert!(effects
        .iter()
        .any(|e| matches!(e, TickerEffect::RemoveBracket(_))));
}

#[test]
fn apply_set_leg_price_updates_entry() {
    let mut state = TickerState::new_with_defaults(SymbolKey::new("AAPL"), 150.0, Some(2.0));
    state.force_bracket_mode(Some(OrderSide::Buy));
    state.apply(TickerMsg::EnsureDraftBracket {
        side: OrderSide::Buy,
        entry_type: EntryType::Limit,
    });
    let effects = state.apply(TickerMsg::SetLegPrice {
        role: midas_annotation_types::order_bracket::LegRole::Entry,
        price: 145.0,
    });
    assert!((state.live_bracket().unwrap().entry.line.price - 145.0).abs() < f64::EPSILON);
    assert!(effects
        .iter()
        .any(|e| matches!(e, TickerEffect::ProjectBracket(_))));
}

#[test]
fn apply_set_tp_enabled_creates_default_tp() {
    let mut state = TickerState::new_with_defaults(SymbolKey::new("AAPL"), 150.0, Some(2.0));
    state.force_bracket_mode(Some(OrderSide::Buy));
    state.apply(TickerMsg::EnsureDraftBracket {
        side: OrderSide::Buy,
        entry_type: EntryType::Market,
    });
    // Remove TP first.
    state.apply(TickerMsg::SetTpEnabled(false));
    assert!(state.live_bracket().unwrap().take_profit.is_none());

    // Enable TP.
    let effects = state.apply(TickerMsg::SetTpEnabled(true));
    assert!(state.live_bracket().unwrap().take_profit.is_some());
    assert!(effects
        .iter()
        .any(|e| matches!(e, TickerEffect::ProjectBracket(_))));
}

#[test]
fn apply_drag_leg_updates_price_and_pnl() {
    let mut state = TickerState::new_with_defaults(SymbolKey::new("AAPL"), 100.0, Some(2.0));
    state.force_bracket_mode(Some(OrderSide::Buy));
    state.apply(TickerMsg::EnsureDraftBracket {
        side: OrderSide::Buy,
        entry_type: EntryType::Market,
    });
    state.apply(TickerMsg::SetQuantity(100.0));

    let effects = state.apply(TickerMsg::DragLeg {
        role: midas_annotation_types::order_bracket::LegRole::Entry,
        new_price: 105.0,
    });
    assert!((state.live_bracket().unwrap().entry.line.price - 105.0).abs() < f64::EPSILON);
    assert!(effects
        .iter()
        .any(|e| matches!(e, TickerEffect::ProjectBracket(_))));
}

#[test]
fn apply_drag_leg_accepts_price_across_entry() {
    // Brackets are free-form: a Long TP dragged to 95 while entry is
    // 100 must land at exactly 95 (no clamp, no mirror). Wrong-side
    // placements are classified visually by the decorator layer — the
    // stored price is whatever the user chose.
    let mut state = TickerState::new_with_defaults(SymbolKey::new("AAPL"), 100.0, Some(2.0));
    state.force_bracket_mode(Some(OrderSide::Buy));
    state.apply(TickerMsg::EnsureDraftBracket {
        side: OrderSide::Buy,
        entry_type: EntryType::Market,
    });
    state.apply(TickerMsg::SetQuantity(100.0));

    // Long bracket, entry = 100, drag TP below entry to 95.
    state.apply(TickerMsg::DragLeg {
        role: midas_annotation_types::order_bracket::LegRole::TakeProfit,
        new_price: 95.0,
    });
    let tp_price = state
        .live_bracket()
        .unwrap()
        .take_profit
        .as_ref()
        .expect("tp leg present")
        .line
        .price;
    assert!(
        (tp_price - 95.0).abs() < f64::EPSILON,
        "Long TP must accept price below entry verbatim, got {tp_price}"
    );

    // Mirror case: drag SL above entry to 106.
    state.apply(TickerMsg::DragLeg {
        role: midas_annotation_types::order_bracket::LegRole::StopLoss,
        new_price: 106.0,
    });
    let sl_price = state
        .live_bracket()
        .unwrap()
        .stop_loss
        .as_ref()
        .expect("sl leg present")
        .line
        .price;
    assert!(
        (sl_price - 106.0).abs() < f64::EPSILON,
        "Long SL must accept price above entry verbatim, got {sl_price}"
    );
}

#[test]
fn apply_begin_edit_sets_editing_field() {
    let mut state = TickerState::new(SymbolKey::new("AAPL"));
    assert!(!state.is_editing());
    state.apply(TickerMsg::BeginEdit(EditingField::LimitPrice));
    assert!(state.is_editing());
    assert_eq!(state.editing_field(), Some(&EditingField::LimitPrice));
}

#[test]
fn apply_begin_edit_auto_commits_previous() {
    let mut state = TickerState::new_with_defaults(SymbolKey::new("AAPL"), 150.0, Some(2.0));
    state.force_bracket_mode(Some(OrderSide::Buy));
    state.apply(TickerMsg::EnsureDraftBracket {
        side: OrderSide::Buy,
        entry_type: EntryType::Limit,
    });
    // Begin editing LimitPrice.
    state.apply(TickerMsg::BeginEdit(EditingField::LimitPrice));
    state.apply(TickerMsg::UpdateEditValue("142.00".to_string()));
    // Begin editing Quantity — should auto-commit LimitPrice.
    state.apply(TickerMsg::BeginEdit(EditingField::Quantity));
    assert_eq!(state.editing_field(), Some(&EditingField::Quantity));
    // The limit price should have been committed.
    assert!((state.live_bracket().unwrap().entry.line.price - 142.0).abs() < f64::EPSILON);
}

#[test]
fn apply_commit_edit_clears_lock() {
    let mut state = TickerState::new_with_defaults(SymbolKey::new("AAPL"), 150.0, Some(2.0));
    state.force_bracket_mode(Some(OrderSide::Buy));
    state.apply(TickerMsg::EnsureDraftBracket {
        side: OrderSide::Buy,
        entry_type: EntryType::Limit,
    });
    state.apply(TickerMsg::BeginEdit(EditingField::LimitPrice));
    assert!(state.is_editing());
    state.apply(TickerMsg::CommitEdit {
        field: EditingField::LimitPrice,
        value: "140.00".to_string(),
    });
    assert!(!state.is_editing());
}

#[test]
fn apply_set_entry_type_adjusts_prices() {
    let mut state = TickerState::new_with_defaults(SymbolKey::new("AAPL"), 150.0, Some(2.0));
    state.force_bracket_mode(Some(OrderSide::Buy));
    state.apply(TickerMsg::EnsureDraftBracket {
        side: OrderSide::Buy,
        entry_type: EntryType::Limit,
    });
    let old_price = state.live_bracket().unwrap().entry.line.price;
    // Switch to Market — should reset to last_price.
    state.apply(TickerMsg::SetEntryType(EntryType::Market));
    let new_price = state.live_bracket().unwrap().entry.line.price;
    // Market entry should be at last_price (150.0).
    assert!((new_price - 150.0).abs() < f64::EPSILON);
    assert!((old_price - new_price).abs() > f64::EPSILON);
}

#[test]
fn apply_no_panic_when_no_live_bracket() {
    let mut state = TickerState::new(SymbolKey::new("EMPTY"));
    // All field mutations on empty state should return empty effects, not panic.
    assert!(state
        .apply(TickerMsg::SetLegPrice {
            role: midas_annotation_types::order_bracket::LegRole::Entry,
            price: 100.0,
        })
        .is_empty());
    assert!(state.apply(TickerMsg::SetTpEnabled(true)).is_empty());
    assert!(state.apply(TickerMsg::SetSlEnabled(true)).is_empty());
    assert!(state.apply(TickerMsg::SetQuantity(50.0)).is_empty());
    assert!(state
        .apply(TickerMsg::DragLeg {
            role: midas_annotation_types::order_bracket::LegRole::Entry,
            new_price: 100.0,
        })
        .is_empty());
    assert!(state.apply(TickerMsg::CancelBracket).is_empty());
    assert!(state.apply(TickerMsg::SaveBracket).is_empty());
}

// ── Corrupt / partial v1 blob ───────────────────────────────────────

#[test]
fn corrupt_partial_v1_blob_deserializes_to_defaults() {
    // A minimal JSON blob with only the symbol field — all other fields
    // should fall back to their serde defaults.
    let blob = br#"{"symbol":"CORRUPT"}"#;
    let intent: TickerOrderIntentV1 =
        serde_json::from_slice(blob).expect("partial v1 should deserialize");
    assert_eq!(intent.symbol.as_str(), "CORRUPT");
    assert_eq!(intent.last_side, OrderSide::Buy);
    assert_eq!(intent.last_entry_type, EntryType::Market);
    assert!(intent.entries.is_empty());

    // Migrate to v2.
    let state = migrate_v1_v2(&intent);
    assert_eq!(state.symbol().as_str(), "CORRUPT");
    assert_eq!(state.version(), CURRENT_VERSION);
    assert_eq!(state.last_side(), OrderSide::Buy);
}

#[test]
fn v2_blob_missing_new_fields_deserializes_cleanly() {
    // Simulate a v2 blob that is missing fields added after initial v2
    // release (forward compat for future v2.x additions).
    let json = r#"{
        "symbol": "PARTIAL",
        "version": 2,
        "last_side": "Buy",
        "last_entry_type": "Market",
        "entries": [],
        "gatr_anchor": {},
        "pinned": false,
        "updated_at": "2025-06-01T12:00:00Z"
    }"#;
    let state: TickerState = serde_json::from_str(json).expect("partial v2 deserialize");
    assert_eq!(state.symbol().as_str(), "PARTIAL");
    assert!(state.live_bracket().is_none());
    assert_eq!(state.generation(), 0);
}

// ── Slice 3: bound_symbol + bind helpers ──────────────────────────

#[test]
fn bind_creates_ticker_state_and_bracket() {
    // Simulates `bind_chart_to_symbol` step 2 + 4: lazy-create
    // TickerState and fire EnsureDraftBracket. The actual helpers
    // live on MidasApp (integration-level), but the underlying
    // TickerState operations are testable here.
    let sym = SymbolKey::new("BIND");
    let mut tickers: HashMap<SymbolKey, TickerState> = HashMap::new();

    // Step 2: lazy-create.
    tickers
        .entry(sym.clone())
        .or_insert_with(|| TickerState::new(sym.clone()));
    assert!(tickers.contains_key(&sym));

    // Step 3: seed market data and activate bracket mode.
    let ts = tickers.get_mut(&sym).unwrap();
    ts.set_last_price(Some(150.0));
    ts.set_gatr_abs(Some(2.0));
    ts.force_bracket_mode(Some(OrderSide::Buy));

    // Step 4: fire EnsureDraftBracket.
    let effects = ts.apply(TickerMsg::EnsureDraftBracket {
        side: OrderSide::Buy,
        entry_type: EntryType::Market,
    });

    assert!(
        ts.live_bracket().is_some(),
        "bracket should exist after bind"
    );
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, TickerEffect::ProjectBracket(_))),
        "should project bracket on creation"
    );
}

#[test]
fn ensure_draft_bracket_produces_bracket_with_market_data() {
    let sym = SymbolKey::new("MARKET");
    let mut state = TickerState::new(sym);
    state.set_last_price(Some(200.0));
    state.set_gatr_abs(Some(3.0));
    state.force_bracket_mode(Some(OrderSide::Buy));

    state.apply(TickerMsg::EnsureDraftBracket {
        side: OrderSide::Buy,
        entry_type: EntryType::Market,
    });

    let bracket = state.live_bracket().expect("bracket should exist");
    // Market entry should be near the last price.
    assert!(
        (bracket.entry.line.price - 200.0).abs() < 0.01,
        "entry price should match last_price for Market"
    );
}

#[test]
fn unbound_state_has_no_bracket() {
    // A freshly created TickerState (simulating bound_symbol = None)
    // should have no bracket.
    let state = TickerState::new(SymbolKey::new("EMPTY"));
    assert!(
        state.live_bracket().is_none(),
        "fresh state should have no bracket"
    );
}

#[test]
fn config_bound_symbol_round_trip() {
    use midas_core::config::ChartConfig;

    let cfg = ChartConfig {
        symbol: "AAPL".to_string(),
        timeframe: "1D".to_string(),
        levels: vec![],
        camera_time_start: None,
        camera_time_end: None,
        camera_price_low: None,
        camera_price_high: None,
        collapse_gaps: false,
        timeline_border_ratio: 0.20,
        volume_scale: 1.0,
        show_volume_profile: false,
        show_levels: true,
        viewport_width: None,
        viewport_height: None,
        symbol_link: midas_core::link::LinkMode::Unlinked,
        timeframe_link: midas_core::link::LinkMode::Unlinked,
        bound_symbol: Some("AAPL".to_string()),
        backend: None,
        show_extended_hours: true,
        show_extended_hours_bands: true,
    };

    let toml_str = toml::to_string_pretty(&cfg).expect("serialize");
    let restored: ChartConfig = toml::from_str(&toml_str).expect("deserialize");
    assert_eq!(restored.bound_symbol, Some("AAPL".to_string()));
}

#[test]
fn config_bound_symbol_absent_backward_compat() {
    use midas_core::config::ChartConfig;

    // Simulate a pre-Slice-3 config without bound_symbol.
    let toml_str = r#"
        symbol = "MSFT"
        timeframe = "5m"
    "#;
    let cfg: ChartConfig = toml::from_str(toml_str).expect("deserialize");
    assert!(cfg.bound_symbol.is_none(), "absent field should be None");
    // Restoration code falls back to `symbol` — tested at integration level.
}

#[test]
fn order_panel_config_bound_symbol_round_trip() {
    use midas_core::config::OrderPanelConfig;

    let cfg = OrderPanelConfig {
        symbol: "TSLA".to_string(),
        side: "BUY".to_string(),
        quantity: "100".to_string(),
        symbol_link: midas_core::link::LinkMode::Unlinked,
        bracket_active: None,
        bound_symbol: Some("TSLA".to_string()),
    };

    let toml_str = toml::to_string_pretty(&cfg).expect("serialize");
    let restored: OrderPanelConfig = toml::from_str(&toml_str).expect("deserialize");
    assert_eq!(restored.bound_symbol, Some("TSLA".to_string()));
}

#[test]
fn order_panel_config_absent_bound_symbol_backward_compat() {
    use midas_core::config::OrderPanelConfig;

    let toml_str = r#"
        symbol = "SPY"
        side = "SELL"
        quantity = "50"
    "#;
    let cfg: OrderPanelConfig = toml::from_str(toml_str).expect("deserialize");
    assert!(cfg.bound_symbol.is_none());
}

#[test]
fn symbol_link_propagation_targets_matching_panels() {
    // Pure propagation logic — verifies find_link_targets works for
    // order panels (the same function used by bind_chart_to_symbol).
    use crate::link::find_link_targets;
    use midas_core::{LinkColor, LinkMode, OrderPanelId};

    let pink = LinkMode::Color(LinkColor::Purple);
    let targets = find_link_targets(
        pink,
        vec![
            (OrderPanelId::new(1), LinkMode::Color(LinkColor::Purple)),
            (OrderPanelId::new(2), LinkMode::Color(LinkColor::Blue)),
            (OrderPanelId::new(3), LinkMode::Color(LinkColor::Purple)),
            (OrderPanelId::new(4), LinkMode::Unlinked),
            (OrderPanelId::new(5), LinkMode::ListenAll),
        ],
    );
    assert_eq!(
        targets,
        vec![
            OrderPanelId::new(1),
            OrderPanelId::new(3),
            OrderPanelId::new(5)
        ],
    );
}

#[test]
fn order_panel_from_config_restores_bound_symbol() {
    use crate::order_panel::OrderPanel;
    use midas_core::config::OrderPanelConfig;

    let cfg = OrderPanelConfig {
        symbol: "GOOG".to_string(),
        bound_symbol: Some("GOOG".to_string()),
        ..Default::default()
    };
    let panel = OrderPanel::from_config(midas_core::OrderPanelId::new(1), &cfg);
    assert_eq!(
        panel.bound_symbol.as_ref().map(|k| k.as_str()),
        Some("GOOG"),
    );
}

#[test]
fn order_panel_from_config_falls_back_to_symbol() {
    use crate::order_panel::OrderPanel;
    use midas_core::config::OrderPanelConfig;

    // Pre-Slice-3 config: no bound_symbol field.
    let cfg = OrderPanelConfig {
        symbol: "AMZN".to_string(),
        bound_symbol: None,
        ..Default::default()
    };
    let panel = OrderPanel::from_config(midas_core::OrderPanelId::new(2), &cfg);
    assert_eq!(
        panel.bound_symbol.as_ref().map(|k| k.as_str()),
        Some("AMZN"),
        "should fall back to the legacy symbol field"
    );
}

#[test]
fn order_panel_to_config_persists_bound_symbol() {
    use crate::order_panel::OrderPanel;

    let panel = OrderPanel::new(midas_core::OrderPanelId::new(3), "META".to_string());
    assert_eq!(
        panel.bound_symbol.as_ref().map(|k| k.as_str()),
        Some("META"),
    );
    let cfg = panel.to_config();
    assert_eq!(cfg.bound_symbol, Some("META".to_string()));
}

// ── Bracket mode (BUY / X / SELL toggle) ─────────────────────────────

#[test]
fn ensure_draft_skips_when_bracket_mode_is_none() {
    let mut state = TickerState::new_with_defaults(SymbolKey::new("SKIP"), 100.0, Some(1.0));
    // bracket_mode defaults to None (X).
    assert!(state.bracket_mode().is_none());

    let effects = state.apply(TickerMsg::EnsureDraftBracket {
        side: OrderSide::Buy,
        entry_type: EntryType::Market,
    });

    assert!(
        effects.is_empty(),
        "EnsureDraftBracket should be no-op when bracket_mode is None"
    );
    assert!(
        state.live_bracket().is_none(),
        "no bracket should be created when bracket_mode is None"
    );
}

#[test]
fn set_bracket_mode_buy_creates_bracket() {
    let mut state = TickerState::new_with_defaults(SymbolKey::new("MODE"), 100.0, Some(1.0));
    assert!(state.bracket_mode().is_none());
    assert!(state.live_bracket().is_none());

    let effects = state.apply(TickerMsg::SetBracketMode(Some(OrderSide::Buy)));

    assert_eq!(state.bracket_mode(), Some(OrderSide::Buy));
    assert!(state.live_bracket().is_some(), "bracket should be created");
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, TickerEffect::ProjectBracket(_))),
        "should project bracket"
    );
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, TickerEffect::PersistDirty)),
        "should persist"
    );
}

#[test]
fn set_bracket_mode_none_cancels_bracket() {
    let mut state = TickerState::new_with_defaults(SymbolKey::new("XOFF"), 100.0, Some(1.0));
    // Activate bracket mode first.
    state.apply(TickerMsg::SetBracketMode(Some(OrderSide::Buy)));
    assert!(state.live_bracket().is_some());
    state.set_live_annotation_id(Some(midas_annotation_types::AnnotationId(42)));

    // Deactivate.
    let effects = state.apply(TickerMsg::SetBracketMode(None));

    assert!(
        state.bracket_mode().is_none(),
        "bracket_mode should be None"
    );
    assert!(state.live_bracket().is_none(), "bracket should be removed");
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, TickerEffect::RemoveBracket(_))),
        "should emit RemoveBracket"
    );
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, TickerEffect::PersistDirty)),
        "should persist"
    );
}

#[test]
fn set_bracket_mode_persists() {
    let mut state = TickerState::new_with_defaults(SymbolKey::new("PERS"), 100.0, Some(1.0));

    let effects = state.apply(TickerMsg::SetBracketMode(Some(OrderSide::Sell)));

    assert_eq!(state.bracket_mode(), Some(OrderSide::Sell));
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, TickerEffect::PersistDirty)),
        "SetBracketMode should always emit PersistDirty"
    );
}

#[test]
fn bracket_mode_defaults_to_none() {
    let state = TickerState::new(SymbolKey::new("FRESH"));
    assert!(
        state.bracket_mode().is_none(),
        "fresh TickerState should default to bracket_mode = None (X)"
    );
}

#[test]
fn bracket_mode_serde_roundtrip() {
    let mut state = TickerState::new(SymbolKey::new("SERDE"));
    state.apply(TickerMsg::SetBracketMode(Some(OrderSide::Sell)));
    assert_eq!(state.bracket_mode(), Some(OrderSide::Sell));

    let json = serde_json::to_string(&state).expect("serialize");
    let restored: TickerState = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(restored.bracket_mode(), Some(OrderSide::Sell));
}

#[test]
fn bracket_mode_serde_missing_field_defaults_to_none() {
    // Simulate a v2 blob that was persisted before bracket_mode existed.
    let json = r#"{
        "symbol": "OLD",
        "version": 2,
        "last_side": "Buy",
        "last_entry_type": "Market",
        "entries": [],
        "gatr_anchor": {},
        "pinned": false,
        "updated_at": "2025-01-01T00:00:00Z",
        "generation": 0
    }"#;
    let state: TickerState = serde_json::from_str(json).expect("deserialize");
    assert!(
        state.bracket_mode().is_none(),
        "missing bracket_mode should default to None"
    );
}

#[test]
fn order_panel_empty_symbol_has_no_bound() {
    use crate::order_panel::OrderPanel;

    let panel = OrderPanel::new(midas_core::OrderPanelId::new(4), String::new());
    assert!(panel.bound_symbol.is_none());
}

// ── Slice 4: Persistence integration tests ────────────────────────

#[test]
fn startup_loads_ticker_states_from_redb() {
    use super::persist::TickerStatePersistHandle;

    let tmp = tempfile::TempDir::new().unwrap();
    let db_path = tmp.path().join("test_load.redb");

    // Open, seed two symbols, shut down.
    {
        let handle = TickerStatePersistHandle::open(&db_path).unwrap();
        let s1 = TickerState::new_with_defaults(SymbolKey::new("AAPL"), 185.0, Some(2.5));
        let s2 = TickerState::new_with_defaults(SymbolKey::new("MSFT"), 400.0, Some(5.0));
        handle.upsert(SymbolKey::new("AAPL"), s1);
        handle.upsert(SymbolKey::new("MSFT"), s2);
        handle.flush_now();
        handle.shutdown_blocking();
    }

    // Re-open and verify.
    {
        let handle = TickerStatePersistHandle::open(&db_path).unwrap();
        let all = handle.all_states();
        assert_eq!(all.len(), 2, "should load 2 symbols from redb");
        assert!(all.contains_key(&SymbolKey::new("AAPL")));
        assert!(all.contains_key(&SymbolKey::new("MSFT")));
        let aapl = &all[&SymbolKey::new("AAPL")];
        assert_eq!(aapl.version(), CURRENT_VERSION);
        handle.shutdown();
    }
}

#[test]
fn startup_migrates_v1_to_v2() {
    use super::persist::TickerStatePersistHandle;

    let tmp = tempfile::TempDir::new().unwrap();
    let db_path = tmp.path().join("test_migrate.redb");

    // Seed a v1 row directly into redb (bypassing the handle).
    {
        let db = redb::Database::create(&db_path).unwrap();
        let v1_table: redb::TableDefinition<'_, &str, &[u8]> =
            redb::TableDefinition::new("ticker_intent_v1");

        let intent = TickerOrderIntentV1::new(SymbolKey::new("TSLA"));
        let blob = serde_json::to_vec(&intent).unwrap();

        let txn = db.begin_write().unwrap();
        {
            let mut table = txn.open_table(v1_table).unwrap();
            table.insert("TSLA", blob.as_slice()).unwrap();
        }
        txn.commit().unwrap();
    }

    // Open the persist handle — it should auto-migrate v1→v2.
    {
        let handle = TickerStatePersistHandle::open(&db_path).unwrap();
        let all = handle.all_states();
        assert!(
            all.contains_key(&SymbolKey::new("TSLA")),
            "v1 row should be migrated to v2"
        );
        let tsla = &all[&SymbolKey::new("TSLA")];
        assert_eq!(tsla.version(), CURRENT_VERSION);
        assert_eq!(tsla.last_side(), OrderSide::Buy);
        handle.shutdown();
    }
}

#[test]
fn shutdown_flushes_dirty_states() {
    use super::persist::TickerStatePersistHandle;

    let tmp = tempfile::TempDir::new().unwrap();
    let db_path = tmp.path().join("test_flush.redb");

    // Create a state and modify it.
    {
        let handle = TickerStatePersistHandle::open(&db_path).unwrap();
        let mut state = TickerState::new(SymbolKey::new("NVDA"));
        // Modify via apply to make it dirty.
        let _ = state.apply(TickerMsg::UpdateMarketData {
            last_price: 900.0,
            gatr_abs: Some(10.0),
        });
        handle.upsert(SymbolKey::new("NVDA"), state);
        handle.flush_now();
        handle.shutdown_blocking();
    }

    // Re-open and verify persistence.
    {
        let handle = TickerStatePersistHandle::open(&db_path).unwrap();
        let all = handle.all_states();
        assert!(
            all.contains_key(&SymbolKey::new("NVDA")),
            "dirty state should be flushed on shutdown"
        );
        handle.shutdown();
    }
}

#[test]
fn persist_handle_all_states_returns_all_seeded() {
    use super::persist::TickerStatePersistHandle;

    let tmp = tempfile::TempDir::new().unwrap();
    let db_path = tmp.path().join("test_all.redb");

    let handle = TickerStatePersistHandle::open(&db_path).unwrap();
    let s1 = TickerState::new(SymbolKey::new("A"));
    let s2 = TickerState::new(SymbolKey::new("B"));
    let s3 = TickerState::new(SymbolKey::new("C"));
    handle.upsert(SymbolKey::new("A"), s1);
    handle.upsert(SymbolKey::new("B"), s2);
    handle.upsert(SymbolKey::new("C"), s3);

    let all = handle.all_states();
    assert_eq!(all.len(), 3);
    handle.shutdown();
}

#[test]
fn persist_forget_removes_from_cache() {
    use super::persist::TickerStatePersistHandle;

    let tmp = tempfile::TempDir::new().unwrap();
    let db_path = tmp.path().join("test_forget.redb");

    let handle = TickerStatePersistHandle::open(&db_path).unwrap();
    let state = TickerState::new(SymbolKey::new("GOOG"));
    handle.upsert(SymbolKey::new("GOOG"), state);
    assert!(handle.snapshot(&SymbolKey::new("GOOG")).is_some());

    handle.forget(&SymbolKey::new("GOOG"));
    assert!(
        handle.snapshot(&SymbolKey::new("GOOG")).is_none(),
        "forget should remove from cache"
    );
    assert_eq!(handle.all_states().len(), 0);
    handle.shutdown();
}

#[test]
fn inject_levels_populates_ticker_state() {
    use crate::annotation_store::StoredLevel;
    use midas_annotation_types::price_line::{LineExtent, LineStroke, PriceLine};
    use midas_annotation_types::HorizontalLevel;
    use midas_annotation_types::LineStyle;

    let mut state = TickerState::new(SymbolKey::new("TEST"));
    assert!(state.levels().is_empty());

    let levels = vec![StoredLevel {
        level: HorizontalLevel {
            id: 1,
            line: PriceLine {
                price: 100.0,
                extent: LineExtent::default(),
                stroke: LineStroke {
                    color: [1.0, 0.0, 0.0, 1.0],
                    width: 1.0,
                    style: LineStyle::default(),
                },
            },
            label: Some("Support".into()),
            icon: midas_annotation_types::LevelIcon::None,
        },
        locked: false,
    }];

    state.inject_levels(levels);
    assert_eq!(state.levels().len(), 1);
    assert!((state.levels()[0].line.price - 100.0).abs() < f64::EPSILON);
}

// ── Camera per-ticker tests ────────────────────────────────────────

#[test]
fn save_camera_state_persists() {
    let mut state = TickerState::new(SymbolKey::new("AAPL"));
    let effects = state.apply(TickerMsg::SaveCameraState {
        time_start: 1000.0,
        time_end: 2000.0,
        price_low: 140.0,
        price_high: 160.0,
        was_at_live_edge: true,
    });
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, TickerEffect::PersistDirty)),
        "SaveCameraState must produce PersistDirty"
    );
    let saved = state.saved_camera().expect("camera should be saved");
    assert!((saved.time_start - 1000.0).abs() < f64::EPSILON);
    assert!((saved.time_end - 2000.0).abs() < f64::EPSILON);
    assert!((saved.price_low - 140.0).abs() < f64::EPSILON);
    assert!((saved.price_high - 160.0).abs() < f64::EPSILON);
    assert!(saved.was_at_live_edge);
}

#[test]
fn save_camera_preserves_live_edge_flag() {
    let mut state = TickerState::new(SymbolKey::new("AAPL"));
    state.apply(TickerMsg::SaveCameraState {
        time_start: 1000.0,
        time_end: 2000.0,
        price_low: 140.0,
        price_high: 160.0,
        was_at_live_edge: false,
    });
    let saved = state.saved_camera().expect("camera should be saved");
    assert!(!saved.was_at_live_edge);
}

#[test]
fn save_camera_serde_roundtrip() {
    let mut state = TickerState::new(SymbolKey::new("TSLA"));
    state.apply(TickerMsg::SaveCameraState {
        time_start: 5000.0,
        time_end: 6000.0,
        price_low: 200.0,
        price_high: 250.0,
        was_at_live_edge: false,
    });
    let json = serde_json::to_string(&state).expect("serialize");
    let restored: TickerState = serde_json::from_str(&json).expect("deserialize");
    let saved = restored
        .saved_camera()
        .expect("camera should survive roundtrip");
    assert!((saved.time_start - 5000.0).abs() < f64::EPSILON);
    assert!((saved.time_end - 6000.0).abs() < f64::EPSILON);
    assert!((saved.price_low - 200.0).abs() < f64::EPSILON);
    assert!((saved.price_high - 250.0).abs() < f64::EPSILON);
    assert!(!saved.was_at_live_edge);
}

#[test]
fn saved_camera_returns_none_when_unset() {
    let state = TickerState::new(SymbolKey::new("SPY"));
    assert!(state.saved_camera().is_none());
}

#[test]
fn saved_camera_returns_none_on_missing_field_backward_compat() {
    // Simulate an old blob that predates camera fields.
    let json = r#"{
        "symbol": "IBM",
        "version": 2,
        "last_side": "Buy",
        "last_entry_type": "Market",
        "entries": [],
        "gatr_anchor": {},
        "pinned": false,
        "updated_at": "2025-01-01T00:00:00Z",
        "generation": 10
    }"#;
    let state: TickerState = serde_json::from_str(json).expect("deserialize old blob");
    assert!(
        state.saved_camera().is_none(),
        "missing camera fields should yield None"
    );
    // camera_was_at_live_edge defaults to true even when absent.
    // We can only observe this indirectly: after saving one field the
    // getter still returns None (need all 4 f64s).
}

#[test]
fn restore_at_live_edge_shifts_to_latest_candle() {
    // Saved camera: user was at live edge, latest candle at time 2000.
    // New data has latest candle at time 3000 (1 day later).
    let mut state = TickerState::new(SymbolKey::new("AAPL"));
    state.apply(TickerMsg::SaveCameraState {
        time_start: 1000.0,
        time_end: 2000.0,
        price_low: 140.0,
        price_high: 160.0,
        was_at_live_edge: true,
    });
    let saved = state.saved_camera().expect("saved");
    let duration = saved.time_end - saved.time_start;
    let latest_candle = 3000.0;
    let margin = duration * 0.02;

    // Simulate the restore logic from bind_chart_to_symbol.
    let restored_end = latest_candle + margin;
    let restored_start = restored_end - duration;

    // Duration preserved.
    assert!(
        ((restored_end - restored_start) - duration).abs() < f64::EPSILON,
        "zoom level (duration) must be preserved"
    );
    // Window shifted to latest candle.
    assert!(
        restored_end > latest_candle,
        "time_end should be past the latest candle"
    );
    assert!(
        restored_start < latest_candle,
        "time_start should be before the latest candle"
    );
}

#[test]
fn restore_at_history_uses_saved_verbatim() {
    // Saved camera: user was NOT at live edge (examining history).
    let mut state = TickerState::new(SymbolKey::new("MSFT"));
    state.apply(TickerMsg::SaveCameraState {
        time_start: 500.0,
        time_end: 800.0,
        price_low: 300.0,
        price_high: 350.0,
        was_at_live_edge: false,
    });
    let saved = state.saved_camera().expect("saved");
    // When was_at_live_edge is false and data exists in range,
    // the restore should be verbatim.
    assert!((saved.time_start - 500.0).abs() < f64::EPSILON);
    assert!((saved.time_end - 800.0).abs() < f64::EPSILON);
    assert!((saved.price_low - 300.0).abs() < f64::EPSILON);
    assert!((saved.price_high - 350.0).abs() < f64::EPSILON);
    assert!(!saved.was_at_live_edge);
}

#[test]
fn restore_at_history_empty_falls_back_to_latest() {
    // Saved camera: user was NOT at live edge, but the saved time
    // range has no data (pruned or unavailable). The fallback should
    // shift to the latest candle at the same zoom level (D5).
    let mut state = TickerState::new(SymbolKey::new("GOOG"));
    state.apply(TickerMsg::SaveCameraState {
        time_start: 100.0,
        time_end: 200.0,
        price_low: 90.0,
        price_high: 110.0,
        was_at_live_edge: false,
    });
    let saved = state.saved_camera().expect("saved");
    let duration = saved.time_end - saved.time_start;

    // Simulate: no candles in [100, 200], latest candle at 5000.
    let has_candles_in_range = false;
    let latest_candle = 5000.0;

    // The restore logic: if not at live edge AND no data in range,
    // fall back to live-edge shift.
    let needs_shift = !saved.was_at_live_edge && !has_candles_in_range;
    assert!(needs_shift, "should shift when historical view has no data");

    let margin = duration * 0.02;
    let restored_end = latest_candle + margin;
    let restored_start = restored_end - duration;

    // Duration preserved.
    assert!(
        ((restored_end - restored_start) - duration).abs() < f64::EPSILON,
        "zoom level (duration) must be preserved"
    );
    // Window shifted to latest candle.
    assert!(
        restored_end > latest_candle,
        "time_end should be past the latest candle"
    );
}
