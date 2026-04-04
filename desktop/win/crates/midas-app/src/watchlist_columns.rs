//! Column definitions for the watchlist grid.
//!
//! Defines [`WatchlistRow`] (the pre-computed row data) and
//! [`WatchlistColumn`] (a 7-variant enum implementing
//! [`midas_grid::GridColumn`]).  Together they let `midas-grid`
//! render the watchlist without knowing anything about market data
//! plumbing.

use iced::widget::{button, text, Space};
use iced::{Color, Element};
use std::cmp::Ordering;

use midas_core::WatchlistId;
use midas_grid::{Alignment, ColumnId, ColumnWidth, GridColumn};

use crate::app::Message;
use crate::watchlist::{COL_CHANGE, COL_DELETE, COL_DRAG, COL_FAV, COL_GATR, COL_PRICE, COL_TICKER};

// ── Colors ──────────────────────────────────────────────────────────

/// Positive change color (green).
const COLOR_POSITIVE: Color = Color::from_rgb(0.2, 0.8, 0.3);
/// Negative change color (red).
const COLOR_NEGATIVE: Color = Color::from_rgb(0.9, 0.25, 0.2);
/// Neutral / no-data color (grey).
const COLOR_NEUTRAL: Color = Color::from_rgb(0.6, 0.6, 0.6);

// ── Row data ────────────────────────────────────────────────────────

/// Pre-computed row data for the watchlist grid.
///
/// Built in `views.rs` from [`crate::watchlist::WatchlistTicker`]
/// plus market data lookups. Includes every value the grid needs
/// so that `cell()` can render without further lookups.
#[derive(Debug, Clone)]
pub struct WatchlistRow {
    /// Ticker symbol, always uppercase.
    pub symbol: String,
    /// Whether this ticker is marked as a favorite.
    pub favorite: bool,
    /// Formatted last price (e.g. `"182.63"`) or `"--"`.
    pub price_text: String,
    /// Formatted change percent (e.g. `"+1.25%"`) or `"--"`.
    pub change_text: String,
    /// Color for the change percent cell.
    pub change_color: Color,
    /// Formatted GATR value or `"--"`.
    pub gatr_text: String,
    /// Color for the GATR cell.
    pub gatr_color: Color,
    /// Watchlist this row belongs to (needed for message construction).
    pub wl_id: WatchlistId,
    /// Parsed last price for numeric sorting (`None` when no data).
    pub price_value: Option<f64>,
    /// Parsed change percent for numeric sorting (`None` when no data).
    pub change_value: Option<f64>,
}

// ── Column enum ─────────────────────────────────────────────────────

/// The seven columns of the watchlist grid.
///
/// Each variant maps to a `ColumnId` constant defined in
/// [`crate::watchlist`] and renders its own header + cell content.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WatchlistColumn {
    /// Drag grip for row reordering.
    DragHandle,
    /// Favorite star toggle.
    Favorite,
    /// Ticker symbol label.
    Ticker,
    /// Last traded price.
    Price,
    /// Daily change percent.
    ChangePercent,
    /// Generalized ATR value.
    GATR,
    /// Delete (remove from watchlist) button.
    Delete,
}

impl WatchlistColumn {
    /// All seven columns in their default display order.
    pub fn all() -> [WatchlistColumn; 7] {
        [
            WatchlistColumn::DragHandle,
            WatchlistColumn::Favorite,
            WatchlistColumn::Ticker,
            WatchlistColumn::Price,
            WatchlistColumn::ChangePercent,
            WatchlistColumn::GATR,
            WatchlistColumn::Delete,
        ]
    }
}

impl GridColumn<WatchlistRow, Message> for WatchlistColumn {
    fn id(&self) -> ColumnId {
        match self {
            Self::DragHandle => COL_DRAG,
            Self::Favorite => COL_FAV,
            Self::Ticker => COL_TICKER,
            Self::Price => COL_PRICE,
            Self::ChangePercent => COL_CHANGE,
            Self::GATR => COL_GATR,
            Self::Delete => COL_DELETE,
        }
    }

    fn header(&self) -> Element<'_, Message> {
        match self {
            Self::DragHandle => Space::new().into(),
            Self::Favorite => text("\u{2605}").size(12).into(),
            Self::Ticker => text("Ticker").size(12).into(),
            Self::Price => text("Price").size(12).into(),
            Self::ChangePercent => text("Chg%").size(12).into(),
            Self::GATR => text("G.ATR").size(12).into(),
            Self::Delete => Space::new().into(),
        }
    }

    fn cell<'a>(&'a self, row: &'a WatchlistRow, _row_index: usize) -> Element<'a, Message> {
        match self {
            Self::DragHandle => {
                button(text("\u{2807}").size(12))
                    .on_press(Message::WatchlistDragStart(
                        row.wl_id,
                        row.symbol.clone(),
                    ))
                    .padding([2, 4])
                    .style(hover_text_button_style)
                    .into()
            }
            Self::Favorite => {
                let star = if row.favorite { "\u{2605}" } else { "\u{2606}" };
                button(text(star).size(12))
                    .on_press(Message::WatchlistToggleFavorite(
                        row.wl_id,
                        row.symbol.clone(),
                    ))
                    .padding([2, 4])
                    .style(hover_text_button_style)
                    .into()
            }
            Self::Ticker => {
                text(&row.symbol).size(13).into()
            }
            Self::Price => {
                text(&row.price_text).size(13).into()
            }
            Self::ChangePercent => {
                text(&row.change_text).size(13).color(row.change_color).into()
            }
            Self::GATR => {
                text(&row.gatr_text).size(13).color(row.gatr_color).into()
            }
            Self::Delete => {
                button(text("\u{00D7}").size(12))
                    .on_press(Message::WatchlistRemoveTicker(
                        row.wl_id,
                        row.symbol.clone(),
                    ))
                    .padding([2, 4])
                    .style(hover_text_button_style)
                    .into()
            }
        }
    }

    fn width(&self) -> ColumnWidth {
        match self {
            Self::DragHandle => ColumnWidth::Fixed(26.0),
            Self::Favorite => ColumnWidth::Fixed(30.0),
            Self::Ticker => ColumnWidth::Flex(1.0),
            Self::Price => ColumnWidth::Flex(1.0),
            Self::ChangePercent => ColumnWidth::Flex(1.0),
            Self::GATR => ColumnWidth::Flex(1.0),
            Self::Delete => ColumnWidth::Fixed(30.0),
        }
    }

    fn min_width(&self) -> f32 {
        match self {
            Self::DragHandle => 26.0,
            Self::Favorite => 30.0,
            Self::Ticker => 40.0,
            Self::Price => 50.0,
            Self::ChangePercent => 45.0,
            Self::GATR => 45.0,
            Self::Delete => 30.0,
        }
    }

    fn resizable(&self) -> bool {
        matches!(
            self,
            Self::Ticker | Self::Price | Self::ChangePercent | Self::GATR
        )
    }

    fn sortable(&self) -> bool {
        matches!(
            self,
            Self::Ticker | Self::Price | Self::ChangePercent | Self::GATR
        )
    }

    fn reorderable(&self) -> bool {
        !matches!(self, Self::DragHandle | Self::Delete)
    }

    fn compare(&self, a: &WatchlistRow, b: &WatchlistRow) -> Ordering {
        match self {
            Self::Ticker => a.symbol.cmp(&b.symbol),
            Self::Price => cmp_option_f64(a.price_value, b.price_value),
            Self::ChangePercent => cmp_option_f64(a.change_value, b.change_value),
            Self::GATR => {
                // Parse GATR text for numeric comparison; fall back to
                // lexicographic if parse fails.
                let av = parse_gatr(&a.gatr_text);
                let bv = parse_gatr(&b.gatr_text);
                cmp_option_f64(av, bv)
            }
            // Non-sortable columns return Equal.
            _ => Ordering::Equal,
        }
    }

    fn align(&self) -> Alignment {
        match self {
            Self::Price | Self::ChangePercent | Self::GATR => Alignment::End,
            _ => Alignment::Start,
        }
    }
}

// ── Helpers ─────────────────────────────────────────────────────────

/// Compare two `Option<f64>` values, placing `None` after `Some`.
fn cmp_option_f64(a: Option<f64>, b: Option<f64>) -> Ordering {
    match (a, b) {
        (Some(av), Some(bv)) => av.partial_cmp(&bv).unwrap_or(Ordering::Equal),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

/// Attempt to parse a GATR display string into a numeric value.
///
/// Strips a trailing `%` if present (e.g. `"3.45%"` -> `3.45`).
/// Returns `None` for placeholder strings like `"--"`.
fn parse_gatr(s: &str) -> Option<f64> {
    let trimmed = s.trim().trim_end_matches('%');
    trimmed.parse::<f64>().ok()
}

/// Button style: muted text by default, white text + subtle background on hover.
///
/// Identical to the `hover_text_button_style` in `views.rs`, duplicated here
/// so that `watchlist_columns` can be used independently of the views module.
fn hover_text_button_style(_theme: &iced::Theme, status: button::Status) -> button::Style {
    let text_color = match status {
        button::Status::Hovered | button::Status::Pressed => Color::WHITE,
        _ => crate::theme::TEXT_MUTED,
    };
    let background = match status {
        button::Status::Hovered => Some(iced::Background::Color(Color::from_rgba(
            1.0, 1.0, 1.0, 0.1,
        ))),
        button::Status::Pressed => Some(iced::Background::Color(Color::from_rgba(
            1.0, 1.0, 1.0, 0.15,
        ))),
        _ => None,
    };
    button::Style {
        text_color,
        background,
        ..Default::default()
    }
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn all_returns_seven_columns() {
        assert_eq!(WatchlistColumn::all().len(), 7);
    }

    #[test]
    fn all_column_ids_are_unique() {
        let ids: HashSet<ColumnId> = WatchlistColumn::all().iter().map(|c| c.id()).collect();
        assert_eq!(ids.len(), 7, "every column must have a unique ColumnId");
    }

    #[test]
    fn sortable_columns() {
        let sortable: Vec<_> = WatchlistColumn::all()
            .into_iter()
            .filter(|c| c.sortable())
            .collect();
        assert_eq!(sortable.len(), 4);
        assert!(sortable.contains(&WatchlistColumn::Ticker));
        assert!(sortable.contains(&WatchlistColumn::Price));
        assert!(sortable.contains(&WatchlistColumn::ChangePercent));
        assert!(sortable.contains(&WatchlistColumn::GATR));
    }

    #[test]
    fn non_sortable_columns() {
        assert!(!WatchlistColumn::DragHandle.sortable());
        assert!(!WatchlistColumn::Favorite.sortable());
        assert!(!WatchlistColumn::Delete.sortable());
    }

    #[test]
    fn reorderable_columns() {
        assert!(!WatchlistColumn::DragHandle.reorderable());
        assert!(!WatchlistColumn::Delete.reorderable());
        // The rest should be reorderable.
        assert!(WatchlistColumn::Favorite.reorderable());
        assert!(WatchlistColumn::Ticker.reorderable());
        assert!(WatchlistColumn::Price.reorderable());
        assert!(WatchlistColumn::ChangePercent.reorderable());
        assert!(WatchlistColumn::GATR.reorderable());
    }

    #[test]
    fn resizable_matches_flex_columns() {
        for col in WatchlistColumn::all() {
            let is_flex = matches!(col.width(), ColumnWidth::Flex(_));
            assert_eq!(
                col.resizable(),
                is_flex,
                "resizable should match Flex width for {col:?}"
            );
        }
    }

    #[test]
    fn fixed_columns_have_correct_widths() {
        assert_eq!(WatchlistColumn::DragHandle.width(), ColumnWidth::Fixed(26.0));
        assert_eq!(WatchlistColumn::Favorite.width(), ColumnWidth::Fixed(30.0));
        assert_eq!(WatchlistColumn::Delete.width(), ColumnWidth::Fixed(30.0));
    }

    #[test]
    fn numeric_alignment() {
        assert_eq!(WatchlistColumn::Price.align(), Alignment::End);
        assert_eq!(WatchlistColumn::ChangePercent.align(), Alignment::End);
        assert_eq!(WatchlistColumn::GATR.align(), Alignment::End);
        assert_eq!(WatchlistColumn::Ticker.align(), Alignment::Start);
        assert_eq!(WatchlistColumn::DragHandle.align(), Alignment::Start);
    }

    #[test]
    fn compare_ticker_alphabetical() {
        let a = test_row("AAPL");
        let b = test_row("MSFT");
        assert_eq!(WatchlistColumn::Ticker.compare(&a, &b), Ordering::Less);
        assert_eq!(WatchlistColumn::Ticker.compare(&b, &a), Ordering::Greater);
        assert_eq!(WatchlistColumn::Ticker.compare(&a, &a), Ordering::Equal);
    }

    #[test]
    fn compare_price_numeric() {
        let a = test_row_with_price("AAPL", Some(150.0));
        let b = test_row_with_price("MSFT", Some(300.0));
        let none = test_row_with_price("???", None);
        assert_eq!(WatchlistColumn::Price.compare(&a, &b), Ordering::Less);
        assert_eq!(WatchlistColumn::Price.compare(&b, &a), Ordering::Greater);
        // None sorts after Some.
        assert_eq!(WatchlistColumn::Price.compare(&a, &none), Ordering::Less);
        assert_eq!(WatchlistColumn::Price.compare(&none, &a), Ordering::Greater);
    }

    #[test]
    fn compare_non_sortable_returns_equal() {
        let a = test_row("AAPL");
        let b = test_row("MSFT");
        assert_eq!(WatchlistColumn::DragHandle.compare(&a, &b), Ordering::Equal);
        assert_eq!(WatchlistColumn::Favorite.compare(&a, &b), Ordering::Equal);
        assert_eq!(WatchlistColumn::Delete.compare(&a, &b), Ordering::Equal);
    }

    #[test]
    fn parse_gatr_strips_percent() {
        assert_eq!(parse_gatr("3.45%"), Some(3.45));
        assert_eq!(parse_gatr("3.45"), Some(3.45));
        assert_eq!(parse_gatr("--"), None);
        assert_eq!(parse_gatr(""), None);
    }

    #[test]
    fn cmp_option_f64_ordering() {
        assert_eq!(cmp_option_f64(Some(1.0), Some(2.0)), Ordering::Less);
        assert_eq!(cmp_option_f64(Some(2.0), Some(1.0)), Ordering::Greater);
        assert_eq!(cmp_option_f64(Some(1.0), Some(1.0)), Ordering::Equal);
        assert_eq!(cmp_option_f64(Some(1.0), None), Ordering::Less);
        assert_eq!(cmp_option_f64(None, Some(1.0)), Ordering::Greater);
        assert_eq!(cmp_option_f64(None, None), Ordering::Equal);
    }

    // ── Test helpers ────────────────────────────────────────────────

    fn test_row(symbol: &str) -> WatchlistRow {
        WatchlistRow {
            symbol: symbol.to_owned(),
            favorite: false,
            price_text: "--".into(),
            change_text: "--".into(),
            change_color: COLOR_NEUTRAL,
            gatr_text: "--".into(),
            gatr_color: COLOR_NEUTRAL,
            wl_id: WatchlistId::new(1),
            price_value: None,
            change_value: None,
        }
    }

    fn test_row_with_price(symbol: &str, price: Option<f64>) -> WatchlistRow {
        WatchlistRow {
            symbol: symbol.to_owned(),
            favorite: false,
            price_text: price.map(|p| format!("{p:.2}")).unwrap_or("--".into()),
            change_text: "--".into(),
            change_color: COLOR_NEUTRAL,
            gatr_text: "--".into(),
            gatr_color: COLOR_NEUTRAL,
            wl_id: WatchlistId::new(1),
            price_value: price,
            change_value: None,
        }
    }
}
