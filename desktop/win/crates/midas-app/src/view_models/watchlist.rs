//! View-model for the watchlist body view (audit P1).
//!
//! Projects the inputs `MidasApp::view_watchlist_body` reads off
//! `&self` (tickers, market cache lookups, sort spec, selection,
//! resize-overlay flag, link-picker state) into a self-contained VM
//! the view function consumes by value. Same pattern as
//! `view_models::account_panel::AccountOrdersTabVm`:
//!
//! - Owning VM (rows are projected per-frame anyway, thumbnails clone
//!   an `Arc<Vec<f32>>`, so the extra Vec allocation is in the noise).
//! - Builder takes a `Fn(&str) -> ThumbnailSnapshot` closure so the VM
//!   stays independent of MidasApp internals.
//! - `selected_row_idx` is the post-sort index — bridging from
//!   `WatchlistPanel::selected_symbol` happens in the builder.

use std::collections::HashMap;

use iced::Color;
use midas_core::{gatr_color, MarketSnapshot};
use midas_grid::ColumnId;

use crate::link::LinkDimension;
use crate::market_cache::MarketDataCache;
use crate::thumbnail_widget::ThumbnailSnapshot;
use crate::watchlist::WatchlistPanel;
use crate::watchlist_columns::WatchlistRow;

/// Projection of `view_watchlist_body`'s inputs. Holds pre-built rows
/// (incl. per-row `ThumbnailSnapshot`), pre-resolved sort indicator,
/// pre-bridged selection index, and the two overlay flags.
#[derive(Debug, Clone)]
pub struct WatchlistBodyVm {
    pub rows: Vec<WatchlistRow>,
    /// Width per column id, copied from `WatchlistPanel::grid_state`.
    pub column_widths: HashMap<ColumnId, f32>,
    /// `(column_id, "↑"|"↓")` for the active sort column, if any.
    pub sort_indicator: Option<(ColumnId, &'static str)>,
    /// Index of the selected row in `rows` after sorting, or `None`
    /// when no symbol is selected (or the selected symbol isn't in
    /// the current ticker list).
    pub selected_row_idx: Option<usize>,
    /// Current text in the "Add ticker..." input.
    pub add_ticker_input: String,
    pub show_resize_overlay: bool,
    /// `Some(dim)` when the link picker is open targeting this
    /// watchlist; `None` otherwise. Carries the dimension so the
    /// view passes it straight to `build_link_picker`.
    pub link_picker_dim: Option<LinkDimension>,
}

impl WatchlistBodyVm {
    /// Build from the source watchlist + market cache + thumbnail
    /// closure. Sort honours `wl.grid_state.sort` after the favorites-
    /// first secondary sort, matching the pre-VM behaviour exactly.
    pub fn build<F>(
        wl: &WatchlistPanel,
        market_cache: &MarketDataCache,
        thumbnail_for: F,
        show_resize_overlay: bool,
        link_picker_dim: Option<LinkDimension>,
    ) -> Self
    where
        F: Fn(&str) -> ThumbnailSnapshot,
    {
        let empty_snapshot = MarketSnapshot::default();
        let mut rows: Vec<WatchlistRow> = wl
            .tickers
            .iter()
            .map(|ticker| {
                let snap = market_cache
                    .get(&crate::annotation_store::SymbolKey::new(&ticker.symbol))
                    .unwrap_or(&empty_snapshot);
                let price_text = snap
                    .last_price
                    .map(|p| format!("{p:.2}"))
                    .unwrap_or_else(|| "--".into());
                let change_text = snap
                    .change_pct
                    .map(|c| format!("{c:+.2}%"))
                    .unwrap_or_else(|| "--".into());
                let change_color = match snap.change_pct {
                    Some(c) if c > 0.0 => Color::from_rgb(0.2, 0.8, 0.3),
                    Some(c) if c < 0.0 => Color::from_rgb(0.9, 0.25, 0.2),
                    _ => Color::from_rgb(0.6, 0.6, 0.6),
                };
                let gatr_text = snap
                    .gatr_pct
                    .map(|pct| format!("G.ATR {:.0}%", pct))
                    .unwrap_or_else(|| "--".into());
                let gatr_color = snap
                    .gatr_pct
                    .map(|_| {
                        let price_up = snap.change_pct.is_none_or(|c| c >= 0.0);
                        let c = gatr_color(price_up);
                        Color::from_rgba(c[0], c[1], c[2], c[3])
                    })
                    .unwrap_or(Color::from_rgb(0.6, 0.6, 0.6));
                WatchlistRow {
                    symbol: ticker.symbol.clone(),
                    favorite: ticker.favorite,
                    price_text,
                    change_text,
                    change_color,
                    gatr_text,
                    gatr_color,
                    wl_id: wl.id,
                    price_value: snap.last_price,
                    change_value: snap.change_pct,
                    thumbnail: thumbnail_for(&ticker.symbol),
                }
            })
            .collect();

        // Favorites first (descending), then by the active grid sort.
        // Matches the prior view's two-stage sort exactly so visual
        // identity is preserved.
        rows.sort_by(|a, b| {
            let fav = b.favorite.cmp(&a.favorite);
            if fav != std::cmp::Ordering::Equal {
                return fav;
            }
            if let Some(sort) = &wl.grid_state.sort {
                use midas_grid::GridColumn;
                let columns = crate::watchlist_columns::WatchlistColumn::all();
                if let Some(col) = columns.iter().find(|c| c.id() == sort.column_id) {
                    let ord = col.compare(a, b);
                    return match sort.direction {
                        midas_grid::SortDirection::Ascending => ord,
                        midas_grid::SortDirection::Descending => ord.reverse(),
                    };
                }
            }
            std::cmp::Ordering::Equal
        });

        let selected_row_idx = wl
            .selected_symbol
            .as_ref()
            .and_then(|sym| rows.iter().position(|r| r.symbol == *sym));

        let column_widths = wl.grid_state.column_widths.clone();

        let sort_indicator = wl
            .grid_state
            .sort
            .as_ref()
            .map(|s| (s.column_id, s.direction.indicator()));

        Self {
            rows,
            column_widths,
            sort_indicator,
            selected_row_idx,
            add_ticker_input: wl.add_ticker_input.clone(),
            show_resize_overlay,
            link_picker_dim,
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
    use std::sync::Arc;

    use midas_core::WatchlistId;

    use super::*;
    use crate::watchlist::{WatchlistPanel, WatchlistTicker};
    use crate::watchlist::{COL_PRICE, COL_TICKER};

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

    fn ticker(symbol: &str, favorite: u8) -> WatchlistTicker {
        WatchlistTicker {
            symbol: symbol.to_string(),
            favorite,
        }
    }

    #[test]
    fn vm_empty_when_no_tickers() {
        let wl = WatchlistPanel::new(WatchlistId::new(1), "Main".into());
        let cache = MarketDataCache::default();
        let vm = WatchlistBodyVm::build(&wl, &cache, empty_thumbnail, false, None);
        assert!(vm.rows.is_empty());
        assert_eq!(vm.selected_row_idx, None);
    }

    #[test]
    fn vm_favorites_sort_to_top() {
        let mut wl = WatchlistPanel::new(WatchlistId::new(1), "Main".into());
        wl.tickers = vec![ticker("AAPL", 0), ticker("MSFT", 5), ticker("NVDA", 0)];
        let cache = MarketDataCache::default();
        let vm = WatchlistBodyVm::build(&wl, &cache, empty_thumbnail, false, None);
        let symbols: Vec<&str> = vm.rows.iter().map(|r| r.symbol.as_str()).collect();
        // Favorited MSFT comes first; AAPL/NVDA preserve insertion order.
        assert_eq!(symbols, vec!["MSFT", "AAPL", "NVDA"]);
    }

    #[test]
    fn vm_selected_row_idx_follows_sort() {
        let mut wl = WatchlistPanel::new(WatchlistId::new(1), "Main".into());
        wl.tickers = vec![ticker("AAPL", 0), ticker("MSFT", 5)];
        wl.selected_symbol = Some("AAPL".into());
        let cache = MarketDataCache::default();
        let vm = WatchlistBodyVm::build(&wl, &cache, empty_thumbnail, false, None);
        // After favorites-first sort, AAPL ends up at index 1.
        assert_eq!(vm.selected_row_idx, Some(1));
    }

    #[test]
    fn vm_selected_row_idx_none_when_symbol_missing() {
        let mut wl = WatchlistPanel::new(WatchlistId::new(1), "Main".into());
        wl.tickers = vec![ticker("AAPL", 0)];
        wl.selected_symbol = Some("ZZZ".into()); // not in tickers
        let cache = MarketDataCache::default();
        let vm = WatchlistBodyVm::build(&wl, &cache, empty_thumbnail, false, None);
        assert_eq!(vm.selected_row_idx, None);
    }

    #[test]
    fn vm_market_data_drives_price_text_and_color() {
        let mut wl = WatchlistPanel::new(WatchlistId::new(1), "Main".into());
        wl.tickers = vec![ticker("AAPL", 0)];
        let mut cache = MarketDataCache::default();
        cache.insert(
            "AAPL".into(),
            MarketSnapshot {
                last_price: Some(184.32),
                change_pct: Some(-1.25),
                ..MarketSnapshot::default()
            },
        );
        let vm = WatchlistBodyVm::build(&wl, &cache, empty_thumbnail, false, None);
        assert_eq!(vm.rows[0].price_text, "184.32");
        assert_eq!(vm.rows[0].change_text, "-1.25%");
        // Negative change → red-ish.
        assert!(vm.rows[0].change_color.r > 0.5);
        assert!(vm.rows[0].change_color.g < 0.5);
    }

    #[test]
    fn vm_missing_market_data_uses_dashes() {
        let mut wl = WatchlistPanel::new(WatchlistId::new(1), "Main".into());
        wl.tickers = vec![ticker("AAPL", 0)];
        let cache = MarketDataCache::default();
        let vm = WatchlistBodyVm::build(&wl, &cache, empty_thumbnail, false, None);
        assert_eq!(vm.rows[0].price_text, "--");
        assert_eq!(vm.rows[0].change_text, "--");
        assert_eq!(vm.rows[0].gatr_text, "--");
    }

    #[test]
    fn vm_overlay_and_picker_flags_round_trip() {
        let wl = WatchlistPanel::new(WatchlistId::new(1), "Main".into());
        let cache = MarketDataCache::default();
        let on_resize = WatchlistBodyVm::build(&wl, &cache, empty_thumbnail, true, None);
        let on_picker = WatchlistBodyVm::build(
            &wl,
            &cache,
            empty_thumbnail,
            false,
            Some(LinkDimension::Symbol),
        );
        assert!(on_resize.show_resize_overlay && on_resize.link_picker_dim.is_none());
        assert!(!on_picker.show_resize_overlay && on_picker.link_picker_dim.is_some());
    }

    #[test]
    fn vm_width_helper_returns_grid_state_widths() {
        let wl = WatchlistPanel::new(WatchlistId::new(1), "Main".into());
        let cache = MarketDataCache::default();
        let vm = WatchlistBodyVm::build(&wl, &cache, empty_thumbnail, false, None);
        // Default widths from `default_column_widths()`. Just assert
        // both columns return non-zero — exact values are pinned in
        // the watchlist module's own tests.
        assert!(vm.width(COL_TICKER) > 0.0);
        assert!(vm.width(COL_PRICE) > 0.0);
    }
}
