//! Column definitions for the Trade History tab.
//!
//! Six fixed-width read-only columns over terminal orders
//! (`Filled` / `Cancelled` / `Rejected`). Mirrors the pattern in
//! [`crate::order_blotter::columns`] but intentionally trimmed:
//! History is a log — users don't sort, resize, or act on rows in v1.
//!
//! Column widths are fixed (not persisted) per plan Decision 4:
//! only the Orders tab persists per-tab widths in v1.

use std::cmp::Ordering;

use chrono::{DateTime, Utc};
use iced::widget::{text, text::Wrapping};
use iced::{Color, Element};

use midas_broker::OrderAction;
use midas_grid::{Alignment, ColumnId, ColumnWidth, GridColumn};
use uuid::Uuid;

use crate::account_panel::AccountMsg;
use crate::order_blotter::{OrderRow, OrderStatus};

// ── Column IDs (stable; used by GridState::column_widths) ────────────

pub const COL_TIMESTAMP: ColumnId = ColumnId("history_timestamp");
pub const COL_SYMBOL: ColumnId = ColumnId("history_symbol");
pub const COL_SIDE: ColumnId = ColumnId("history_side");
pub const COL_QTY: ColumnId = ColumnId("history_qty");
pub const COL_FILL_PRICE: ColumnId = ColumnId("history_fill_price");
pub const COL_STATUS: ColumnId = ColumnId("history_status");

// ── Colours (mirror Orders tab so tints read as the same vocabulary) ─

const SIDE_BUY: Color = Color::from_rgb(0.30, 0.54, 0.96);
const SIDE_SELL: Color = Color::from_rgb(0.88, 0.31, 0.27);
const STATUS_FILLED: Color = Color::from_rgb(0.27, 0.75, 0.47);
const STATUS_WARN: Color = Color::from_rgb(0.91, 0.60, 0.26);

// ── DisplayRow ───────────────────────────────────────────────────────

/// Pre-computed, render-ready row for the History grid.
///
/// Built once per blotter-generation change and cached on the owning
/// [`super::history_tab::HistoryTab`]; avoids re-formatting on every
/// frame.
#[derive(Debug, Clone)]
#[allow(dead_code)] // `order_id` is reserved for post-v1 row-click wiring.
pub struct DisplayRow {
    /// Full broker-assigned Uuid for the order. Session-scoped selection
    /// key; History is read-only in v1 but the field is retained for
    /// future row-click wiring.
    pub order_id: Uuid,
    /// Time of the last state change on the order (fill / cancel /
    /// reject). Drives the default descending sort.
    pub timestamp: DateTime<Utc>,
    /// Rendered timestamp text (`"YYYY-MM-DD HH:MM:SS"`, UTC).
    pub timestamp_text: String,
    pub symbol: String,
    pub side: OrderAction,
    /// Formatted quantity — integer shares shown without decimals.
    pub qty_text: String,
    /// Formatted fill price — empty string when the leg never filled.
    pub fill_price_text: String,
    pub status: OrderStatus,
}

impl DisplayRow {
    /// Project a blotter row into a display row. Only makes sense for
    /// rows whose `status.is_terminal()` is true; caller is responsible
    /// for filtering.
    pub fn from_row(row: &OrderRow) -> Self {
        Self {
            order_id: row.order_id,
            timestamp: row.last_update_at,
            timestamp_text: row.last_update_at.format("%Y-%m-%d %H:%M:%S").to_string(),
            symbol: row.symbol.clone(),
            side: row.side,
            qty_text: format_qty(row.filled_qty.max(row.quantity)),
            fill_price_text: row
                .avg_fill_price
                .map(|p| format!("{p:.2}"))
                .unwrap_or_default(),
            status: row.status,
        }
    }
}

fn format_qty(q: f64) -> String {
    if q == q.trunc() {
        format!("{}", q as i64)
    } else {
        format!("{q:.4}")
    }
}

// ── Column enum ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HistoryColumn {
    Timestamp,
    Symbol,
    Side,
    Qty,
    FillPrice,
    Status,
}

impl HistoryColumn {
    pub const ALL: [HistoryColumn; 6] = [
        Self::Timestamp,
        Self::Symbol,
        Self::Side,
        Self::Qty,
        Self::FillPrice,
        Self::Status,
    ];

    /// `(id, width)` tuples to seed [`midas_grid::GridState`] on panel
    /// creation. Widths match the `width()` impl below.
    pub fn default_widths() -> Vec<(ColumnId, f32)> {
        vec![
            (COL_TIMESTAMP, 160.0),
            (COL_SYMBOL, 80.0),
            (COL_SIDE, 60.0),
            (COL_QTY, 80.0),
            (COL_FILL_PRICE, 100.0),
            (COL_STATUS, 100.0),
        ]
    }

    pub fn ids() -> Vec<ColumnId> {
        Self::ALL.iter().map(|c| c.id()).collect()
    }
}

impl GridColumn<DisplayRow, AccountMsg> for HistoryColumn {
    fn id(&self) -> ColumnId {
        match self {
            Self::Timestamp => COL_TIMESTAMP,
            Self::Symbol => COL_SYMBOL,
            Self::Side => COL_SIDE,
            Self::Qty => COL_QTY,
            Self::FillPrice => COL_FILL_PRICE,
            Self::Status => COL_STATUS,
        }
    }

    fn header(&self) -> Element<'_, AccountMsg> {
        let label = match self {
            Self::Timestamp => "Time",
            Self::Symbol => "Symbol",
            Self::Side => "Side",
            Self::Qty => "Qty",
            Self::FillPrice => "Fill Price",
            Self::Status => "Status",
        };
        text(label).size(11).into()
    }

    fn cell<'a>(&'a self, row: &'a DisplayRow, _row_index: usize) -> Element<'a, AccountMsg> {
        match self {
            Self::Timestamp => text(&row.timestamp_text)
                .size(11)
                .wrapping(Wrapping::None)
                .into(),
            Self::Symbol => text(&row.symbol).size(12).wrapping(Wrapping::None).into(),
            Self::Side => text(match row.side {
                OrderAction::Buy => "Buy",
                OrderAction::Sell => "Sell",
            })
            .size(12)
            .color(match row.side {
                OrderAction::Buy => SIDE_BUY,
                OrderAction::Sell => SIDE_SELL,
            })
            .wrapping(Wrapping::None)
            .into(),
            Self::Qty => text(&row.qty_text).size(12).wrapping(Wrapping::None).into(),
            Self::FillPrice => text(&row.fill_price_text)
                .size(12)
                .wrapping(Wrapping::None)
                .into(),
            Self::Status => text(row.status.as_str())
                .size(12)
                .color(match row.status {
                    OrderStatus::Filled => STATUS_FILLED,
                    OrderStatus::Cancelled | OrderStatus::Rejected => STATUS_WARN,
                    // Non-terminal statuses never appear in History, but
                    // fall through to a neutral colour defensively.
                    _ => Color::from_rgb(0.78, 0.78, 0.78),
                })
                .wrapping(Wrapping::None)
                .into(),
        }
    }

    fn width(&self) -> ColumnWidth {
        match self {
            Self::Timestamp => ColumnWidth::Fixed(160.0),
            Self::Symbol => ColumnWidth::Fixed(80.0),
            Self::Side => ColumnWidth::Fixed(60.0),
            Self::Qty => ColumnWidth::Fixed(80.0),
            Self::FillPrice => ColumnWidth::Fixed(100.0),
            Self::Status => ColumnWidth::Fixed(100.0),
        }
    }

    fn min_width(&self) -> f32 {
        match self {
            Self::Side | Self::Status => 48.0,
            _ => 60.0,
        }
    }

    fn resizable(&self) -> bool {
        // Runtime-resizable in v1 (widths live on `HistoryTab::grid_state`
        // and are NOT persisted to `AppConfig` — plan Decision 4).
        true
    }

    fn sortable(&self) -> bool {
        // Default descending-by-timestamp sort is fixed in v1.
        false
    }

    fn reorderable(&self) -> bool {
        false
    }

    fn compare(&self, a: &DisplayRow, b: &DisplayRow) -> Ordering {
        match self {
            Self::Timestamp => a.timestamp.cmp(&b.timestamp),
            Self::Symbol => a.symbol.cmp(&b.symbol),
            Self::Side => side_rank(a.side).cmp(&side_rank(b.side)),
            Self::Qty => a.qty_text.cmp(&b.qty_text),
            Self::FillPrice => a.fill_price_text.cmp(&b.fill_price_text),
            Self::Status => status_rank(a.status).cmp(&status_rank(b.status)),
        }
    }

    fn align(&self) -> Alignment {
        match self {
            Self::Timestamp | Self::Symbol | Self::Side | Self::Status => Alignment::Start,
            Self::Qty | Self::FillPrice => Alignment::End,
        }
    }
}

fn side_rank(s: OrderAction) -> u8 {
    match s {
        OrderAction::Buy => 0,
        OrderAction::Sell => 1,
    }
}

fn status_rank(s: OrderStatus) -> u8 {
    match s {
        OrderStatus::Working => 0,
        OrderStatus::PartiallyFilled => 1,
        OrderStatus::Filled => 2,
        OrderStatus::Cancelled => 3,
        OrderStatus::Rejected => 4,
    }
}
