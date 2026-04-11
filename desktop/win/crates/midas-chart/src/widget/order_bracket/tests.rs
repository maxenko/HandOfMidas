use super::*;
use crate::widget::price_line::{LineExtent, LineStroke, PriceLine};
use smallvec::smallvec;

fn make_leg(price: f64) -> BracketLeg {
    make_leg_role(price, LegRole::Entry)
}

fn make_leg_role(price: f64, role: LegRole) -> BracketLeg {
    BracketLeg {
        line: PriceLine {
            price,
            extent: LineExtent::FullWidth,
            stroke: LineStroke {
                color: [0.0, 0.0, 0.0, 1.0],
                width: 1.0,
                style: LineStyle::default(),
            },
        },
        role,
        projected_pnl: None,
        projected_pnl_pct: None,
    }
}

fn make_bracket(entry: f64, tp: f64, sl: f64) -> OrderBracket {
    OrderBracket {
        entry: make_leg_role(entry, LegRole::Entry),
        take_profit: Some(make_leg_role(tp, LegRole::TakeProfit)),
        stop_loss: Some(make_leg_role(sl, LegRole::StopLoss)),
        side: BracketSide::Long,
        status: BracketStatus::Draft,
        quantity: None,
        saved: false,
        filled_qty: None,
        entry_type: EntryType::Market,
        entry_stop_price: None,
        wrong_side_warning: false,
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
    assert!((decoded.entry.line.price - 185.50).abs() < f64::EPSILON);
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
    // Slice 8a-i: BracketLeg moved to nested `line: PriceLine`. Brackets
    // are not persisted to disk (per M9 finding) so a v1 fallback is not
    // load-bearing — instead verify the new shape with `projected_pnl`
    // fields omitted still defaults to None via `#[serde(default)]`.
    let json = r#"{
        "line": {
            "price": 192.0,
            "extent": "FullWidth",
            "stroke": {
                "color": [0.2, 0.78, 0.35, 1.0],
                "width": 1.0,
                "style": "Solid"
            }
        },
        "role": "TakeProfit"
    }"#;
    let leg: BracketLeg = serde_json::from_str(json).expect("should deserialize");
    assert!(leg.projected_pnl.is_none());
    assert!(leg.projected_pnl_pct.is_none());
    assert!((leg.line.price - 192.0).abs() < f64::EPSILON);
    assert_eq!(leg.role, LegRole::TakeProfit);
}

// -----------------------------------------------------------------------
// Phase 3: leg_style
// -----------------------------------------------------------------------

#[test]
fn test_leg_style_active_entry() {
    let mut b = make_bracket(185.0, 192.0, 182.0);
    b.status = BracketStatus::Active;
    let __stroke = b.leg_style(LegRole::Entry);
    let style = __stroke.style;
    let width = __stroke.width;
    let color = __stroke.color;
    assert_eq!(style, LineStyle::Solid);
    assert!((width - 1.5).abs() < f32::EPSILON);
    // Active = alpha_mult 1.0, base alpha 1.0 => full alpha
    assert!((color[3] - 1.0).abs() < f32::EPSILON);
}

#[test]
fn leg_style_draft_tp_uses_pattern() {
    let mut b = make_bracket(185.0, 192.0, 182.0);
    b.status = BracketStatus::Draft;
    let __stroke = b.leg_style(LegRole::TakeProfit);
    let style = __stroke.style;
    let width = __stroke.width;
    let color = __stroke.color;
    assert_eq!(style, LineStyle::Pattern(smallvec![6.0, 4.0]));
    assert!((width - 1.0).abs() < f32::EPSILON);
    // Draft unsaved alpha_mult = 0.50, base TP alpha = 1.0 => 0.50
    assert!((color[3] - 0.50).abs() < f32::EPSILON);
}

#[test]
fn leg_style_sl_uses_pattern() {
    let mut b = make_bracket(185.0, 192.0, 182.0);
    b.status = BracketStatus::Cancelled;
    let __stroke = b.leg_style(LegRole::StopLoss);
    let style = __stroke.style;
    let width = __stroke.width;
    let color = __stroke.color;
    // SL is always dotted (orange), regardless of status.
    assert_eq!(style, LineStyle::dotted());
    assert!((width - 1.0).abs() < f32::EPSILON);
    // Orange base [1.0, 0.60, 0.0], cancelled alpha_mult = 0.2
    assert!((color[0] - 1.0).abs() < 0.01);
    assert!((color[1] - 0.60).abs() < 0.01);
    assert!((color[2] - 0.0).abs() < 0.01);
    assert!((color[3] - 0.2).abs() < f32::EPSILON);
}

// -----------------------------------------------------------------------
// Phase 3: format labels
// -----------------------------------------------------------------------

#[test]
fn test_format_entry_label_long_active() {
    let mut b = make_bracket(185.50, 192.0, 182.0);
    b.side = BracketSide::Long;
    b.status = BracketStatus::Active;
    b.quantity = Some(100.0);
    let label = format_entry_label(&b);
    assert!(
        label.starts_with('\u{25B2}'),
        "Active Long should start with up arrow, got: {label}"
    );
    assert!(label.contains("185.50"));
    assert!(label.contains("100sh"));
}

#[test]
fn test_format_entry_label_short_active() {
    let mut b = make_bracket(185.50, 178.0, 188.0);
    b.side = BracketSide::Short;
    b.status = BracketStatus::Active;
    b.quantity = Some(50.0);
    let label = format_entry_label(&b);
    assert!(
        label.starts_with('\u{25BC}'),
        "Active Short should start with down arrow, got: {label}"
    );
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
    let label = format_sl_label(&leg, BracketStatus::Active);
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
    assert_eq!(
        zones.len(),
        2,
        "Active bracket with TP+SL should produce 2 zones"
    );

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
    assert!(
        zones.is_empty(),
        "Draft bracket should produce no zone rects"
    );
}

// -----------------------------------------------------------------------
// Slice 2: side-colored entry line
// -----------------------------------------------------------------------

#[test]
fn leg_style_entry_green_for_long() {
    let mut b = make_bracket(185.0, 192.0, 182.0);
    b.side = BracketSide::Long;
    b.status = BracketStatus::Active;
    let __stroke = b.leg_style(LegRole::Entry);
    let _style = __stroke.style;
    let _width = __stroke.width;
    let color = __stroke.color;
    // Long entry uses BRACKET_LONG_ENTRY_COLOR (green-ish)
    assert!(
        (color[0] - 0.20).abs() < 0.01
            && (color[1] - 0.78).abs() < 0.01
            && (color[2] - 0.35).abs() < 0.01,
        "Long entry should be green, got: {:?}",
        color
    );
}

#[test]
fn leg_style_entry_red_for_short() {
    let mut b = make_bracket(185.0, 178.0, 188.0);
    b.side = BracketSide::Short;
    b.status = BracketStatus::Active;
    let __stroke = b.leg_style(LegRole::Entry);
    let _style = __stroke.style;
    let _width = __stroke.width;
    let color = __stroke.color;
    // Short entry uses BRACKET_SHORT_ENTRY_COLOR (red-ish)
    assert!(
        (color[0] - 0.90).abs() < 0.01
            && (color[1] - 0.25).abs() < 0.01
            && (color[2] - 0.25).abs() < 0.01,
        "Short entry should be red, got: {:?}",
        color
    );
}

// -----------------------------------------------------------------------
// Slice 2: status-aware alpha progression
// -----------------------------------------------------------------------

#[test]
fn leg_style_draft_unsaved_alpha() {
    let b = make_bracket(185.0, 192.0, 182.0);
    // Draft, saved = false by default
    let __stroke = b.leg_style(LegRole::Entry);
    let _style = __stroke.style;
    let _width = __stroke.width;
    let color = __stroke.color;
    assert!(
        (color[3] - 0.50).abs() < f32::EPSILON,
        "Draft unsaved alpha should be 0.50, got {}",
        color[3]
    );
}

#[test]
fn leg_style_draft_saved_alpha() {
    let mut b = make_bracket(185.0, 192.0, 182.0);
    b.saved = true;
    let __stroke = b.leg_style(LegRole::Entry);
    let _style = __stroke.style;
    let _width = __stroke.width;
    let color = __stroke.color;
    assert!(
        (color[3] - 0.65).abs() < f32::EPSILON,
        "Draft saved alpha should be 0.65, got {}",
        color[3]
    );
}

#[test]
fn leg_style_pending_alpha() {
    let mut b = make_bracket(185.0, 192.0, 182.0);
    b.status = BracketStatus::Pending;
    let __stroke = b.leg_style(LegRole::Entry);
    let _style = __stroke.style;
    let _width = __stroke.width;
    let color = __stroke.color;
    assert!(
        (color[3] - 0.80).abs() < f32::EPSILON,
        "Pending alpha should be 0.80, got {}",
        color[3]
    );
}

#[test]
fn leg_style_active_alpha() {
    let mut b = make_bracket(185.0, 192.0, 182.0);
    b.status = BracketStatus::Active;
    let __stroke = b.leg_style(LegRole::Entry);
    let _style = __stroke.style;
    let _width = __stroke.width;
    let color = __stroke.color;
    assert!(
        (color[3] - 1.0).abs() < f32::EPSILON,
        "Active alpha should be 1.0, got {}",
        color[3]
    );
}

// -----------------------------------------------------------------------
// Slice 2: status-aware entry label formatting
// -----------------------------------------------------------------------

#[test]
fn format_entry_label_draft() {
    let mut b = make_bracket(171.59, 180.0, 165.0);
    b.side = BracketSide::Long;
    b.status = BracketStatus::Draft;
    let label = format_entry_label(&b);
    assert_eq!(label, "BUY @ 171.59");
}

#[test]
fn format_entry_label_draft_short() {
    let mut b = make_bracket(171.59, 165.0, 180.0);
    b.side = BracketSide::Short;
    b.status = BracketStatus::Draft;
    let label = format_entry_label(&b);
    assert_eq!(label, "SELL @ 171.59");
}

#[test]
fn format_entry_label_active() {
    let mut b = make_bracket(171.59, 180.0, 165.0);
    b.side = BracketSide::Long;
    b.status = BracketStatus::Active;
    b.quantity = Some(100.0);
    let label = format_entry_label(&b);
    assert_eq!(label, "\u{25B2} 171.59  100sh");
}

#[test]
fn format_entry_label_partial_fill() {
    let mut b = make_bracket(171.59, 180.0, 165.0);
    b.side = BracketSide::Long;
    b.status = BracketStatus::PartialFill;
    b.filled_qty = Some(50.0);
    b.quantity = Some(100.0);
    let label = format_entry_label(&b);
    assert_eq!(label, "BUY @ 171.59  \u{25D1} 50/100sh");
}

#[test]
fn format_entry_label_pending() {
    let mut b = make_bracket(171.59, 180.0, 165.0);
    b.side = BracketSide::Long;
    b.status = BracketStatus::Pending;
    let label = format_entry_label(&b);
    assert_eq!(label, "BUY @ 171.59  \u{23F3}");
}

// -----------------------------------------------------------------------
// Entry type label prefixes
// -----------------------------------------------------------------------

#[test]
fn format_entry_label_market_buy_draft() {
    let mut b = make_bracket(182.00, 192.0, 175.0);
    b.entry_type = EntryType::Market;
    b.side = BracketSide::Long;
    b.status = BracketStatus::Draft;
    assert_eq!(format_entry_label(&b), "BUY @ 182.00");
}

#[test]
fn format_entry_label_market_sell_draft() {
    let mut b = make_bracket(182.00, 175.0, 192.0);
    b.entry_type = EntryType::Market;
    b.side = BracketSide::Short;
    b.status = BracketStatus::Draft;
    assert_eq!(format_entry_label(&b), "SELL @ 182.00");
}

#[test]
fn format_entry_label_limit_buy_draft() {
    let mut b = make_bracket(180.00, 192.0, 175.0);
    b.entry_type = EntryType::Limit;
    b.side = BracketSide::Long;
    b.status = BracketStatus::Draft;
    assert_eq!(format_entry_label(&b), "LMT BUY @ 180.00");
}

#[test]
fn format_entry_label_limit_sell_draft() {
    let mut b = make_bracket(190.00, 180.0, 195.0);
    b.entry_type = EntryType::Limit;
    b.side = BracketSide::Short;
    b.status = BracketStatus::Draft;
    assert_eq!(format_entry_label(&b), "LMT SELL @ 190.00");
}

#[test]
fn format_entry_label_stop_buy_draft() {
    let mut b = make_bracket(185.00, 195.0, 180.0);
    b.entry_type = EntryType::Stop;
    b.side = BracketSide::Long;
    b.status = BracketStatus::Draft;
    assert_eq!(format_entry_label(&b), "STP BUY @ 185.00");
}

#[test]
fn format_entry_label_stop_sell_draft() {
    let mut b = make_bracket(175.00, 165.0, 180.0);
    b.entry_type = EntryType::Stop;
    b.side = BracketSide::Short;
    b.status = BracketStatus::Draft;
    assert_eq!(format_entry_label(&b), "STP SELL @ 175.00");
}

#[test]
fn format_entry_label_stop_limit_buy_draft() {
    let mut b = make_bracket(184.50, 195.0, 180.0);
    b.entry_type = EntryType::StopLimit;
    b.entry_stop_price = Some(185.00);
    b.side = BracketSide::Long;
    b.status = BracketStatus::Draft;
    assert_eq!(format_entry_label(&b), "STP LMT BUY @ 185.00/184.50");
}

#[test]
fn format_entry_label_stop_limit_sell_draft() {
    let mut b = make_bracket(175.50, 165.0, 180.0);
    b.entry_type = EntryType::StopLimit;
    b.entry_stop_price = Some(175.00);
    b.side = BracketSide::Short;
    b.status = BracketStatus::Draft;
    assert_eq!(format_entry_label(&b), "STP LMT SELL @ 175.00/175.50");
}

#[test]
fn format_entry_label_limit_pending() {
    let mut b = make_bracket(180.00, 192.0, 175.0);
    b.entry_type = EntryType::Limit;
    b.side = BracketSide::Long;
    b.status = BracketStatus::Pending;
    assert_eq!(format_entry_label(&b), "LMT BUY @ 180.00  \u{23F3}");
}

#[test]
fn entry_type_default_is_market() {
    assert_eq!(EntryType::default(), EntryType::Market);
}

#[test]
fn serde_backward_compat_missing_entry_type() {
    // Slice 8a-i: BracketLeg moved to nested `line: PriceLine`. Brackets
    // are not persisted to disk (per M9 finding), so this test now uses
    // the new shape and only verifies that the optional bracket-level
    // fields (`entry_type`, `entry_stop_price`, `wrong_side_warning`)
    // still default correctly when omitted from JSON.
    let json = r#"{
        "entry": {
            "line": {
                "price": 185.0,
                "extent": "FullWidth",
                "stroke": {
                    "color": [0.2, 0.78, 0.35, 1.0],
                    "width": 1.0,
                    "style": "Solid"
                }
            },
            "role": "Entry"
        },
        "take_profit": null,
        "stop_loss": null,
        "side": "Long",
        "status": "Draft",
        "quantity": null
    }"#;
    let b: OrderBracket = serde_json::from_str(json).expect("should deserialize");
    assert_eq!(b.entry_type, EntryType::Market);
    assert!(b.entry_stop_price.is_none());
    assert!(!b.wrong_side_warning);
}

// -----------------------------------------------------------------------
// Slice 2: status-aware SL label formatting
// -----------------------------------------------------------------------

#[test]
fn format_sl_label_draft() {
    let leg = make_leg(169.95);
    let label = format_sl_label(&leg, BracketStatus::Draft);
    assert_eq!(label, "SL @ 169.95");
}

// -----------------------------------------------------------------------
// Slice 2: segmented_line
// -----------------------------------------------------------------------

#[test]
fn segmented_line_solid_single_instance() {
    let segs = super::segmented_line(
        0.0,
        100.0,
        50.0,
        1.0,
        [1.0, 1.0, 1.0, 1.0],
        &LineStyle::Solid,
    );
    assert_eq!(segs.len(), 1);
    assert!((segs[0].rect[0] - 0.0).abs() < f32::EPSILON);
    assert!((segs[0].rect[2] - 100.0).abs() < f32::EPSILON);
}

#[test]
fn segmented_line_dashed_count() {
    let segs = super::segmented_line(
        0.0,
        100.0,
        50.0,
        1.0,
        [1.0, 1.0, 1.0, 1.0],
        &LineStyle::Pattern(smallvec![6.0, 4.0]),
    );
    // 100px / (6+4) = 10 on-segments
    assert_eq!(
        segs.len(),
        10,
        "expected 10 dash segments, got {}",
        segs.len()
    );
}

#[test]
fn segmented_line_dotted_count() {
    let segs = super::segmented_line(
        0.0,
        100.0,
        50.0,
        1.0,
        [1.0, 1.0, 1.0, 1.0],
        &LineStyle::dotted(),
    );
    // 100px / (1+3) = 25 on-segments
    assert_eq!(
        segs.len(),
        25,
        "expected 25 dot segments, got {}",
        segs.len()
    );
}

#[test]
fn segmented_line_empty_pattern_is_solid() {
    let segs = super::segmented_line(
        0.0,
        100.0,
        50.0,
        1.0,
        [1.0; 4],
        &LineStyle::Pattern(smallvec![]),
    );
    assert_eq!(segs.len(), 1, "empty pattern must degenerate to solid");
    assert!((segs[0].rect[0] - 0.0).abs() < f32::EPSILON);
    assert!((segs[0].rect[2] - 100.0).abs() < f32::EPSILON);
}

#[test]
fn segmented_line_dash_dot_alternates_run_lengths() {
    // dash-dot: 6-on 3-off 1-on 3-off → cycle 13. Over 26px expect 4 on-runs.
    let segs = super::segmented_line(0.0, 26.0, 50.0, 1.0, [1.0; 4], &LineStyle::dash_dot());
    assert_eq!(segs.len(), 4, "expected 4 on-runs, got {}", segs.len());
    // Widths alternate 6, 1, 6, 1.
    let w0 = segs[0].rect[2] - segs[0].rect[0];
    let w1 = segs[1].rect[2] - segs[1].rect[0];
    let w2 = segs[2].rect[2] - segs[2].rect[0];
    let w3 = segs[3].rect[2] - segs[3].rect[0];
    assert!((w0 - 6.0).abs() < 1e-4);
    assert!((w1 - 1.0).abs() < 1e-4);
    assert!((w2 - 6.0).abs() < 1e-4);
    assert!((w3 - 1.0).abs() < 1e-4);
}

#[test]
fn segmented_line_pattern_wraps_cyclically() {
    // 3-entry odd-count pattern: [a, b, c] walked cyclically flips the
    // on/off phase every wrap. First cycle: on a / off b / on c. Second
    // cycle: off a / on b / off c. Etc.
    // Use [4, 2, 3] (cycle sum = 9). Over 18px (two full cycles) we expect:
    //   cycle 1: on(0..4)  off(4..6)  on(6..9)
    //   cycle 2: off(9..13) on(13..15) off(15..18)
    // Total on-runs: 3. Widths: 4, 3, 2.
    let segs = super::segmented_line(
        0.0,
        18.0,
        50.0,
        1.0,
        [1.0; 4],
        &LineStyle::Pattern(smallvec![4.0, 2.0, 3.0]),
    );
    assert_eq!(
        segs.len(),
        3,
        "expected 3 cyclic on-runs, got {}",
        segs.len()
    );
    let widths: Vec<f32> = segs.iter().map(|s| s.rect[2] - s.rect[0]).collect();
    assert!((widths[0] - 4.0).abs() < 1e-4, "{:?}", widths);
    assert!((widths[1] - 3.0).abs() < 1e-4, "{:?}", widths);
    assert!((widths[2] - 2.0).abs() < 1e-4, "{:?}", widths);
}

#[test]
fn segmented_line_zero_width_run_skipped() {
    // A [0, 3] pattern would emit zero-width on-runs if run skipping is
    // broken. Must produce zero instances over any length.
    let segs = super::segmented_line(
        0.0,
        100.0,
        50.0,
        1.0,
        [1.0; 4],
        &LineStyle::Pattern(smallvec![0.0, 3.0]),
    );
    assert!(
        segs.is_empty(),
        "zero-width on runs must not emit instances, got {}",
        segs.len()
    );
}

// -----------------------------------------------------------------------
// Entry type color dispatch
// -----------------------------------------------------------------------

#[test]
fn leg_style_entry_green_for_long_stop() {
    let mut b = make_bracket(185.0, 192.0, 182.0);
    b.side = BracketSide::Long;
    b.entry_type = EntryType::Stop;
    b.status = BracketStatus::Active;
    let __stroke = b.leg_style(LegRole::Entry);
    let _style = __stroke.style;
    let _width = __stroke.width;
    let color = __stroke.color;
    assert!(
        (color[0] - 0.20).abs() < 0.01
            && (color[1] - 0.78).abs() < 0.01
            && (color[2] - 0.35).abs() < 0.01,
        "Long Stop entry should be green, got: {:?}",
        color
    );
}

#[test]
fn leg_style_entry_lime_for_long_stop_limit() {
    let mut b = make_bracket(184.50, 195.0, 180.0);
    b.side = BracketSide::Long;
    b.entry_type = EntryType::StopLimit;
    b.entry_stop_price = Some(185.00);
    b.status = BracketStatus::Active;
    let __stroke = b.leg_style(LegRole::Entry);
    let _style = __stroke.style;
    let _width = __stroke.width;
    let color = __stroke.color;
    assert!(
        (color[0] - 0.50).abs() < 0.01
            && (color[1] - 0.90).abs() < 0.01
            && (color[2] - 0.20).abs() < 0.01,
        "Long StopLimit entry should be lime green, got: {:?}",
        color
    );
}

#[test]
fn leg_style_entry_red_for_short_stop() {
    let mut b = make_bracket(185.0, 178.0, 188.0);
    b.side = BracketSide::Short;
    b.entry_type = EntryType::Stop;
    b.status = BracketStatus::Active;
    let __stroke = b.leg_style(LegRole::Entry);
    let _style = __stroke.style;
    let _width = __stroke.width;
    let color = __stroke.color;
    assert!(
        (color[0] - 0.90).abs() < 0.01
            && (color[1] - 0.25).abs() < 0.01
            && (color[2] - 0.25).abs() < 0.01,
        "Short Stop entry should be red, got: {:?}",
        color
    );
}

#[test]
fn leg_style_entry_pink_for_short_stop_limit() {
    let mut b = make_bracket(175.50, 165.0, 180.0);
    b.side = BracketSide::Short;
    b.entry_type = EntryType::StopLimit;
    b.entry_stop_price = Some(175.00);
    b.status = BracketStatus::Active;
    let __stroke = b.leg_style(LegRole::Entry);
    let _style = __stroke.style;
    let _width = __stroke.width;
    let color = __stroke.color;
    assert!(
        (color[0] - 0.90).abs() < 0.01
            && (color[1] - 0.30).abs() < 0.01
            && (color[2] - 0.50).abs() < 0.01,
        "Short StopLimit entry should be pink-red, got: {:?}",
        color
    );
}

// -----------------------------------------------------------------------
// SL always dotted regardless of status
// -----------------------------------------------------------------------

#[test]
fn leg_style_sl_always_dotted_active() {
    let mut b = make_bracket(185.0, 192.0, 182.0);
    b.status = BracketStatus::Active;
    let __stroke = b.leg_style(LegRole::StopLoss);
    let style = __stroke.style;
    let width = __stroke.width;
    let color = __stroke.color;
    assert_eq!(
        style,
        LineStyle::dotted(),
        "Active SL should still be dotted, got: {:?}",
        style
    );
    assert!((width - 1.5).abs() < f32::EPSILON);
    // Orange base, full alpha for Active
    assert!((color[0] - 1.0).abs() < 0.01);
    assert!((color[1] - 0.60).abs() < 0.01);
    assert!((color[2] - 0.0).abs() < 0.01);
    assert!((color[3] - 1.0).abs() < f32::EPSILON);
}

#[test]
fn leg_style_sl_always_dotted_draft() {
    let b = make_bracket(185.0, 192.0, 182.0);
    // status is Draft by default
    let __stroke = b.leg_style(LegRole::StopLoss);
    let style = __stroke.style;
    let _width = __stroke.width;
    let color = __stroke.color;
    assert_eq!(
        style,
        LineStyle::dotted(),
        "Draft SL should be dotted (not dashed), got: {:?}",
        style
    );
    // Orange base, draft unsaved alpha = 0.50
    assert!((color[0] - 1.0).abs() < 0.01);
    assert!((color[1] - 0.60).abs() < 0.01);
    assert!((color[2] - 0.0).abs() < 0.01);
    assert!((color[3] - 0.50).abs() < f32::EPSILON);
}

// -----------------------------------------------------------------------
// SL zone fill is orange
// -----------------------------------------------------------------------

#[test]
fn bracket_zone_rects_sl_orange() {
    let mut b = make_bracket(185.0, 192.0, 182.0);
    b.status = BracketStatus::Active;
    let price_to_y = |p: f64| ((200.0 - p) * 10.0) as f32;
    let entry_y = price_to_y(185.0);
    let zones = bracket_zone_rects(&b, entry_y, 1920.0, price_to_y);
    assert_eq!(zones.len(), 2);
    // SL zone is second element
    let (_sl_rect, sl_color) = &zones[1];
    assert!(
        (sl_color[0] - 1.0).abs() < 0.01
            && (sl_color[1] - 0.60).abs() < 0.01
            && (sl_color[2] - 0.0).abs() < 0.01
            && (sl_color[3] - 0.06).abs() < 0.001,
        "SL zone should be orange at 6% alpha, got: {:?}",
        sl_color
    );
}

// -----------------------------------------------------------------------
// Hover highlight
// -----------------------------------------------------------------------

use crate::camera::Camera2D;
use crate::widget::compute::{ComputeContext, Viewport};
use crate::widget::hit_test::HitZoneKind;
use crate::widget::theme::Theme;
use crate::widget::AnnotationId;
use midas_data::CandleBuffer;

/// Build a minimal `ComputeContext` for hover highlight tests.
fn make_compute_ctx<'a>(
    camera: &'a Camera2D,
    data: &'a dyn midas_core::CandleData,
    theme: &'a Theme,
    hovered: Option<(AnnotationId, HitZoneKind)>,
) -> ComputeContext<'a> {
    ComputeContext {
        camera,
        data,
        viewport: Viewport {
            width: camera.viewport_width,
            height: camera.viewport_height,
        },
        theme,
        snap_fn: &|_| None,
        candle_duration_ms: 60_000.0,
        collapse_gaps: false,
        separator_y: camera.viewport_height as f32 * 0.80,
        dpi_scale: 1.0,
        hovered_annotation: hovered,
        hovered_decorator_groups: &[],
        selected_annotation: None,
        drag_ghost: None,
    }
}

#[test]
fn compute_bracket_tp_hovered_wider() {
    let mut b = make_bracket(185.0, 192.0, 182.0);
    b.status = BracketStatus::Active;
    let camera = Camera2D {
        viewport_width: 1920,
        viewport_height: 1080,
        time_start: 0.0,
        time_end: 100_000.0,
        price_low: 170.0,
        price_high: 200.0,
        dpi_scale: 1.0,
    };
    let data = CandleBuffer::new();
    let theme = Theme::default();
    let aid = AnnotationId(42);

    let ctx = make_compute_ctx(&camera, &data, &theme, Some((aid, HitZoneKind::BracketTP)));
    let out = compute_bracket(&b, aid, &ctx, 1.0);

    let ctx_no = make_compute_ctx(&camera, &data, &theme, None);
    let out_no = compute_bracket(&b, aid, &ctx_no, 1.0);

    let max_h_hovered = out
        .lines
        .iter()
        .map(|l| (l.rect[3] - l.rect[1]).abs())
        .fold(0.0_f32, f32::max);
    let max_h_normal = out_no
        .lines
        .iter()
        .map(|l| (l.rect[3] - l.rect[1]).abs())
        .fold(0.0_f32, f32::max);
    assert!(
        max_h_hovered > max_h_normal,
        "Hovered TP should produce wider (taller) line rects: hovered={max_h_hovered}, normal={max_h_normal}"
    );
}

#[test]
fn compute_bracket_sl_hovered_wider() {
    let mut b = make_bracket(185.0, 192.0, 182.0);
    b.status = BracketStatus::Active;
    let camera = Camera2D {
        viewport_width: 1920,
        viewport_height: 1080,
        time_start: 0.0,
        time_end: 100_000.0,
        price_low: 170.0,
        price_high: 200.0,
        dpi_scale: 1.0,
    };
    let data = CandleBuffer::new();
    let theme = Theme::default();
    let aid = AnnotationId(42);

    let ctx = make_compute_ctx(&camera, &data, &theme, Some((aid, HitZoneKind::BracketSL)));
    let out = compute_bracket(&b, aid, &ctx, 1.0);

    let ctx_no = make_compute_ctx(&camera, &data, &theme, None);
    let out_no = compute_bracket(&b, aid, &ctx_no, 1.0);

    let max_h_hovered = out
        .lines
        .iter()
        .map(|l| (l.rect[3] - l.rect[1]).abs())
        .fold(0.0_f32, f32::max);
    let max_h_normal = out_no
        .lines
        .iter()
        .map(|l| (l.rect[3] - l.rect[1]).abs())
        .fold(0.0_f32, f32::max);
    assert!(
        max_h_hovered > max_h_normal,
        "Hovered SL should produce wider line rects: hovered={max_h_hovered}, normal={max_h_normal}"
    );
}

#[test]
fn compute_bracket_no_hover_normal_width() {
    let mut b = make_bracket(185.0, 192.0, 182.0);
    b.status = BracketStatus::Active;
    let camera = Camera2D {
        viewport_width: 1920,
        viewport_height: 1080,
        time_start: 0.0,
        time_end: 100_000.0,
        price_low: 170.0,
        price_high: 200.0,
        dpi_scale: 1.0,
    };
    let data = CandleBuffer::new();
    let theme = Theme::default();
    let aid = AnnotationId(42);

    // Hover on a different annotation
    let ctx = make_compute_ctx(
        &camera,
        &data,
        &theme,
        Some((AnnotationId(999), HitZoneKind::BracketTP)),
    );
    let out = compute_bracket(&b, aid, &ctx, 1.0);

    // No hover at all
    let ctx_no = make_compute_ctx(&camera, &data, &theme, None);
    let out_no = compute_bracket(&b, aid, &ctx_no, 1.0);

    // Both should produce identical line geometry
    assert_eq!(out.lines.len(), out_no.lines.len());
    for (a, b) in out.lines.iter().zip(out_no.lines.iter()) {
        assert!(
            (a.rect[3] - a.rect[1] - (b.rect[3] - b.rect[1])).abs() < f32::EPSILON,
            "Line heights should be identical when not hovered"
        );
    }
}

// -----------------------------------------------------------------------
// Slice 8a-ii: decorator emission parity + legacy-hit-zone regression
// -----------------------------------------------------------------------

/// Decorator-parity snapshot for a canonical Draft Long Market bracket.
///
/// Slice 8a-ii replaced the three per-leg emission shims with
/// `entry/tp/sl_decorator_group()` calls routed through
/// `compute_decorator_group()`. The primitive shape is now:
///
/// - **badges**: Non-Rect shapes (PointLeft, Circle) route through the
///   SDF badge pipeline. We expect at least one `BadgeInstance` per leg
///   (entry, TP, SL) plus the extra Circle overlay on the TP position
///   counter segment — so `>= 4`.
/// - **fills**: Badge dividers + entry button bg fills from the draft
///   action buttons still emit into the grid pipeline. No zone rects
///   (zones only emit for Active/PartialFill).
/// - **lines**: Segmented stroke for entry + TP + SL lines. Non-empty.
/// - **hit_zones**: Bracket drag hit zones (TP, SL) + legacy draft
///   buttons (Submit/Save/Cancel/CancelSL) + decorator segment hit zones.
///
/// The individual primitive counts depend on viewport width (segmented
/// line count) and layout measurements, so this test focuses on
/// structural invariants rather than exact tallies.
#[test]
fn compute_bracket_decorator_parity_snapshot() {
    let b = make_bracket(185.0, 192.0, 182.0);
    let camera = Camera2D {
        viewport_width: 1920,
        viewport_height: 1080,
        time_start: 0.0,
        time_end: 100_000.0,
        price_low: 170.0,
        price_high: 200.0,
        dpi_scale: 1.0,
    };
    let data = CandleBuffer::new();
    let theme = Theme::default();
    let aid = AnnotationId(7);
    let ctx = make_compute_ctx(&camera, &data, &theme, None);

    let out = compute_bracket(&b, aid, &ctx, 1.0);

    assert!(
        out.badges.len() >= 4,
        "expected >=4 BadgeInstance (entry + TP + TP-circle + SL), got {}",
        out.badges.len()
    );
    assert!(
        !out.lines.is_empty(),
        "Draft bracket emits patterned lines for entry/TP/SL"
    );
    // No zone fills (Active/PartialFill only).
    let zone_fills: Vec<_> = out
        .fills
        .iter()
        .filter(|f| f.rect[2] - f.rect[0] > 1000.0 && f.color[3] < 0.1)
        .collect();
    assert!(
        zone_fills.is_empty(),
        "Draft bracket should not emit wide translucent zone fills"
    );
    // R:R label still emits when both TP and SL are present.
    assert!(
        out.labels.iter().any(|l| l.text.starts_with("R:R")),
        "R:R label must still emit when TP+SL both set"
    );
    // The entry-badge segments include the quantity placeholder and the
    // formatted price for the entry leg.
    assert!(
        out.labels.iter().any(|l| l.text == "185.00"),
        "entry price segment must render as `185.00`"
    );
    assert!(
        out.labels.iter().any(|l| l.text == "192.00"),
        "TP price segment must render as `192.00`"
    );
    assert!(
        out.labels.iter().any(|l| l.text == "182.00"),
        "SL price segment must render as `182.00`"
    );
}

/// Slice 8b contract: Draft brackets emit decorator hit zones for the
/// entry/TP/SL groups (carrying `DecoratorAction::{Submit, Save,
/// CloseAnnotation, RemoveStopLoss, CreateStopLoss}` etc.) and no
/// longer produce any of the legacy bracket-button hit zones — those
/// enum variants were deleted along with the `emit_*_button` helpers.
#[test]
fn compute_bracket_emits_decorator_hit_zones_for_draft_bracket() {
    let b = make_bracket(185.0, 192.0, 182.0);
    let camera = Camera2D {
        viewport_width: 1920,
        viewport_height: 1080,
        time_start: 0.0,
        time_end: 100_000.0,
        price_low: 170.0,
        price_high: 200.0,
        dpi_scale: 1.0,
    };
    let data = CandleBuffer::new();
    let theme = Theme::default();
    let aid = AnnotationId(7);
    // Seed hover + group expansion so `OnGroupHover` items (Submit /
    // Save / Close / RemoveSL) actually emit from the compute pass.
    let hovered_groups: [(AnnotationId, u16); 3] = [(aid, 0), (aid, 1), (aid, 2)];
    let ctx = ComputeContext {
        camera: &camera,
        data: &data,
        viewport: Viewport {
            width: camera.viewport_width,
            height: camera.viewport_height,
        },
        theme: &theme,
        snap_fn: &|_| None,
        candle_duration_ms: 60_000.0,
        collapse_gaps: false,
        separator_y: camera.viewport_height as f32 * 0.80,
        dpi_scale: 1.0,
        hovered_annotation: Some((aid, HitZoneKind::LevelLine)),
        hovered_decorator_groups: &hovered_groups,
        selected_annotation: None,
        drag_ghost: None,
    };

    let out = compute_bracket(&b, aid, &ctx, 1.0);

    let has_action = |expected: DecoratorAction| {
        out.hit_zones.iter().any(|z| {
            matches!(
                z.kind,
                HitZoneKind::Decorator { action, .. } if action == expected
            )
        })
    };
    assert!(
        has_action(DecoratorAction::Submit),
        "entry decorator group must emit a Submit hit zone for draft brackets"
    );
    assert!(
        has_action(DecoratorAction::Save),
        "entry decorator group must emit a Save hit zone for draft brackets"
    );
    assert!(
        has_action(DecoratorAction::CloseAnnotation),
        "entry decorator group must emit a CloseAnnotation hit zone (bracket cancel)"
    );
    assert!(
        has_action(DecoratorAction::RemoveStopLoss),
        "SL decorator group must emit a RemoveStopLoss hit zone when SL is attached"
    );
}

#[test]
fn bracket_leg_new_shape_round_trip() {
    let leg = BracketLeg {
        line: PriceLine {
            price: 192.50,
            extent: LineExtent::FullWidth,
            stroke: LineStroke {
                color: [0.20, 0.78, 0.35, 1.0],
                width: 1.5,
                style: LineStyle::Solid,
            },
        },
        role: LegRole::TakeProfit,
        projected_pnl: Some(650.0),
        projected_pnl_pct: Some(2.34),
    };
    let json = serde_json::to_string(&leg).expect("serialize");
    let decoded: BracketLeg = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(decoded, leg);
    assert!((decoded.line.price - 192.50).abs() < f64::EPSILON);
    assert_eq!(decoded.role, LegRole::TakeProfit);
    assert_eq!(decoded.line.stroke.style, LineStyle::Solid);
    assert!((decoded.line.stroke.width - 1.5).abs() < f32::EPSILON);
}

// -----------------------------------------------------------------------
// Slice 8a-ii: decorator group constructors
// -----------------------------------------------------------------------

use super::decorators::{entry_decorator_group, sl_decorator_group, tp_decorator_group};
use crate::widget::decorator::{
    BadgeShape, DecoratorAction, DecoratorAnchor, FlexDirection, ItemContent, Visibility,
};

#[test]
fn entry_decorator_group_limit_long_has_three_segments() {
    let mut b = make_bracket(185.0, 192.0, 182.0);
    b.entry_type = EntryType::Limit;
    b.side = BracketSide::Long;
    b.quantity = Some(5000.0);
    let group = entry_decorator_group(&b);

    assert_eq!(group.group_id, 0);
    assert!(matches!(group.anchor, DecoratorAnchor::RightEdge));
    assert!(matches!(group.direction, FlexDirection::Row));

    // Items: [close, submit, save, main badge, quick-create stack]
    // `make_bracket` is a Draft bracket with a non-zero entry price,
    // so both `Submit` and `Save` buttons are emitted before the badge.
    assert_eq!(group.items.len(), 5);

    let badge = match &group.items[3].content {
        ItemContent::Badge(b) => b.as_ref(),
        _ => panic!("expected badge at slot 3"),
    };
    assert!(matches!(badge.shape, BadgeShape::PointLeft { .. }));
    assert_eq!(badge.segments.len(), 3);
    // Segment 0: type glyph, Segment 1: quantity "5000", Segment 2: price "185.00"
    assert_eq!(badge.segments[1].text, "5000");
    assert_eq!(badge.segments[2].text, "185.00");
    // Segment actions match the plan's wiring.
    assert_eq!(
        badge.segments[0].action,
        Some(DecoratorAction::CycleEntryType)
    );
    assert_eq!(badge.segments[1].action, Some(DecoratorAction::EditQuantity));
    assert_eq!(badge.segments[2].action, Some(DecoratorAction::EditPrice));
}

#[test]
fn entry_decorator_group_hover_close_button_is_on_group_hover_only() {
    let b = make_bracket(185.0, 192.0, 182.0);
    let group = entry_decorator_group(&b);
    let close = &group.items[0];
    assert!(matches!(close.visibility, Visibility::OnGroupHover));
    assert_eq!(close.action, Some(DecoratorAction::CloseAnnotation));
}

#[test]
fn entry_decorator_group_tp_sl_stack_is_on_group_hover_only() {
    let b = make_bracket(185.0, 192.0, 182.0);
    let group = entry_decorator_group(&b);
    // Draft bracket: [close, submit, save, badge, stack] — stack is last.
    let stack_item = group.items.last().expect("non-empty items");
    assert!(matches!(stack_item.visibility, Visibility::OnGroupHover));
    let inner = match &stack_item.content {
        ItemContent::Stack(g) => g.as_ref(),
        _ => panic!("expected Stack as the final item"),
    };
    assert!(matches!(inner.direction, FlexDirection::Column));
    assert_eq!(inner.items.len(), 2);
    assert_eq!(
        inner.items[0].action,
        Some(DecoratorAction::CreateTakeProfit)
    );
    assert_eq!(inner.items[1].action, Some(DecoratorAction::CreateStopLoss));
}

#[test]
fn tp_decorator_group_position_count_is_circle_segment() {
    let b = make_bracket(185.0, 192.0, 182.0);
    let group = tp_decorator_group(&b).expect("TP present");
    assert_eq!(group.group_id, 1);
    let badge = match &group.items[0].content {
        ItemContent::Badge(bb) => bb.as_ref(),
        _ => panic!("expected badge"),
    };
    // Segments: [T, position_count, pct, price]
    assert_eq!(badge.segments.len(), 4);
    let circle_seg = &badge.segments[1];
    assert!(matches!(
        circle_seg.shape_override,
        Some(BadgeShape::Circle)
    ));
    assert_eq!(circle_seg.fill_override, Some([0.0, 0.0, 0.0, 1.0]));
}

#[test]
fn tp_decorator_group_none_when_tp_missing() {
    let mut b = make_bracket(185.0, 192.0, 182.0);
    b.take_profit = None;
    assert!(tp_decorator_group(&b).is_none());
}

#[test]
fn entry_decorator_nested_stack_has_distinct_group_id_from_tp_sl() {
    // Regression for BUG 2: the entry group's nested hover stack used
    // to share `group_id: 1` with `tp_decorator_group`, so any hover
    // on the TP line also flagged the entry's nested stack as hovered.
    // The nested stack id is now namespaced above 0x80 and must not
    // collide with either sibling top-level id (TP=1, SL=2).
    let b = make_bracket(185.0, 192.0, 182.0);
    let entry = entry_decorator_group(&b);
    let stack_item = entry.items.last().expect("non-empty items");
    let inner = match &stack_item.content {
        ItemContent::Stack(g) => g.as_ref(),
        _ => panic!("expected Stack as the final entry item"),
    };
    assert_ne!(inner.group_id, 1, "must not collide with tp group_id");
    assert_ne!(inner.group_id, 2, "must not collide with sl group_id");
    assert!(
        inner.group_id >= 0x80,
        "nested stack ids live in the 0x80+ namespace, got {}",
        inner.group_id
    );

    // Sanity: TP and SL still sit on their canonical top-level ids.
    assert_eq!(tp_decorator_group(&b).expect("tp").group_id, 1);
    assert_eq!(sl_decorator_group(&b).expect("sl").group_id, 2);
}

#[test]
fn sl_decorator_group_uses_orange_fill() {
    let b = make_bracket(185.0, 192.0, 182.0);
    let group = sl_decorator_group(&b).expect("SL present");
    assert_eq!(group.group_id, 2);
    // SL group is `[hover close button, main badge]`.
    let close = &group.items[0];
    assert!(matches!(close.visibility, Visibility::OnGroupHover));
    assert_eq!(close.action, Some(DecoratorAction::RemoveStopLoss));
    let badge = match &group.items[1].content {
        ItemContent::Badge(bb) => bb.as_ref(),
        _ => panic!("expected badge at slot 1"),
    };
    // Orange SL base color.
    assert!(
        (badge.fill[0] - 1.0).abs() < 0.01
            && (badge.fill[1] - 0.60).abs() < 0.01
            && (badge.fill[2] - 0.0).abs() < 0.01,
        "SL badge fill should be orange, got {:?}",
        badge.fill
    );
    // Segments: [S, risk, price]
    assert_eq!(badge.segments.len(), 3);
    assert_eq!(badge.segments[0].text, "S");
    assert_eq!(badge.segments[2].text, "182.00");
}

#[test]
fn sl_decorator_group_none_when_sl_missing() {
    let mut b = make_bracket(185.0, 192.0, 182.0);
    b.stop_loss = None;
    assert!(sl_decorator_group(&b).is_none());
}

#[test]
fn bracket_status_active_resolves_to_solid_stroke_on_price_line() {
    let mut b = make_bracket(185.0, 192.0, 182.0);
    b.status = BracketStatus::Active;
    let stroke = b.leg_style(LegRole::TakeProfit);
    assert_eq!(stroke.style, LineStyle::Solid);
}

#[test]
fn bracket_status_draft_resolves_to_pattern_stroke_on_price_line() {
    let mut b = make_bracket(185.0, 192.0, 182.0);
    b.status = BracketStatus::Draft;
    let stroke = b.leg_style(LegRole::TakeProfit);
    assert_eq!(stroke.style, LineStyle::Pattern(smallvec![6.0, 4.0]));
}

// -----------------------------------------------------------------------
// Slice 0: SL stroke is dotted across every BracketStatus variant.
// -----------------------------------------------------------------------

/// The Slice 0 contract from the ticker-order-state plan: the SL leg
/// must render dotted no matter what status the bracket is in. The
/// match in `leg_style()` is exhaustive on `BracketStatus`, so this
/// test iterates every variant and asserts the dotted invariant. If a
/// new `BracketStatus` variant is ever added, the `match` below must
/// grow a new arm — at which point this test forces a decision about
/// the SL stroke for that variant.
#[test]
fn leg_style_sl_dotted_across_all_statuses() {
    let statuses = [
        BracketStatus::Draft,
        BracketStatus::Pending,
        BracketStatus::PartialFill,
        BracketStatus::Active,
        BracketStatus::Closed,
        BracketStatus::Cancelled,
    ];
    for status in statuses {
        // Compile-time exhaustiveness guard: if a new BracketStatus
        // variant lands, this match needs updating and the compiler
        // will catch the omission here.
        match status {
            BracketStatus::Draft
            | BracketStatus::Pending
            | BracketStatus::PartialFill
            | BracketStatus::Active
            | BracketStatus::Closed
            | BracketStatus::Cancelled => {}
        }
        let mut b = make_bracket(185.0, 192.0, 182.0);
        b.status = status;
        let stroke = b.leg_style(LegRole::StopLoss);
        assert_eq!(
            stroke.style,
            LineStyle::dotted(),
            "SL stroke must be dotted for status {:?}, got {:?}",
            status,
            stroke.style
        );
    }
}

// -----------------------------------------------------------------------
// Slice 0: tight TP/SL overlap stacking test.
// -----------------------------------------------------------------------

/// When TP and SL prices land close enough that the decorator badges
/// would overlap horizontally, `compute_bracket` must offset each
/// badge vertically in screen space so they clear each other.
///
/// We build a Draft bracket with entry=100.00, tp=100.10, sl=99.90 on
/// a camera where one price unit maps to roughly 10.8 px, so the raw
/// gap between TP and SL lines is ~2.2 px — well below the 22 px
/// badge-height threshold the compute pass uses. The test asserts
/// that the resulting BadgeInstance rects for the TP and SL leg
/// decorators do not overlap on the Y axis.
#[test]
fn compute_bracket_tight_prices_stack_badges_vertically() {
    let b = make_bracket(100.00, 100.10, 99.90);
    let camera = Camera2D {
        viewport_width: 1920,
        viewport_height: 1080,
        time_start: 0.0,
        time_end: 100_000.0,
        price_low: 50.0,
        price_high: 150.0,
        dpi_scale: 1.0,
    };
    let data = CandleBuffer::new();
    let theme = Theme::default();
    let aid = AnnotationId(11);
    let ctx = make_compute_ctx(&camera, &data, &theme, None);

    // Raw Y gap between the TP and SL price lines on this camera.
    let tp_y_raw = camera.price_to_y(100.10);
    let sl_y_raw = camera.price_to_y(99.90);
    assert!(
        (tp_y_raw - sl_y_raw).abs() < 22.0,
        "fixture must place TP/SL inside the overlap threshold, got gap {}",
        (tp_y_raw - sl_y_raw).abs()
    );

    let out = compute_bracket(&b, aid, &ctx, 1.0);

    // TP and SL decorator badges are uniquely identifiable via their
    // price-segment labels ("100.10" and "99.90"). These labels are
    // centered on the decorator badge's Y, so the label screen_y is
    // the same as the badge center-Y.
    let tp_label = out
        .labels
        .iter()
        .find(|l| l.text == "100.10")
        .expect("TP price segment must render as 100.10");
    let sl_label = out
        .labels
        .iter()
        .find(|l| l.text == "99.90")
        .expect("SL price segment must render as 99.90");

    // TP must sit visually above SL (y decreases with rising price
    // and the stacking shim moves TP up / SL down).
    assert!(
        tp_label.screen_y < sl_label.screen_y,
        "TP badge (y={}) must sit above SL badge (y={}) after stacking",
        tp_label.screen_y,
        sl_label.screen_y,
    );

    // The gap between the two badges must be at least `badge_height`
    // so the 20-px-tall bodies clear each other vertically.
    let gap = sl_label.screen_y - tp_label.screen_y;
    assert!(
        gap >= 20.0,
        "TP/SL badge gap must be >= badge height (20px) after stacking, got {}",
        gap
    );

    // Underlying leg lines still emit at the true prices: segmented
    // line rects centered on `price_to_y(100.10)` and `price_to_y(99.90)`.
    let tp_line_present = out
        .lines
        .iter()
        .any(|l| ((l.rect[1] + l.rect[3]) * 0.5 - tp_y_raw).abs() < 2.0);
    let sl_line_present = out
        .lines
        .iter()
        .any(|l| ((l.rect[1] + l.rect[3]) * 0.5 - sl_y_raw).abs() < 2.0);
    assert!(
        tp_line_present,
        "TP leg line must still render at the true TP price"
    );
    assert!(
        sl_line_present,
        "SL leg line must still render at the true SL price"
    );
}

/// Regression guard: when TP and SL are far apart the stacking branch
/// must *not* fire — badges keep their natural anchor on each price
/// line. This prevents the overlap shim from accidentally displacing
/// decorator groups in the common case.
#[test]
fn compute_bracket_wide_prices_do_not_shift_badges() {
    let b = make_bracket(185.0, 192.0, 178.0);
    let camera = Camera2D {
        viewport_width: 1920,
        viewport_height: 1080,
        time_start: 0.0,
        time_end: 100_000.0,
        price_low: 170.0,
        price_high: 200.0,
        dpi_scale: 1.0,
    };
    let data = CandleBuffer::new();
    let theme = Theme::default();
    let aid = AnnotationId(13);
    let ctx = make_compute_ctx(&camera, &data, &theme, None);

    let out = compute_bracket(&b, aid, &ctx, 1.0);

    // With this camera the TP and SL lines are ~500 px apart — the
    // stacking shim must leave their decorator anchors on the real
    // price lines.
    let tp_label = out
        .labels
        .iter()
        .find(|l| l.text == "192.00")
        .expect("TP price label present");
    let sl_label = out
        .labels
        .iter()
        .find(|l| l.text == "178.00")
        .expect("SL price label present");
    let expected_tp_y = camera.price_to_y(192.0);
    let expected_sl_y = camera.price_to_y(178.0);
    assert!(
        (tp_label.screen_y - expected_tp_y).abs() < 1.0,
        "TP badge should sit on the TP price line ({}), got {}",
        expected_tp_y,
        tp_label.screen_y
    );
    assert!(
        (sl_label.screen_y - expected_sl_y).abs() < 1.0,
        "SL badge should sit on the SL price line ({}), got {}",
        expected_sl_y,
        sl_label.screen_y
    );
}
