//! Wrong-side classification for order-bracket TP / SL legs.
//!
//! A TP leg is "wrong-side" when it lands on the side of `entry` that
//! would contradict the bracket's trade direction — a Long TP at or
//! below entry, a Long SL at or above entry, a Short TP at or above
//! entry, or a Short SL at or below entry. Equal-price is wrong-side
//! too.
//!
//! Ported verbatim from the legacy classifier at
//! `desktop/win/crates/midas-chart/src/widget/order_bracket/mod.rs:246-252`.
//! Plan `00-index.md` C2 pinned inclusive `<=` / `>=` semantics:
//! equal price IS wrong-side. Strict `<` / `>` would disagree with the
//! legacy render path and break muscle-memory.

use crate::tools::Side;

/// Which leg of a bracket a wrong-side classification applies to.
///
/// Narrower than [`LegRole`] — entry and future stop-trigger legs
/// cannot be wrong-side (they define the reference frame), so only TP
/// and SL are meaningful inputs to the classifier.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum LegKind {
    /// Take-profit leg.
    Tp,
    /// Stop-loss leg.
    Sl,
}

/// True iff `leg_price` is on the directional side of `entry` that
/// contradicts `side`.
///
/// Plan C2 correction — verified against
/// `desktop/win/crates/midas-chart/src/widget/order_bracket/mod.rs:246-252`:
/// equality is wrong-side (`<=` / `>=`, NOT strict `<` / `>`).
///
/// The function is pure: no allocation, no logging.
#[inline]
pub fn is_leg_on_wrong_side(side: Side, entry: f64, leg_price: f64, leg_kind: LegKind) -> bool {
    match (side, leg_kind) {
        (Side::Long, LegKind::Tp) => leg_price <= entry,
        (Side::Long, LegKind::Sl) => leg_price >= entry,
        (Side::Short, LegKind::Tp) => leg_price >= entry,
        (Side::Short, LegKind::Sl) => leg_price <= entry,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Strictly wrong-side: 4 tests ─────────────────────────────────

    #[test]
    fn long_tp_below_entry_is_wrong_side() {
        assert!(is_leg_on_wrong_side(Side::Long, 100.0, 99.0, LegKind::Tp));
    }

    #[test]
    fn long_sl_above_entry_is_wrong_side() {
        assert!(is_leg_on_wrong_side(Side::Long, 100.0, 101.0, LegKind::Sl));
    }

    #[test]
    fn short_tp_above_entry_is_wrong_side() {
        assert!(is_leg_on_wrong_side(Side::Short, 100.0, 101.0, LegKind::Tp,));
    }

    #[test]
    fn short_sl_below_entry_is_wrong_side() {
        assert!(is_leg_on_wrong_side(Side::Short, 100.0, 99.0, LegKind::Sl));
    }

    // ── Equal-price IS wrong-side (C2 inclusive semantics): 4 tests ──

    #[test]
    fn long_tp_equal_entry_is_wrong_side() {
        assert!(is_leg_on_wrong_side(Side::Long, 100.0, 100.0, LegKind::Tp));
    }

    #[test]
    fn long_sl_equal_entry_is_wrong_side() {
        assert!(is_leg_on_wrong_side(Side::Long, 100.0, 100.0, LegKind::Sl));
    }

    #[test]
    fn short_tp_equal_entry_is_wrong_side() {
        assert!(is_leg_on_wrong_side(Side::Short, 100.0, 100.0, LegKind::Tp));
    }

    #[test]
    fn short_sl_equal_entry_is_wrong_side() {
        assert!(is_leg_on_wrong_side(Side::Short, 100.0, 100.0, LegKind::Sl));
    }

    // ── Correct-side sanity: not wrong-side ──────────────────────────

    #[test]
    fn long_tp_above_entry_is_correct_side() {
        assert!(!is_leg_on_wrong_side(Side::Long, 100.0, 105.0, LegKind::Tp));
    }

    #[test]
    fn long_sl_below_entry_is_correct_side() {
        assert!(!is_leg_on_wrong_side(Side::Long, 100.0, 95.0, LegKind::Sl));
    }

    #[test]
    fn short_tp_below_entry_is_correct_side() {
        assert!(!is_leg_on_wrong_side(Side::Short, 100.0, 95.0, LegKind::Tp));
    }

    #[test]
    fn short_sl_above_entry_is_correct_side() {
        assert!(!is_leg_on_wrong_side(
            Side::Short,
            100.0,
            105.0,
            LegKind::Sl
        ));
    }
}
