//! Order bracket annotation: entry + optional TP/SL.
//!
//! A compound annotation representing a trade idea. The chart crate
//! sees these as pure visual geometry. The app layer maps them to
//! broker orders.

use super::level::LineStyle;
use serde::{Deserialize, Serialize};

/// An order bracket: entry line + optional take-profit and stop-loss.
///
/// The chart crate uses `BracketStatus` for visual styling only.
/// The app layer maps brackets to broker order instances.
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

/// Blue-gray entry line.
const BRACKET_ENTRY_COLOR: [f32; 4] = [0.55, 0.65, 0.80, 1.0];
/// Green take-profit line.
const BRACKET_TP_COLOR: [f32; 4] = [0.20, 0.78, 0.35, 1.0];
/// Red stop-loss line.
const BRACKET_SL_COLOR: [f32; 4] = [0.90, 0.25, 0.25, 1.0];
/// Green zone fill at 6% alpha (between entry and TP).
const BRACKET_TP_ZONE: [f32; 4] = [0.20, 0.78, 0.35, 0.06];
/// Red zone fill at 6% alpha (between entry and SL).
const BRACKET_SL_ZONE: [f32; 4] = [0.90, 0.25, 0.25, 0.06];

// ── Phase 3: chart rendering helpers ─────────────────────────────────

impl OrderBracket {
    /// Compute line style for a leg based on bracket status.
    ///
    /// Returns `(LineStyle, line_width, color)`. The base color comes from
    /// the leg role (entry = blue-gray, TP = green, SL = red), then the
    /// bracket status modulates style, width, and alpha.
    pub fn leg_style(&self, role: LegRole) -> (LineStyle, f32, [f32; 4]) {
        let base_color = match role {
            LegRole::Entry => BRACKET_ENTRY_COLOR,
            LegRole::TakeProfit => BRACKET_TP_COLOR,
            LegRole::StopLoss => BRACKET_SL_COLOR,
        };

        let (style, width, alpha_mult) = match self.status {
            BracketStatus::Draft => (
                LineStyle::Dashed {
                    dash_len: 6.0,
                    gap_len: 4.0,
                },
                1.0,
                0.8,
            ),
            BracketStatus::Pending => (
                LineStyle::Dotted { dot_spacing: 4.0 },
                1.0,
                0.7,
            ),
            BracketStatus::PartialFill => (LineStyle::Solid, 1.5, 0.9),
            BracketStatus::Active => (LineStyle::Solid, 1.5, 1.0),
            BracketStatus::Closed => (LineStyle::Solid, 1.0, 0.3),
            BracketStatus::Cancelled => (LineStyle::Solid, 1.0, 0.2),
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

/// Format entry label: "▲ 185.50  100sh" for buy, "▼ 185.50  100sh" for sell.
pub fn format_entry_label(bracket: &OrderBracket) -> String {
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

/// Format TP label: "TP 192.00  +$650" or "TP 192.00".
pub fn format_tp_label(leg: &BracketLeg) -> String {
    let pnl_str = leg
        .projected_pnl
        .map(|pnl| format!("  +${:.0}", pnl.abs()))
        .unwrap_or_default();
    format!("TP {:.2}{}", leg.price, pnl_str)
}

/// Format SL label: "SL 182.00  -$350" or "SL 182.00".
pub fn format_sl_label(leg: &BracketLeg) -> String {
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

        let sl_text = format_sl_label(sl);
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

#[cfg(test)]
mod tests;
