//! View-model for the Account panel's header chrome.
//!
//! Projects the inputs needed by [`crate::app::MidasApp::view_account_body`]
//! to render its tab strip + disconnect banner. Sub-tab bodies still
//! consume `&self` — that migration lands in follow-up slices.
//!
//! # Pattern
//!
//! 1. View-model is a plain `Copy`-friendly struct of computed values.
//! 2. Builder is a `&self` method on [`crate::app::MidasApp`] that
//!    gathers all inputs and returns the struct.
//! 3. Tests construct the struct directly and assert on its fields, OR
//!    drive the builder through a stub `MidasApp` (later slices).

use midas_core::config::AccountTab;

/// Projection of `MidasApp` state needed to render the Account panel's
/// header chrome (tab strip badges + disconnect banner visibility).
///
/// All fields are pre-computed: badge counts already cap at the visual
/// limit, and `show_disconnect_banner` already incorporates the "user
/// dismissed" flag. The view function is then a pure function of this
/// struct + the user's tab-clicked callbacks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AccountPanelHeaderVm {
    /// Currently selected tab — drives which `TabItem` is rendered as
    /// active and which body the parent dispatches to.
    pub active_tab: AccountTab,
    /// Working-order count (non-terminal rows) for the Orders tab badge.
    /// Pre-capped at [`Self::BADGE_CAP`].
    pub working_count: usize,
    /// Terminal-order count for the Trade History tab badge. Pre-capped
    /// at [`Self::BADGE_CAP`].
    pub history_count: usize,
    /// Open-position count for the Positions tab badge. Pre-capped at
    /// [`Self::BADGE_CAP`].
    pub positions_count: usize,
    /// MRU symbol count for the Recent Instruments tab badge. Pre-capped
    /// at [`Self::RECENTS_BADGE_CAP`] (tighter cap — the MRU list is
    /// itself bounded by `MAX_RECENTS`).
    pub recents_count: usize,
    /// Whether the disconnect banner should render this frame. Already
    /// folds in the panel's `disconnect_banner_ack` flag.
    pub show_disconnect_banner: bool,
}

impl AccountPanelHeaderVm {
    /// Visual cap for tab badges that draw from unbounded sources
    /// (orders, positions). Anything above this would blow up the
    /// fixed-width badge layout.
    pub const BADGE_CAP: usize = 999;
    /// Tighter cap for the Recents badge — the underlying MRU list is
    /// already bounded, so a 2-digit cap is enough.
    pub const RECENTS_BADGE_CAP: usize = 99;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vm(active: AccountTab) -> AccountPanelHeaderVm {
        AccountPanelHeaderVm {
            active_tab: active,
            working_count: 0,
            history_count: 0,
            positions_count: 0,
            recents_count: 0,
            show_disconnect_banner: false,
        }
    }

    #[test]
    fn vm_round_trips_active_tab() {
        let v = vm(AccountTab::Positions);
        assert_eq!(v.active_tab, AccountTab::Positions);
    }

    #[test]
    fn vm_is_copy() {
        // Compile-time check: the view-model is cheap to pass by value
        // so callsites don't have to clone it.
        fn assert_copy<T: Copy>() {}
        assert_copy::<AccountPanelHeaderVm>();
    }

    #[test]
    fn badge_caps_have_expected_values() {
        // Pin the caps so a future "let's bump the badge width" tweak
        // forces a deliberate test update rather than silently changing
        // visible behaviour.
        assert_eq!(AccountPanelHeaderVm::BADGE_CAP, 999);
        assert_eq!(AccountPanelHeaderVm::RECENTS_BADGE_CAP, 99);
    }
}
