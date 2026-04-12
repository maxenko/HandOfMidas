//! Pure-function GATR snap rule.
//!
//! This module implements the "≥100% GATR drift" re-anchor rule
//! described in `plan/ticker-order-state/README.md` section D4. It is a
//! pure function of `(intent, current_price, gatr_abs)` with no
//! ambient state — callers stash the returned [`SnapPlan`] and apply
//! it via [`crate::order_panel::reposition_bracket`] upstream.
//!
//! # Guards (evaluated top-to-bottom)
//!
//! 1. `intent.pinned == true` → `None`. The pin decorator is the user's
//!    explicit opt-out.
//! 2. `intent.updated_at` within the last hour → `None`. Fresh user
//!    edits are sacred; the rule only fires against stale state.
//! 3. `!current_price.is_finite()` or absent/tiny GATR → `None`. A
//!    collapsed denominator would produce garbage deltas.
//! 4. `intent.gatr_anchor.anchor_price.is_none()` → `None`. The first
//!    save is the anchor-seed; no snap happens that frame.
//! 5. `intent.live_annotation_id.is_none()` → `None`. Nothing on the
//!    chart to reposition.
//! 6. Otherwise, delegate to
//!    [`crate::order_panel::should_reposition`] verbatim. On a true
//!    result, return a [`SnapPlan`] carrying the shift delta and the
//!    fresh anchor at the current price.

use chrono::Utc;

use super::{GatrAnchor, TickerOrderIntent};

/// Minimum recency (in seconds) before the snap rule is allowed to
/// fire. A user who touched the intent within this window is treated
/// as "still working on it" — the rule defers to their intent.
pub const RECENCY_GUARD_SECS: i64 = 60 * 60;

/// Minimum absolute GATR value before the rule is allowed to fire.
/// Prevents a divide-by-tiny producing absurd "drift ratios".
pub const MIN_GATR_ABS: f64 = 1e-9;

/// A proposed snap action returned by [`maybe_snap`].
///
/// The caller applies this by:
/// 1. Stashing the pre-snap bracket state for undo.
/// 2. Calling [`crate::order_panel::reposition_bracket`] on the linked
///    annotation with `current_price` (which equals
///    `intent.gatr_anchor.anchor_price + plan.delta`).
/// 3. Writing `plan.new_anchor` into the intent's `gatr_anchor`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SnapPlan {
    /// Signed price delta to apply to every bracket leg.
    /// Equal to `current_price - old_anchor_price`.
    pub delta: f64,
    /// The fresh anchor to write on the intent after the snap lands.
    pub new_anchor: GatrAnchor,
    /// Why the snap fired. Reserved for future policies (e.g., a
    /// proportional-rescale variant) — Slice 4 only emits
    /// [`SnapReason::DriftExceeded`].
    pub reason: SnapReason,
}

/// Why a [`SnapPlan`] was produced.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnapReason {
    /// The anchor-to-current drift exceeded the GATR threshold.
    DriftExceeded,
}

/// Evaluate the snap rule against the given inputs.
///
/// Returns `Some(plan)` when the caller should reposition the bracket,
/// `None` otherwise. See the module docs for the guard order.
///
/// `current_price` is typically `market_cache.get(symbol).last_price`.
/// `gatr_abs` is the absolute GATR value from the same snapshot —
/// `None` when the symbol has no daily data.
pub fn maybe_snap(
    intent: &TickerOrderIntent,
    current_price: f64,
    gatr_abs: Option<f64>,
) -> Option<SnapPlan> {
    // Guard 1: pin.
    if intent.pinned {
        return None;
    }
    // Guard 2: recency.
    let now = Utc::now();
    if now
        .signed_duration_since(intent.updated_at)
        .num_seconds()
        < RECENCY_GUARD_SECS
    {
        return None;
    }
    // Guard 3: finite inputs + non-tiny GATR.
    if !current_price.is_finite() {
        return None;
    }
    let gatr = match gatr_abs {
        Some(g) if g.is_finite() && g > MIN_GATR_ABS => g,
        _ => return None,
    };
    // Guard 4: anchor must be seeded.
    let anchor_price = intent.gatr_anchor.anchor_price?;
    if !anchor_price.is_finite() {
        return None;
    }
    // Guard 5: must have a live bracket to reposition.
    intent.live_annotation_id?;

    // Delegate the threshold check to the existing helper so the
    // semantics match the recall path at `app.rs:1683-1698` exactly.
    // Using the fresh anchor GATR here would be ideal, but the
    // persisted anchor also captured a GATR — both reflect "what was
    // typical at the time we anchored". We pass the *current* gatr
    // because `should_reposition` interprets its third argument as
    // the threshold (current volatility), which is the correct
    // semantic for "has price drifted more than one current day's
    // worth of range away from where the bracket was anchored".
    if !crate::order_panel::should_reposition(anchor_price, current_price, Some(gatr)) {
        return None;
    }

    let delta = current_price - anchor_price;
    Some(SnapPlan {
        delta,
        new_anchor: GatrAnchor {
            anchor_price: Some(current_price),
            anchor_gatr: Some(gatr),
        },
        reason: SnapReason::DriftExceeded,
    })
}

#[cfg(test)]
mod tests;
