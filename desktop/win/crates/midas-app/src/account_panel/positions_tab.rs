//! Positions tab inside the [`super::AccountPanel`].
//!
//! Renders the app-wide [`super::positions_store::PositionStore`] as an
//! 11-column fixed-width grid plus a trailing close-X action cell.
//! Derived fields (P/L, market value, change %) are computed inline on
//! [`super::positions_columns::DisplayRow`] at build time.
//!
//! # Caching
//!
//! Display rows are cached on the tab and rebuilt only when
//! [`super::positions_store::PositionStore::generation`] advances past
//! [`PositionsTab::last_seen_generation`]. Callers invoke
//! [`PositionsTab::rebuild_rows_if_stale`] from the app's `update()`
//! path; iced's `view()` is `&self`, so mutation cannot happen lazily.
//!
//! # Sort
//!
//! Fixed ascending sort by symbol in v1. Not user-configurable. The
//! grid is declared non-sortable so header clicks are inert.
//!
//! # Close-X safety
//!
//! The close-X cell emits [`super::positions_msg::PositionsMsg::CloseRequested`]
//! regardless of broker state. The handler enforces the disconnect
//! guard — UI disable alone is insufficient per CLAUDE.md rule #3 and
//! the plan's non-goals list. The tab merely renders the button at
//! reduced opacity when disconnected to communicate the state.

use std::collections::HashMap;

use iced::widget::{container, scrollable, Column as IcedColumn, Row as IcedRow, Space};
use iced::{Border, Color, Element, Fill};

use midas_core::AccountPanelId;
use midas_grid::{Alignment, ColumnId, ColumnWidth, GridColumn, GridState};
use midas_ui::UiTheme;

use super::empty_state::empty_state;
use super::positions_columns::{close_x_button, DisplayRow, PositionsColumn};
use super::positions_store::PositionStore;
use super::AccountMsg;

/// Per-Account-panel view state for the Positions tab.
#[derive(Debug, Clone)]
#[allow(dead_code)] // `grid_state`, `selected_row` reserved for post-v1 interactions.
pub struct PositionsTab {
    /// Grid state (column widths + scroll offset). Fixed-width columns
    /// in v1; `column_widths` is seeded from
    /// [`PositionsColumn::default_widths`] on construction.
    pub grid_state: GridState,
    /// Selected row, keyed by symbol. Always `None` in v1 (read-only
    /// except for the explicit close-X action); reserved for future
    /// detail-view wiring.
    pub selected_row: Option<String>,
    /// Latest [`PositionStore::generation`] value reflected in
    /// `cached_rows`. `0` means the cache has never been built.
    pub last_seen_generation: u64,
    /// Precomputed display rows, sorted ascending by symbol. Rebuilt
    /// only when the store mutates (see
    /// [`Self::rebuild_rows_if_stale`]).
    cached_rows: Vec<DisplayRow>,
}

impl Default for PositionsTab {
    fn default() -> Self {
        Self::new()
    }
}

impl PositionsTab {
    /// Fresh tab with fixed default column widths and no cached rows.
    pub fn new() -> Self {
        let ids = PositionsColumn::ids();
        let widths: HashMap<ColumnId, f32> =
            PositionsColumn::default_widths().into_iter().collect();
        Self {
            grid_state: GridState::new(ids, widths),
            selected_row: None,
            last_seen_generation: 0,
            cached_rows: Vec::new(),
        }
    }

    /// Read-only access to the cached display rows. Exposed for tests
    /// and potential future introspection.
    #[allow(dead_code)] // Exposed for tests / future introspection.
    pub fn cached_rows(&self) -> &[DisplayRow] {
        &self.cached_rows
    }

    /// Rebuild [`Self::cached_rows`] from `store` when it has mutated
    /// since the last rebuild. No-op otherwise. Call from the app's
    /// `update()` handler after every mutation that could touch the
    /// store (batch apply, single-event apply, tab selection change).
    pub fn rebuild_rows_if_stale(&mut self, store: &PositionStore) {
        if self.last_seen_generation == store.generation() {
            return;
        }
        let mut rows: Vec<DisplayRow> = store.positions().map(DisplayRow::from_raw).collect();
        // Ascending by symbol — fixed sort in v1.
        rows.sort_by(|a, b| a.symbol.cmp(&b.symbol));
        self.cached_rows = rows;
        self.last_seen_generation = store.generation();
    }

    /// Render the tab body. Assumes [`Self::rebuild_rows_if_stale`] has
    /// been called this tick — the view itself is pure-`&self`.
    ///
    /// `broker_connected` drives close-X button opacity; the handler
    /// enforces the disconnect guard regardless of what this flag says.
    pub fn view<'a>(
        &'a self,
        theme: &UiTheme,
        id: AccountPanelId,
        broker_connected: bool,
    ) -> Element<'a, AccountMsg> {
        if self.cached_rows.is_empty() {
            return empty_state("No open positions", theme);
        }

        // Borrow the const `ALL` array directly so cells borrowing from
        // `&col` carry a `'static` lifetime. Mirrors the History tab.
        let columns: &'static [PositionsColumn; 11] = &PositionsColumn::ALL;

        // ── Header row ──────────────────────────────────────────────
        let header: Element<'_, AccountMsg> = {
            let mut cells: Vec<Element<'_, AccountMsg>> = Vec::with_capacity(columns.len() * 2);
            for (i, col) in columns.iter().enumerate() {
                let w = match col.width() {
                    ColumnWidth::Fixed(px) => px,
                    _ => 80.0,
                };
                cells.push(
                    container(col.header())
                        .width(w)
                        .height(22.0)
                        .padding([2, 4])
                        .align_x(match col.align() {
                            Alignment::Start => iced::alignment::Horizontal::Left,
                            Alignment::Center => iced::alignment::Horizontal::Center,
                            Alignment::End => iced::alignment::Horizontal::Right,
                        })
                        .align_y(iced::alignment::Vertical::Center)
                        .clip(true)
                        .into(),
                );
                if i < columns.len() - 1 {
                    cells.push(Space::new().width(4).height(Fill).into());
                }
            }
            container(IcedRow::with_children(cells).padding([0, 4]))
                .width(Fill)
                .style(|_| container::Style {
                    border: Border {
                        color: midas_grid::GRID_HEADER_BORDER_COLOR,
                        width: 1.0,
                        radius: 0.0.into(),
                    },
                    ..Default::default()
                })
                .into()
        };

        // ── Body rows ───────────────────────────────────────────────
        let mut body = IcedColumn::new();
        for row in &self.cached_rows {
            let mut cells: Vec<Element<'_, AccountMsg>> = Vec::with_capacity(columns.len() * 2);
            for (i, col) in columns.iter().enumerate() {
                let w = match col.width() {
                    ColumnWidth::Fixed(px) => px,
                    _ => 80.0,
                };
                // Close-X column is special-cased because the cell's
                // disabled state depends on `broker_connected`, which
                // the `GridColumn::cell` trait impl can't observe
                // (signature is `(&self, &DisplayRow, usize)`).
                let inner: Element<'_, AccountMsg> = if matches!(col, PositionsColumn::CloseAction)
                {
                    close_x_cell(&row.symbol, broker_connected)
                } else {
                    col.cell(row, 0)
                };
                cells.push(
                    container(inner)
                        .width(w)
                        .height(22.0)
                        .padding([2, 4])
                        .align_x(match col.align() {
                            Alignment::Start => iced::alignment::Horizontal::Left,
                            Alignment::Center => iced::alignment::Horizontal::Center,
                            Alignment::End => iced::alignment::Horizontal::Right,
                        })
                        .align_y(iced::alignment::Vertical::Center)
                        .clip(true)
                        .style(|_| container::Style {
                            border: Border {
                                color: midas_grid::GRID_BORDER_COLOR,
                                width: 1.0,
                                radius: 0.0.into(),
                            },
                            ..Default::default()
                        })
                        .into(),
                );
                if i < columns.len() - 1 {
                    cells.push(Space::new().width(4).height(Fill).into());
                }
            }
            body = body.push(
                container(IcedRow::with_children(cells).padding([0, 4])).style(|_| {
                    container::Style {
                        background: Some(iced::Background::Color(Color::TRANSPARENT)),
                        ..Default::default()
                    }
                }),
            );
        }

        // Stable scrollable ID — unique per Account pane so switching
        // tabs preserves scroll position, and multiple Account panes
        // don't share state. Matches the History-tab pattern.
        let scroll_id: iced::widget::Id = format!("account-{}-positions", id.0).into();
        let body_scroll = scrollable(body).id(scroll_id).height(Fill);

        IcedColumn::new()
            .push(header)
            .push(body_scroll)
            .width(Fill)
            .height(Fill)
            .into()
    }
}

/// Close-X cell wrapped with disabled styling when the broker is
/// offline. The handler-level guard is authoritative; this is a visual
/// affordance only.
///
/// Tooltip mention ("Disconnected — close unavailable") is documented
/// in the plan but omitted from the inline cell: iced tooltips require
/// the containing scroll body to be wide enough to show the popup, and
/// a 44px column is too narrow to host one reliably. The disconnect
/// banner above the tab strip carries the same message at panel scope.
fn close_x_cell(symbol: &str, connected: bool) -> Element<'_, AccountMsg> {
    if connected {
        close_x_button(symbol, true).into()
    } else {
        // Still emit the message — handler enforces the guard. The
        // opacity drop is the visual cue.
        let btn = close_x_button(symbol, false);
        // Wrap in a transparent container to prevent any hover effect
        // from implying the button is actionable. iced's `button` does
        // not yet expose a first-class "disabled + clickable" mode, so
        // we rely on colour alpha and the handler guard together.
        container(btn).into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::account_panel::positions_store::PositionRaw;

    fn store_with_positions(entries: &[(&str, f64, f64)]) -> PositionStore {
        let mut store = PositionStore::new();
        for (sym, qty, cost) in entries {
            let batch = vec![PositionRaw {
                symbol: (*sym).to_owned(),
                qty: *qty,
                avg_cost: *cost,
                last_price: None,
                last_price_ts: None,
                session_open_price: None,
            }];
            store.apply_batch(&batch);
        }
        store
    }

    #[test]
    fn new_has_empty_cache_and_zero_generation() {
        let tab = PositionsTab::new();
        assert!(tab.cached_rows.is_empty());
        assert_eq!(tab.last_seen_generation, 0);
        assert!(tab.selected_row.is_none());
    }

    #[test]
    fn rebuild_populates_cache_from_store() {
        let store = store_with_positions(&[("AAPL", 100.0, 150.0), ("GME", -50.0, 20.0)]);
        let mut tab = PositionsTab::new();
        tab.rebuild_rows_if_stale(&store);

        assert_eq!(tab.cached_rows.len(), 2);
        assert_eq!(tab.last_seen_generation, store.generation());
    }

    #[test]
    fn rebuild_is_noop_when_generation_unchanged() {
        let store = store_with_positions(&[("AAPL", 100.0, 150.0)]);
        let mut tab = PositionsTab::new();
        tab.rebuild_rows_if_stale(&store);
        let first_ptr = tab.cached_rows.as_ptr();
        let first_gen = tab.last_seen_generation;

        tab.rebuild_rows_if_stale(&store);
        assert_eq!(tab.last_seen_generation, first_gen);
        // Cache pointer unchanged → no reallocation happened.
        assert_eq!(tab.cached_rows.as_ptr(), first_ptr);
    }

    #[test]
    fn rebuild_refreshes_when_generation_bumps() {
        let mut store = store_with_positions(&[("AAPL", 100.0, 150.0)]);
        let mut tab = PositionsTab::new();
        tab.rebuild_rows_if_stale(&store);
        assert_eq!(tab.cached_rows.len(), 1);

        // Bump the store: add a second symbol.
        store.apply_batch(&[PositionRaw {
            symbol: "GME".to_owned(),
            qty: -50.0,
            avg_cost: 20.0,
            last_price: None,
            last_price_ts: None,
            session_open_price: None,
        }]);
        tab.rebuild_rows_if_stale(&store);
        assert_eq!(tab.cached_rows.len(), 2);
        assert_eq!(tab.last_seen_generation, store.generation());
    }

    #[test]
    fn rebuild_sorts_by_symbol_ascending() {
        let store = store_with_positions(&[
            ("GME", -50.0, 20.0),
            ("AAPL", 100.0, 150.0),
            ("AS", 200.0, 12.0),
        ]);
        let mut tab = PositionsTab::new();
        tab.rebuild_rows_if_stale(&store);

        let symbols: Vec<&str> = tab.cached_rows.iter().map(|r| r.symbol.as_str()).collect();
        assert_eq!(symbols, vec!["AAPL", "AS", "GME"]);
    }

    #[test]
    fn rebuild_drops_rows_when_store_empties() {
        let mut store = store_with_positions(&[("AAPL", 100.0, 150.0)]);
        let mut tab = PositionsTab::new();
        tab.rebuild_rows_if_stale(&store);
        assert_eq!(tab.cached_rows.len(), 1);

        // Remove the position (qty=0 triggers deletion in the store).
        store.apply(&midas_broker::BrokerEvent::PositionUpdate {
            account: "TEST".to_owned(),
            symbol: "AAPL".to_owned(),
            con_id: 0,
            quantity: 0.0,
            avg_cost: 150.0,
        });
        tab.rebuild_rows_if_stale(&store);
        assert!(tab.cached_rows.is_empty());
        assert_eq!(tab.last_seen_generation, store.generation());
    }

    #[test]
    fn display_row_derivations_match_raw() {
        let raw = PositionRaw {
            symbol: "AAPL".to_owned(),
            qty: 10.0,
            avg_cost: 100.0,
            last_price: Some(110.0),
            last_price_ts: None,
            session_open_price: Some(105.0),
        };
        let row = DisplayRow::from_raw(&raw);
        assert_eq!(row.symbol, "AAPL");
        assert!(row.is_long());
        assert_eq!(row.abs_qty(), 10.0);
        assert_eq!(row.unrealized_pnl(), Some(100.0)); // 10 * (110 - 100)
        assert_eq!(row.market_value(), Some(1100.0)); // 10 * 110
                                                      // (110 - 105) / 105 * 100 ≈ 4.7619
        let pct = row.change_pct().unwrap();
        assert!((pct - 4.7619).abs() < 0.01);
    }

    // The `close_x_cell` helper returns a non-`Send` `Element`; we can
    // at least verify it builds without panicking in both connected
    // and disconnected states.
    #[test]
    fn close_x_cell_builds_in_both_connection_states() {
        let _connected = close_x_cell("AAPL", true);
        let _disconnected = close_x_cell("AAPL", false);
    }
}
