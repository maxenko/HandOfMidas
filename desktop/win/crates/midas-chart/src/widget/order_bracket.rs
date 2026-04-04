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
mod tests {
    use super::*;

    fn make_leg(price: f64) -> BracketLeg {
        BracketLeg {
            price,
            timestamp: None,
            color: None,
            style: LineStyle::default(),
            line_width: 1.0,
            label: None,
            projected_pnl: None,
            projected_pnl_pct: None,
        }
    }

    fn make_bracket(entry: f64, tp: f64, sl: f64) -> OrderBracket {
        OrderBracket {
            entry: make_leg(entry),
            take_profit: Some(make_leg(tp)),
            stop_loss: Some(make_leg(sl)),
            side: BracketSide::Long,
            status: BracketStatus::Draft,
            quantity: None,
        }
    }

    #[test]
    fn risk_reward_long() {
        let b = make_bracket(100.0, 110.0, 95.0);
        let rr = b.risk_reward().unwrap();
        assert!((rr - 2.0).abs() < 0.01, "expected R:R 2.0, got {}", rr);
    }

    #[test]
    fn risk_reward_short() {
        let mut b = make_bracket(100.0, 90.0, 105.0);
        b.side = BracketSide::Short;
        let rr = b.risk_reward().unwrap();
        assert!((rr - 2.0).abs() < 0.01, "expected R:R 2.0, got {}", rr);
    }

    #[test]
    fn risk_reward_missing_tp() {
        let mut b = make_bracket(100.0, 110.0, 95.0);
        b.take_profit = None;
        assert!(b.risk_reward().is_none());
    }

    #[test]
    fn risk_reward_missing_sl() {
        let mut b = make_bracket(100.0, 110.0, 95.0);
        b.stop_loss = None;
        assert!(b.risk_reward().is_none());
    }

    #[test]
    fn risk_reward_zero_risk() {
        let b = make_bracket(100.0, 110.0, 100.0);
        assert!(b.risk_reward().is_none());
    }

    #[test]
    fn serde_round_trip() {
        let b = make_bracket(185.50, 192.0, 182.0);
        let json = serde_json::to_string(&b).expect("serialize");
        let decoded: OrderBracket = serde_json::from_str(&json).expect("deserialize");
        assert!((decoded.entry.price - 185.50).abs() < f64::EPSILON);
        assert_eq!(decoded.side, BracketSide::Long);
        assert_eq!(decoded.status, BracketStatus::Draft);
    }

    #[test]
    fn bracket_status_default_is_draft() {
        assert_eq!(BracketStatus::default(), BracketStatus::Draft);
    }

    // -----------------------------------------------------------------------
    // dollar_risk / dollar_reward
    // -----------------------------------------------------------------------

    #[test]
    fn dollar_risk_long() {
        let mut b = make_bracket(185.0, 192.0, 182.0);
        b.quantity = Some(100.0);
        let risk = b.dollar_risk().unwrap();
        assert!((risk - 300.0).abs() < 0.01, "expected $300, got {risk}");
    }

    #[test]
    fn dollar_reward_long() {
        let mut b = make_bracket(185.0, 192.0, 182.0);
        b.quantity = Some(100.0);
        let reward = b.dollar_reward().unwrap();
        assert!((reward - 700.0).abs() < 0.01, "expected $700, got {reward}");
    }

    #[test]
    fn dollar_risk_short() {
        let mut b = make_bracket(185.0, 178.0, 188.0);
        b.side = BracketSide::Short;
        b.quantity = Some(100.0);
        let risk = b.dollar_risk().unwrap();
        assert!((risk - 300.0).abs() < 0.01, "expected $300, got {risk}");
    }

    #[test]
    fn dollar_reward_short() {
        let mut b = make_bracket(185.0, 178.0, 188.0);
        b.side = BracketSide::Short;
        b.quantity = Some(100.0);
        let reward = b.dollar_reward().unwrap();
        assert!((reward - 700.0).abs() < 0.01, "expected $700, got {reward}");
    }

    #[test]
    fn dollar_risk_no_sl() {
        let mut b = make_bracket(185.0, 192.0, 182.0);
        b.stop_loss = None;
        b.quantity = Some(100.0);
        assert!(b.dollar_risk().is_none());
    }

    #[test]
    fn dollar_reward_no_tp() {
        let mut b = make_bracket(185.0, 192.0, 182.0);
        b.take_profit = None;
        b.quantity = Some(100.0);
        assert!(b.dollar_reward().is_none());
    }

    #[test]
    fn dollar_risk_zero_qty() {
        let b = make_bracket(185.0, 192.0, 182.0);
        // quantity is None
        assert!(b.dollar_risk().is_none());
    }

    // -----------------------------------------------------------------------
    // serde backward compatibility (new fields default to None)
    // -----------------------------------------------------------------------

    #[test]
    fn serde_backward_compat_missing_pnl_fields() {
        // Simulate old JSON without projected_pnl fields
        let json = r#"{
            "price": 192.0,
            "timestamp": null,
            "color": null,
            "style": "Solid",
            "line_width": 1.0,
            "label": null
        }"#;
        let leg: BracketLeg = serde_json::from_str(json).expect("should deserialize");
        assert!(leg.projected_pnl.is_none());
        assert!(leg.projected_pnl_pct.is_none());
    }

    // -----------------------------------------------------------------------
    // Phase 3: leg_style
    // -----------------------------------------------------------------------

    #[test]
    fn test_leg_style_active_entry() {
        let mut b = make_bracket(185.0, 192.0, 182.0);
        b.status = BracketStatus::Active;
        let (style, width, color) = b.leg_style(LegRole::Entry);
        assert_eq!(style, LineStyle::Solid);
        assert!((width - 1.5).abs() < f32::EPSILON);
        // Active = alpha_mult 1.0, base alpha 1.0 => full alpha
        assert!((color[3] - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_leg_style_draft_tp() {
        let mut b = make_bracket(185.0, 192.0, 182.0);
        b.status = BracketStatus::Draft;
        let (style, width, color) = b.leg_style(LegRole::TakeProfit);
        assert!(matches!(style, LineStyle::Dashed { .. }));
        assert!((width - 1.0).abs() < f32::EPSILON);
        // Draft alpha_mult = 0.8, base TP alpha = 1.0 => 0.8
        assert!((color[3] - 0.8).abs() < f32::EPSILON);
    }

    #[test]
    fn test_leg_style_cancelled_sl() {
        let mut b = make_bracket(185.0, 192.0, 182.0);
        b.status = BracketStatus::Cancelled;
        let (style, width, color) = b.leg_style(LegRole::StopLoss);
        assert_eq!(style, LineStyle::Solid);
        assert!((width - 1.0).abs() < f32::EPSILON);
        // Cancelled alpha_mult = 0.2, base SL alpha = 1.0 => 0.2
        assert!((color[3] - 0.2).abs() < f32::EPSILON);
    }

    // -----------------------------------------------------------------------
    // Phase 3: format labels
    // -----------------------------------------------------------------------

    #[test]
    fn test_format_entry_label_long() {
        let mut b = make_bracket(185.50, 192.0, 182.0);
        b.side = BracketSide::Long;
        b.quantity = Some(100.0);
        let label = format_entry_label(&b);
        assert!(label.starts_with('\u{25B2}'), "Long should start with up arrow, got: {label}");
        assert!(label.contains("185.50"));
        assert!(label.contains("100sh"));
    }

    #[test]
    fn test_format_entry_label_short() {
        let mut b = make_bracket(185.50, 178.0, 188.0);
        b.side = BracketSide::Short;
        b.quantity = Some(50.0);
        let label = format_entry_label(&b);
        assert!(label.starts_with('\u{25BC}'), "Short should start with down arrow, got: {label}");
        assert!(label.contains("185.50"));
        assert!(label.contains("50sh"));
    }

    #[test]
    fn test_format_tp_label_with_pnl() {
        let mut leg = make_leg(192.0);
        leg.projected_pnl = Some(650.0);
        let label = format_tp_label(&leg);
        assert!(label.starts_with("TP 192.00"));
        assert!(label.contains("+$650"), "expected +$650, got: {label}");
    }

    #[test]
    fn test_format_sl_label_no_pnl() {
        let leg = make_leg(182.0);
        let label = format_sl_label(&leg);
        assert_eq!(label, "SL 182.00");
    }

    // -----------------------------------------------------------------------
    // Phase 3: bracket_zone_rects
    // -----------------------------------------------------------------------

    #[test]
    fn test_bracket_zone_rects_active() {
        let mut b = make_bracket(185.0, 192.0, 182.0);
        b.status = BracketStatus::Active;
        // Simple linear price_to_y: price 200 -> y 0, price 180 -> y 200
        let price_to_y = |p: f64| ((200.0 - p) * 10.0) as f32;
        let entry_y = price_to_y(185.0); // 150.0
        let zones = bracket_zone_rects(&b, entry_y, 1920.0, price_to_y);
        assert_eq!(zones.len(), 2, "Active bracket with TP+SL should produce 2 zones");

        // TP zone: entry_y=150, tp_y=price_to_y(192)=80 => top=80, bottom=150
        let (tp_rect, tp_color) = &zones[0];
        assert!((tp_rect[0] - 0.0).abs() < f32::EPSILON);
        assert!((tp_rect[1] - 80.0).abs() < f32::EPSILON);
        assert!((tp_rect[2] - 1920.0).abs() < f32::EPSILON);
        assert!((tp_rect[3] - 150.0).abs() < f32::EPSILON);
        assert!((tp_color[3] - 0.06).abs() < 0.001);

        // SL zone: entry_y=150, sl_y=price_to_y(182)=180 => top=150, bottom=180
        let (sl_rect, sl_color) = &zones[1];
        assert!((sl_rect[1] - 150.0).abs() < f32::EPSILON);
        assert!((sl_rect[3] - 180.0).abs() < f32::EPSILON);
        assert!((sl_color[3] - 0.06).abs() < 0.001);
    }

    #[test]
    fn test_bracket_zone_rects_draft() {
        let b = make_bracket(185.0, 192.0, 182.0);
        // status is Draft by default
        let price_to_y = |p: f64| ((200.0 - p) * 10.0) as f32;
        let entry_y = price_to_y(185.0);
        let zones = bracket_zone_rects(&b, entry_y, 1920.0, price_to_y);
        assert!(zones.is_empty(), "Draft bracket should produce no zone rects");
    }
}
