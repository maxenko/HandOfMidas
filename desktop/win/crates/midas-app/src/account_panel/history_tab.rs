//! Trade History tab inside the [`super::AccountPanel`].
//!
//! Read-only filtered view of [`crate::order_blotter::OrderBlotter`]
//! showing every leg whose status is terminal
//! (`Filled` / `Cancelled` / `Rejected`), sorted most-recent-first by
//! `last_update_at`.
//!
//! # Read-only in v1
//!
//! Row clicks are a no-op — the tab does not broadcast symbols, does
//! not select into an editor, does not expose sort or column-width
//! persistence. Slice 3 intentionally ships the minimum feature set
//! (plan Decision 3); richer interaction is scheduled for a later
//! iteration.
//!
//! # Caching
//!
//! The display rows are cached on the tab and rebuilt only when
//! [`OrderBlotter::generation`] advances past
//! [`HistoryTab::last_seen_generation`]. Callers invoke
//! [`HistoryTab::rebuild_rows_if_stale`] from the app's `update()`
//! path before rendering — iced's `view()` is `&self`, so mutation
//! cannot happen lazily during render.

use std::collections::HashMap;

use midas_grid::{ColumnId, GridState};

use crate::account_panel::history_columns::{DisplayRow, HistoryColumn};
use crate::order_blotter::OrderBlotter;

/// Per-Account-panel view state for the Trade History tab.
#[derive(Debug, Clone)]
#[allow(dead_code)] // `grid_state` / `selected_row` are reserved for post-v1 interactions.
pub struct HistoryTab {
    /// Grid state (column widths + scroll offset). Fixed-width columns
    /// in v1; `column_widths` is seeded from
    /// [`HistoryColumn::default_widths`] on construction. Reserved for
    /// when a later slice lets the user sort / resize History columns.
    pub grid_state: GridState,
    /// Selected row, keyed by broker-assigned order UUID. Always
    /// `None` in v1 (read-only tab); reserved for future detail-view
    /// wiring.
    pub selected_row: Option<uuid::Uuid>,
    /// Latest [`OrderBlotter::generation`] value reflected in
    /// `cached_rows`. `0` means the cache has never been built.
    pub last_seen_generation: u64,
    /// Precomputed terminal rows, sorted descending by timestamp.
    /// Rebuilt only when the blotter mutates (see
    /// [`Self::rebuild_rows_if_stale`]).
    cached_rows: Vec<DisplayRow>,
}

impl Default for HistoryTab {
    fn default() -> Self {
        Self::new()
    }
}

impl HistoryTab {
    /// Fresh tab with fixed default column widths and no cached rows.
    pub fn new() -> Self {
        let ids = HistoryColumn::ids();
        let widths: HashMap<ColumnId, f32> = HistoryColumn::default_widths().into_iter().collect();
        Self {
            grid_state: GridState::new(ids, widths),
            selected_row: None,
            last_seen_generation: 0,
            cached_rows: Vec::new(),
        }
    }

    /// Read-only access to the cached display rows. The `views.rs`
    /// render path (`view_account_history_tab`) reads this slice
    /// directly; tests use it to assert filter / sort behaviour.
    pub fn cached_rows(&self) -> &[DisplayRow] {
        &self.cached_rows
    }

    /// Rebuild [`Self::cached_rows`] from `blotter` when it has mutated
    /// since the last rebuild. No-op otherwise. Call from the app's
    /// `update()` handler after every mutation that could touch the
    /// blotter (broker-event application, initial hydration, tab
    /// selection change).
    pub fn rebuild_rows_if_stale(&mut self, blotter: &OrderBlotter) {
        // `OrderBlotter::generation` starts at 0 and bumps on every
        // mutation (`apply`, `hydrate`). `last_seen_generation` starts
        // at 0 too — so an untouched blotter skips the rebuild, which
        // is correct because there are no rows to project.
        if self.last_seen_generation == blotter.generation() {
            return;
        }
        let mut rows: Vec<DisplayRow> = blotter
            .rows()
            .filter(|r| r.status.is_terminal())
            .map(DisplayRow::from_row)
            .collect();
        // Most-recent-first. Ties broken arbitrarily (BTreeMap order).
        rows.sort_by_key(|r| std::cmp::Reverse(r.timestamp));
        self.cached_rows = rows;
        self.last_seen_generation = blotter.generation();
    }

    // Rendering lives in `app::views::view_account_history_tab` so the
    // History grid can use the same `midas_grid` helpers (resizable
    // headers, alternating row backgrounds, selection chrome) as the
    // Orders tab and the watchlist. Keeping the render path in one
    // place avoids divergence between grids.
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::order_blotter::OrderStatus;
    use midas_broker::BrokerEvent;
    use midas_broker::OrderAction as BrokerAction;
    use midas_broker::OrderKind as BrokerKind;
    use midas_broker::TimeInForce as BrokerTif;
    use uuid::Uuid;

    // Mirror of the test-helper uuid generator in `order_blotter::mod`.
    fn rand_id() -> u128 {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0x1000);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        (n as u128) | (0x43_u128 << 96)
    }

    fn bracket(parent: Uuid, tp: Option<Uuid>, sl: Option<Uuid>) -> BrokerEvent {
        BrokerEvent::BracketCreated {
            parent_id: parent,
            take_profit_id: tp,
            stop_loss_id: sl,
            symbol: "MSFT".to_owned(),
            action: BrokerAction::Buy,
            quantity: 50.0,
            tp_price: tp.map(|_| 420.0),
            sl_price: sl.map(|_| 390.0),
            reference_price: Some(400.0),
            entry_kind: BrokerKind::Limit,
            entry_limit_price: Some(400.0),
            entry_stop_price: None,
            sl_limit_price: None,
            tp_tif: Some(BrokerTif::Day),
            sl_tif: Some(BrokerTif::Gtc),
        }
    }

    #[test]
    fn new_has_empty_cache_and_zero_generation() {
        let tab = HistoryTab::new();
        assert!(tab.cached_rows.is_empty());
        assert_eq!(tab.last_seen_generation, 0);
        assert!(tab.selected_row.is_none());
    }

    #[test]
    fn rebuild_filters_terminal_rows_only() {
        let mut blotter = OrderBlotter::new();
        let parent = Uuid::from_u128(rand_id());
        let tp = Uuid::from_u128(rand_id());
        let sl = Uuid::from_u128(rand_id());
        blotter.apply(&bracket(parent, Some(tp), Some(sl)));
        // Only the parent goes terminal (Filled). TP / SL remain Working.
        blotter.apply(&BrokerEvent::OrderStatusChanged {
            order_id: parent,
            old_status: "Submitted".to_owned(),
            new_status: "Filled".to_owned(),
            filled_qty: 50.0,
            remaining_qty: 0.0,
            avg_fill_price: 400.25,
        });

        let mut tab = HistoryTab::new();
        tab.rebuild_rows_if_stale(&blotter);

        assert_eq!(tab.cached_rows.len(), 1, "only the Filled leg is retained");
        assert_eq!(tab.cached_rows[0].order_id, parent);
        assert_eq!(tab.cached_rows[0].status, OrderStatus::Filled);
        assert_eq!(tab.last_seen_generation, blotter.generation());
    }

    #[test]
    fn rebuild_sorts_descending_by_timestamp() {
        let mut blotter = OrderBlotter::new();
        let parent_a = Uuid::from_u128(rand_id());
        let parent_b = Uuid::from_u128(rand_id());
        let parent_c = Uuid::from_u128(rand_id());
        blotter.apply(&bracket(parent_a, None, None));
        blotter.apply(&bracket(parent_b, None, None));
        blotter.apply(&bracket(parent_c, None, None));

        // Terminalise A first, then B, then C. The `last_update_at`
        // timestamp is stamped as `Utc::now()` inside each apply call;
        // we sleep-free because successive `Utc::now()` calls are
        // monotonic at microsecond granularity on every platform we
        // target. If two timestamps collide the sort is stable w.r.t.
        // the BTreeMap iteration order — the assertion below allows
        // for that by only checking the head of the vec.
        blotter.apply(&BrokerEvent::OrderStatusChanged {
            order_id: parent_a,
            old_status: "Submitted".to_owned(),
            new_status: "Filled".to_owned(),
            filled_qty: 50.0,
            remaining_qty: 0.0,
            avg_fill_price: 400.0,
        });
        blotter.apply(&BrokerEvent::OrderCancelled {
            order_id: parent_b,
            reason: "user".to_owned(),
        });
        blotter.apply(&BrokerEvent::OrderRejected {
            order_id: parent_c,
            reason: "invalid".to_owned(),
        });

        let mut tab = HistoryTab::new();
        tab.rebuild_rows_if_stale(&blotter);

        assert_eq!(tab.cached_rows.len(), 3);
        // Descending: every row's timestamp must be >= the next one's.
        for pair in tab.cached_rows.windows(2) {
            assert!(
                pair[0].timestamp >= pair[1].timestamp,
                "rows must be sorted most-recent-first"
            );
        }
        // The most recently mutated parent (C — rejected last) should
        // lead the list.
        assert_eq!(tab.cached_rows[0].order_id, parent_c);
    }

    #[test]
    fn rebuild_is_noop_when_generation_unchanged() {
        let mut blotter = OrderBlotter::new();
        let parent = Uuid::from_u128(rand_id());
        blotter.apply(&bracket(parent, None, None));
        blotter.apply(&BrokerEvent::OrderStatusChanged {
            order_id: parent,
            old_status: "Submitted".to_owned(),
            new_status: "Filled".to_owned(),
            filled_qty: 50.0,
            remaining_qty: 0.0,
            avg_fill_price: 400.0,
        });

        let mut tab = HistoryTab::new();
        tab.rebuild_rows_if_stale(&blotter);
        let first_ptr = tab.cached_rows.as_ptr();
        let first_gen = tab.last_seen_generation;

        // Second call with unchanged blotter must not rebuild the Vec.
        tab.rebuild_rows_if_stale(&blotter);
        assert_eq!(tab.last_seen_generation, first_gen);
        // Cache pointer unchanged → no reallocation happened.
        assert_eq!(tab.cached_rows.as_ptr(), first_ptr);
    }

    #[test]
    fn rebuild_drops_rows_when_blotter_has_none_terminal() {
        // A blotter with only Working rows has generation > 0 but no
        // terminal rows; cache must stay empty.
        let mut blotter = OrderBlotter::new();
        let parent = Uuid::from_u128(rand_id());
        blotter.apply(&bracket(parent, None, None));

        let mut tab = HistoryTab::new();
        tab.rebuild_rows_if_stale(&blotter);
        assert!(tab.cached_rows.is_empty());
        assert_eq!(tab.last_seen_generation, blotter.generation());
    }
}
