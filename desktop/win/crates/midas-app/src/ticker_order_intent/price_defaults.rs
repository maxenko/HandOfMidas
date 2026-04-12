//! Sensible default prices for a freshly-initialized bracket.
//!
//! This module is the single source of truth for "what should a
//! never-touched `(side, entry_type)` compound key look like on first
//! load?" and "what does the GATR snap fall back to when its math would
//! land a leg exactly on the current price?".
//!
//! # Why this exists
//!
//! Two related failure modes motivated this module:
//!
//! 1. A never-touched `EntryMemory` bucket has `entry_price_or_offset =
//!    None`, so hydration leaves the panel's Limit / Stop inputs empty
//!    (or literal `"0"`). The user then sees a bracket whose legs are
//!    all stacked at the current price or, worse, at `$0`.
//! 2. After a GATR snap, the delta shift can place a leg *exactly* on
//!    the current price (or can collapse the Stop and Limit lines of a
//!    StopLimit onto each other). Two overlapping price lines look like
//!    one line on screen and the user cannot click either cleanly.
//!
//! [`default_initial_prices`] produces a small, rule-based offset from
//! the current price that is always at least `0.1 × step` away from
//! the market, and guarantees Stop ≠ Limit for StopLimit entries. The
//! returned prices are consumed by:
//!
//! - The reducer's auto-draft path ([`super::reducer::apply_ensure_draft_bracket`])
//!   when it builds a fresh `Draft` bracket on chart load.
//! - The reducer's snap path
//!   ([`super::reducer::shift_entry_memory_prices`]) as a fall-back when
//!   a delta shift would leave a field too close to market.
//! - Panel hydration, so a never-touched bucket lands on real numbers
//!   rather than the empty string.
//!
//! # Offset rules
//!
//! Let `step = max(gatr_abs.unwrap_or(current * 0.005), 0.01)`. Then:
//!
//! | side + type      | entry              | stop_trigger | take_profit (if on) | stop_loss (if on) |
//! |------------------|--------------------|--------------|---------------------|-------------------|
//! | Buy Market       | current            | —            | entry + 2×step      | entry − 1×step    |
//! | Buy Limit        | current − 1×step   | —            | entry + 2×step      | entry − 1×step    |
//! | Buy Stop         | current + 1×step   | —            | entry + 2×step      | entry − 1×step    |
//! | Buy StopLimit    | current + 1.5×step | current+1×step | entry + 2×step    | entry − 1×step    |
//! | Sell Market      | current            | —            | entry − 2×step      | entry + 1×step    |
//! | Sell Limit       | current + 1×step   | —            | entry − 2×step      | entry + 1×step    |
//! | Sell Stop        | current − 1×step   | —            | entry − 2×step      | entry + 1×step    |
//! | Sell StopLimit   | current − 1.5×step | current−1×step | entry − 2×step    | entry + 1×step    |
//!
//! The 1:2 R:R shape of the initial TP / SL is a deliberate starting
//! point — the user is expected to drag the legs from there.

use midas_chart::widget::order_bracket::EntryType;

use crate::order_panel::OrderSide;

/// Minimum separation (as a fraction of `step`) between any two prices
/// in a draft bracket. Used by the snap fall-back to detect "too close
/// to market" and "Stop collapsed onto Limit" conditions.
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
///
/// The returned prices satisfy:
///
/// - `entry != current_price` for all non-Market entry types.
/// - `stop_trigger != entry` when `entry_type == StopLimit`.
/// - `take_profit` and `stop_loss` sit on the correct side of `entry`
///   for the direction (Buy: TP > entry > SL; Sell: SL > entry > TP).
/// - Every two adjacent levels are at least `MIN_OFFSET_FRACTION × step`
///   apart so they do not render as a single visual line.
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
        (OrderSide::Buy, EntryType::StopLimit) => (
            current_price + 1.5 * step,
            Some(current_price + step),
        ),
        (OrderSide::Sell, EntryType::StopLimit) => (
            current_price - 1.5 * step,
            Some(current_price - step),
        ),
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
pub fn too_close(price: f64, anchor: f64, step: f64) -> bool {
    (price - anchor).abs() < MIN_OFFSET_FRACTION * step
}

#[cfg(test)]
mod tests;
