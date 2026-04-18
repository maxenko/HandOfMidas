//! Multi-panel workspace layout using iced's `pane_grid` widget.
//!
//! Wraps `pane_grid::State` to provide workspace-level operations:
//! creating layouts from presets, splitting/closing panes, and tracking
//! which pane is focused.
//!
//! Panes can hold either chart panels or watchlist panels, distinguished
//! by the [`PanelContent`] enum.

use iced::widget::pane_grid;

use midas_core::{AccountPanelId, ChartId, OrderBlotterId, OrderPanelId, WatchlistId};

// ── Panel content ────────────────────────────────────────────────────

/// What kind of panel a pane is displaying.
#[derive(Debug, Clone)]
pub enum PanelContent {
    /// A chart panel identified by a stable `ChartId`.
    Chart(ChartId),
    /// A watchlist panel identified by a stable `WatchlistId`.
    Watchlist(WatchlistId),
    /// An order panel identified by a stable `OrderPanelId`.
    Order(OrderPanelId),
    /// An order-blotter panel (order history grid).
    ///
    /// Legacy: retained for enum-shape compatibility with existing
    /// callers during the Account-panel refactor. New panes use
    /// `Account` instead. Migration rewrites `OrderBlotter` pane slots
    /// at config-load time.
    OrderBlotter(OrderBlotterId),
    /// A tabbed Account panel (Positions / Orders / History / Recents).
    Account(AccountPanelId),
}

// ── Per-pane state ───────────────────────────────────────────────────

/// State stored inside each pane of the grid.
///
/// The `pane_grid::State` holds one `PaneState` per visible pane.
/// The actual panel data lives in `MidasApp::charts` / `MidasApp::watchlists`;
/// this struct only stores the mapping from pane to panel and focus state.
#[derive(Debug, Clone)]
pub struct PaneState {
    /// What this pane is displaying (chart or watchlist).
    pub content: PanelContent,
    /// Whether this pane currently has keyboard/toolbar focus.
    pub is_focused: bool,
}

impl PaneState {
    /// Create a new pane state for a chart panel.
    pub fn chart(chart_id: ChartId) -> Self {
        Self {
            content: PanelContent::Chart(chart_id),
            is_focused: false,
        }
    }

    /// Create a new pane state for a watchlist panel.
    pub fn watchlist(id: WatchlistId) -> Self {
        Self {
            content: PanelContent::Watchlist(id),
            is_focused: false,
        }
    }

    /// Create a new pane state for an order panel.
    pub fn order(id: OrderPanelId) -> Self {
        Self {
            content: PanelContent::Order(id),
            is_focused: false,
        }
    }

    /// Create a new pane state for an order-blotter panel.
    ///
    /// Legacy — new code uses [`Self::account`]. Retained for enum
    /// shape parity with persisted layouts.
    #[allow(dead_code)]
    pub fn order_blotter(id: OrderBlotterId) -> Self {
        Self {
            content: PanelContent::OrderBlotter(id),
            is_focused: false,
        }
    }

    /// Create a new pane state for an Account (tabbed) panel.
    pub fn account(id: AccountPanelId) -> Self {
        Self {
            content: PanelContent::Account(id),
            is_focused: false,
        }
    }

    /// Convenience: returns `Some(chart_id)` if this pane holds a chart.
    pub fn chart_id(&self) -> Option<ChartId> {
        match self.content {
            PanelContent::Chart(id) => Some(id),
            PanelContent::Watchlist(_)
            | PanelContent::Order(_)
            | PanelContent::OrderBlotter(_)
            | PanelContent::Account(_) => None,
        }
    }
}

// ── Workspace layout ─────────────────────────────────────────────────

/// Manages the spatial arrangement of chart panes using iced's
/// built-in `pane_grid` binary split tree.
///
/// All layout mutations (split, close, resize) go through this struct,
/// which delegates to `pane_grid::State` internally.
pub struct WorkspaceLayout {
    /// The iced pane grid state (binary split tree + per-pane data).
    pub panes: pane_grid::State<PaneState>,
    /// The currently focused pane, if any.
    pub focus: Option<pane_grid::Pane>,
    /// Monotonic counter for generating unique `ChartId` values.
    pub(crate) next_chart_id: u32,
    /// Monotonic counter for generating unique `WatchlistId` values.
    pub(crate) next_watchlist_id: u32,
    /// Monotonic counter for generating unique `OrderPanelId` values.
    pub(crate) next_order_panel_id: u32,
    /// Monotonic counter for generating unique `OrderBlotterId` values.
    ///
    /// Legacy — kept for layout-restore parity; account panels have
    /// their own counter. This value is never incremented during a
    /// normal run (migration rewrites blotter slots to `Account`).
    #[allow(dead_code)]
    pub(crate) next_order_blotter_id: u32,
    /// Monotonic counter for generating unique `AccountPanelId` values.
    pub(crate) next_account_panel_id: u32,
}

impl WorkspaceLayout {
    /// Create a workspace with a single pane filling the entire space.
    ///
    /// Returns the layout and the `ChartId` assigned to the initial pane.
    pub fn single() -> (Self, ChartId) {
        let first_id = ChartId::new(1);
        let (panes, pane) = pane_grid::State::new(PaneState::chart(first_id));

        let mut layout = Self {
            panes,
            focus: Some(pane),
            next_chart_id: 2,
            next_watchlist_id: 1,
            next_order_panel_id: 1,
            next_order_blotter_id: 1,
            next_account_panel_id: 1,
        };

        // Mark the initial pane as focused.
        if let Some(state) = layout.panes.get_mut(pane) {
            state.is_focused = true;
        }

        (layout, first_id)
    }

    /// Allocate a new unique `ChartId`.
    pub fn next_chart_id(&mut self) -> ChartId {
        let id = ChartId::new(self.next_chart_id);
        self.next_chart_id += 1;
        id
    }

    /// Allocate a new unique `WatchlistId`.
    pub fn next_watchlist_id(&mut self) -> WatchlistId {
        let id = WatchlistId::new(self.next_watchlist_id);
        self.next_watchlist_id += 1;
        id
    }

    /// Allocate a new unique `OrderPanelId`.
    pub fn next_order_panel_id(&mut self) -> OrderPanelId {
        let id = OrderPanelId::new(self.next_order_panel_id);
        self.next_order_panel_id += 1;
        id
    }

    /// Allocate a new unique `OrderBlotterId`.
    ///
    /// Legacy — no live call sites; retained for ABI parity.
    #[allow(dead_code)]
    pub fn next_order_blotter_id(&mut self) -> OrderBlotterId {
        let id = OrderBlotterId::new(self.next_order_blotter_id);
        self.next_order_blotter_id += 1;
        id
    }

    /// Allocate a new unique `AccountPanelId`.
    pub fn next_account_panel_id(&mut self) -> AccountPanelId {
        let id = AccountPanelId::new(self.next_account_panel_id);
        self.next_account_panel_id += 1;
        id
    }

    /// Split the given pane along the specified axis.
    ///
    /// Always creates a new **chart** pane in the new half, regardless
    /// of what panel type the source pane holds. Returns the new
    /// `ChartId` and `Pane` handle if the split succeeded.
    pub fn split(
        &mut self,
        axis: pane_grid::Axis,
        pane: pane_grid::Pane,
    ) -> Option<(ChartId, pane_grid::Pane)> {
        let new_chart_id = self.next_chart_id();
        let new_state = PaneState::chart(new_chart_id);

        let result = self.panes.split(axis, pane, new_state);
        result.map(|(new_pane, _split)| (new_chart_id, new_pane))
    }

    /// Close a pane and remove it from the layout.
    ///
    /// Returns the [`PanelContent`] that was in the closed pane, or `None`
    /// if the pane could not be closed (e.g., it is the last pane).
    pub fn close(&mut self, pane: pane_grid::Pane) -> Option<PanelContent> {
        // Don't close the last pane.
        if self.pane_count() <= 1 {
            return None;
        }

        if let Some((removed_state, sibling)) = self.panes.close(pane) {
            // If the closed pane was focused, move focus to the sibling.
            if self.focus == Some(pane) {
                self.set_focus(sibling);
            }
            Some(removed_state.content)
        } else {
            None
        }
    }

    /// Set focus to the given pane, clearing focus from all others.
    pub fn set_focus(&mut self, pane: pane_grid::Pane) {
        // Clear previous focus.
        if let Some(old) = self.focus {
            if let Some(state) = self.panes.get_mut(old) {
                state.is_focused = false;
            }
        }

        // Set new focus.
        self.focus = Some(pane);
        if let Some(state) = self.panes.get_mut(pane) {
            state.is_focused = true;
        }
    }

    /// Get the `ChartId` of the currently focused pane (if it holds a chart).
    pub fn focused_chart_id(&self) -> Option<ChartId> {
        self.focus
            .and_then(|pane| self.panes.get(pane).and_then(|s| s.chart_id()))
    }

    /// Count the number of visible panes.
    pub fn pane_count(&self) -> usize {
        self.panes.panes.len()
    }

    /// Get all `ChartId` values currently in the layout (filters out watchlists).
    pub fn chart_ids(&self) -> Vec<ChartId> {
        self.panes
            .panes
            .values()
            .filter_map(|s| s.chart_id())
            .collect()
    }

    /// Find the pane displaying the given `ChartId`.
    pub fn find_pane(&self, chart_id: ChartId) -> Option<pane_grid::Pane> {
        self.panes
            .panes
            .iter()
            .find(|(_, state)| state.chart_id() == Some(chart_id))
            .map(|(pane, _)| *pane)
    }

    /// Find the pane displaying the given `WatchlistId`.
    #[allow(dead_code)] // part of planned API
    pub fn find_watchlist_pane(&self, wl_id: WatchlistId) -> Option<pane_grid::Pane> {
        self.panes
            .panes
            .iter()
            .find(|(_, state)| matches!(state.content, PanelContent::Watchlist(id) if id == wl_id))
            .map(|(pane, _)| *pane)
    }

    /// Find the pane displaying the given `OrderPanelId`.
    #[allow(dead_code)] // part of planned API
    pub fn find_order_pane(&self, op_id: OrderPanelId) -> Option<pane_grid::Pane> {
        self.panes
            .panes
            .iter()
            .find(|(_, state)| matches!(state.content, PanelContent::Order(id) if id == op_id))
            .map(|(pane, _)| *pane)
    }

    /// Find the first order panel pane in the workspace (any ID).
    pub fn find_any_order_pane(&self) -> Option<pane_grid::Pane> {
        self.panes.panes.iter().find_map(|(pane, state)| {
            matches!(state.content, PanelContent::Order(_)).then_some(*pane)
        })
    }

    /// Get the first pane in the state (by BTreeMap order).
    fn first_pane(&self) -> Option<pane_grid::Pane> {
        self.panes.panes.keys().next().copied()
    }

    /// Apply a layout preset, creating the necessary panes and charts.
    ///
    /// Returns the list of `ChartId` values for all panes in the new
    /// layout (so the caller can insert `ChartPanel` entries).
    pub fn apply_preset(&mut self, preset: &LayoutPresetKind) -> Vec<ChartId> {
        match preset {
            LayoutPresetKind::Single => self.preset_single(),
            LayoutPresetKind::SplitH => self.preset_split(pane_grid::Axis::Vertical),
            LayoutPresetKind::SplitV => self.preset_split(pane_grid::Axis::Horizontal),
            LayoutPresetKind::Grid2x2 => self.preset_grid_2x2(),
        }
    }

    /// Reset to a single-pane layout, reusing the focused chart if
    /// possible.
    fn preset_single(&mut self) -> Vec<ChartId> {
        let keep_id = self.focused_chart_id().unwrap_or_else(|| {
            self.panes
                .panes
                .values()
                .find_map(|s| s.chart_id())
                .unwrap_or_else(|| self.next_chart_id())
        });

        let mut state = PaneState::chart(keep_id);
        state.is_focused = true;
        let (new_panes, pane) = pane_grid::State::new(state);
        self.panes = new_panes;
        self.focus = Some(pane);

        vec![keep_id]
    }

    /// Create a two-pane split layout.
    fn preset_split(&mut self, axis: pane_grid::Axis) -> Vec<ChartId> {
        let id_a = self.focused_chart_id().unwrap_or_else(|| {
            self.panes
                .panes
                .values()
                .find_map(|s| s.chart_id())
                .unwrap_or_else(|| self.next_chart_id())
        });
        let id_b = self.next_chart_id();

        let mut state_a = PaneState::chart(id_a);
        state_a.is_focused = true;
        let state_b = PaneState::chart(id_b);

        let config = pane_grid::Configuration::Split {
            axis,
            ratio: 0.5,
            a: Box::new(pane_grid::Configuration::Pane(state_a)),
            b: Box::new(pane_grid::Configuration::Pane(state_b)),
        };

        let new_panes = pane_grid::State::with_configuration(config);
        self.panes = new_panes;
        // Find and focus the first pane (which holds id_a).
        if let Some(pane) = self.find_pane(id_a) {
            self.focus = Some(pane);
        } else {
            self.focus = self.first_pane();
        }

        vec![id_a, id_b]
    }

    /// Create a 2x2 grid layout (4 panes).
    fn preset_grid_2x2(&mut self) -> Vec<ChartId> {
        let existing: Vec<ChartId> = self.chart_ids();
        let mut ids = Vec::with_capacity(4);
        for i in 0..4 {
            if i < existing.len() {
                ids.push(existing[i]);
            } else {
                ids.push(self.next_chart_id());
            }
        }

        let mut state_a = PaneState::chart(ids[0]);
        state_a.is_focused = true;

        let config = pane_grid::Configuration::Split {
            axis: pane_grid::Axis::Vertical,
            ratio: 0.5,
            a: Box::new(pane_grid::Configuration::Split {
                axis: pane_grid::Axis::Horizontal,
                ratio: 0.5,
                a: Box::new(pane_grid::Configuration::Pane(state_a)),
                b: Box::new(pane_grid::Configuration::Pane(PaneState::chart(ids[1]))),
            }),
            b: Box::new(pane_grid::Configuration::Split {
                axis: pane_grid::Axis::Horizontal,
                ratio: 0.5,
                a: Box::new(pane_grid::Configuration::Pane(PaneState::chart(ids[2]))),
                b: Box::new(pane_grid::Configuration::Pane(PaneState::chart(ids[3]))),
            }),
        };

        let new_panes = pane_grid::State::with_configuration(config);
        self.panes = new_panes;
        if let Some(pane) = self.find_pane(ids[0]) {
            self.focus = Some(pane);
        } else {
            self.focus = self.first_pane();
        }

        ids
    }
}

// ── Layout presets ───────────────────────────────────────────────────

/// Predefined workspace layout configurations.
///
/// These define the structural arrangement of panes. Each preset
/// builds a specific `pane_grid::Configuration` tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LayoutPresetKind {
    /// Single chart filling the entire workspace.
    Single,
    /// Two charts side by side (vertical divider, horizontal
    /// arrangement).
    SplitH,
    /// Two charts stacked (horizontal divider, vertical arrangement).
    SplitV,
    /// Four charts in a 2x2 grid.
    Grid2x2,
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests;
