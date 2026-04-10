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
    // Draft unsaved alpha_mult = 0.50, base TP alpha = 1.0 => 0.50
    assert!((color[3] - 0.50).abs() < f32::EPSILON);
}

#[test]
fn test_leg_style_cancelled_sl() {
    let mut b = make_bracket(185.0, 192.0, 182.0);
    b.status = BracketStatus::Cancelled;
    let (style, width, color) = b.leg_style(LegRole::StopLoss);
    // SL is always dotted (orange), regardless of status.
    assert!(matches!(style, LineStyle::Dotted { .. }));
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
    let (_style, _width, color) = b.leg_style(LegRole::Entry);
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
    let (_style, _width, color) = b.leg_style(LegRole::Entry);
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
    let (_style, _width, color) = b.leg_style(LegRole::Entry);
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
    let (_style, _width, color) = b.leg_style(LegRole::Entry);
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
    let (_style, _width, color) = b.leg_style(LegRole::Entry);
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
    let (_style, _width, color) = b.leg_style(LegRole::Entry);
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
    // Simulate old JSON without entry_type / entry_stop_price / wrong_side_warning
    let json = r#"{
        "entry": {"price": 185.0, "timestamp": null, "color": null, "style": "Solid", "line_width": 1.0, "label": null},
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
        &LineStyle::Dashed {
            dash_len: 6.0,
            gap_len: 4.0,
        },
    );
    // 100px / (6+4) = 10 segments
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
        &LineStyle::Dotted { dot_spacing: 4.0 },
    );
    // 100px / 4.0 = 25 dots
    assert_eq!(
        segs.len(),
        25,
        "expected 25 dot segments, got {}",
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
    let (_style, _width, color) = b.leg_style(LegRole::Entry);
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
    let (_style, _width, color) = b.leg_style(LegRole::Entry);
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
    let (_style, _width, color) = b.leg_style(LegRole::Entry);
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
    let (_style, _width, color) = b.leg_style(LegRole::Entry);
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
    let (style, width, color) = b.leg_style(LegRole::StopLoss);
    assert!(
        matches!(style, LineStyle::Dotted { .. }),
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
    let (style, _width, color) = b.leg_style(LegRole::StopLoss);
    assert!(
        matches!(style, LineStyle::Dotted { .. }),
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
