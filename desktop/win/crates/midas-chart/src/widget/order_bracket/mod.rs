//! Order bracket annotation: entry + optional TP/SL.
//!
//! A compound annotation representing a trade idea. The chart crate
//! sees these as pure visual geometry. The app layer maps them to
//! broker orders.

use super::level::LineStyle;
use serde::{Deserialize, Serialize};

/// Entry order type for a bracket. Defined in midas-chart (not broker)
/// to maintain sans-IO boundary. The app layer maps `EntryType` →
/// broker `OrderKind` at the bridge.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum EntryType {
    /// Market order — entry tracks last price, not draggable.
    #[default]
    Market,
    /// Limit order — entry at user-specified limit price.
    Limit,
    /// Stop order — entry at user-specified stop trigger price.
    Stop,
    /// Stop-Limit order — stop trigger + limit execution price.
    StopLimit,
}

/// An order bracket: entry line + optional take-profit and stop-loss.
///
/// The chart crate uses `BracketStatus` for visual styling only.
/// The app layer maps brackets to broker order instances.
///
/// # `entry.price` semantics by `entry_type`
///
/// - **Market / Limit**: target fill price (exact for risk calculations).
/// - **Stop**: trigger price (fill is approximate — market at trigger).
/// - **StopLimit**: limit execution price; the stop trigger lives in
///   `entry_stop_price`. Risk calculations use `entry.price` as-is,
///   which is exact for Limit and approximate for Stop (acceptable V1).
// TODO: consider EntryPrice enum to make per-type semantics compiler-enforced
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OrderBracket {
    /// The entry price line. Always present.
    pub entry: BracketLeg,
    /// Take-profit target. None if user hasn't set one yet.
    pub take_profit: Option<BracketLeg>,
    /// Stop-loss level. None if user hasn't set one yet.
    pub stop_loss: Option<BracketLeg>,
    /// Trade direction. Determines which side TP/SL go on.
    pub side: BracketSide,
    /// Visual status. Used for styling only in chart crate.
    pub status: BracketStatus,
    /// Display quantity (informational label, not order routing).
    pub quantity: Option<f64>,
    /// Whether the bracket has been explicitly saved/pinned by the user.
    /// Saved drafts survive [X] toggle and render at higher alpha.
    #[serde(default)]
    pub saved: bool,
    /// Quantity filled so far. Set by the app layer from execution
    /// reports. Used by `format_entry_label()` for partial-fill display.
    #[serde(default)]
    pub filled_qty: Option<f64>,
    /// Entry order type. Defaults to Market for backward compat.
    #[serde(default)]
    pub entry_type: EntryType,
    /// Stop trigger price for StopLimit entries. `None` for all other types.
    /// When `entry_type == StopLimit`, `entry.price` is the limit price
    /// and this field holds the stop trigger price.
    #[serde(default)]
    pub entry_stop_price: Option<f64>,
    /// True when the entry price is on the "wrong" side of market
    /// (e.g., Limit BUY above ask). Set by the app layer; rendered
    /// as amber warning on the entry label by the chart.
    #[serde(default)]
    pub wrong_side_warning: bool,
}

/// A single leg of an order bracket.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BracketLeg {
    /// Price level for this leg.
    pub price: f64,
    /// Optional time anchor. None = full-width ray from left edge.
    pub timestamp: Option<i64>,
    /// Override color. If None, derived from BracketSide + leg role.
    pub color: Option<[f32; 4]>,
    /// Line style for this leg.
    pub style: LineStyle,
    /// Line thickness in logical pixels.
    pub line_width: f32,
    /// Text shown next to the price label.
    pub label: Option<String>,
    /// Projected dollar P&L at this leg's price level.
    /// Computed by the app layer using entry fill price and quantity.
    #[serde(default)]
    pub projected_pnl: Option<f64>,
    /// Projected percentage P&L.
    #[serde(default)]
    pub projected_pnl_pct: Option<f64>,
}

/// Trade direction for a bracket.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BracketSide {
    /// Long position: entry below TP, above SL.
    Long,
    /// Short position: entry above TP, below SL.
    Short,
}

/// Visual status of a bracket. Drives line style and opacity.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum BracketStatus {
    /// Being drawn on chart, not yet actionable. Dashed lines.
    #[default]
    Draft,
    /// Submitted to broker, awaiting entry fill. Dotted lines.
    Pending,
    /// Entry partially filled.
    PartialFill,
    /// Entry filled, TP/SL orders live at broker. Solid lines.
    Active,
    /// TP or SL triggered, position closed. Dimmed solid lines.
    Closed,
    /// User or broker cancelled. Dimmed solid lines.
    Cancelled,
}

/// Chart-local leg role enum. Defined in midas-chart — NOT imported from
/// midas-broker. The chart crate must not depend on broker types.
/// The app layer maps BracketRole (broker) to LegRole (chart) when
/// creating annotations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LegRole {
    Entry,
    TakeProfit,
    StopLoss,
}

impl OrderBracket {
    /// Compute risk:reward ratio. Returns None if TP or SL is missing,
    /// or if risk is effectively zero.
    pub fn risk_reward(&self) -> Option<f64> {
        let tp = self.take_profit.as_ref()?;
        let sl = self.stop_loss.as_ref()?;
        let risk = (self.entry.price - sl.price).abs();
        let reward = (tp.price - self.entry.price).abs();
        if risk < f64::EPSILON {
            return None;
        }
        Some(reward / risk)
    }

    /// Absolute dollar risk. Returns None if SL is missing or qty is zero.
    pub fn dollar_risk(&self) -> Option<f64> {
        let sl = self.stop_loss.as_ref()?;
        let qty = self.quantity?;
        Some((self.entry.price - sl.price).abs() * qty)
    }

    /// Absolute dollar reward. Returns None if TP is missing or qty is zero.
    pub fn dollar_reward(&self) -> Option<f64> {
        let tp = self.take_profit.as_ref()?;
        let qty = self.quantity?;
        Some((tp.price - self.entry.price).abs() * qty)
    }
}

// ── Default bracket colors (RGBA, linear space) ──────────────────────

/// Green take-profit line.
const BRACKET_TP_COLOR: [f32; 4] = [0.20, 0.78, 0.35, 1.0];
/// Red stop-loss line.
const BRACKET_SL_COLOR: [f32; 4] = [0.90, 0.25, 0.25, 1.0];
/// Green entry line for long positions.
const BRACKET_LONG_ENTRY_COLOR: [f32; 4] = [0.20, 0.78, 0.35, 1.0];
/// Red entry line for short positions.
const BRACKET_SHORT_ENTRY_COLOR: [f32; 4] = [0.90, 0.25, 0.25, 1.0];
/// Green zone fill at 6% alpha (between entry and TP).
const BRACKET_TP_ZONE: [f32; 4] = [0.20, 0.78, 0.35, 0.06];
/// Red zone fill at 6% alpha (between entry and SL).
const BRACKET_SL_ZONE: [f32; 4] = [0.90, 0.25, 0.25, 0.06];

// ── Phase 3: chart rendering helpers ─────────────────────────────────

impl OrderBracket {
    /// Compute line style for a leg based on bracket status.
    ///
    /// Returns `(LineStyle, line_width, color)`. The base color comes
    /// from the leg role: entry uses side-colored green (Long) or red
    /// (Short), TP = green, SL = red. The bracket status modulates
    /// line style, width, and alpha. Draft brackets additionally
    /// distinguish saved (0.65) from unsaved (0.50).
    pub fn leg_style(&self, role: LegRole) -> (LineStyle, f32, [f32; 4]) {
        let base_color = match role {
            LegRole::Entry => match self.side {
                BracketSide::Long => BRACKET_LONG_ENTRY_COLOR,
                BracketSide::Short => BRACKET_SHORT_ENTRY_COLOR,
            },
            LegRole::TakeProfit => BRACKET_TP_COLOR,
            LegRole::StopLoss => BRACKET_SL_COLOR,
        };

        let (style, width, alpha_mult) = match self.status {
            BracketStatus::Draft => {
                let alpha = if self.saved { 0.65 } else { 0.50 };
                (
                    LineStyle::Dashed {
                        dash_len: 6.0,
                        gap_len: 4.0,
                    },
                    1.0,
                    alpha,
                )
            }
            BracketStatus::Pending => (LineStyle::Dotted { dot_spacing: 4.0 }, 1.0, 0.80),
            BracketStatus::PartialFill => (LineStyle::Solid, 1.5, 0.90),
            BracketStatus::Active => (LineStyle::Solid, 1.5, 1.0),
            BracketStatus::Closed => (LineStyle::Solid, 1.0, 0.30),
            BracketStatus::Cancelled => (LineStyle::Solid, 1.0, 0.20),
        };

        let mut color = base_color;
        color[3] *= alpha_mult;
        (style, width, color)
    }
}

/// Compute zone fill rectangles for the TP and SL regions of a bracket.
///
/// Zone fills are only drawn when the bracket is `Active`. Each zone is a
/// translucent rectangle spanning from the entry price to the TP or SL
/// price across the full chart width.
///
/// Returns a `Vec` of `(rect, color)` tuples where `rect` is
/// `[left, top, right, bottom]` in screen pixels.
pub fn bracket_zone_rects(
    bracket: &OrderBracket,
    entry_y: f32,
    chart_width: f32,
    price_to_y: impl Fn(f64) -> f32,
) -> Vec<([f32; 4], [f32; 4])> {
    if bracket.status != BracketStatus::Active && bracket.status != BracketStatus::PartialFill {
        return Vec::new();
    }

    let mut zones = Vec::new();

    if let Some(ref tp) = bracket.take_profit {
        let tp_y = price_to_y(tp.price);
        let top = entry_y.min(tp_y);
        let bottom = entry_y.max(tp_y);
        zones.push(([0.0, top, chart_width, bottom], BRACKET_TP_ZONE));
    }

    if let Some(ref sl) = bracket.stop_loss {
        let sl_y = price_to_y(sl.price);
        let top = entry_y.min(sl_y);
        let bottom = entry_y.max(sl_y);
        zones.push(([0.0, top, chart_width, bottom], BRACKET_SL_ZONE));
    }

    zones
}

// ── Label formatting helpers ─────────────────────────────────────────

/// Entry type label prefix. Market has no prefix (backward compat).
fn entry_type_prefix(entry_type: EntryType) -> &'static str {
    match entry_type {
        EntryType::Market => "",
        EntryType::Limit => "LMT ",
        EntryType::Stop => "STP ",
        EntryType::StopLimit => "STP LMT ",
    }
}

/// Format the price portion of the entry label.
///
/// For StopLimit, shows `"stop/limit"` (e.g., `"185.00/184.50"`).
/// For all other types, shows the single entry price.
fn format_entry_price(bracket: &OrderBracket) -> String {
    if bracket.entry_type == EntryType::StopLimit {
        if let Some(stop_price) = bracket.entry_stop_price {
            return format!("{:.2}/{:.2}", stop_price, bracket.entry.price);
        }
    }
    format!("{:.2}", bracket.entry.price)
}

/// Format entry label based on bracket status, side, and entry type.
///
/// - **Draft**: `"BUY @ 171.59"` / `"LMT BUY @ 180.00"` / `"STP LMT BUY @ 185.00/184.50"`
/// - **Pending**: `"BUY @ 171.59  ⏳"` / `"LMT BUY @ 180.00  ⏳"`
/// - **PartialFill**: `"BUY @ 171.59  ◑ 50/100sh"`
/// - **Active / Closed / Cancelled**: `"▲ 171.59  100sh"` (original)
pub fn format_entry_label(bracket: &OrderBracket) -> String {
    let prefix = entry_type_prefix(bracket.entry_type);
    let price_str = format_entry_price(bracket);

    match bracket.status {
        BracketStatus::Draft => {
            let verb = match bracket.side {
                BracketSide::Long => "BUY",
                BracketSide::Short => "SELL",
            };
            format!("{}{} @ {}", prefix, verb, price_str)
        }
        BracketStatus::Pending => {
            let verb = match bracket.side {
                BracketSide::Long => "BUY",
                BracketSide::Short => "SELL",
            };
            format!("{}{} @ {}  \u{23F3}", prefix, verb, price_str)
        }
        BracketStatus::PartialFill => {
            let verb = match bracket.side {
                BracketSide::Long => "BUY",
                BracketSide::Short => "SELL",
            };
            let filled = bracket.filled_qty.unwrap_or(0.0);
            let total = bracket.quantity.unwrap_or(0.0);
            format!(
                "{}{} @ {}  \u{25D1} {:.0}/{:.0}sh",
                prefix, verb, price_str, filled, total
            )
        }
        BracketStatus::Active | BracketStatus::Closed | BracketStatus::Cancelled => {
            let arrow = match bracket.side {
                BracketSide::Long => "\u{25B2}",
                BracketSide::Short => "\u{25BC}",
            };
            let qty_str = bracket
                .quantity
                .map(|q| format!("  {:.0}sh", q))
                .unwrap_or_default();
            format!("{} {:.2}{}", arrow, bracket.entry.price, qty_str)
        }
    }
}

/// Format TP label: "TP 192.00  +$650" or "TP 192.00".
pub fn format_tp_label(leg: &BracketLeg) -> String {
    let pnl_str = leg
        .projected_pnl
        .map(|pnl| format!("  +${:.0}", pnl.abs()))
        .unwrap_or_default();
    format!("TP {:.2}{}", leg.price, pnl_str)
}

/// Format SL label. Draft brackets show `"SL @ {price:.2}"`.
/// Other statuses show `"SL {price:.2}"` with optional P&L.
pub fn format_sl_label(leg: &BracketLeg, status: BracketStatus) -> String {
    if status == BracketStatus::Draft {
        return format!("SL @ {:.2}", leg.price);
    }
    let pnl_str = leg
        .projected_pnl
        .map(|pnl| format!("  -${:.0}", pnl.abs()))
        .unwrap_or_default();
    format!("SL {:.2}{}", leg.price, pnl_str)
}

// ── Phase 3: compute_bracket ────────────────────────────────────────

use super::compute::{ComputeContext, LabelAnchor, WidgetLabel, WidgetOutput};
use super::hit_test::{CursorIcon, HitZone, HitZoneKind};
use super::AnnotationId;
use crate::instances::GridLineInstance;

/// Compute render primitives for a bracket annotation.
///
/// Produces lines for entry/TP/SL, zone fills (Active brackets only),
/// hit zones for interaction, and text labels for prices and R:R.
/// The `alpha` parameter is the annotation's `Presence::alpha()` value.
pub fn compute_bracket(
    bracket: &OrderBracket,
    annotation_id: AnnotationId,
    ctx: &ComputeContext<'_>,
    alpha: f32,
) -> WidgetOutput {
    let mut output = WidgetOutput::default();
    let vp_width = ctx.viewport.width as f32;

    // ── Entry line ──────────────────────────────────────────────────
    let entry_y = ctx.camera.price_to_y(bracket.entry.price);
    let (_entry_style, entry_width, entry_color) = bracket.leg_style(LegRole::Entry);
    let mut ec = entry_color;
    ec[3] *= alpha;

    output.lines.push(GridLineInstance {
        rect: [0.0, entry_y, vp_width, entry_y + entry_width],
        color: ec,
    });

    let entry_label_text = format_entry_label(bracket);
    output.labels.push(WidgetLabel {
        text: entry_label_text,
        screen_x: vp_width - 10.0,
        screen_y: entry_y,
        bg_color: [0.12, 0.12, 0.15, 0.85 * alpha],
        text_color: ec,
        font_size: 11.0,
        anchor: LabelAnchor::Right,
    });

    // ── Take-profit line ────────────────────────────────────────────
    if let Some(ref tp) = bracket.take_profit {
        let tp_y = ctx.camera.price_to_y(tp.price);
        let (_tp_style, tp_width, tp_color) = bracket.leg_style(LegRole::TakeProfit);
        let mut tc = tp_color;
        tc[3] *= alpha;

        output.lines.push(GridLineInstance {
            rect: [0.0, tp_y, vp_width, tp_y + tp_width],
            color: tc,
        });

        output.hit_zones.push(HitZone {
            annotation_id,
            rect: [0.0, tp_y - 6.0, vp_width, tp_y + 6.0],
            kind: HitZoneKind::BracketTP,
            cursor: CursorIcon::ResizeNS,
        });

        let tp_text = format_tp_label(tp);
        output.labels.push(WidgetLabel {
            text: tp_text,
            screen_x: vp_width - 10.0,
            screen_y: tp_y,
            bg_color: [0.12, 0.12, 0.15, 0.85 * alpha],
            text_color: tc,
            font_size: 11.0,
            anchor: LabelAnchor::Right,
        });
    }

    // ── Stop-loss line ──────────────────────────────────────────────
    if let Some(ref sl) = bracket.stop_loss {
        let sl_y = ctx.camera.price_to_y(sl.price);
        let (_sl_style, sl_width, sl_color) = bracket.leg_style(LegRole::StopLoss);
        let mut sc = sl_color;
        sc[3] *= alpha;

        output.lines.push(GridLineInstance {
            rect: [0.0, sl_y, vp_width, sl_y + sl_width],
            color: sc,
        });

        output.hit_zones.push(HitZone {
            annotation_id,
            rect: [0.0, sl_y - 6.0, vp_width, sl_y + 6.0],
            kind: HitZoneKind::BracketSL,
            cursor: CursorIcon::ResizeNS,
        });

        let sl_text = format_sl_label(sl, bracket.status);
        output.labels.push(WidgetLabel {
            text: sl_text,
            screen_x: vp_width - 10.0,
            screen_y: sl_y,
            bg_color: [0.12, 0.12, 0.15, 0.85 * alpha],
            text_color: sc,
            font_size: 11.0,
            anchor: LabelAnchor::Right,
        });
    }

    // ── Draft action buttons (entry line) ─────────────────────────
    if bracket.status == BracketStatus::Draft {
        emit_entry_buttons(
            bracket,
            annotation_id,
            entry_y,
            vp_width,
            alpha,
            &mut output,
        );
    }

    // ── Draft [X] button on SL line ────────────────────────────
    if bracket.status == BracketStatus::Draft {
        if let Some(ref sl) = bracket.stop_loss {
            let sl_y = ctx.camera.price_to_y(sl.price);
            emit_sl_cancel_button(annotation_id, sl_y, vp_width, alpha, &mut output);
        }
    }

    // ── Zone fills (Active brackets only) ───────────────────────────
    let zones = bracket_zone_rects(bracket, entry_y, vp_width, |p| ctx.camera.price_to_y(p));
    for (rect, mut zone_color) in zones {
        zone_color[3] *= alpha;
        output.fills.push(GridLineInstance {
            rect,
            color: zone_color,
        });
    }

    // ── R:R label (when both TP and SL are present) ─────────────────
    if let Some(rr) = bracket.risk_reward() {
        output.labels.push(WidgetLabel {
            text: format!("R:R {:.2}:1", rr),
            screen_x: vp_width - 120.0,
            screen_y: entry_y,
            bg_color: [0.12, 0.12, 0.15, 0.75 * alpha],
            text_color: [0.7, 0.7, 0.7, 1.0 * alpha],
            font_size: 10.0,
            anchor: LabelAnchor::Right,
        });
    }

    output
}

// ── Button constants ────────────────────────────────────────────────

/// Button half-height for hit zone rects (pixels above/below label y).
const BTN_HIT_HALF_H: f32 = 8.0;
/// Right-edge padding from viewport edge for the first button.
const BTN_RIGHT_PAD: f32 = 8.0;
/// Horizontal gap between adjacent buttons.
const BTN_GAP: f32 = 4.0;
/// Text color for all action buttons (white with slight transparency).
const BTN_TEXT_COLOR: [f32; 4] = [1.0, 1.0, 1.0, 0.95];

/// Estimate pixel width of a button label: char_count * 7.0 + 12.0.
fn btn_width(text: &str) -> f32 {
    text.len() as f32 * 7.0 + 12.0
}

/// Emit right-aligned action buttons on the entry line for a Draft bracket.
///
/// Button order (right to left): `[X]`, `[Submit]`, `[Save]`, `[SL]`.
/// Buttons that would overflow beyond x=0 are omitted. `[Submit]` is
/// omitted when entry price is zero (no market data). `[SL]` is
/// omitted when `stop_loss` is already set.
fn emit_entry_buttons(
    bracket: &OrderBracket,
    annotation_id: AnnotationId,
    entry_y: f32,
    vp_width: f32,
    alpha: f32,
    output: &mut WidgetOutput,
) {
    let submit_bg = match bracket.side {
        BracketSide::Long => [0.20, 0.78, 0.35, 0.85 * alpha],
        BracketSide::Short => [0.90, 0.25, 0.25, 0.85 * alpha],
    };
    let cancel_bg = [0.4, 0.4, 0.4, 0.85 * alpha];
    let save_bg = [0.35, 0.45, 0.65, 0.85 * alpha];
    let sl_bg = [0.85, 0.55, 0.20, 0.85 * alpha];
    let text_color = [
        BTN_TEXT_COLOR[0],
        BTN_TEXT_COLOR[1],
        BTN_TEXT_COLOR[2],
        BTN_TEXT_COLOR[3] * alpha,
    ];

    let mut cursor = vp_width - BTN_RIGHT_PAD;

    // [X] cancel button
    let x_text = "X";
    let x_w = btn_width(x_text);
    let x_right = cursor;
    let x_left = x_right - x_w;
    if x_left < 0.0 {
        return;
    }
    push_button(
        output,
        ButtonSpec {
            text: x_text,
            right_x: x_right,
            center_y: entry_y,
            width: x_w,
            bg_color: cancel_bg,
            text_color,
            annotation_id,
            kind: HitZoneKind::BracketCancel,
        },
    );
    cursor = x_left - BTN_GAP;

    // [Submit] button — only if entry price is non-zero
    if bracket.entry.price != 0.0 {
        let submit_text = "Submit";
        let submit_w = btn_width(submit_text);
        let submit_right = cursor;
        let submit_left = submit_right - submit_w;
        if submit_left < 0.0 {
            return;
        }
        push_button(
            output,
            ButtonSpec {
                text: submit_text,
                right_x: submit_right,
                center_y: entry_y,
                width: submit_w,
                bg_color: submit_bg,
                text_color,
                annotation_id,
                kind: HitZoneKind::BracketSubmit,
            },
        );
        cursor = submit_left - BTN_GAP;
    }

    // [Save] button
    let save_text = "Save";
    let save_w = btn_width(save_text);
    let save_right = cursor;
    let save_left = save_right - save_w;
    if save_left < 0.0 {
        return;
    }
    push_button(
        output,
        ButtonSpec {
            text: save_text,
            right_x: save_right,
            center_y: entry_y,
            width: save_w,
            bg_color: save_bg,
            text_color,
            annotation_id,
            kind: HitZoneKind::BracketSave,
        },
    );
    cursor = save_left - BTN_GAP;

    // [SL] button — only when stop_loss is not yet set
    if bracket.stop_loss.is_none() {
        let sl_text = "SL";
        let sl_w = btn_width(sl_text);
        let sl_right = cursor;
        let sl_left = sl_right - sl_w;
        if sl_left < 0.0 {
            return;
        }
        push_button(
            output,
            ButtonSpec {
                text: sl_text,
                right_x: sl_right,
                center_y: entry_y,
                width: sl_w,
                bg_color: sl_bg,
                text_color,
                annotation_id,
                kind: HitZoneKind::BracketToggleSL,
            },
        );
    }
}

/// Emit an [X] cancel button on the SL line for a Draft bracket.
fn emit_sl_cancel_button(
    annotation_id: AnnotationId,
    sl_y: f32,
    vp_width: f32,
    alpha: f32,
    output: &mut WidgetOutput,
) {
    let cancel_bg = [0.4, 0.4, 0.4, 0.85 * alpha];
    let text_color = [
        BTN_TEXT_COLOR[0],
        BTN_TEXT_COLOR[1],
        BTN_TEXT_COLOR[2],
        BTN_TEXT_COLOR[3] * alpha,
    ];

    let x_text = "X";
    let x_w = btn_width(x_text);
    let x_right = vp_width - BTN_RIGHT_PAD;
    let x_left = x_right - x_w;
    if x_left < 0.0 {
        return;
    }
    push_button(
        output,
        ButtonSpec {
            text: x_text,
            right_x: x_right,
            center_y: sl_y,
            width: x_w,
            bg_color: cancel_bg,
            text_color,
            annotation_id,
            kind: HitZoneKind::BracketCancelSL,
        },
    );
}

/// Parameters for a single action button on a bracket line.
struct ButtonSpec<'a> {
    /// Button label text.
    text: &'a str,
    /// Right edge X coordinate.
    right_x: f32,
    /// Center Y coordinate of the line.
    center_y: f32,
    /// Estimated pixel width.
    width: f32,
    /// Background color.
    bg_color: [f32; 4],
    /// Text color.
    text_color: [f32; 4],
    /// Owning annotation ID.
    annotation_id: AnnotationId,
    /// Hit zone kind for click dispatch.
    kind: HitZoneKind,
}

/// Push a button label + hit zone into the output.
fn push_button(output: &mut WidgetOutput, spec: ButtonSpec<'_>) {
    output.labels.push(WidgetLabel {
        text: spec.text.to_string(),
        screen_x: spec.right_x,
        screen_y: spec.center_y,
        bg_color: spec.bg_color,
        text_color: spec.text_color,
        font_size: 11.0,
        anchor: LabelAnchor::Right,
    });
    output.hit_zones.push(HitZone {
        annotation_id: spec.annotation_id,
        rect: [
            spec.right_x - spec.width,
            spec.center_y - BTN_HIT_HALF_H,
            spec.right_x,
            spec.center_y + BTN_HIT_HALF_H,
        ],
        kind: spec.kind,
        cursor: CursorIcon::Pointer,
    });
}

#[cfg(test)]
mod tests;
