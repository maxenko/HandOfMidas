//! Unit tests for [`super::default_initial_prices`] and the two
//! helpers that back the Fix 2 "sane initial offsets" rules.

use midas_chart::widget::order_bracket::EntryType;

use super::*;
use crate::order_panel::OrderSide;

const CURRENT: f64 = 100.0;
const GATR: f64 = 2.0;

#[test]
fn default_initial_prices_buy_limit_offsets_below_market() {
    let p = default_initial_prices(OrderSide::Buy, EntryType::Limit, CURRENT, Some(GATR));
    assert!(
        p.entry < CURRENT,
        "Buy Limit entry should sit below market, got {}",
        p.entry
    );
    assert!(
        (p.entry - (CURRENT - GATR)).abs() < 1e-9,
        "entry offset should be one step below market"
    );
    assert!(p.stop_trigger.is_none());
    assert!(p.take_profit > p.entry, "TP must be above entry for Buy");
    assert!(p.stop_loss < p.entry, "SL must be below entry for Buy");
}

#[test]
fn default_initial_prices_buy_stop_limit_has_distinct_stop_and_limit() {
    let p = default_initial_prices(OrderSide::Buy, EntryType::StopLimit, CURRENT, Some(GATR));
    let stop = p.stop_trigger.expect("StopLimit must emit a trigger");
    // Buy StopLimit: stop is the trigger (above market), limit is
    // slightly above the trigger so the order can fill.
    assert!(stop > CURRENT, "Buy StopLimit trigger above market");
    assert!(
        p.entry > stop,
        "Buy StopLimit limit price should sit above the trigger, got entry={} trigger={}",
        p.entry,
        stop
    );
    assert!(
        (p.entry - stop).abs() > MIN_OFFSET_FRACTION * GATR,
        "Stop and Limit must be visually distinct"
    );
}

#[test]
fn default_initial_prices_sell_stop_limit_below_market_with_correct_ordering() {
    let p = default_initial_prices(OrderSide::Sell, EntryType::StopLimit, CURRENT, Some(GATR));
    let stop = p.stop_trigger.expect("StopLimit must emit a trigger");
    assert!(stop < CURRENT, "Sell StopLimit trigger below market");
    assert!(
        p.entry < stop,
        "Sell StopLimit limit price should sit below the trigger"
    );
    assert!(p.take_profit < p.entry, "Sell TP must be below entry");
    assert!(p.stop_loss > p.entry, "Sell SL must be above entry");
}

#[test]
fn default_initial_prices_step_falls_back_to_5pct_when_gatr_missing() {
    // Expect `step = max(current * 0.005, 0.01) = 0.5`.
    let p = default_initial_prices(OrderSide::Buy, EntryType::Limit, CURRENT, None);
    assert!(
        (p.entry - (CURRENT - 0.5)).abs() < 1e-9,
        "fallback step should be 0.5% of current, got entry={}",
        p.entry
    );
}

#[test]
fn default_initial_prices_buy_take_profit_above_entry_for_long() {
    let p = default_initial_prices(OrderSide::Buy, EntryType::Market, CURRENT, Some(GATR));
    assert!(p.take_profit > p.entry);
    // 1:2 R:R by construction: TP offset == 2× step, SL offset == 1×step.
    assert!(
        (p.take_profit - p.entry - 2.0 * GATR).abs() < 1e-9,
        "TP should be 2×step above entry"
    );
    assert!(
        (p.entry - p.stop_loss - GATR).abs() < 1e-9,
        "SL should be 1×step below entry"
    );
}

#[test]
fn default_initial_prices_sell_take_profit_below_entry_for_short() {
    let p = default_initial_prices(OrderSide::Sell, EntryType::Market, CURRENT, Some(GATR));
    assert!(p.take_profit < p.entry);
    assert!(p.stop_loss > p.entry);
}

#[test]
fn default_initial_prices_step_is_floored_at_one_cent() {
    // With current=0.001 and no GATR the naive 0.5% step would be 5e-6 —
    // round it up to 0.01 so the offsets are still grabbable.
    let p = default_initial_prices(OrderSide::Buy, EntryType::Limit, 0.001, None);
    assert!(
        (p.entry - 0.001 + 0.01).abs() < 1e-12,
        "step should floor at 0.01, got entry={}",
        p.entry
    );
}

#[test]
fn resolve_step_prefers_gatr() {
    assert!((resolve_step(100.0, Some(2.5)) - 2.5).abs() < 1e-9);
}

#[test]
fn resolve_step_falls_back_when_gatr_is_zero_or_nan() {
    assert!((resolve_step(100.0, Some(0.0)) - 0.5).abs() < 1e-9);
    assert!((resolve_step(100.0, Some(f64::NAN)) - 0.5).abs() < 1e-9);
    assert!((resolve_step(100.0, None) - 0.5).abs() < 1e-9);
}

#[test]
fn too_close_detects_within_threshold() {
    // step=2.0 → threshold = 0.2
    assert!(too_close(100.1, 100.0, 2.0));
    assert!(too_close(99.95, 100.0, 2.0));
    assert!(!too_close(100.25, 100.0, 2.0));
}
