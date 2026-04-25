//! Sensible default prices for a freshly-initialized bracket.
//!
//! This module is the single source of truth for "what should a
//! never-touched `(side, entry_type)` compound key look like on first
//! load?" and "what does the GATR snap fall back to when its math would
//! land a leg exactly on the current price?".

use midas_annotation_types::order_bracket::EntryType;

use crate::order_panel::OrderSide;

/// Minimum separation (as a fraction of `step`) between any two prices
/// in a draft bracket. Used by the snap fall-back to detect "too close
/// to market" and "Stop collapsed onto Limit" conditions.
#[allow(dead_code)] // used by snap fallback in future slices
pub const MIN_OFFSET_FRACTION: f64 = 0.1;

/// Four default prices resolved for a `(side, entry_type)` compound.
///
/// All fields are absolute prices, not offsets. TP and SL are present
/// regardless of whether the user has enabled them — callers decide
/// whether to use them based on the panel's `tp_enabled` / `sl_enabled`
/// toggles.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct InitialPrices {
    /// Resolved entry price. For StopLimit this is the *limit*
    /// execution price; the stop trigger is in `stop_trigger`.
    pub entry: f64,
    /// Stop trigger price. `Some` only for `EntryType::StopLimit`.
    pub stop_trigger: Option<f64>,
    /// Default take-profit price, positioned for a ~2:1 R:R on the
    /// initial draft.
    pub take_profit: f64,
    /// Default stop-loss price, positioned for a ~1×step risk budget.
    pub stop_loss: f64,
}

/// Compute sensible default prices for a bracket at a given
/// `(side, entry_type)` compound.
///
/// `current_price` is the resolved market price (typically
/// `MarketSnapshot::last_price`). `gatr_abs` is the symbol's current
/// G-ATR — when `None`, the fall-back is 0.5% of `current_price`
/// (floored at `0.01`).
pub fn default_initial_prices(
    side: OrderSide,
    entry_type: EntryType,
    current_price: f64,
    gatr_abs: Option<f64>,
) -> InitialPrices {
    let step = resolve_step(current_price, gatr_abs);

    // Per-type entry / stop_trigger.
    let (entry, stop_trigger) = match (side, entry_type) {
        (OrderSide::Buy, EntryType::Market) => (current_price, None),
        (OrderSide::Sell, EntryType::Market) => (current_price, None),
        (OrderSide::Buy, EntryType::Limit) => (current_price - step, None),
        (OrderSide::Sell, EntryType::Limit) => (current_price + step, None),
        (OrderSide::Buy, EntryType::Stop) => (current_price + step, None),
        (OrderSide::Sell, EntryType::Stop) => (current_price - step, None),
        (OrderSide::Buy, EntryType::StopLimit) => {
            (current_price + 1.5 * step, Some(current_price + step))
        }
        (OrderSide::Sell, EntryType::StopLimit) => {
            (current_price - 1.5 * step, Some(current_price - step))
        }
    };

    // TP / SL derived from the chosen entry so the 1:2 R:R shape holds.
    let (take_profit, stop_loss) = match side {
        OrderSide::Buy => (entry + 2.0 * step, entry - step),
        OrderSide::Sell => (entry - 2.0 * step, entry + step),
    };

    InitialPrices {
        entry,
        stop_trigger,
        take_profit,
        stop_loss,
    }
}

/// Resolve the per-bracket `step` unit used by the offset rules.
///
/// Prefers `gatr_abs` when it is a finite positive number; otherwise
/// falls back to 0.5% of `current_price`. Always clamped to `>= 0.01`
/// so a tiny fallback does not collapse the offsets.
pub fn resolve_step(current_price: f64, gatr_abs: Option<f64>) -> f64 {
    let step = match gatr_abs {
        Some(g) if g.is_finite() && g > 0.0 => g,
        _ => current_price.abs() * 0.005,
    };
    step.max(0.01)
}

/// Check whether `price` is within `MIN_OFFSET_FRACTION × step` of
/// `anchor`. Used by the snap fall-back to decide when to replace a
/// shifted field with a fresh default.
#[allow(dead_code)] // used by snap fallback in future slices
pub fn too_close(price: f64, anchor: f64, step: f64) -> bool {
    (price - anchor).abs() < MIN_OFFSET_FRACTION * step
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buy_market_defaults_produce_sensible_shape() {
        let p = default_initial_prices(OrderSide::Buy, EntryType::Market, 100.0, Some(5.0));
        assert_eq!(p.entry, 100.0);
        assert!(p.stop_trigger.is_none());
        assert!(p.take_profit > p.entry);
        assert!(p.stop_loss < p.entry);
    }

    #[test]
    fn sell_limit_defaults_produce_sensible_shape() {
        let p = default_initial_prices(OrderSide::Sell, EntryType::Limit, 100.0, Some(5.0));
        assert!(p.entry > 100.0);
        assert!(p.take_profit < p.entry);
        assert!(p.stop_loss > p.entry);
    }

    #[test]
    fn resolve_step_fallback_when_no_gatr() {
        let step = resolve_step(200.0, None);
        assert!((step - 1.0).abs() < 1e-9); // 0.5% of 200 = 1.0
    }

    #[test]
    fn resolve_step_floor_at_001() {
        let step = resolve_step(0.001, None);
        assert_eq!(step, 0.01);
    }

    #[test]
    fn too_close_detects_overlap() {
        let step = 5.0;
        assert!(too_close(100.0, 100.4, step)); // 0.4 < 0.1 * 5 = 0.5
        assert!(!too_close(100.0, 99.0, step)); // 1.0 >= 0.5
    }
}
