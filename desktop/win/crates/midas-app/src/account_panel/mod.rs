//! Dockable tabbed Account panel.
//!
//! Replaces the single-purpose `OrderBlotterPanel` with a four-tab
//! shell:
//!
//! - **Positions** — placeholder in Slice 1; live grid in Slices 4–5.
//! - **Orders** — today's blotter, preserving 100% of functionality.
//! - **Trade History** — placeholder in Slice 1; Slice 3 ships a
//!   filtered-terminal view over [`crate::order_blotter::OrderBlotter`].
//! - **Recent Instruments** — placeholder in Slice 1; Slice 2 ships
//!   an MRU list.
//!
//! Single-account assumption documented on [`AccountPanel`] — multi-account
//! support is explicitly out of scope for v1.

pub mod empty_state;
pub mod history_columns;
pub mod history_tab;
pub mod orders_tab;
pub mod positions_columns;
pub mod positions_msg;
pub mod positions_store;
pub mod positions_tab;
pub mod recents_tab;
pub mod subscription;

pub use history_tab::HistoryTab;
pub use orders_tab::OrdersTab;
pub use positions_msg::PositionsMsg;
pub use positions_store::{PositionRaw, PositionStore};
pub use positions_tab::PositionsTab;
#[allow(unused_imports)] // Re-exported for callers outside the module tree.
pub use recents_tab::RecentsTab;
pub use subscription::positions_subscription;
#[allow(unused_imports)] // Wired in S7d/e; until the router is constructed both paths coexist.
pub use subscription::{router_positions_subscription, PositionEventsSource};

use midas_core::config::{AccountPanelConfig, AccountTab};
use midas_core::AccountPanelId;

/// Per-pane state for an Account panel.
///
/// Owns the active-tab selection + every tab's view-model. The shared
/// order row store lives on [`crate::app::MidasApp::order_blotter`] so
/// multiple Account panes stay coherent.
///
/// **Single-account assumption:** v1 treats the connected broker's
/// active account as the sole account; there is no per-panel account
/// filter. Multi-account support is a post-v1 concern.
#[derive(Debug, Clone)]
#[allow(dead_code)] // `id` is read in Slice 2+ handlers.
pub struct AccountPanel {
    /// Stable identifier for this panel (per workspace).
    pub id: AccountPanelId,
    /// User-visible panel name (shown in the pane's title bar).
    pub name: String,
    /// Which tab is currently active.
    pub active_tab: AccountTab,
    /// View-model for the Orders tab.
    pub orders: OrdersTab,
    /// View-model for the Trade History tab. Populated lazily from
    /// the shared [`crate::order_blotter::OrderBlotter`] via
    /// [`HistoryTab::rebuild_rows_if_stale`]; not persisted.
    pub history: HistoryTab,
    /// View-model for the Positions tab. Populated lazily from the
    /// app-wide [`PositionStore`] via
    /// [`PositionsTab::rebuild_rows_if_stale`]; not persisted (plan
    /// Decision 4).
    pub positions: PositionsTab,
    /// View-model for the Recents tab. Carries column widths only;
    /// row data is pulled from `MidasApp::recent_symbols` at render
    /// time. Runtime-only (not persisted).
    pub recents: RecentsTab,
    /// Whether the user has dismissed the "Broker disconnected" banner
    /// for the current disconnect episode. Reset to `false` the next
    /// time the broker transitions from connected -> disconnected
    /// (see [`Self::apply_connection_change`]). Session-scoped; not
    /// persisted.
    pub disconnect_banner_ack: bool,
    /// Previously-observed broker-connection state. Used to detect
    /// disconnect transitions so we can reset `disconnect_banner_ack`.
    /// Initialized to `true` (optimistic) so the first render after a
    /// reconnect doesn't flash the banner.
    pub last_known_connected: bool,
}

impl AccountPanel {
    /// Fresh panel with the Orders tab active.
    pub fn new(id: AccountPanelId, name: impl Into<String>) -> Self {
        Self {
            id,
            name: name.into(),
            active_tab: AccountTab::default(),
            orders: OrdersTab::new(),
            history: HistoryTab::new(),
            positions: PositionsTab::new(),
            recents: RecentsTab::new(),
            disconnect_banner_ack: false,
            last_known_connected: true,
        }
    }

    /// Rehydrate from a persisted [`AccountPanelConfig`].
    pub fn from_config(id: AccountPanelId, cfg: &AccountPanelConfig) -> Self {
        Self {
            id,
            name: cfg.name.clone(),
            active_tab: cfg.active_tab,
            orders: OrdersTab::from_config(&cfg.orders),
            history: HistoryTab::new(),
            positions: PositionsTab::new(),
            recents: RecentsTab::new(),
            disconnect_banner_ack: false,
            last_known_connected: true,
        }
    }

    /// Project back to a persistable config.
    pub fn to_config(&self) -> AccountPanelConfig {
        AccountPanelConfig {
            name: self.name.clone(),
            active_tab: self.active_tab,
            orders: self.orders.to_config(),
        }
    }

    /// Update the cached connection state. Resets the banner-ack flag
    /// on a `connected -> disconnected` edge so a fresh disconnect
    /// shows the banner again, even if the user dismissed the prior
    /// episode. The banner does NOT auto-dismiss on reconnect — that
    /// still requires an explicit user click.
    pub fn apply_connection_change(&mut self, now_connected: bool) {
        if self.last_known_connected && !now_connected {
            self.disconnect_banner_ack = false;
        }
        self.last_known_connected = now_connected;
    }

    /// Whether the disconnect banner should render this frame.
    pub fn should_show_disconnect_banner(&self, broker_connected: bool) -> bool {
        !broker_connected && !self.disconnect_banner_ack
    }
}

/// Messages emitted by widgets inside an Account panel.
///
/// Tab-content messages wrap the relevant tab's inner message enum so
/// the outer [`crate::app::Message::Account`] dispatch stays flat.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum AccountMsg {
    /// Switch the active tab.
    TabSelected(AccountTab),
    /// Grid chrome event from the Orders tab (sort / row-select).
    Orders(midas_grid::GridMessage),
    /// Coalesced batch of position updates, produced by
    /// [`subscription::positions_subscription`]. Written through to the
    /// app-wide `PositionStore` rather than per-panel state so every
    /// open Account pane sees the same positions.
    PositionsBatchApplied(Vec<PositionRaw>),
    /// Positions-tab event (close-position, future row select / resize).
    /// Slice 5.
    Positions(PositionsMsg),
    /// The user clicked the "×" on the disconnect banner. Flips
    /// `disconnect_banner_ack` to `true`; the banner stays dismissed
    /// until the next connected -> disconnected transition resets it.
    DisconnectBannerDismissed,
    /// The user clicked a row in the Recent Instruments tab. Handler
    /// reuses the symbol-submit path so the focused chart updates
    /// exactly as if the user had typed the symbol.
    RecentClicked(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_defaults_to_orders_tab() {
        let panel = AccountPanel::new(AccountPanelId::new(1), "Account");
        assert_eq!(panel.active_tab, AccountTab::Orders);
        assert_eq!(panel.name, "Account");
    }

    #[test]
    fn config_round_trip_preserves_fields() {
        let mut panel = AccountPanel::new(AccountPanelId::new(7), "My Account");
        panel.active_tab = AccountTab::Positions;

        let cfg = panel.to_config();
        let restored = AccountPanel::from_config(AccountPanelId::new(7), &cfg);

        assert_eq!(restored.name, "My Account");
        assert_eq!(restored.active_tab, AccountTab::Positions);
        assert_eq!(restored.id, AccountPanelId::new(7));
    }

    #[test]
    fn disconnect_banner_hidden_when_connected() {
        let panel = AccountPanel::new(AccountPanelId::new(1), "A");
        assert!(!panel.should_show_disconnect_banner(true));
    }

    #[test]
    fn disconnect_banner_shown_when_disconnected_and_unacked() {
        let panel = AccountPanel::new(AccountPanelId::new(1), "A");
        assert!(panel.should_show_disconnect_banner(false));
    }

    #[test]
    fn disconnect_banner_hidden_after_ack() {
        let mut panel = AccountPanel::new(AccountPanelId::new(1), "A");
        panel.disconnect_banner_ack = true;
        assert!(!panel.should_show_disconnect_banner(false));
    }

    #[test]
    fn reconnect_does_not_auto_clear_banner_ack() {
        let mut panel = AccountPanel::new(AccountPanelId::new(1), "A");
        // Disconnect, user dismisses banner.
        panel.apply_connection_change(false);
        panel.disconnect_banner_ack = true;
        // Reconnect — ack must persist (plan: "Banner must NOT
        // auto-dismiss on reconnect — requires explicit ack").
        panel.apply_connection_change(true);
        assert!(panel.disconnect_banner_ack);
        // Subsequent disconnect edge resets the ack so the next
        // outage shows the banner again.
        panel.apply_connection_change(false);
        assert!(!panel.disconnect_banner_ack);
    }

    #[test]
    fn disconnect_banner_ack_survives_repeated_disconnect_states() {
        let mut panel = AccountPanel::new(AccountPanelId::new(1), "A");
        panel.apply_connection_change(false);
        panel.disconnect_banner_ack = true;
        // Still disconnected — ack must stay true (no edge).
        panel.apply_connection_change(false);
        assert!(panel.disconnect_banner_ack);
    }
}
