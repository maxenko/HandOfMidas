//! App-global panel ID allocator and the typed `PanelId` enum.
//!
//! Counters used to live on `WorkspaceLayout`. With multiple windows
//! coming online, two layouts would otherwise mint colliding IDs into
//! the app-global `MidasApp::charts` / `watchlists` / `order_panels` /
//! `account_panels` maps. This allocator is the single source of truth
//! for fresh IDs.
//!
//! `PanelId` is the key for `MidasApp::panel_to_window` — the runtime
//! invariant that links every panel back to its owning window.

#[cfg(feature = "session_chart")]
use midas_core::SessionChartId;
use midas_core::{AccountPanelId, ChartId, OrderPanelId, WatchlistId};

/// Typed handle to a single panel — the key used by
/// `panel_to_window: HashMap<PanelId, WindowKey>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PanelId {
    Chart(ChartId),
    Watchlist(WatchlistId),
    Order(OrderPanelId),
    Account(AccountPanelId),
    /// Slice F2: per-pane id for a session-chart panel. Feature-gated
    /// on `session_chart` because the supporting widget + driver are
    /// only present when the feature is on.
    #[cfg(feature = "session_chart")]
    SessionChart(SessionChartId),
}

/// Monotonic ID counters for every panel kind. `MidasApp` owns one of
/// these; layout code mints fresh IDs by passing a `&mut` reference
/// or by pre-allocating before the call.
#[derive(Debug, Clone)]
pub struct PanelIdAllocator {
    next_chart: u32,
    next_watchlist: u32,
    next_order_panel: u32,
    next_account_panel: u32,
    /// Slice F2: monotonic counter for session-chart panes. Always
    /// present even when the `session_chart` feature is off so the
    /// allocator's layout doesn't depend on cargo flags. Only read
    /// via `next_session_chart()`, which is itself feature-gated.
    #[cfg_attr(not(feature = "session_chart"), allow(dead_code))]
    next_session_chart: u32,
}

impl Default for PanelIdAllocator {
    fn default() -> Self {
        Self {
            next_chart: 1,
            next_watchlist: 1,
            next_order_panel: 1,
            next_account_panel: 1,
            next_session_chart: 1,
        }
    }
}

impl PanelIdAllocator {
    /// Allocate a new unique [`ChartId`].
    pub fn next_chart(&mut self) -> ChartId {
        let id = ChartId::new(self.next_chart);
        self.next_chart += 1;
        id
    }

    /// Allocate a new unique [`WatchlistId`].
    pub fn next_watchlist(&mut self) -> WatchlistId {
        let id = WatchlistId::new(self.next_watchlist);
        self.next_watchlist += 1;
        id
    }

    /// Allocate a new unique [`OrderPanelId`].
    pub fn next_order_panel(&mut self) -> OrderPanelId {
        let id = OrderPanelId::new(self.next_order_panel);
        self.next_order_panel += 1;
        id
    }

    /// Allocate a new unique [`AccountPanelId`].
    pub fn next_account_panel(&mut self) -> AccountPanelId {
        let id = AccountPanelId::new(self.next_account_panel);
        self.next_account_panel += 1;
        id
    }

    /// Allocate a new unique [`SessionChartId`]. Slice F2.
    #[cfg(feature = "session_chart")]
    pub fn next_session_chart(&mut self) -> SessionChartId {
        let id = SessionChartId::new(self.next_session_chart);
        self.next_session_chart += 1;
        id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_allocator_starts_at_one() {
        let mut a = PanelIdAllocator::default();
        assert_eq!(a.next_chart().0, 1);
        assert_eq!(a.next_watchlist().0, 1);
        assert_eq!(a.next_order_panel().0, 1);
        assert_eq!(a.next_account_panel().0, 1);
    }

    #[test]
    fn allocations_are_monotonic_per_kind() {
        let mut a = PanelIdAllocator::default();
        let c1 = a.next_chart();
        let c2 = a.next_chart();
        assert!(c2.0 > c1.0);

        // Watchlist counter is independent of chart counter.
        let w1 = a.next_watchlist();
        assert_eq!(w1.0, 1);
    }
}
