//! Column definitions for the Orders (blotter) grid.
//!
//! Mirrors the Watchlist column pattern: a [`DisplayRow`] flat struct
//! + an `OrderBlotterColumn` enum implementing [`midas_grid::GridColumn`].

use std::cmp::Ordering;

use iced::widget::{container, row, text, text::Wrapping};
use iced::{Color, Element};

use midas_core::broker::{EntryKind, OrderAction, TimeInForce};
use midas_grid::{Alignment, ColumnId, ColumnWidth, GridColumn};

use crate::app::Message;

use super::{LegRole, OrderRow, OrderStatus};

// ── Column IDs (wire-stable; persisted into `column_widths` map) ─────

pub const COL_SYMBOL: ColumnId = ColumnId("symbol");
pub const COL_SIDE: ColumnId = ColumnId("side");
pub const COL_TYPE: ColumnId = ColumnId("type");
pub const COL_QTY: ColumnId = ColumnId("qty");
pub const COL_AVG_FILL: ColumnId = ColumnId("avg_fill");
pub const COL_LIMIT: ColumnId = ColumnId("limit");
pub const COL_STOP: ColumnId = ColumnId("stop");
pub const COL_TP: ColumnId = ColumnId("tp");
pub const COL_SL: ColumnId = ColumnId("sl");
pub const COL_STATUS: ColumnId = ColumnId("status");
pub const COL_LAST_UPDATE: ColumnId = ColumnId("last_update");
pub const COL_INSTRUCTION: ColumnId = ColumnId("instruction");
pub const COL_ORDER_ID: ColumnId = ColumnId("order_id");

// ── Colours ──────────────────────────────────────────────────────────

/// Side=Buy cell colour. Mid-blue from the target screenshot.
const SIDE_BUY: Color = Color::from_rgb(0.30, 0.54, 0.96);
/// Side=Sell cell colour. Desaturated red.
const SIDE_SELL: Color = Color::from_rgb(0.88, 0.31, 0.27);
/// Status=Filled green.
const STATUS_FILLED: Color = Color::from_rgb(0.27, 0.75, 0.47);
/// Status=Cancelled / Rejected amber.
const STATUS_WARN: Color = Color::from_rgb(0.91, 0.60, 0.26);
/// Status=Working / neutral.
const STATUS_NEUTRAL: Color = Color::from_rgb(0.78, 0.78, 0.78);
/// Symbol badge background (neutral grey; Side tint drives the pill).
const BADGE_BG: Color = Color::from_rgba(1.0, 1.0, 1.0, 0.08);

// ── DisplayRow ───────────────────────────────────────────────────────

/// Pre-computed, render-ready row data for the Orders grid.
///
/// Built once per blotter-generation change; avoids re-formatting on
/// every frame. `leg_role` and `kind` are retained for future filtering
/// and per-role cell styling — Slice 4 exposes the basics.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct DisplayRow {
    pub order_id: String,
    pub order_id_sort_key: u128,
    pub symbol: String,
    pub side: OrderAction,
    pub leg_role: LegRole,
    pub kind: EntryKind,
    pub kind_text: String,
    pub qty_text: String,
    pub qty_value: f64,
    pub avg_fill_text: String,
    pub avg_fill_value: Option<f64>,
    pub limit_text: String,
    pub limit_value: Option<f64>,
    pub stop_text: String,
    pub stop_value: Option<f64>,
    pub tp_text: String,
    pub tp_value: Option<f64>,
    pub sl_text: String,
    pub sl_value: Option<f64>,
    pub status: OrderStatus,
    pub last_update_text: String,
    pub last_update_sort_key: i64,
    pub instruction_text: String,
}

impl DisplayRow {
    pub fn from_row(row: &OrderRow) -> Self {
        let kind_text = match (row.kind, row.leg_role) {
            // For SL leg, plan calls "Stop" / "Stop Limit" in Type.
            (EntryKind::Stop, _) => "Stop Loss".to_owned(),
            (EntryKind::StopLimit, LegRole::StopLoss) => "Stop Limit".to_owned(),
            (EntryKind::StopLimit, _) => "Stop Limit".to_owned(),
            (EntryKind::Market, _) => "Market".to_owned(),
            (EntryKind::Limit, _) => "Limit".to_owned(),
        };
        Self {
            order_id: short_uuid(row.order_id),
            order_id_sort_key: row.order_id.as_u128(),
            symbol: row.symbol.clone(),
            side: row.side,
            leg_role: row.leg_role,
            kind: row.kind,
            kind_text,
            qty_text: format_qty(row.filled_qty.max(row.quantity)),
            qty_value: row.filled_qty.max(row.quantity),
            avg_fill_text: row.avg_fill_price.map(format_price).unwrap_or_default(),
            avg_fill_value: row.avg_fill_price,
            limit_text: row.limit_price.map(format_price).unwrap_or_default(),
            limit_value: row.limit_price,
            stop_text: row.stop_price.map(format_price).unwrap_or_default(),
            stop_value: row.stop_price,
            tp_text: row.tp_price.map(format_price).unwrap_or_default(),
            tp_value: row.tp_price,
            sl_text: row.sl_price.map(format_price).unwrap_or_default(),
            sl_value: row.sl_price,
            status: row.status,
            last_update_text: row.last_update_at.format("%Y-%m-%d %H:%M:%S").to_string(),
            last_update_sort_key: row.last_update_at.timestamp_millis(),
            instruction_text: format_tif(row.time_in_force),
        }
    }
}

fn short_uuid(id: uuid::Uuid) -> String {
    // Broker-assigned UIDs are long; show the last 10 hex chars which
    // is enough to disambiguate in a dev session. Full ID is in tooltips
    // (future growth).
    let s = id.simple().to_string();
    let tail = s.len().saturating_sub(10);
    s[tail..].to_owned()
}

fn format_qty(q: f64) -> String {
    if q == q.trunc() {
        format!("{}", q as i64)
    } else {
        format!("{q:.4}")
    }
}

fn format_price(p: f64) -> String {
    format!("{p:.2}")
}

fn format_tif(tif: Option<TimeInForce>) -> String {
    match tif {
        Some(TimeInForce::Day) => "Day",
        Some(TimeInForce::Gtc) => "Good Till Cancel",
        Some(TimeInForce::Ioc) => "IOC",
        Some(TimeInForce::Gtd) => "Good Till Date",
        Some(TimeInForce::Opg) => "At Open",
        None => "",
    }
    .to_owned()
}

// ── Column enum ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OrderBlotterColumn {
    Symbol,
    Side,
    Type,
    Qty,
    AvgFill,
    Limit,
    Stop,
    TakeProfit,
    StopLoss,
    Status,
    LastUpdate,
    Instruction,
    OrderId,
}

impl OrderBlotterColumn {
    pub const ALL: [OrderBlotterColumn; 13] = [
        Self::Symbol,
        Self::Side,
        Self::Type,
        Self::Qty,
        Self::AvgFill,
        Self::Limit,
        Self::Stop,
        Self::TakeProfit,
        Self::StopLoss,
        Self::Status,
        Self::LastUpdate,
        Self::Instruction,
        Self::OrderId,
    ];

    pub fn default_widths() -> Vec<(ColumnId, f32)> {
        vec![
            (COL_SYMBOL, 96.0),
            (COL_SIDE, 52.0),
            (COL_TYPE, 96.0),
            (COL_QTY, 72.0),
            (COL_AVG_FILL, 96.0),
            (COL_LIMIT, 96.0),
            (COL_STOP, 96.0),
            (COL_TP, 96.0),
            (COL_SL, 96.0),
            (COL_STATUS, 80.0),
            (COL_LAST_UPDATE, 140.0),
            (COL_INSTRUCTION, 140.0),
            (COL_ORDER_ID, 110.0),
        ]
    }

    pub fn ids() -> Vec<ColumnId> {
        Self::ALL.iter().map(|c| c.id()).collect()
    }
}

impl GridColumn<DisplayRow, Message> for OrderBlotterColumn {
    fn id(&self) -> ColumnId {
        match self {
            Self::Symbol => COL_SYMBOL,
            Self::Side => COL_SIDE,
            Self::Type => COL_TYPE,
            Self::Qty => COL_QTY,
            Self::AvgFill => COL_AVG_FILL,
            Self::Limit => COL_LIMIT,
            Self::Stop => COL_STOP,
            Self::TakeProfit => COL_TP,
            Self::StopLoss => COL_SL,
            Self::Status => COL_STATUS,
            Self::LastUpdate => COL_LAST_UPDATE,
            Self::Instruction => COL_INSTRUCTION,
            Self::OrderId => COL_ORDER_ID,
        }
    }

    fn header(&self) -> Element<'_, Message> {
        let label = match self {
            Self::Symbol => "Symbol",
            Self::Side => "Side",
            Self::Type => "Type",
            Self::Qty => "Qty",
            Self::AvgFill => "Avg Fill Price",
            Self::Limit => "Limit Price",
            Self::Stop => "Stop Price",
            Self::TakeProfit => "Take Profit",
            Self::StopLoss => "Stop Loss",
            Self::Status => "Status",
            Self::LastUpdate => "Last Update Time",
            Self::Instruction => "Instruction",
            Self::OrderId => "Order ID",
        };
        text(label).size(11).into()
    }

    fn cell<'a>(&'a self, row: &'a DisplayRow, _row_index: usize) -> Element<'a, Message> {
        match self {
            Self::Symbol => symbol_badge(row.symbol.clone(), row.side).into(),
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
            Self::Type => text(&row.kind_text)
                .size(12)
                .wrapping(Wrapping::None)
                .into(),
            Self::Qty => text(&row.qty_text).size(12).wrapping(Wrapping::None).into(),
            Self::AvgFill => text(&row.avg_fill_text)
                .size(12)
                .wrapping(Wrapping::None)
                .into(),
            Self::Limit => text(&row.limit_text)
                .size(12)
                .wrapping(Wrapping::None)
                .into(),
            Self::Stop => text(&row.stop_text)
                .size(12)
                .wrapping(Wrapping::None)
                .into(),
            Self::TakeProfit => text(&row.tp_text).size(12).wrapping(Wrapping::None).into(),
            Self::StopLoss => text(&row.sl_text).size(12).wrapping(Wrapping::None).into(),
            Self::Status => text(row.status.as_str())
                .size(12)
                .color(match row.status {
                    OrderStatus::Filled => STATUS_FILLED,
                    OrderStatus::Cancelled | OrderStatus::Rejected => STATUS_WARN,
                    _ => STATUS_NEUTRAL,
                })
                .wrapping(Wrapping::None)
                .into(),
            Self::LastUpdate => text(&row.last_update_text)
                .size(11)
                .wrapping(Wrapping::None)
                .into(),
            Self::Instruction => text(&row.instruction_text)
                .size(12)
                .wrapping(Wrapping::None)
                .into(),
            Self::OrderId => text(&row.order_id).size(11).wrapping(Wrapping::None).into(),
        }
    }

    fn width(&self) -> ColumnWidth {
        match self {
            Self::Symbol => ColumnWidth::Fixed(96.0),
            Self::Side => ColumnWidth::Fixed(52.0),
            Self::Type => ColumnWidth::Flex(1.0),
            Self::Qty => ColumnWidth::Flex(1.0),
            Self::AvgFill => ColumnWidth::Flex(1.2),
            Self::Limit => ColumnWidth::Flex(1.2),
            Self::Stop => ColumnWidth::Flex(1.2),
            Self::TakeProfit => ColumnWidth::Flex(1.2),
            Self::StopLoss => ColumnWidth::Flex(1.2),
            Self::Status => ColumnWidth::Fixed(80.0),
            Self::LastUpdate => ColumnWidth::Flex(1.5),
            Self::Instruction => ColumnWidth::Flex(1.3),
            Self::OrderId => ColumnWidth::Fixed(110.0),
        }
    }

    fn min_width(&self) -> f32 {
        match self {
            Self::Side | Self::Status => 48.0,
            Self::Symbol => 80.0,
            _ => 60.0,
        }
    }

    fn resizable(&self) -> bool {
        !matches!(self, Self::Side | Self::Status)
    }

    fn sortable(&self) -> bool {
        !matches!(self, Self::Symbol)
    }

    fn reorderable(&self) -> bool {
        true
    }

    fn compare(&self, a: &DisplayRow, b: &DisplayRow) -> Ordering {
        match self {
            Self::Side => side_rank(a.side).cmp(&side_rank(b.side)),
            Self::Type => a.kind_text.cmp(&b.kind_text),
            Self::Qty => a
                .qty_value
                .partial_cmp(&b.qty_value)
                .unwrap_or(Ordering::Equal),
            Self::AvgFill => cmp_opt_f64(a.avg_fill_value, b.avg_fill_value),
            Self::Limit => cmp_opt_f64(a.limit_value, b.limit_value),
            Self::Stop => cmp_opt_f64(a.stop_value, b.stop_value),
            Self::TakeProfit => cmp_opt_f64(a.tp_value, b.tp_value),
            Self::StopLoss => cmp_opt_f64(a.sl_value, b.sl_value),
            Self::Status => status_rank(a.status).cmp(&status_rank(b.status)),
            Self::LastUpdate => a.last_update_sort_key.cmp(&b.last_update_sort_key),
            Self::Instruction => a.instruction_text.cmp(&b.instruction_text),
            Self::OrderId => a.order_id_sort_key.cmp(&b.order_id_sort_key),
            Self::Symbol => a.symbol.cmp(&b.symbol),
        }
    }

    fn align(&self) -> Alignment {
        match self {
            Self::Symbol | Self::Side | Self::Type | Self::Status | Self::Instruction => {
                Alignment::Start
            }
            Self::Qty
            | Self::AvgFill
            | Self::Limit
            | Self::Stop
            | Self::TakeProfit
            | Self::StopLoss
            | Self::LastUpdate
            | Self::OrderId => Alignment::End,
        }
    }
}

fn cmp_opt_f64(a: Option<f64>, b: Option<f64>) -> Ordering {
    match (a, b) {
        (Some(av), Some(bv)) => av.partial_cmp(&bv).unwrap_or(Ordering::Equal),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
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

// ── Symbol badge widget ──────────────────────────────────────────────

/// Render a symbol badge: coloured pill with the ticker text.
///
/// Colour tint matches the row's `side` (blue for Buy, red for Sell) so
/// the Symbol column doubles as an at-a-glance side marker, matching the
/// target design.
pub fn symbol_badge(
    symbol: String,
    side: OrderAction,
) -> iced::widget::Container<'static, Message> {
    let tint = match side {
        OrderAction::Buy => SIDE_BUY,
        OrderAction::Sell => SIDE_SELL,
    };
    let border_color = tint;
    container(
        row![text(symbol).size(11).color(Color::WHITE)]
            .padding([2, 6])
            .spacing(4)
            .align_y(iced::Alignment::Center),
    )
    .style(move |_theme| container::Style {
        background: Some(iced::Background::Color(BADGE_BG)),
        border: iced::Border {
            color: border_color,
            width: 1.0,
            radius: 3.0.into(),
        },
        ..Default::default()
    })
}
