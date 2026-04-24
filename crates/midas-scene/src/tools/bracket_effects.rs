//! Scene-side projection of bracket [`ToolEffect`]s into a tiny data
//! class the app translates into real `TickerMsg`s.
//!
//! Why this layer exists: `midas-scene` stays dep-light (no desktop
//! imports), so the app (`desktop/win/crates/midas-app`) owns the
//! `TickerMsg` enum. This module emits a scene-side
//! [`TickerMsgProjection`] that carries enough information for the
//! widget to pick the right `TickerMsg::EnsureDraftBracket` /
//! `SetLegPrice` / `SaveBracket` / `CancelBracket` — without `midas-
//! scene` depending on `midas-app`.
//!
//! The widget does the final translation in
//! `desktop/win/crates/midas-app/src/session_chart/widget.rs`.

use crate::tools::{LegRole, Side, ToolEffect};

/// Scene-side intent derived from a bracket [`ToolEffect`]. The widget
/// translates each variant into the matching `TickerMsg` (existing
/// draft-then-save sequence — no new `TickerMsg` variant).
///
/// Per plan C1 / architecture rule 8: all bracket mutation flows
/// through `TickerState::apply`. This projection is a data description,
/// not a mutation.
#[derive(Clone, Debug, PartialEq)]
pub enum TickerMsgProjection {
    /// Translates to `TickerMsg::EnsureDraftBracket { side, entry_type:
    /// Limit }` + `TickerMsg::SetLegPrice { role: Entry, price }`.
    /// (Entry type `Limit` is the default for tool-driven brackets; the
    /// toolbar exposes Market/Stop/StopLimit as separate variants in a
    /// later slice.)
    EnsureDraftBracketLimit { side: Side, entry: f64 },
    /// Translates to `TickerMsg::SetLegPrice { role, price }` +
    /// `TickerMsg::SetTpEnabled(true)` when `role == Tp`, or
    /// `TickerMsg::SetSlEnabled(true)` when `role == Sl`.
    SetDraftLeg { role: LegRole, price: f64 },
    /// Translates to `TickerMsg::SaveBracket`.
    CommitDraftBracket,
    /// Translates to `TickerMsg::CancelBracket`.
    CancelDraftBracket,
    /// Translates to `TickerMsg::SetLegPrice { role, price }` on an
    /// existing live bracket (drag-move on TP / SL handles).
    UpdateLiveBracketLeg { id: u64, role: LegRole, price: f64 },
}

/// Project a [`ToolEffect`] bracket variant into [`TickerMsgProjection`].
/// Non-bracket variants return `None` so callers can chain with level-
/// effect translation in one pass.
pub fn project_effect_to_ticker_msg(eff: &ToolEffect) -> Option<TickerMsgProjection> {
    match eff {
        ToolEffect::BeginDraftBracket { side, entry } => {
            Some(TickerMsgProjection::EnsureDraftBracketLimit {
                side: *side,
                entry: *entry,
            })
        }
        ToolEffect::SetDraftLeg { role, price } => Some(TickerMsgProjection::SetDraftLeg {
            role: *role,
            price: *price,
        }),
        ToolEffect::CommitDraftBracket => Some(TickerMsgProjection::CommitDraftBracket),
        ToolEffect::CancelDraftBracket => Some(TickerMsgProjection::CancelDraftBracket),
        ToolEffect::UpdateBracketLeg { id, role, price } => {
            Some(TickerMsgProjection::UpdateLiveBracketLeg {
                id: *id,
                role: *role,
                price: *price,
            })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_begin_draft_bracket() {
        let e = ToolEffect::BeginDraftBracket {
            side: Side::Long,
            entry: 100.0,
        };
        assert_eq!(
            project_effect_to_ticker_msg(&e),
            Some(TickerMsgProjection::EnsureDraftBracketLimit {
                side: Side::Long,
                entry: 100.0
            })
        );
    }

    #[test]
    fn project_set_draft_leg_tp() {
        let e = ToolEffect::SetDraftLeg {
            role: LegRole::Tp,
            price: 105.0,
        };
        assert_eq!(
            project_effect_to_ticker_msg(&e),
            Some(TickerMsgProjection::SetDraftLeg {
                role: LegRole::Tp,
                price: 105.0
            })
        );
    }

    #[test]
    fn project_commit_draft_bracket() {
        let e = ToolEffect::CommitDraftBracket;
        assert_eq!(
            project_effect_to_ticker_msg(&e),
            Some(TickerMsgProjection::CommitDraftBracket)
        );
    }

    #[test]
    fn project_cancel_draft_bracket() {
        let e = ToolEffect::CancelDraftBracket;
        assert_eq!(
            project_effect_to_ticker_msg(&e),
            Some(TickerMsgProjection::CancelDraftBracket)
        );
    }

    #[test]
    fn project_update_bracket_leg() {
        let e = ToolEffect::UpdateBracketLeg {
            id: 42,
            role: LegRole::Sl,
            price: 90.0,
        };
        assert_eq!(
            project_effect_to_ticker_msg(&e),
            Some(TickerMsgProjection::UpdateLiveBracketLeg {
                id: 42,
                role: LegRole::Sl,
                price: 90.0
            })
        );
    }

    #[test]
    fn project_non_bracket_variants_return_none() {
        let e = ToolEffect::CreateLevel {
            price: 100.0,
            lock: false,
        };
        assert!(project_effect_to_ticker_msg(&e).is_none());
        let e = ToolEffect::UpdateLevel {
            id: 1,
            price: 105.0,
        };
        assert!(project_effect_to_ticker_msg(&e).is_none());
        let e = ToolEffect::DeleteLevel { id: 1 };
        assert!(project_effect_to_ticker_msg(&e).is_none());
    }
}
