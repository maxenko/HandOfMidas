//! Unit tests for the `ticker_state` module.
//!
//! Covers serde round-trip, v1->v2 migration, factory defaults, and
//! `apply()` stub behavior.

use std::collections::HashMap;

use midas_chart::widget::order_bracket::EntryType;

use crate::annotation_store::SymbolKey;
use crate::order_panel::OrderSide;
use crate::ticker_order_intent::{EntryMemory, GatrAnchor, TickerOrderIntent};

use super::apply::TickerMsg;
use super::{migrate_v1_v2, EditingField, TickerState, CURRENT_VERSION};

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

    let intent = TickerOrderIntent {
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
    let intent = TickerOrderIntent {
        version: 1,
        symbol: SymbolKey::new("TSLA"),
        last_side: OrderSide::Buy,
        last_entry_type: EntryType::Market,
        entries: HashMap::new(),
        gatr_anchor: GatrAnchor::default(),
        live_annotation_id: Some(midas_chart::widget::AnnotationId(42)),
        broker_order_id: None,
        pinned: false,
        updated_at: chrono::Utc::now(),
    };

    let state = migrate_v1_v2(&intent);
    assert_eq!(
        state.live_annotation_id(),
        Some(midas_chart::widget::AnnotationId(42))
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
            let mem = state
                .entries()
                .get(&(side, entry_type))
                .unwrap_or_else(|| {
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

    let intent = TickerOrderIntent {
        version: 1,
        symbol: SymbolKey::new("NVDA"),
        last_side: OrderSide::Buy,
        last_entry_type: EntryType::Market,
        entries,
        gatr_anchor: GatrAnchor {
            anchor_price: Some(800.0),
            anchor_gatr: Some(15.0),
        },
        live_annotation_id: Some(midas_chart::widget::AnnotationId(99)),
        broker_order_id: None,
        pinned: true,
        updated_at: chrono::Utc::now(),
    };

    let state = TickerState::from_legacy(intent, Vec::new(), None, None);
    assert_eq!(state.symbol().as_str(), "NVDA");
    assert!(state.pinned());
    assert_eq!(
        state.live_annotation_id(),
        Some(midas_chart::widget::AnnotationId(99))
    );
}

// ── apply() stubs ───────────────────────────────────────────────────

#[test]
fn apply_stubs_return_empty_effects() {
    let mut state = TickerState::new(SymbolKey::new("TEST"));

    // Exercise every TickerMsg variant and verify empty effects.
    let messages: Vec<TickerMsg> = vec![
        TickerMsg::EnsureDraftBracket {
            side: OrderSide::Buy,
            entry_type: EntryType::Market,
        },
        TickerMsg::CancelBracket,
        TickerMsg::SaveBracket,
        TickerMsg::DeleteBracket,
        TickerMsg::RecallBracket,
        TickerMsg::SetLegPrice {
            role: midas_chart::widget::order_bracket::LegRole::Entry,
            price: 100.0,
        },
        TickerMsg::SetTpEnabled(true),
        TickerMsg::SetSlEnabled(false),
        TickerMsg::SetQuantity(50.0),
        TickerMsg::SetSide(OrderSide::Sell),
        TickerMsg::SetEntryType(EntryType::Limit),
        TickerMsg::DragLeg {
            role: midas_chart::widget::order_bracket::LegRole::TakeProfit,
            new_price: 200.0,
        },
        TickerMsg::BeginEdit(EditingField::LimitPrice),
        TickerMsg::UpdateEditValue("123.45".to_string()),
        TickerMsg::CommitEdit {
            field: EditingField::LimitPrice,
            value: "123.45".to_string(),
        },
        TickerMsg::CancelEdit,
        TickerMsg::MaybeSnap {
            current_price: 150.0,
            gatr_abs: Some(2.0),
        },
        TickerMsg::TogglePin,
        TickerMsg::UndoSnap,
        TickerMsg::UpdateMarketData {
            last_price: 151.0,
            gatr_abs: Some(2.1),
        },
        TickerMsg::AddLevel(crate::level_store::StoredLevel {
            level: midas_chart::HorizontalLevel {
                id: 1,
                line: midas_chart::widget::price_line::PriceLine {
                    price: 100.0,
                    extent: midas_chart::widget::price_line::LineExtent::FullWidth,
                    stroke: midas_chart::widget::price_line::LineStroke {
                        color: [1.0, 1.0, 1.0, 1.0],
                        width: 1.0,
                        style: midas_chart::widget::LineStyle::Solid,
                    },
                },
                label: None,
                icon: midas_chart::LevelIcon::default(),
            },
            locked: false,
        }),
        TickerMsg::RemoveLevel(0),
        TickerMsg::UpdateLevel {
            index: 0,
            level: crate::level_store::StoredLevel {
                level: midas_chart::HorizontalLevel {
                    id: 1,
                    line: midas_chart::widget::price_line::PriceLine {
                        price: 105.0,
                        extent: midas_chart::widget::price_line::LineExtent::FullWidth,
                        stroke: midas_chart::widget::price_line::LineStroke {
                        color: [1.0, 1.0, 1.0, 1.0],
                        width: 1.0,
                        style: midas_chart::widget::LineStyle::Solid,
                    },
                    },
                    label: None,
                    icon: midas_chart::LevelIcon::default(),
                },
                locked: false,
            },
        },
        TickerMsg::ToggleLevelLock(0),
        TickerMsg::SubmitOrder,
        TickerMsg::OrderPending {
            order_id: uuid::Uuid::nil(),
        },
        TickerMsg::OrderFilled {
            filled_qty: 100.0,
            avg_price: 150.0,
        },
        TickerMsg::OrderPartialFill { filled_qty: 50.0 },
        TickerMsg::OrderRejected {
            reason: "test".to_string(),
        },
        TickerMsg::OrderCancelled,
        TickerMsg::Hydrated(Box::new(TickerState::new(SymbolKey::new("TEST")))),
    ];

    for msg in messages {
        let effects = state.apply(msg);
        assert!(effects.is_empty(), "stub should return empty effects");
    }
}

// ── Corrupt / partial v1 blob ───────────────────────────────────────

#[test]
fn corrupt_partial_v1_blob_deserializes_to_defaults() {
    // A minimal JSON blob with only the symbol field — all other fields
    // should fall back to their serde defaults.
    let blob = br#"{"symbol":"CORRUPT"}"#;
    let intent: TickerOrderIntent =
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
