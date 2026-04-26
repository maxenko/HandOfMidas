//! Pure state-transition helpers for the Slice 4 VP settings popup.
//!
//! The settings popup is a single-target affordance: at most one popup
//! is open across the whole workspace, tracked by `MidasApp::vp_settings_open:
//! Option<ChartId>`. The state transitions ARE the popup's whole story —
//! the rest of the slice is rendering — so we factor them into pure
//! functions here, callable from the [`Message::ToggleVpSettingsPanel`]
//! handler in `handlers.rs`, and exhaustively unit-testable without
//! constructing a full [`MidasApp`](crate::MidasApp).
//!
//! See `plan/volume-profile-anchored/04-slice-4-gear-popup-ui.md` §S4a.

use midas_core::ChartId;

/// Toggle the popup's chart-id target. Sending the same id twice
/// closes the popup; sending a different id switches focus to that
/// chart's popup (only one popup may be open at a time).
#[inline]
pub fn toggle(current: Option<ChartId>, target: ChartId) -> Option<ChartId> {
    if current == Some(target) {
        None
    } else {
        Some(target)
    }
}

/// Clear the popup if the target chart's pane just closed. Returns
/// the new value; either the input unchanged or `None`.
#[inline]
pub fn clear_if_closed(current: Option<ChartId>, closed: ChartId) -> Option<ChartId> {
    current.filter(|&id| id != closed)
}

/// Clear the popup if Volume Profile rendering just turned off for
/// the popup's target chart. Returns the new value.
#[inline]
pub fn clear_if_vp_off(
    current: Option<ChartId>,
    chart_id: ChartId,
    vp_now_off: bool,
) -> Option<ChartId> {
    if vp_now_off {
        current.filter(|&id| id != chart_id)
    } else {
        current
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cid(n: u32) -> ChartId {
        ChartId(n)
    }

    /// Test #1 — toggle opens then closes the popup for the SAME chart.
    #[test]
    fn toggle_panel_opens_and_closes_for_same_chart() {
        let mut state: Option<ChartId> = None;
        state = toggle(state, cid(0));
        assert_eq!(state, Some(cid(0)), "first toggle opens");
        state = toggle(state, cid(0));
        assert_eq!(state, None, "second toggle on same chart closes");
    }

    /// Test #2 — toggling on a DIFFERENT chart switches focus rather
    /// than opening a second popup. One popup at a time invariant.
    #[test]
    fn toggle_panel_switches_between_charts() {
        let mut state: Option<ChartId> = Some(cid(0));
        state = toggle(state, cid(1));
        assert_eq!(
            state,
            Some(cid(1)),
            "toggle on a different chart switches focus"
        );
    }

    /// Test #5 — turning VP off for the open chart auto-dismisses the
    /// popup; turning VP off for an UNRELATED chart does not.
    #[test]
    fn vp_off_dismisses_only_open_chart() {
        let state = Some(cid(0));
        // VP off on chart 0 → dismiss.
        assert_eq!(clear_if_vp_off(state, cid(0), true), None);
        // VP off on chart 1 (popup still on chart 0) → no change.
        assert_eq!(clear_if_vp_off(state, cid(1), true), Some(cid(0)));
        // VP toggled ON (vp_now_off = false) → no change.
        assert_eq!(clear_if_vp_off(state, cid(0), false), Some(cid(0)));
    }

    /// Test #6 — closing the popup's target pane clears the popup.
    /// Closing a different pane leaves the popup intact.
    #[test]
    fn pane_close_clears_only_target_chart() {
        let state = Some(cid(0));
        assert_eq!(clear_if_closed(state, cid(0)), None);
        assert_eq!(clear_if_closed(state, cid(1)), Some(cid(0)));
        // Empty start state: pane close is a no-op regardless.
        assert_eq!(clear_if_closed(None, cid(0)), None);
    }
}
