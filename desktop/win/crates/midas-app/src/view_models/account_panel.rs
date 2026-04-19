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

use std::time::Instant;

use midas_core::config::AccountTab;

use std::collections::{HashMap, HashSet};

use midas_grid::ColumnId;
use uuid::Uuid;

use crate::account_panel::history_columns::{DisplayRow as HistoryDisplayRow, HistoryColumn};
use crate::account_panel::history_tab::HistoryTab;
use crate::account_panel::orders_tab::OrdersTab;
use crate::account_panel::recents_tab::{
    format_elapsed, RecentsTab, COL_RECENTS_LAST_SEEN, COL_RECENTS_TICKER,
};
use crate::app::RecentEntry;
use crate::order_blotter::columns::{
    DisplayRow as OrdersDisplayRow, OrderBlotterColumn, COL_SYMBOL,
};
use crate::order_blotter::OrderBlotter;
use crate::thumbnail_widget::ThumbnailSnapshot;

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

/// One row in the Recent Instruments grid, pre-formatted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecentRowVm {
    /// Ticker symbol the user navigated to (also the click payload).
    pub symbol: String,
    /// Pre-formatted "last seen" label (e.g. `"3m ago"`, `"2d ago"`,
    /// `"—"`). Computed once per build so the view function stays
    /// pure render — see [`format_elapsed`] for the rules.
    pub last_seen_label: String,
}

/// Projection of the inputs `view_account_recents_tab` needs:
/// pre-formatted rows, current grid column widths, and whether the
/// resize-drag overlay is active for this panel.
///
/// Build via [`crate::app::MidasApp::account_recents_tab_vm`] in
/// production; build directly via [`Self::build`] in tests.
#[derive(Debug, Clone, PartialEq)]
pub struct AccountRecentsTabVm {
    /// One entry per MRU symbol, in insertion order. Empty when the
    /// MRU list is empty — the view shows the empty-state message in
    /// that case.
    pub rows: Vec<RecentRowVm>,
    /// Column widths in `[ticker, last_seen]` order. Lifted out of
    /// `RecentsTab::grid_state` so the view doesn't reach into the
    /// panel struct.
    pub column_widths: [f32; 2],
    /// True while the user is dragging a column-resize handle on this
    /// panel. Drives whether the overlay `mouse_area` is stacked on
    /// top of the grid.
    pub show_resize_overlay: bool,
}

impl AccountRecentsTabVm {
    /// Build from raw inputs. `now` is parameterised so tests get
    /// deterministic `last_seen_label` values; production callers
    /// pass [`Instant::now`]. `entries` accepts any iterator over
    /// `&RecentEntry` so production's `VecDeque<RecentEntry>` and the
    /// tests' `Vec<RecentEntry>` work without an intermediate copy.
    pub fn build<'a, I>(
        recents: &RecentsTab,
        entries: I,
        show_resize_overlay: bool,
        now: Instant,
    ) -> Self
    where
        I: IntoIterator<Item = &'a RecentEntry>,
    {
        let rows = entries
            .into_iter()
            .map(|e| RecentRowVm {
                symbol: e.symbol.clone(),
                last_seen_label: format_elapsed(e.last_seen, now),
            })
            .collect();
        let column_widths = [
            recents.grid_state.column_width(COL_RECENTS_TICKER),
            recents.grid_state.column_width(COL_RECENTS_LAST_SEEN),
        ];
        Self {
            rows,
            column_widths,
            show_resize_overlay,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

/// Projection of the inputs `view_account_history_tab` needs:
/// terminal-order display rows + the panel's column widths + an
/// overlay flag.
///
/// **Borrowing VM** — `rows` borrows directly from the source
/// `HistoryTab` rather than cloning. The view's call to
/// `HistoryColumn::cell(&row, idx)` returns an `Element<'_, _>` that
/// borrows the row's pre-formatted strings; cloning into the VM and
/// then returning the Element from the view would drop the VM mid-
/// frame and dangle the cell's borrow. `DisplayRow` is small but
/// row counts cap in the low thousands, so avoiding the per-frame
/// clone also saves real allocation churn.
///
/// The lifetime parameter is the only escape — every consumer of the
/// VM picks up `'a` matching the source `MidasApp` borrow, which is
/// exactly the lifetime the prior code already had on `&self`.
#[derive(Debug, Clone)]
pub struct AccountHistoryTabVm<'a> {
    pub rows: &'a [HistoryDisplayRow],
    /// Per-column widths in [`HistoryColumn::ALL`] order.
    pub column_widths: [f32; 6],
    pub show_resize_overlay: bool,
}

impl<'a> AccountHistoryTabVm<'a> {
    pub fn build(history: &'a HistoryTab, show_resize_overlay: bool) -> Self {
        // Trait import is local: `id()` is the GridColumn trait method;
        // the VM module otherwise has no business with grid types.
        use midas_grid::GridColumn;
        let column_widths = HistoryColumn::ALL.map(|c| history.grid_state.column_width(c.id()));
        Self {
            rows: history.cached_rows(),
            column_widths,
            show_resize_overlay,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

/// One column visible in the Orders grid, post-`hidden_columns` filter.
///
/// Carries the original index into `OrderBlotterColumn::ALL` so resize-
/// drag callbacks can reference the source column position rather than
/// the post-filter visible position (the resize-state machine on
/// `MidasApp` is keyed by absolute index).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VisibleOrdersColumn {
    pub id: ColumnId,
    pub label: &'static str,
    pub sortable: bool,
    pub all_col_idx: usize,
}

/// Full projection of `view_account_orders_tab`'s inputs. Combines
/// the chrome filter (visible columns, hidden set, overlay flags) with
/// the body data (sorted rows, per-row thumbnails, column widths,
/// selection, sort indicator).
///
/// **Owning** — rows are projected via `DisplayRow::from_row` per
/// frame anyway, and thumbnails clone an `Arc<Vec<f32>>`, so the
/// extra Vec allocation is in the noise relative to the existing per-
/// frame work. Owning lets the consumer outlive `MidasApp` borrows
/// without lifetime gymnastics.
#[derive(Debug, Clone)]
pub struct AccountOrdersTabVm {
    /// True iff the source `OrderBlotter` had no rows. Drives the
    /// empty-state placeholder.
    pub is_empty: bool,
    /// Columns currently rendered, in the same order as the source
    /// `OrderBlotterColumn::ALL` (filtered, not reordered).
    pub visible_columns: Vec<VisibleOrdersColumn>,
    /// Cloned for the column-selector popup. Borrowing would force a
    /// lifetime parameter onto the VM for a tiny set; the clone is
    /// bounded by the visible-column count.
    pub hidden_columns: HashSet<ColumnId>,
    pub show_resize_overlay: bool,
    pub show_column_selector: bool,
    /// Display rows in the order the grid should render them — already
    /// sorted by the active sort column (no-op when `sort` is `None`).
    pub sorted_rows: Vec<OrdersDisplayRow>,
    /// Per-row thumbnail snapshot, parallel index with [`Self::sorted_rows`].
    /// Pre-projected so the view doesn't reach into thumbnail stores.
    pub row_thumbnails: Vec<ThumbnailSnapshot>,
    /// Width per column id. The header iterates `visible_columns`; the
    /// body iterates the same and looks the width up here. Also keyed
    /// for hidden columns — looking those up is harmless.
    pub column_widths: HashMap<ColumnId, f32>,
    /// Currently-selected order, if any (for row-highlight rendering).
    pub selected_row: Option<Uuid>,
    /// `(column_id, "↑"|"↓")` for the column the grid is sorted by, if
    /// any. The view paints this as the column-header indicator.
    pub sort_indicator: Option<(ColumnId, &'static str)>,
}

impl AccountOrdersTabVm {
    /// `all_columns` is the static `[(ColumnId, label, sortable); N]`
    /// table from `views.rs`. Passed in (rather than reaching into the
    /// `order_blotter::columns` module) so the VM module stays
    /// independent of the view's column-ordering choices and tests
    /// can pass a synthetic table.
    ///
    /// `thumbnail_for` is a closure (not a `&MidasApp` method) so the
    /// VM stays independent of MidasApp internals — tests stub it
    /// trivially.
    pub fn build<F>(
        orders: &OrdersTab,
        blotter: &OrderBlotter,
        all_columns: &[(ColumnId, &'static str, bool)],
        thumbnail_for: F,
        show_resize_overlay: bool,
        show_column_selector: bool,
    ) -> Self
    where
        F: Fn(&str) -> ThumbnailSnapshot,
    {
        let visible_columns = all_columns
            .iter()
            .enumerate()
            .filter(|(_, (col_id, _, _))| {
                // Symbol is always visible — it's the row identity.
                *col_id == COL_SYMBOL || !orders.hidden_columns.contains(col_id)
            })
            .map(|(idx, (col_id, label, sortable))| VisibleOrdersColumn {
                id: *col_id,
                label,
                sortable: *sortable,
                all_col_idx: idx,
            })
            .collect();

        // Project + sort rows once. Sort honours `grid_state.sort`;
        // when the active sort column doesn't match any known
        // OrderBlotterColumn the sort silently no-ops, matching the
        // pre-VM behaviour.
        let mut sorted_rows: Vec<OrdersDisplayRow> =
            blotter.rows().map(OrdersDisplayRow::from_row).collect();
        if let Some(sort) = orders.grid_state.sort.as_ref() {
            use midas_grid::GridColumn;
            if let Some(col) = OrderBlotterColumn::ALL
                .iter()
                .find(|c| c.id() == sort.column_id)
            {
                sorted_rows.sort_by(|a, b| {
                    let ord = col.compare(a, b);
                    match sort.direction {
                        midas_grid::SortDirection::Ascending => ord,
                        midas_grid::SortDirection::Descending => ord.reverse(),
                    }
                });
            }
        }

        let row_thumbnails: Vec<ThumbnailSnapshot> = sorted_rows
            .iter()
            .map(|r| thumbnail_for(&r.symbol))
            .collect();

        let column_widths: HashMap<ColumnId, f32> = orders.grid_state.column_widths.clone();

        let sort_indicator = orders
            .grid_state
            .sort
            .as_ref()
            .map(|s| (s.column_id, s.direction.indicator()));

        Self {
            is_empty: blotter.is_empty(),
            visible_columns,
            hidden_columns: orders.hidden_columns.clone(),
            show_resize_overlay,
            show_column_selector,
            sorted_rows,
            row_thumbnails,
            column_widths,
            selected_row: orders.selected_row,
            sort_indicator,
        }
    }

    /// Helper for the view's per-cell width lookup. Falls back to 0.0
    /// for unknown ids (caller-error-only — every visible column id
    /// is keyed in [`Self::column_widths`]).
    pub fn width(&self, id: ColumnId) -> f32 {
        self.column_widths.get(&id).copied().unwrap_or(0.0)
    }
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

    // ── AccountRecentsTabVm ───────────────────────────────────────

    use std::time::Duration;

    /// 30-day-future base `Instant` so subtractions stay non-negative
    /// on freshly-booted hosts (matches `recents_tab::tests::late_now`).
    fn late_now() -> Instant {
        Instant::now()
            .checked_add(Duration::from_secs(30 * 24 * 60 * 60))
            .expect("Instant + 30d must fit")
    }

    fn entry(symbol: &str, ago: Option<Duration>) -> RecentEntry {
        RecentEntry {
            symbol: symbol.to_string(),
            last_seen: ago.map(|d| late_now() - d),
        }
    }

    #[test]
    fn recents_vm_empty_when_no_entries() {
        let recents = RecentsTab::new();
        let vm = AccountRecentsTabVm::build(&recents, &[], false, late_now());
        assert!(vm.is_empty());
        assert!(vm.rows.is_empty());
    }

    #[test]
    fn recents_vm_preserves_entry_order() {
        let recents = RecentsTab::new();
        let entries = vec![
            entry("AAPL", Some(Duration::from_secs(30))),
            entry("MSFT", Some(Duration::from_secs(120))),
            entry("NVDA", None),
        ];
        let vm = AccountRecentsTabVm::build(&recents, &entries, false, late_now());
        let symbols: Vec<&str> = vm.rows.iter().map(|r| r.symbol.as_str()).collect();
        assert_eq!(symbols, vec!["AAPL", "MSFT", "NVDA"]);
    }

    #[test]
    fn recents_vm_pre_formats_last_seen_labels() {
        let recents = RecentsTab::new();
        let entries = vec![
            entry("AAPL", Some(Duration::from_secs(30))), // < 1 min
            entry("MSFT", Some(Duration::from_secs(180))), // 3 min
            entry("NVDA", None),
        ];
        let vm = AccountRecentsTabVm::build(&recents, &entries, false, late_now());
        assert_eq!(vm.rows[0].last_seen_label, "just now");
        assert_eq!(vm.rows[1].last_seen_label, "3m ago");
        assert_eq!(vm.rows[2].last_seen_label, "—");
    }

    #[test]
    fn recents_vm_column_widths_match_grid_state() {
        let recents = RecentsTab::new();
        // Default widths from recents_tab::default_widths(): 220, 100.
        let vm = AccountRecentsTabVm::build(&recents, &[], false, late_now());
        assert_eq!(vm.column_widths, [220.0, 100.0]);
    }

    #[test]
    fn recents_vm_overlay_flag_round_trips() {
        let recents = RecentsTab::new();
        let on = AccountRecentsTabVm::build(&recents, &[], true, late_now());
        let off = AccountRecentsTabVm::build(&recents, &[], false, late_now());
        assert!(on.show_resize_overlay);
        assert!(!off.show_resize_overlay);
    }

    // ── AccountHistoryTabVm ───────────────────────────────────────

    #[test]
    fn history_vm_empty_when_no_rows() {
        let history = HistoryTab::new();
        let vm = AccountHistoryTabVm::build(&history, false);
        assert!(vm.is_empty());
        assert!(vm.rows.is_empty());
    }

    #[test]
    fn history_vm_column_widths_match_defaults() {
        let history = HistoryTab::new();
        let vm = AccountHistoryTabVm::build(&history, false);
        // Default widths from history_columns::default_widths().
        assert_eq!(vm.column_widths, [160.0, 80.0, 60.0, 80.0, 100.0, 100.0]);
    }

    #[test]
    fn history_vm_overlay_flag_round_trips() {
        let history = HistoryTab::new();
        let on = AccountHistoryTabVm::build(&history, true);
        let off = AccountHistoryTabVm::build(&history, false);
        assert!(on.show_resize_overlay);
        assert!(!off.show_resize_overlay);
    }

    // ── AccountOrdersTabVm ─────────────────────────────────────

    use std::sync::Arc;

    use crate::account_panel::orders_tab::OrdersTab;
    use crate::order_blotter::columns::{COL_QTY, COL_SIDE, COL_STATUS};
    use crate::order_blotter::OrderBlotter;
    use crate::thumbnail_widget::ThumbnailSnapshot;

    fn synthetic_columns() -> Vec<(ColumnId, &'static str, bool)> {
        vec![
            (COL_SYMBOL, "Symbol", false),
            (COL_SIDE, "Side", true),
            (COL_QTY, "Qty", true),
            (COL_STATUS, "Status", true),
        ]
    }

    /// Empty thumbnail stub — tests don't render, so the actual values
    /// don't matter; we only assert parallel-index alignment.
    fn empty_thumbnail(_symbol: &str) -> ThumbnailSnapshot {
        ThumbnailSnapshot {
            widget_key: 0,
            closes: Arc::new(Vec::new()),
            y_min: 0.0,
            y_max: 0.0,
            color: [0.0; 4],
            generation: 0,
            label: String::new(),
        }
    }

    fn build_chrome(orders: &OrdersTab, resize: bool, picker: bool) -> AccountOrdersTabVm {
        let cols = synthetic_columns();
        let blotter = OrderBlotter::new();
        AccountOrdersTabVm::build(orders, &blotter, &cols, empty_thumbnail, resize, picker)
    }

    #[test]
    fn orders_chrome_vm_includes_all_columns_when_none_hidden() {
        let orders = OrdersTab::new();
        let vm = build_chrome(&orders, false, false);
        let ids: Vec<ColumnId> = vm.visible_columns.iter().map(|c| c.id).collect();
        assert_eq!(ids, vec![COL_SYMBOL, COL_SIDE, COL_QTY, COL_STATUS]);
    }

    #[test]
    fn orders_chrome_vm_filters_hidden_columns() {
        let mut orders = OrdersTab::new();
        orders.hidden_columns.insert(COL_QTY);
        orders.hidden_columns.insert(COL_STATUS);
        let vm = build_chrome(&orders, false, false);
        let ids: Vec<ColumnId> = vm.visible_columns.iter().map(|c| c.id).collect();
        assert_eq!(ids, vec![COL_SYMBOL, COL_SIDE]);
    }

    #[test]
    fn orders_chrome_vm_symbol_column_always_visible_even_if_hidden() {
        let mut orders = OrdersTab::new();
        orders.hidden_columns.insert(COL_SYMBOL); // user attempt — must be ignored
        let vm = build_chrome(&orders, false, false);
        let ids: Vec<ColumnId> = vm.visible_columns.iter().map(|c| c.id).collect();
        assert!(ids.contains(&COL_SYMBOL));
    }

    #[test]
    fn orders_chrome_vm_preserves_all_col_idx() {
        let mut orders = OrdersTab::new();
        orders.hidden_columns.insert(COL_SIDE); // hide the middle one
        let vm = build_chrome(&orders, false, false);
        // After hiding Side (idx 1), the remaining columns keep their
        // *original* indices: Symbol=0, Qty=2, Status=3.
        let idxs: Vec<usize> = vm.visible_columns.iter().map(|c| c.all_col_idx).collect();
        assert_eq!(idxs, vec![0, 2, 3]);
    }

    #[test]
    fn orders_chrome_vm_overlay_flags_round_trip() {
        let orders = OrdersTab::new();
        let on_resize = build_chrome(&orders, true, false);
        let on_picker = build_chrome(&orders, false, true);
        assert!(on_resize.show_resize_overlay && !on_resize.show_column_selector);
        assert!(!on_picker.show_resize_overlay && on_picker.show_column_selector);
    }

    #[test]
    fn orders_tab_vm_is_empty_tracks_blotter() {
        let orders = OrdersTab::new();
        let blotter = OrderBlotter::new();
        let vm = AccountOrdersTabVm::build(
            &orders,
            &blotter,
            &synthetic_columns(),
            empty_thumbnail,
            false,
            false,
        );
        assert!(vm.is_empty);
        assert!(vm.sorted_rows.is_empty());
        assert!(vm.row_thumbnails.is_empty());
    }

    #[test]
    fn orders_tab_vm_sort_indicator_reflects_grid_state() {
        let mut orders = OrdersTab::new();
        orders.grid_state.sort = Some(midas_grid::SortSpec {
            column_id: COL_SIDE,
            direction: midas_grid::SortDirection::Descending,
        });
        let vm = build_chrome(&orders, false, false);
        let (col, dir) = vm.sort_indicator.expect("sort indicator must be Some");
        assert_eq!(col, COL_SIDE);
        // Match whatever `SortDirection::indicator()` actually returns
        // — the projection just forwards it; the glyph is owned by
        // midas_grid.
        assert_eq!(dir, midas_grid::SortDirection::Descending.indicator());
    }

    #[test]
    fn orders_tab_vm_selected_row_round_trips() {
        let mut orders = OrdersTab::new();
        let id = uuid::Uuid::nil();
        orders.selected_row = Some(id);
        let vm = build_chrome(&orders, false, false);
        assert_eq!(vm.selected_row, Some(id));
    }

    #[test]
    fn orders_tab_vm_width_helper_returns_grid_state_widths() {
        // Default OrdersTab sets up 14 columns at the order_blotter
        // defaults; query a known one.
        let orders = OrdersTab::new();
        let vm = build_chrome(&orders, false, false);
        // OrderBlotterColumn::default_widths puts Symbol at 96.0.
        assert_eq!(vm.width(COL_SYMBOL), 96.0);
    }
}
