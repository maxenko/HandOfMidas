//! Table-driven tests for [`super::maybe_snap`].
//!
//! Every row corresponds to a line in the Slice 4 testing table in
//! `plan/ticker-order-state/README.md`. A passing test matrix is the
//! contract: if a new guard is added, it is added alongside a new row
//! here.

use std::collections::HashMap;

use chrono::{Duration, Utc};
use midas_chart::widget::order_bracket::EntryType;
use midas_chart::widget::AnnotationId;

use crate::annotation_store::SymbolKey;
use crate::order_panel::OrderSide;
use crate::ticker_order_intent::{GatrAnchor, TickerOrderIntent, CURRENT_VERSION};

use super::{maybe_snap, SnapReason};

/// Build a [`TickerOrderIntent`] fixture for the snap table. Every
/// field the rule reads is explicit; everything else defaults.
#[allow(clippy::too_many_arguments)]
fn fixture(
    anchor_price: Option<f64>,
    anchor_gatr: Option<f64>,
    pinned: bool,
    updated_minutes_ago: i64,
    live_id: Option<u64>,
) -> TickerOrderIntent {
    TickerOrderIntent {
        version: CURRENT_VERSION,
        symbol: SymbolKey::new("AAPL"),
        last_side: OrderSide::Buy,
        last_entry_type: EntryType::Limit,
        entries: HashMap::new(),
        gatr_anchor: GatrAnchor {
            anchor_price,
            anchor_gatr,
        },
        live_annotation_id: live_id.map(AnnotationId),
        broker_order_id: None,
        pinned,
        updated_at: Utc::now() - Duration::try_minutes(updated_minutes_ago).unwrap(),
    }
}

#[test]
fn drift_under_threshold_no_snap() {
    // anchor 100, current 100.5, gatr 1.0 → delta 0.5 < gatr, None.
    let intent = fixture(Some(100.0), Some(1.0), false, 120, Some(7));
    assert!(maybe_snap(&intent, 100.5, Some(1.0)).is_none());
}

#[test]
fn drift_equal_threshold_no_snap() {
    // `should_reposition` uses strict `>`, so exactly one GATR is not
    // enough to trip the rule. This is the documented behavior.
    let intent = fixture(Some(100.0), Some(1.0), false, 120, Some(7));
    assert!(maybe_snap(&intent, 101.0, Some(1.0)).is_none());
}

#[test]
fn drift_over_threshold_emits_plan() {
    // anchor 100, current 102, gatr 1.0 → delta +2.0, snap fires.
    let intent = fixture(Some(100.0), Some(1.0), false, 120, Some(7));
    let plan = maybe_snap(&intent, 102.0, Some(1.0)).expect("snap should fire");
    assert!((plan.delta - 2.0).abs() < 1e-9);
    assert_eq!(plan.new_anchor.anchor_price, Some(102.0));
    assert_eq!(plan.new_anchor.anchor_gatr, Some(1.0));
    assert_eq!(plan.reason, SnapReason::DriftExceeded);
}

#[test]
fn pinned_guard_blocks_snap() {
    let intent = fixture(Some(100.0), Some(1.0), true, 120, Some(7));
    assert!(maybe_snap(&intent, 110.0, Some(1.0)).is_none());
}

#[test]
fn recent_edit_blocks_snap() {
    // 10 minutes ago is inside the 1 hour recency guard.
    let intent = fixture(Some(100.0), Some(1.0), false, 10, Some(7));
    assert!(maybe_snap(&intent, 110.0, Some(1.0)).is_none());
}

#[test]
fn nan_price_blocks_snap() {
    let intent = fixture(Some(100.0), Some(1.0), false, 120, Some(7));
    assert!(maybe_snap(&intent, f64::NAN, Some(1.0)).is_none());
}

#[test]
fn nan_gatr_blocks_snap() {
    let intent = fixture(Some(100.0), Some(1.0), false, 120, Some(7));
    assert!(maybe_snap(&intent, 110.0, Some(f64::NAN)).is_none());
}

#[test]
fn no_anchor_blocks_snap() {
    // First save has not happened yet — there is nothing to drift from.
    let intent = fixture(None, Some(1.0), false, 120, Some(7));
    assert!(maybe_snap(&intent, 110.0, Some(1.0)).is_none());
}

#[test]
fn no_gatr_daily_blocks_snap() {
    let intent = fixture(Some(100.0), None, false, 120, Some(7));
    assert!(maybe_snap(&intent, 110.0, None).is_none());
}

#[test]
fn tiny_gatr_blocks_snap() {
    // 1e-10 is below MIN_GATR_ABS → guarded out.
    let intent = fixture(Some(100.0), Some(1e-10), false, 120, Some(7));
    assert!(maybe_snap(&intent, 110.0, Some(1e-10)).is_none());
}

#[test]
fn no_live_bracket_still_emits_plan() {
    // Single-source-of-truth refactor: the absence of a chart bracket
    // does not block the snap. A panel-only intent (user typed a stale
    // limit price with no chart bracket drawn) is still eligible —
    // the reducer will shift the EntryMemory and skip the annotation
    // side-effect.
    let intent = fixture(Some(100.0), Some(1.0), false, 120, None);
    let plan = maybe_snap(&intent, 110.0, Some(1.0))
        .expect("panel-only intent should still snap");
    assert!((plan.delta - 10.0).abs() < 1e-9);
    assert_eq!(plan.new_anchor.anchor_price, Some(110.0));
}

#[test]
fn panel_only_intent_with_stale_price_fires() {
    // PLTR-style fixture: no chart bracket, stale $100 anchor, current
    // price $14.45 with GATR 0.40 → ratio 213× GATR → snap fires.
    let intent = fixture(Some(100.0), Some(0.40), false, 120, None);
    let plan = maybe_snap(&intent, 14.45, Some(0.40))
        .expect("panel-only stale intent should snap");
    assert!((plan.delta + 85.55).abs() < 1e-9);
    assert_eq!(plan.new_anchor.anchor_price, Some(14.45));
    assert_eq!(plan.reason, SnapReason::DriftExceeded);
}

#[test]
fn negative_delta_is_reported_as_signed() {
    // Anchor above current: the snap plan should carry a negative delta.
    let intent = fixture(Some(100.0), Some(1.0), false, 120, Some(7));
    let plan = maybe_snap(&intent, 97.0, Some(1.0)).expect("snap should fire");
    assert!((plan.delta + 3.0).abs() < 1e-9);
}
