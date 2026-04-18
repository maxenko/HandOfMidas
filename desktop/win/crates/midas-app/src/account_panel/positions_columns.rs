//! Column definitions for the Positions tab.
//!
//! Eleven fixed-width columns over the app-wide
//! [`crate::account_panel::PositionStore`]. Mirrors the pattern in
//! [`crate::account_panel::history_columns`] but extends the grid with
//! a trailing close-position action cell.
//!
//! Derived fields (Change %, Unrealized P/L, Market Value) are computed
//! in [`DisplayRow::from_raw`] from the raw position record plus the
//! most-recent last-price tick. Realized P/L and Daily P/L render as
//! em-dash always — no broker event feeds them per-symbol in v1 (plan
//! Decision 2 + Slice 4 probe).
//!
//! Widths are fixed (not persisted) per plan Decision 4: only the
//! Orders tab persists per-tab widths in v1.

use std::cmp::Ordering;

use iced::widget::{button, container, row, text, text::Wrapping};
use iced::{Background, Border, Color, Element};

use midas_grid::{Alignment, ColumnId, ColumnWidth, GridColumn};

use crate::account_panel::positions_store::PositionRaw;
use crate::account_panel::AccountMsg;

use super::positions_msg::PositionsMsg;

// ── Column IDs (stable; used by GridState::column_widths) ────────────

pub const COL_SYMBOL: ColumnId = ColumnId("positions_symbol");
pub const COL_SIDE: ColumnId = ColumnId("positions_side");
pub const COL_QTY: ColumnId = ColumnId("positions_qty");
pub const COL_AVG_PRICE: ColumnId = ColumnId("positions_avg_price");
pub const COL_LAST_PRICE: ColumnId = ColumnId("positions_last_price");
pub const COL_CHANGE_PCT: ColumnId = ColumnId("positions_change_pct");
pub const COL_UNREALIZED: ColumnId = ColumnId("positions_unrealized");
pub const COL_REALIZED: ColumnId = ColumnId("positions_realized");
pub const COL_DAILY: ColumnId = ColumnId("positions_daily");
pub const COL_MARKET_VALUE: ColumnId = ColumnId("positions_market_value");
pub const COL_CLOSE_ACTION: ColumnId = ColumnId("positions_close");

// ── Colours (mirror Orders/History so tints read as the same vocab) ──

/// Long (Buy) side tint — mid-blue.
const SIDE_LONG: Color = Color::from_rgb(0.30, 0.54, 0.96);
/// Short (Sell) side tint — desaturated red.
const SIDE_SHORT: Color = Color::from_rgb(0.88, 0.31, 0.27);
/// Positive change — green. Matches Orders' STATUS_FILLED.
const CHANGE_POS: Color = Color::from_rgb(0.27, 0.75, 0.47);
/// Negative change — red (reuses SIDE_SHORT for consistency).
const CHANGE_NEG: Color = SIDE_SHORT;
/// Neutral / em-dash cells.
const NEUTRAL_TEXT: Color = Color::from_rgb(0.78, 0.78, 0.78);
/// Symbol badge background.
const BADGE_BG: Color = Color::from_rgba(1.0, 1.0, 1.0, 0.08);

/// Rendered as "—" (em-dash) whenever the value is unavailable.
pub const EM_DASH: &str = "\u{2014}";

// ── DisplayRow ───────────────────────────────────────────────────────

/// Pre-computed, render-ready row data for the Positions grid.
///
/// Rebuilt per generation change on the owning
/// [`super::positions_tab::PositionsTab`].
#[derive(Debug, Clone, PartialEq)]
#[allow(dead_code)] // Some fields (`qty`, `avg_cost`, etc.) feed tests; kept for parity.
pub struct DisplayRow {
    pub symbol: String,
    /// Signed share count (the raw broker-side value).
    pub qty: f64,
    /// Average cost basis per share.
    pub avg_cost: f64,
    /// Last-trade price, `None` until the first Tick arrives.
    pub last_price: Option<f32>,
    /// Session-open price. `None` until the broker surfaces day-open.
    pub session_open_price: Option<f32>,
}

impl DisplayRow {
    /// Project a raw position record into a display row.
    pub fn from_raw(raw: &PositionRaw) -> Self {
        Self {
            symbol: raw.symbol.clone(),
            qty: raw.qty,
            avg_cost: raw.avg_cost,
            last_price: raw.last_price,
            session_open_price: raw.session_open_price,
        }
    }

    /// `true` if the position is long (qty > 0).
    pub fn is_long(&self) -> bool {
        self.qty > 0.0
    }

    /// Unsigned share count for display purposes.
    pub fn abs_qty(&self) -> f64 {
        self.qty.abs()
    }

    /// `qty * (last_price - avg_cost)`. `None` if `last_price` is
    /// missing. Sign convention: positive = profit regardless of side
    /// (short positions gain when price drops because `qty` is negative).
    pub fn unrealized_pnl(&self) -> Option<f64> {
        let lp = self.last_price? as f64;
        Some(self.qty * (lp - self.avg_cost))
    }

    /// `(last - session_open) / session_open * 100`. `None` if either
    /// input is missing or `session_open_price` is ~0 (avoids div-by-0).
    pub fn change_pct(&self) -> Option<f64> {
        let lp = self.last_price? as f64;
        let open = self.session_open_price? as f64;
        if open.abs() < f64::EPSILON {
            return None;
        }
        Some((lp - open) / open * 100.0)
    }

    /// `abs(qty) * last_price`. `None` if `last_price` is missing.
    pub fn market_value(&self) -> Option<f64> {
        let lp = self.last_price? as f64;
        Some(self.qty.abs() * lp)
    }
}

// ── Formatting helpers ───────────────────────────────────────────────

/// Format an integer-ish quantity (e.g. `10`, `300`) without decimals.
pub fn format_qty(q: f64) -> String {
    if q == q.trunc() {
        format!("{}", q as i64)
    } else {
        format!("{q:.4}")
    }
}

/// Format a price like `"150.00"` with 2 decimals. `None` → em-dash.
pub fn format_price_opt(p: Option<f32>) -> String {
    match p {
        Some(v) => format!("{v:.2}"),
        None => EM_DASH.to_owned(),
    }
}

/// Format a change percentage like `"+5.11%"` / `"-9.83%"`. `None` →
/// em-dash.
pub fn format_change_pct(pct: Option<f64>) -> String {
    match pct {
        Some(v) => format!("{v:+.2}%"),
        None => EM_DASH.to_owned(),
    }
}

/// Format an unrealized-P/L value like `"+327.00 usd"` / `"-971.46 usd"`.
/// `None` → em-dash.
pub fn format_pnl_usd(pnl: Option<f64>) -> String {
    match pnl {
        Some(v) => format!("{v:+.2} usd"),
        None => EM_DASH.to_owned(),
    }
}

/// Format a market value like `"3,052.31"` with thousand-separators.
/// `None` → em-dash.
pub fn format_market_value(mv: Option<f64>) -> String {
    let Some(v) = mv else {
        return EM_DASH.to_owned();
    };
    let negative = v < 0.0;
    let abs = v.abs();
    let whole = abs.trunc() as u128;
    let frac = (abs - abs.trunc()).mul_add(100.0, 0.5) as u64; // round half-up
    let frac = frac.min(99);
    let whole_str = thousands(whole);
    if negative {
        format!("-{whole_str}.{frac:02}")
    } else {
        format!("{whole_str}.{frac:02}")
    }
}

/// Insert thousand-separator commas into an unsigned integer.
fn thousands(mut n: u128) -> String {
    if n == 0 {
        return "0".to_owned();
    }
    let mut parts: Vec<String> = Vec::new();
    while n > 0 {
        let chunk = (n % 1000) as u16;
        n /= 1000;
        if n > 0 {
            parts.push(format!("{chunk:03}"));
        } else {
            parts.push(format!("{chunk}"));
        }
    }
    parts.reverse();
    parts.join(",")
}

// ── Column enum ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PositionsColumn {
    Symbol,
    Side,
    Qty,
    AvgPrice,
    LastPrice,
    ChangePct,
    UnrealizedPnl,
    RealizedPnl,
    DailyPnl,
    MarketValue,
    CloseAction,
}

impl PositionsColumn {
    pub const ALL: [PositionsColumn; 11] = [
        Self::Symbol,
        Self::Side,
        Self::Qty,
        Self::AvgPrice,
        Self::LastPrice,
        Self::ChangePct,
        Self::UnrealizedPnl,
        Self::RealizedPnl,
        Self::DailyPnl,
        Self::MarketValue,
        Self::CloseAction,
    ];

    /// `(id, width)` tuples to seed [`midas_grid::GridState`] on panel
    /// creation. Widths match the `width()` impl below.
    pub fn default_widths() -> Vec<(ColumnId, f32)> {
        vec![
            (COL_SYMBOL, 96.0),
            (COL_SIDE, 60.0),
            (COL_QTY, 70.0),
            (COL_AVG_PRICE, 90.0),
            (COL_LAST_PRICE, 90.0),
            (COL_CHANGE_PCT, 90.0),
            (COL_UNREALIZED, 120.0),
            (COL_REALIZED, 100.0),
            (COL_DAILY, 100.0),
            (COL_MARKET_VALUE, 110.0),
            (COL_CLOSE_ACTION, 44.0),
        ]
    }

    pub fn ids() -> Vec<ColumnId> {
        Self::ALL.iter().map(|c| c.id()).collect()
    }
}

impl GridColumn<DisplayRow, AccountMsg> for PositionsColumn {
    fn id(&self) -> ColumnId {
        match self {
            Self::Symbol => COL_SYMBOL,
            Self::Side => COL_SIDE,
            Self::Qty => COL_QTY,
            Self::AvgPrice => COL_AVG_PRICE,
            Self::LastPrice => COL_LAST_PRICE,
            Self::ChangePct => COL_CHANGE_PCT,
            Self::UnrealizedPnl => COL_UNREALIZED,
            Self::RealizedPnl => COL_REALIZED,
            Self::DailyPnl => COL_DAILY,
            Self::MarketValue => COL_MARKET_VALUE,
            Self::CloseAction => COL_CLOSE_ACTION,
        }
    }

    fn header(&self) -> Element<'_, AccountMsg> {
        let label = match self {
            Self::Symbol => "Symbol",
            Self::Side => "Side",
            Self::Qty => "Qty",
            Self::AvgPrice => "Avg Price",
            Self::LastPrice => "Last",
            Self::ChangePct => "Change %",
            Self::UnrealizedPnl => "Unrealized P/L",
            Self::RealizedPnl => "Realized P/L",
            Self::DailyPnl => "Daily P/L",
            Self::MarketValue => "Market Value",
            // Close action has no header label — a single-character
            // column header would clash with the "×" cell glyph.
            Self::CloseAction => "",
        };
        text(label).size(11).into()
    }

    fn cell<'a>(&'a self, row: &'a DisplayRow, _row_index: usize) -> Element<'a, AccountMsg> {
        match self {
            Self::Symbol => symbol_badge(&row.symbol, row.is_long()).into(),
            Self::Side => {
                let (label, color) = if row.is_long() {
                    ("Long", SIDE_LONG)
                } else {
                    ("Short", SIDE_SHORT)
                };
                text(label)
                    .size(12)
                    .color(color)
                    .wrapping(Wrapping::None)
                    .into()
            }
            Self::Qty => text(format_qty(row.abs_qty()))
                .size(12)
                .wrapping(Wrapping::None)
                .into(),
            Self::AvgPrice => text(format!("{:.2}", row.avg_cost))
                .size(12)
                .wrapping(Wrapping::None)
                .into(),
            Self::LastPrice => text(format_price_opt(row.last_price))
                .size(12)
                .color(if row.last_price.is_some() {
                    Color::WHITE
                } else {
                    NEUTRAL_TEXT
                })
                .wrapping(Wrapping::None)
                .into(),
            Self::ChangePct => {
                let pct = row.change_pct();
                let color = pct_color(pct);
                text(format_change_pct(pct))
                    .size(12)
                    .color(color)
                    .wrapping(Wrapping::None)
                    .into()
            }
            Self::UnrealizedPnl => {
                let pnl = row.unrealized_pnl();
                let color = pct_color(pnl);
                text(format_pnl_usd(pnl))
                    .size(12)
                    .color(color)
                    .wrapping(Wrapping::None)
                    .into()
            }
            // Realized / Daily P/L always render em-dash in v1.
            Self::RealizedPnl | Self::DailyPnl => text(EM_DASH)
                .size(12)
                .color(NEUTRAL_TEXT)
                .wrapping(Wrapping::None)
                .into(),
            Self::MarketValue => text(format_market_value(row.market_value()))
                .size(12)
                .color(if row.last_price.is_some() {
                    Color::WHITE
                } else {
                    NEUTRAL_TEXT
                })
                .wrapping(Wrapping::None)
                .into(),
            // Close-X cell: the CELL is the action. Opacity + tooltip
            // are applied by the tab view (which knows the broker state);
            // a `&DisplayRow` has no view of broker connectivity, so the
            // button here always emits the message.
            Self::CloseAction => close_x_button(&row.symbol, /* connected = */ true).into(),
        }
    }

    fn width(&self) -> ColumnWidth {
        match self {
            Self::Symbol => ColumnWidth::Fixed(96.0),
            Self::Side => ColumnWidth::Fixed(60.0),
            Self::Qty => ColumnWidth::Fixed(70.0),
            Self::AvgPrice => ColumnWidth::Fixed(90.0),
            Self::LastPrice => ColumnWidth::Fixed(90.0),
            Self::ChangePct => ColumnWidth::Fixed(90.0),
            Self::UnrealizedPnl => ColumnWidth::Fixed(120.0),
            Self::RealizedPnl => ColumnWidth::Fixed(100.0),
            Self::DailyPnl => ColumnWidth::Fixed(100.0),
            Self::MarketValue => ColumnWidth::Fixed(110.0),
            Self::CloseAction => ColumnWidth::Fixed(44.0),
        }
    }

    fn min_width(&self) -> f32 {
        match self {
            Self::Side | Self::CloseAction => 40.0,
            _ => 60.0,
        }
    }

    fn resizable(&self) -> bool {
        // v1 does not persist Positions widths — keep the chrome static.
        false
    }

    fn sortable(&self) -> bool {
        // Default symbol-ascending sort is fixed in v1.
        false
    }

    fn reorderable(&self) -> bool {
        false
    }

    fn compare(&self, a: &DisplayRow, b: &DisplayRow) -> Ordering {
        match self {
            Self::Symbol => a.symbol.cmp(&b.symbol),
            Self::Side => side_rank(a.is_long()).cmp(&side_rank(b.is_long())),
            Self::Qty => a.qty.partial_cmp(&b.qty).unwrap_or(Ordering::Equal),
            Self::AvgPrice => a
                .avg_cost
                .partial_cmp(&b.avg_cost)
                .unwrap_or(Ordering::Equal),
            Self::LastPrice => cmp_opt_f32(a.last_price, b.last_price),
            Self::ChangePct => cmp_opt_f64(a.change_pct(), b.change_pct()),
            Self::UnrealizedPnl => cmp_opt_f64(a.unrealized_pnl(), b.unrealized_pnl()),
            Self::RealizedPnl | Self::DailyPnl | Self::CloseAction => Ordering::Equal,
            Self::MarketValue => cmp_opt_f64(a.market_value(), b.market_value()),
        }
    }

    fn align(&self) -> Alignment {
        match self {
            Self::Symbol | Self::Side => Alignment::Start,
            Self::CloseAction => Alignment::Center,
            _ => Alignment::End,
        }
    }
}

// ── Helpers ──────────────────────────────────────────────────────────

fn cmp_opt_f64(a: Option<f64>, b: Option<f64>) -> Ordering {
    match (a, b) {
        (Some(av), Some(bv)) => av.partial_cmp(&bv).unwrap_or(Ordering::Equal),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

fn cmp_opt_f32(a: Option<f32>, b: Option<f32>) -> Ordering {
    match (a, b) {
        (Some(av), Some(bv)) => av.partial_cmp(&bv).unwrap_or(Ordering::Equal),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

fn side_rank(is_long: bool) -> u8 {
    if is_long {
        0
    } else {
        1
    }
}

/// Colour for a signed value — green when positive, red when negative,
/// neutral when unavailable or exactly zero.
fn pct_color(v: Option<f64>) -> Color {
    match v {
        Some(x) if x > 0.0 => CHANGE_POS,
        Some(x) if x < 0.0 => CHANGE_NEG,
        _ => NEUTRAL_TEXT,
    }
}

/// Coloured symbol badge matching the Orders-tab pattern. Tint reflects
/// position side: blue for Long, red for Short.
fn symbol_badge(symbol: &str, is_long: bool) -> iced::widget::Container<'static, AccountMsg> {
    let tint = if is_long { SIDE_LONG } else { SIDE_SHORT };
    container(
        row![text(symbol.to_owned()).size(11).color(Color::WHITE)]
            .padding([2, 6])
            .spacing(4)
            .align_y(iced::Alignment::Center),
    )
    .style(move |_theme| container::Style {
        background: Some(Background::Color(BADGE_BG)),
        border: Border {
            color: tint,
            width: 1.0,
            radius: 3.0.into(),
        },
        ..Default::default()
    })
}

/// Close-position "×" button cell.
///
/// Always emits `PositionsMsg::CloseRequested(symbol)` when clicked —
/// the handler-level connection guard is authoritative. When
/// `connected` is `false` the button renders at 40% alpha to communicate
/// disabled state; the tab view layer is responsible for wrapping the
/// button in a tooltip because `Tooltip` needs `&UiTheme` which a
/// `GridColumn::cell` impl cannot reach.
pub fn close_x_button(symbol: &str, connected: bool) -> iced::widget::Button<'static, AccountMsg> {
    let owned = symbol.to_owned();
    let alpha = if connected { 1.0 } else { 0.4 };
    let glyph_color = Color {
        a: alpha,
        ..Color::WHITE
    };
    button(
        text("\u{00D7}")
            .size(14)
            .color(glyph_color)
            .wrapping(Wrapping::None),
    )
    .padding([0, 6])
    .on_press(AccountMsg::Positions(PositionsMsg::CloseRequested(owned)))
    .style(move |_theme, _status| iced::widget::button::Style {
        background: None,
        text_color: glyph_color,
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row_with(
        symbol: &str,
        qty: f64,
        avg: f64,
        last: Option<f32>,
        open: Option<f32>,
    ) -> DisplayRow {
        DisplayRow {
            symbol: symbol.to_owned(),
            qty,
            avg_cost: avg,
            last_price: last,
            session_open_price: open,
        }
    }

    // ── Math derivations ────────────────────────────────────────────

    #[test]
    fn is_long_and_abs_qty_track_qty_sign() {
        let long = row_with("AAPL", 10.0, 150.0, None, None);
        let short = row_with("GME", -5.0, 20.0, None, None);
        assert!(long.is_long());
        assert!(!short.is_long());
        assert_eq!(long.abs_qty(), 10.0);
        assert_eq!(short.abs_qty(), 5.0);
    }

    #[test]
    fn unrealized_pnl_long_positive_when_last_above_cost() {
        let r = row_with("AAPL", 10.0, 100.0, Some(110.0), None);
        // 10 * (110 - 100) = +100
        assert_eq!(r.unrealized_pnl(), Some(100.0));
    }

    #[test]
    fn unrealized_pnl_short_positive_when_last_below_cost() {
        let r = row_with("GME", -5.0, 20.0, Some(15.0), None);
        // -5 * (15 - 20) = -5 * -5 = +25 (short gains when price drops)
        assert_eq!(r.unrealized_pnl(), Some(25.0));
    }

    #[test]
    fn unrealized_pnl_none_without_last_price() {
        let r = row_with("AAPL", 10.0, 100.0, None, None);
        assert_eq!(r.unrealized_pnl(), None);
    }

    #[test]
    fn change_pct_requires_both_last_and_open() {
        let full = row_with("AAPL", 10.0, 100.0, Some(105.0), Some(100.0));
        assert_eq!(full.change_pct(), Some(5.0));

        let missing_open = row_with("AAPL", 10.0, 100.0, Some(105.0), None);
        assert_eq!(missing_open.change_pct(), None);

        let missing_last = row_with("AAPL", 10.0, 100.0, None, Some(100.0));
        assert_eq!(missing_last.change_pct(), None);
    }

    #[test]
    fn change_pct_handles_zero_open_without_infinity() {
        let r = row_with("AAPL", 10.0, 100.0, Some(105.0), Some(0.0));
        assert_eq!(r.change_pct(), None);
    }

    #[test]
    fn market_value_uses_abs_qty() {
        let long = row_with("AAPL", 10.0, 100.0, Some(150.0), None);
        let short = row_with("GME", -5.0, 20.0, Some(18.0), None);
        assert_eq!(long.market_value(), Some(1500.0));
        assert_eq!(short.market_value(), Some(90.0));
    }

    #[test]
    fn market_value_none_without_last_price() {
        let r = row_with("AAPL", 10.0, 100.0, None, None);
        assert_eq!(r.market_value(), None);
    }

    // ── Formatting helpers ──────────────────────────────────────────

    #[test]
    fn format_qty_integer_has_no_decimals() {
        assert_eq!(format_qty(10.0), "10");
        assert_eq!(format_qty(300.0), "300");
    }

    #[test]
    fn format_qty_fractional_uses_four_decimals() {
        assert_eq!(format_qty(10.5), "10.5000");
    }

    #[test]
    fn format_price_opt_none_is_em_dash() {
        assert_eq!(format_price_opt(None), EM_DASH);
        assert_eq!(format_price_opt(Some(150.0)), "150.00");
    }

    #[test]
    fn format_change_pct_signs_both_directions() {
        assert_eq!(format_change_pct(Some(5.11)), "+5.11%");
        assert_eq!(format_change_pct(Some(-9.83)), "-9.83%");
        assert_eq!(format_change_pct(None), EM_DASH);
    }

    #[test]
    fn format_pnl_usd_signs_and_suffix() {
        assert_eq!(format_pnl_usd(Some(327.0)), "+327.00 usd");
        assert_eq!(format_pnl_usd(Some(-971.46)), "-971.46 usd");
        assert_eq!(format_pnl_usd(None), EM_DASH);
    }

    #[test]
    fn format_market_value_uses_thousand_separators() {
        assert_eq!(format_market_value(Some(3_052.31)), "3,052.31");
        assert_eq!(format_market_value(Some(12_345_678.90)), "12,345,678.90");
        assert_eq!(format_market_value(Some(0.0)), "0.00");
        assert_eq!(format_market_value(Some(999.99)), "999.99");
    }

    #[test]
    fn format_market_value_none_is_em_dash() {
        assert_eq!(format_market_value(None), EM_DASH);
    }

    #[test]
    fn thousands_helper_zero_small_and_large() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(5), "5");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1_000), "1,000");
        assert_eq!(thousands(1_234_567), "1,234,567");
    }

    #[test]
    fn pct_color_maps_sign_to_tint() {
        assert_eq!(pct_color(Some(1.0)), CHANGE_POS);
        assert_eq!(pct_color(Some(-1.0)), CHANGE_NEG);
        assert_eq!(pct_color(Some(0.0)), NEUTRAL_TEXT);
        assert_eq!(pct_color(None), NEUTRAL_TEXT);
    }

    #[test]
    fn positions_column_ids_are_unique() {
        let ids: Vec<&str> = PositionsColumn::ALL.iter().map(|c| c.id().0).collect();
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(
            sorted.len(),
            ids.len(),
            "column IDs must be unique so GridState::column_widths maps cleanly"
        );
    }

    #[test]
    fn default_widths_cover_every_column() {
        let widths = PositionsColumn::default_widths();
        assert_eq!(widths.len(), PositionsColumn::ALL.len());
    }
}
