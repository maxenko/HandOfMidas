//! One-time bootstrap: seed the per-ticker intent store from any
//! pre-existing `OrderBracket` annotations.
//!
//! Called from `MidasApp::new()` after the annotation store has been
//! restored from disk. For every symbol that has at least one live
//! bracket annotation but **no** intent row in the ticker-intent store,
//! we create a fresh [`TickerOrderIntent`] populated from the first
//! bracket's leg prices. This guarantees that on the first run after
//! the feature lands, the panel's hydration path does not silently
//! disagree with the visible bracket on the chart.
//!
//! # GATR anchor lifecycle rule
//!
//! Bootstrap upserts use [`IntentSource::Bootstrap`], which tells the
//! reducer (Slice 4) **not** to seed the GATR anchor. The anchor is
//! recorded on the first user-initiated `Upsert` (Panel or Chart
//! source). Leaving `gatr_anchor.anchor_price = None` here is the
//! whole point — Bootstrap is a silent hydration, not an endorsement.

use std::collections::HashMap;

use midas_chart::widget::order_bracket::BracketSide;
use midas_chart::widget::AnnotationKind;

use crate::annotation_store::{AnnotationStore, SymbolKey};
use crate::order_panel::{OrderSide, PriceInputMode, StopLossType};

use super::handle::TickerIntentAccess;
use super::{
    EntryMemory, GatrAnchor, IntentSource, OrderIntentMsg, TickerOrderIntent, CURRENT_VERSION,
};

/// Iterate every symbol in `annotations`; for those without an entry
/// in `handle`, seed a fresh intent from the first `OrderBracket`.
///
/// Pure over the two trait-shaped inputs so tests can drive this with
/// an ordinary `AnnotationStore` + a real [`super::TickerOrderIntentHandle`]
/// (backed by a temporary redb file).
pub(crate) fn bootstrap_from_annotations(
    annotations: &AnnotationStore,
    handle: &impl TickerIntentAccess,
) {
    // Collect symbols first — we only need the `&str` values and the
    // store's iterator borrows it.
    let symbols: Vec<String> = annotations.symbols().map(|s| s.to_string()).collect();

    for symbol in symbols {
        let key = SymbolKey::new(&symbol);
        if handle.snapshot(&key).is_some() {
            continue;
        }

        let anns = annotations.get(&symbol);
        let Some((ann_id, bracket)) = anns.iter().find_map(|a| match &a.kind {
            AnnotationKind::OrderBracket(b) => Some((a.id, b.as_ref())),
            _ => None,
        }) else {
            continue;
        };

        let side = match bracket.side {
            BracketSide::Long => OrderSide::Buy,
            BracketSide::Short => OrderSide::Sell,
        };
        let entry_type = bracket.entry_type;

        let memory = EntryMemory {
            entry_price_or_offset: Some(bracket.entry.line.price),
            quantity: bracket.quantity,
            tp_enabled: bracket.take_profit.is_some(),
            tp_value: bracket
                .take_profit
                .as_ref()
                .map(|tp| format!("{:.2}", tp.line.price))
                .unwrap_or_default(),
            tp_mode: PriceInputMode::Absolute,
            sl_enabled: bracket.stop_loss.is_some(),
            sl_value: bracket
                .stop_loss
                .as_ref()
                .map(|sl| format!("{:.2}", sl.line.price))
                .unwrap_or_default(),
            sl_mode: PriceInputMode::Absolute,
            sl_type: StopLossType::Stop,
            sl_limit_value: String::new(),
        };

        let mut entries = HashMap::new();
        entries.insert((side, entry_type), memory);

        let intent = TickerOrderIntent {
            version: CURRENT_VERSION,
            symbol: key.clone(),
            last_side: side,
            last_entry_type: entry_type,
            entries,
            // Bootstrap deliberately leaves the anchor unset — the
            // first user touch will seed it (D4 lifecycle rule).
            gatr_anchor: GatrAnchor::default(),
            live_annotation_id: Some(ann_id),
            broker_order_id: None,
            pinned: false,
            updated_at: chrono::Utc::now(),
        };

        let outcome = handle.upsert(OrderIntentMsg::Upsert {
            symbol: key,
            intent: Box::new(intent),
            source: IntentSource::Bootstrap,
        });
        tracing::debug!("ticker-intent bootstrap for {symbol}: {outcome:?}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use midas_chart::widget::level::LineStyle;
    use midas_chart::widget::order_bracket::{
        BracketLeg, BracketStatus, EntryType, LegRole, OrderBracket,
    };
    use midas_chart::widget::{AnnotationKind, LineExtent, LineStroke, PriceLine};

    use crate::ticker_order_intent::TickerOrderIntentHandle;

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

    #[test]
    fn bootstrap_seeds_intent_for_symbol_with_bracket() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("bootstrap.redb");
        let handle = TickerOrderIntentHandle::open(path).expect("open handle");

        let mut store = AnnotationStore::new();
        let bracket = OrderBracket {
            entry: make_leg(100.0),
            take_profit: Some(make_leg(105.0)),
            stop_loss: Some(make_leg(98.0)),
            side: BracketSide::Long,
            status: BracketStatus::Draft,
            quantity: Some(10.0),
            saved: true,
            filled_qty: None,
            entry_type: EntryType::Stop,
            entry_stop_price: None,
            wrong_side_warning: false,
        };
        store.add("AAPL", AnnotationKind::OrderBracket(Box::new(bracket)));

        // Pre-condition: no intent for AAPL.
        assert!(handle.snapshot(&SymbolKey::new("AAPL")).is_none());

        bootstrap_from_annotations(&store, &handle);

        // Post-condition: intent exists, seeded in (Buy, Stop) bucket.
        let intent = handle
            .snapshot(&SymbolKey::new("AAPL"))
            .expect("intent seeded");
        assert_eq!(intent.symbol, SymbolKey::new("AAPL"));
        assert_eq!(intent.last_side, OrderSide::Buy);
        assert_eq!(intent.last_entry_type, EntryType::Stop);
        let memory = intent
            .entries
            .get(&(OrderSide::Buy, EntryType::Stop))
            .expect("bucket populated");
        assert_eq!(memory.entry_price_or_offset, Some(100.0));
        assert_eq!(memory.quantity, Some(10.0));
        assert!(memory.tp_enabled);
        assert_eq!(memory.tp_value, "105.00");
        assert!(memory.sl_enabled);
        assert_eq!(memory.sl_value, "98.00");

        // GATR anchor lifecycle: Bootstrap must NOT seed the anchor.
        assert_eq!(intent.gatr_anchor.anchor_price, None);
        assert_eq!(intent.gatr_anchor.anchor_gatr, None);
    }

    #[test]
    fn bootstrap_skips_symbol_with_existing_intent() {
        use super::super::store::UpsertOutcome;

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("bootstrap-skip.redb");
        let handle = TickerOrderIntentHandle::open(path).expect("open handle");

        // Pre-seed an intent for MSFT.
        let key = SymbolKey::new("MSFT");
        let mut entries = HashMap::new();
        entries.insert(
            (OrderSide::Sell, EntryType::Limit),
            EntryMemory {
                entry_price_or_offset: Some(999.99),
                ..EntryMemory::default()
            },
        );
        let pre = TickerOrderIntent {
            version: CURRENT_VERSION,
            symbol: key.clone(),
            last_side: OrderSide::Sell,
            last_entry_type: EntryType::Limit,
            entries,
            gatr_anchor: GatrAnchor::default(),
            live_annotation_id: None,
            broker_order_id: None,
            pinned: false,
            updated_at: chrono::Utc::now(),
        };
        let outcome = handle.upsert(OrderIntentMsg::Upsert {
            symbol: key.clone(),
            intent: Box::new(pre),
            source: IntentSource::Panel,
        });
        assert!(matches!(outcome, UpsertOutcome::Applied { .. }));

        // Now add an annotation that would normally bootstrap — but the
        // intent already exists, so bootstrap must be a no-op for it.
        let mut store = AnnotationStore::new();
        let bracket = OrderBracket {
            entry: make_leg(500.0),
            take_profit: None,
            stop_loss: None,
            side: BracketSide::Short,
            status: BracketStatus::Draft,
            quantity: None,
            saved: true,
            filled_qty: None,
            entry_type: EntryType::Market,
            entry_stop_price: None,
            wrong_side_warning: false,
        };
        store.add("MSFT", AnnotationKind::OrderBracket(Box::new(bracket)));

        bootstrap_from_annotations(&store, &handle);

        // The pre-seeded intent is unchanged — bootstrap did not stomp it.
        let intent = handle.snapshot(&key).expect("intent still there");
        assert_eq!(intent.last_entry_type, EntryType::Limit);
        let memory = intent
            .entries
            .get(&(OrderSide::Sell, EntryType::Limit))
            .expect("original bucket intact");
        assert_eq!(memory.entry_price_or_offset, Some(999.99));
    }
}
