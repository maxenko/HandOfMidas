use super::*;

#[test]
fn resolve_absolute_returns_value() {
    let price = resolve_price(PriceInputMode::Absolute, 192.0, 185.0, OrderSide::Buy, true);
    assert!((price - 192.0).abs() < f64::EPSILON);
}

#[test]
fn resolve_offset_buy_tp() {
    let price = resolve_price(PriceInputMode::Offset, 6.50, 185.0, OrderSide::Buy, true);
    assert!((price - 191.50).abs() < 0.01);
}

#[test]
fn resolve_offset_buy_sl() {
    let price = resolve_price(PriceInputMode::Offset, 3.0, 185.0, OrderSide::Buy, false);
    assert!((price - 182.0).abs() < 0.01);
}

#[test]
fn resolve_percent_buy_tp() {
    let price = resolve_price(PriceInputMode::Percent, 3.5, 185.0, OrderSide::Buy, true);
    assert!((price - 191.475).abs() < 0.01);
}

#[test]
fn resolve_percent_buy_sl() {
    let price = resolve_price(PriceInputMode::Percent, 1.9, 185.0, OrderSide::Buy, false);
    assert!((price - 181.485).abs() < 0.01);
}

#[test]
fn resolve_offset_sell_tp() {
    let price = resolve_price(PriceInputMode::Offset, 6.50, 185.0, OrderSide::Sell, true);
    assert!((price - 178.50).abs() < 0.01);
}

#[test]
fn resolve_percent_sell_sl() {
    let price = resolve_price(PriceInputMode::Percent, 1.9, 185.0, OrderSide::Sell, false);
    assert!((price - 188.515).abs() < 0.01);
}

#[test]
fn risk_reward_full() {
    let rr = calculate_risk_reward(185.0, Some(192.0), Some(182.0), 100.0).unwrap();
    assert!((rr.risk_per_share - 3.0).abs() < 0.01);
    assert!((rr.reward_per_share - 7.0).abs() < 0.01);
    assert!((rr.total_risk - 300.0).abs() < 0.01);
    assert!((rr.total_reward - 700.0).abs() < 0.01);
    assert!((rr.ratio - 2.333).abs() < 0.01);
}

#[test]
fn risk_reward_no_sl() {
    assert!(calculate_risk_reward(185.0, Some(192.0), None, 100.0).is_none());
}

#[test]
fn risk_reward_zero_risk() {
    assert!(calculate_risk_reward(185.0, Some(192.0), Some(185.0), 100.0).is_none());
}

#[test]
fn validate_missing_symbol() {
    let state = OrderPanelState::default();
    let errors = validate_panel(&state);
    assert!(errors.iter().any(|(f, _)| f == "symbol"));
}

#[test]
fn validate_buy_tp_below_price_rejected() {
    let state = OrderPanelState {
        symbol: "AAPL".to_string(),
        last_price: Some(185.0),
        tp_enabled: true,
        tp_value: "180.0".to_string(),
        sl_enabled: false,
        ..Default::default()
    };
    let errors = validate_panel(&state);
    assert!(errors.iter().any(|(f, _)| f == "tp"));
}

#[test]
fn validate_valid_bracket() {
    let state = OrderPanelState {
        symbol: "AAPL".to_string(),
        last_price: Some(185.0),
        quantity: "100".to_string(),
        tp_enabled: true,
        tp_value: "192.0".to_string(),
        sl_enabled: true,
        sl_value: "182.0".to_string(),
        ..Default::default()
    };
    let errors = validate_panel(&state);
    assert!(errors.is_empty(), "expected no errors, got: {errors:?}");
}

// -- create_bracket_annotation tests --

#[test]
fn create_bracket_long_with_tp_sl() {
    use midas_chart::widget::order_bracket::*;

    let bracket =
        create_bracket_annotation(BracketSide::Long, 185.0, Some(192.0), Some(182.0), 100.0);
    assert_eq!(bracket.side, BracketSide::Long);
    assert_eq!(bracket.status, BracketStatus::Pending);
    assert!((bracket.entry.line.price - 185.0).abs() < f64::EPSILON);
    assert!(bracket.take_profit.is_some());
    assert!((bracket.take_profit.unwrap().line.price - 192.0).abs() < f64::EPSILON);
    assert!(bracket.stop_loss.is_some());
    assert!((bracket.stop_loss.unwrap().line.price - 182.0).abs() < f64::EPSILON);
    assert_eq!(bracket.quantity, Some(100.0));
}

#[test]
fn create_bracket_short_no_tp() {
    use midas_chart::widget::order_bracket::*;

    let bracket = create_bracket_annotation(BracketSide::Short, 185.0, None, Some(188.0), 50.0);
    assert_eq!(bracket.side, BracketSide::Short);
    assert_eq!(bracket.status, BracketStatus::Pending);
    assert!(bracket.take_profit.is_none());
    assert!(bracket.stop_loss.is_some());
    assert_eq!(bracket.quantity, Some(50.0));
}

#[test]
fn create_bracket_no_legs() {
    use midas_chart::widget::order_bracket::*;

    let bracket = create_bracket_annotation(BracketSide::Long, 185.0, None, None, 200.0);
    assert!(bracket.take_profit.is_none());
    assert!(bracket.stop_loss.is_none());
    assert!((bracket.entry.line.price - 185.0).abs() < f64::EPSILON);
}

// -- map_lifecycle_to_chart_status tests --

#[test]
fn lifecycle_submitted_maps_to_pending() {
    use midas_chart::widget::order_bracket::BracketStatus;
    assert_eq!(
        map_lifecycle_to_chart_status("Submitted"),
        BracketStatus::Pending
    );
}

#[test]
fn lifecycle_entry_filled_maps_to_active() {
    use midas_chart::widget::order_bracket::BracketStatus;
    assert_eq!(
        map_lifecycle_to_chart_status("EntryFilled"),
        BracketStatus::Active
    );
}

#[test]
fn lifecycle_take_profit_hit_maps_to_closed() {
    use midas_chart::widget::order_bracket::BracketStatus;
    assert_eq!(
        map_lifecycle_to_chart_status("TakeProfitHit"),
        BracketStatus::Closed
    );
}

#[test]
fn lifecycle_stop_loss_hit_maps_to_closed() {
    use midas_chart::widget::order_bracket::BracketStatus;
    assert_eq!(
        map_lifecycle_to_chart_status("StopLossHit"),
        BracketStatus::Closed
    );
}

#[test]
fn lifecycle_cancelled_maps_to_cancelled() {
    use midas_chart::widget::order_bracket::BracketStatus;
    assert_eq!(
        map_lifecycle_to_chart_status("Cancelled"),
        BracketStatus::Cancelled
    );
}

#[test]
fn lifecycle_rejected_maps_to_cancelled() {
    use midas_chart::widget::order_bracket::BracketStatus;
    assert_eq!(
        map_lifecycle_to_chart_status("Rejected"),
        BracketStatus::Cancelled
    );
}

#[test]
fn lifecycle_unknown_defaults_to_pending() {
    use midas_chart::widget::order_bracket::BracketStatus;
    assert_eq!(
        map_lifecycle_to_chart_status("SomethingNew"),
        BracketStatus::Pending
    );
}

#[test]
fn validate_non_numeric_quantity() {
    let state = OrderPanelState {
        symbol: "AAPL".to_string(),
        last_price: Some(185.0),
        quantity: "abc".to_string(),
        ..Default::default()
    };
    let errors = validate_panel(&state);
    assert!(
        errors
            .iter()
            .any(|(f, msg)| f == "quantity" && msg.contains("not a number")),
        "expected 'not a number' error for quantity, got: {errors:?}"
    );
}

#[test]
fn validate_non_numeric_tp_value() {
    let state = OrderPanelState {
        symbol: "AAPL".to_string(),
        last_price: Some(185.0),
        quantity: "100".to_string(),
        tp_enabled: true,
        tp_value: "xyz".to_string(),
        sl_enabled: false,
        ..Default::default()
    };
    let errors = validate_panel(&state);
    assert!(
        errors
            .iter()
            .any(|(f, msg)| f == "tp" && msg.contains("not a number")),
        "expected 'not a number' error for TP, got: {errors:?}"
    );
    // Should not double-report with generic "Invalid TP price"
    let tp_errors: Vec<_> = errors.iter().filter(|(f, _)| f == "tp").collect();
    assert_eq!(
        tp_errors.len(),
        1,
        "TP should have exactly one error, got: {tp_errors:?}"
    );
}

#[test]
fn validate_non_numeric_sl_value() {
    let state = OrderPanelState {
        symbol: "AAPL".to_string(),
        last_price: Some(185.0),
        quantity: "100".to_string(),
        tp_enabled: false,
        sl_enabled: true,
        sl_value: "???".to_string(),
        ..Default::default()
    };
    let errors = validate_panel(&state);
    assert!(
        errors
            .iter()
            .any(|(f, msg)| f == "sl" && msg.contains("not a number")),
        "expected 'not a number' error for SL, got: {errors:?}"
    );
    let sl_errors: Vec<_> = errors.iter().filter(|(f, _)| f == "sl").collect();
    assert_eq!(
        sl_errors.len(),
        1,
        "SL should have exactly one error, got: {sl_errors:?}"
    );
}

// -- validate_bracket tests --

#[test]
fn validate_bracket_valid_long_with_sl() {
    use midas_chart::widget::level::LineStyle;
    use midas_chart::widget::order_bracket::*;

    let bracket = OrderBracket {
        entry: BracketLeg {
            line: midas_chart::widget::PriceLine {
                price: 185.0,
                extent: midas_chart::widget::LineExtent::FullWidth,
                stroke: midas_chart::widget::LineStroke {
                    color: [0.0, 0.0, 0.0, 1.0],
                    width: 1.5,
                    style: LineStyle::Solid,
                },
            },
            role: LegRole::Entry,
            projected_pnl: None,
            projected_pnl_pct: None,
        },
        take_profit: None,
        stop_loss: Some(BracketLeg {
            line: midas_chart::widget::PriceLine {
                price: 180.0,
                extent: midas_chart::widget::LineExtent::FullWidth,
                stroke: midas_chart::widget::LineStroke {
                    color: [0.0, 0.0, 0.0, 1.0],
                    width: 1.5,
                    style: LineStyle::Solid,
                },
            },
            role: LegRole::Entry,
            projected_pnl: None,
            projected_pnl_pct: None,
        }),
        side: BracketSide::Long,
        status: BracketStatus::Draft,
        quantity: None,
        saved: false,
        filled_qty: None,
        entry_type: EntryType::Market,
        entry_stop_price: None,
        wrong_side_warning: false,
    };
    let errors = validate_bracket(&bracket, 100.0);
    assert!(errors.is_empty(), "expected no errors, got: {errors:?}");
}

#[test]
fn validate_bracket_zero_entry_price() {
    use midas_chart::widget::level::LineStyle;
    use midas_chart::widget::order_bracket::*;

    let bracket = OrderBracket {
        entry: BracketLeg {
            line: midas_chart::widget::PriceLine {
                price: 0.0,
                extent: midas_chart::widget::LineExtent::FullWidth,
                stroke: midas_chart::widget::LineStroke {
                    color: [0.0, 0.0, 0.0, 1.0],
                    width: 1.5,
                    style: LineStyle::Solid,
                },
            },
            role: LegRole::Entry,
            projected_pnl: None,
            projected_pnl_pct: None,
        },
        take_profit: None,
        stop_loss: None,
        side: BracketSide::Long,
        status: BracketStatus::Draft,
        quantity: None,
        saved: false,
        filled_qty: None,
        entry_type: EntryType::Market,
        entry_stop_price: None,
        wrong_side_warning: false,
    };
    let errors = validate_bracket(&bracket, 100.0);
    assert!(
        errors.iter().any(|(f, _)| f == "entry"),
        "expected entry error, got: {errors:?}"
    );
}

#[test]
fn validate_bracket_zero_quantity() {
    use midas_chart::widget::level::LineStyle;
    use midas_chart::widget::order_bracket::*;

    let bracket = OrderBracket {
        entry: BracketLeg {
            line: midas_chart::widget::PriceLine {
                price: 185.0,
                extent: midas_chart::widget::LineExtent::FullWidth,
                stroke: midas_chart::widget::LineStroke {
                    color: [0.0, 0.0, 0.0, 1.0],
                    width: 1.5,
                    style: LineStyle::Solid,
                },
            },
            role: LegRole::Entry,
            projected_pnl: None,
            projected_pnl_pct: None,
        },
        take_profit: None,
        stop_loss: None,
        side: BracketSide::Long,
        status: BracketStatus::Draft,
        quantity: None,
        saved: false,
        filled_qty: None,
        entry_type: EntryType::Market,
        entry_stop_price: None,
        wrong_side_warning: false,
    };
    let errors = validate_bracket(&bracket, 0.0);
    assert!(
        errors.iter().any(|(f, _)| f == "quantity"),
        "expected quantity error, got: {errors:?}"
    );
}

#[test]
fn validate_bracket_long_sl_above_entry() {
    use midas_chart::widget::level::LineStyle;
    use midas_chart::widget::order_bracket::*;

    let bracket = OrderBracket {
        entry: BracketLeg {
            line: midas_chart::widget::PriceLine {
                price: 185.0,
                extent: midas_chart::widget::LineExtent::FullWidth,
                stroke: midas_chart::widget::LineStroke {
                    color: [0.0, 0.0, 0.0, 1.0],
                    width: 1.5,
                    style: LineStyle::Solid,
                },
            },
            role: LegRole::Entry,
            projected_pnl: None,
            projected_pnl_pct: None,
        },
        take_profit: None,
        stop_loss: Some(BracketLeg {
            line: midas_chart::widget::PriceLine {
                price: 190.0,
                extent: midas_chart::widget::LineExtent::FullWidth,
                stroke: midas_chart::widget::LineStroke {
                    color: [0.0, 0.0, 0.0, 1.0],
                    width: 1.5,
                    style: LineStyle::Solid,
                },
            },
            role: LegRole::Entry,
            projected_pnl: None,
            projected_pnl_pct: None,
        }),
        side: BracketSide::Long,
        status: BracketStatus::Draft,
        quantity: None,
        saved: false,
        filled_qty: None,
        entry_type: EntryType::Market,
        entry_stop_price: None,
        wrong_side_warning: false,
    };
    let errors = validate_bracket(&bracket, 100.0);
    assert!(
        errors
            .iter()
            .any(|(f, msg)| f == "sl" && msg.contains("below entry")),
        "expected SL constraint error, got: {errors:?}"
    );
}

#[test]
fn validate_bracket_short_sl_below_entry() {
    use midas_chart::widget::level::LineStyle;
    use midas_chart::widget::order_bracket::*;

    let bracket = OrderBracket {
        entry: BracketLeg {
            line: midas_chart::widget::PriceLine {
                price: 185.0,
                extent: midas_chart::widget::LineExtent::FullWidth,
                stroke: midas_chart::widget::LineStroke {
                    color: [0.0, 0.0, 0.0, 1.0],
                    width: 1.5,
                    style: LineStyle::Solid,
                },
            },
            role: LegRole::Entry,
            projected_pnl: None,
            projected_pnl_pct: None,
        },
        take_profit: None,
        stop_loss: Some(BracketLeg {
            line: midas_chart::widget::PriceLine {
                price: 180.0,
                extent: midas_chart::widget::LineExtent::FullWidth,
                stroke: midas_chart::widget::LineStroke {
                    color: [0.0, 0.0, 0.0, 1.0],
                    width: 1.5,
                    style: LineStyle::Solid,
                },
            },
            role: LegRole::Entry,
            projected_pnl: None,
            projected_pnl_pct: None,
        }),
        side: BracketSide::Short,
        status: BracketStatus::Draft,
        quantity: None,
        saved: false,
        filled_qty: None,
        entry_type: EntryType::Market,
        entry_stop_price: None,
        wrong_side_warning: false,
    };
    let errors = validate_bracket(&bracket, 100.0);
    assert!(
        errors
            .iter()
            .any(|(f, msg)| f == "sl" && msg.contains("above entry")),
        "expected SL constraint error, got: {errors:?}"
    );
}

// -- StopLimit validation tests --

#[test]
fn validate_bracket_stop_limit_missing_stop_price() {
    use midas_chart::widget::level::LineStyle;
    use midas_chart::widget::order_bracket::*;

    let bracket = OrderBracket {
        entry: BracketLeg {
            line: midas_chart::widget::PriceLine {
                price: 184.50,
                extent: midas_chart::widget::LineExtent::FullWidth,
                stroke: midas_chart::widget::LineStroke {
                    color: [0.0, 0.0, 0.0, 1.0],
                    width: 1.5,
                    style: LineStyle::Solid,
                },
            },
            role: LegRole::Entry,
            projected_pnl: None,
            projected_pnl_pct: None,
        },
        take_profit: None,
        stop_loss: None,
        side: BracketSide::Long,
        status: BracketStatus::Draft,
        quantity: None,
        saved: false,
        filled_qty: None,
        entry_type: EntryType::StopLimit,
        entry_stop_price: None, // Missing!
        wrong_side_warning: false,
    };
    let errors = validate_bracket(&bracket, 100.0);
    assert!(
        errors
            .iter()
            .any(|(f, msg)| f == "entry" && msg.contains("stop trigger")),
        "expected missing stop price error, got: {errors:?}"
    );
}

#[test]
fn validate_bracket_stop_limit_buy_limit_above_stop() {
    use midas_chart::widget::level::LineStyle;
    use midas_chart::widget::order_bracket::*;

    let bracket = OrderBracket {
        entry: BracketLeg {
            line: midas_chart::widget::PriceLine {
                price: 186.00,
                extent: midas_chart::widget::LineExtent::FullWidth,
                stroke: midas_chart::widget::LineStroke {
                    color: [0.0, 0.0, 0.0, 1.0],
                    width: 1.5,
                    style: LineStyle::Solid,
                },
            },
            role: LegRole::Entry,
            projected_pnl: None,
            projected_pnl_pct: None,
        },
        take_profit: None,
        stop_loss: None,
        side: BracketSide::Long,
        status: BracketStatus::Draft,
        quantity: None,
        saved: false,
        filled_qty: None,
        entry_type: EntryType::StopLimit,
        entry_stop_price: Some(185.00),
        wrong_side_warning: false,
    };
    let errors = validate_bracket(&bracket, 100.0);
    assert!(
        errors
            .iter()
            .any(|(f, msg)| f == "entry" && msg.contains("at or below")),
        "expected limit>stop error for BUY, got: {errors:?}"
    );
}

#[test]
fn validate_bracket_stop_limit_valid_buy() {
    use midas_chart::widget::level::LineStyle;
    use midas_chart::widget::order_bracket::*;

    let bracket = OrderBracket {
        entry: BracketLeg {
            line: midas_chart::widget::PriceLine {
                price: 184.50,
                extent: midas_chart::widget::LineExtent::FullWidth,
                stroke: midas_chart::widget::LineStroke {
                    color: [0.0, 0.0, 0.0, 1.0],
                    width: 1.5,
                    style: LineStyle::Solid,
                },
            },
            role: LegRole::Entry,
            projected_pnl: None,
            projected_pnl_pct: None,
        },
        take_profit: None,
        stop_loss: None,
        side: BracketSide::Long,
        status: BracketStatus::Draft,
        quantity: None,
        saved: false,
        filled_qty: None,
        entry_type: EntryType::StopLimit,
        entry_stop_price: Some(185.00),
        wrong_side_warning: false,
    };
    let errors = validate_bracket(&bracket, 100.0);
    assert!(
        !errors.iter().any(|(f, _)| f == "entry"),
        "expected no entry errors, got: {errors:?}"
    );
}

// -- OrderPanel (dockable) tests --

#[test]
fn order_panel_new_sets_symbol() {
    let id = OrderPanelId::new(1);
    let panel = OrderPanel::new(id, "AAPL".to_string());
    assert_eq!(panel.id, id);
    assert_eq!(panel.state.symbol, "AAPL");
    assert!(panel.state.visible);
    assert_eq!(panel.state.side, OrderSide::Buy);
    assert_eq!(panel.state.quantity, "100");
}

#[test]
fn order_panel_to_config_roundtrip() {
    let id = OrderPanelId::new(5);
    let mut panel = OrderPanel::new(id, "MSFT".to_string());
    panel.state.side = OrderSide::Sell;
    panel.state.quantity = "250".to_string();
    panel.symbol_link = LinkMode::ListenAll;

    let config = panel.to_config();
    assert_eq!(config.symbol, "MSFT");
    assert_eq!(config.side, "SELL");
    assert_eq!(config.quantity, "250");
    assert_eq!(config.symbol_link, LinkMode::ListenAll);

    let restored = OrderPanel::from_config(id, &config);
    assert_eq!(restored.id, id);
    assert_eq!(restored.state.symbol, "MSFT");
    assert_eq!(restored.state.side, OrderSide::Sell);
    assert_eq!(restored.state.quantity, "250");
    assert_eq!(restored.symbol_link, LinkMode::ListenAll);
    assert!(restored.state.visible);
}

#[test]
fn order_panel_from_config_defaults() {
    let config = midas_core::config::OrderPanelConfig::default();
    let panel = OrderPanel::from_config(OrderPanelId::new(1), &config);
    assert_eq!(panel.state.side, OrderSide::Buy);
    assert_eq!(panel.state.quantity, "100");
    assert!(panel.state.symbol.is_empty());
}

// -- bracket_active config persistence tests --

#[test]
fn to_config_persists_bracket_active_buy() {
    let id = OrderPanelId::new(1);
    let mut panel = OrderPanel::new(id, "AAPL".to_string());
    panel.state.bracket_active = Some(OrderSide::Buy);

    let config = panel.to_config();
    assert_eq!(config.bracket_active, Some("BUY".to_string()));
}

#[test]
fn to_config_persists_bracket_active_sell() {
    let id = OrderPanelId::new(1);
    let mut panel = OrderPanel::new(id, "AAPL".to_string());
    panel.state.bracket_active = Some(OrderSide::Sell);

    let config = panel.to_config();
    assert_eq!(config.bracket_active, Some("SELL".to_string()));
}

#[test]
fn to_config_persists_bracket_active_none() {
    let id = OrderPanelId::new(1);
    let panel = OrderPanel::new(id, "AAPL".to_string());

    let config = panel.to_config();
    assert_eq!(config.bracket_active, None);
}

#[test]
fn from_config_restores_bracket_active_buy() {
    let config = midas_core::config::OrderPanelConfig {
        bracket_active: Some("BUY".to_string()),
        ..Default::default()
    };
    let panel = OrderPanel::from_config(OrderPanelId::new(1), &config);
    assert_eq!(panel.state.bracket_active, Some(OrderSide::Buy));
}

#[test]
fn from_config_restores_bracket_active_sell() {
    let config = midas_core::config::OrderPanelConfig {
        bracket_active: Some("SELL".to_string()),
        ..Default::default()
    };
    let panel = OrderPanel::from_config(OrderPanelId::new(1), &config);
    assert_eq!(panel.state.bracket_active, Some(OrderSide::Sell));
}

#[test]
fn from_config_bracket_active_none_when_missing() {
    let config = midas_core::config::OrderPanelConfig::default();
    let panel = OrderPanel::from_config(OrderPanelId::new(1), &config);
    assert_eq!(panel.state.bracket_active, None);
}

#[test]
fn from_config_bracket_active_none_for_unknown_value() {
    let config = midas_core::config::OrderPanelConfig {
        bracket_active: Some("UNKNOWN".to_string()),
        ..Default::default()
    };
    let panel = OrderPanel::from_config(OrderPanelId::new(1), &config);
    assert_eq!(panel.state.bracket_active, None);
}

#[test]
fn bracket_active_roundtrip_buy() {
    let id = OrderPanelId::new(1);
    let mut panel = OrderPanel::new(id, "TSLA".to_string());
    panel.state.bracket_active = Some(OrderSide::Buy);

    let config = panel.to_config();
    let restored = OrderPanel::from_config(id, &config);
    assert_eq!(restored.state.bracket_active, Some(OrderSide::Buy),);
}

#[test]
fn bracket_active_roundtrip_sell() {
    let id = OrderPanelId::new(1);
    let mut panel = OrderPanel::new(id, "TSLA".to_string());
    panel.state.bracket_active = Some(OrderSide::Sell);

    let config = panel.to_config();
    let restored = OrderPanel::from_config(id, &config);
    assert_eq!(restored.state.bracket_active, Some(OrderSide::Sell),);
}

#[test]
fn bracket_active_roundtrip_none() {
    let id = OrderPanelId::new(1);
    let panel = OrderPanel::new(id, "TSLA".to_string());

    let config = panel.to_config();
    let restored = OrderPanel::from_config(id, &config);
    assert_eq!(restored.state.bracket_active, None);
}

// -- default_bracket_prices tests --

#[test]
fn default_bracket_prices_buy_both_enabled() {
    let (entry, tp, sl) = default_bracket_prices(100.0, OrderSide::Buy, true, true, None);
    assert!((entry - 100.0).abs() < f64::EPSILON);
    assert!((tp.unwrap() - 101.0).abs() < 0.01);
    assert!((sl.unwrap() - 99.5).abs() < 0.01);
}

#[test]
fn default_bracket_prices_sell_both_enabled() {
    let (entry, tp, sl) = default_bracket_prices(100.0, OrderSide::Sell, true, true, None);
    assert!((entry - 100.0).abs() < f64::EPSILON);
    assert!((tp.unwrap() - 99.0).abs() < 0.01);
    assert!((sl.unwrap() - 100.5).abs() < 0.01);
}

#[test]
fn default_bracket_prices_tp_disabled() {
    let (_, tp, sl) = default_bracket_prices(100.0, OrderSide::Buy, false, true, None);
    assert!(tp.is_none());
    assert!(sl.is_some());
}

#[test]
fn default_bracket_prices_sl_disabled() {
    let (_, tp, sl) = default_bracket_prices(100.0, OrderSide::Sell, true, false, None);
    assert!(tp.is_some());
    assert!(sl.is_none());
}

#[test]
fn default_bracket_prices_pixel_minimum_overrides_pct() {
    // Simulate zoomed-out chart: $100 range over 1000px = $0.10/px
    // 30px minimum → $3.00 minimum offset, which is > 1% ($1.00)
    let ppp = 0.10; // price per pixel
    let (entry, tp, sl) = default_bracket_prices(100.0, OrderSide::Buy, true, true, Some(ppp));
    assert!((entry - 100.0).abs() < f64::EPSILON);
    // TP offset should be $3.00 (pixel min), not $1.00 (1%)
    assert!(
        (tp.unwrap() - 103.0).abs() < 0.01,
        "TP should be 103.0 (30px min), got {}",
        tp.unwrap()
    );
    // SL offset should be $3.00 (pixel min), not $0.50 (0.5%)
    assert!(
        (sl.unwrap() - 97.0).abs() < 0.01,
        "SL should be 97.0 (30px min), got {}",
        sl.unwrap()
    );
}

// -- sync_panel_from_bracket tests --

#[test]
fn sync_panel_from_bracket_populates_tp_sl() {
    use midas_chart::widget::level::LineStyle;
    use midas_chart::widget::order_bracket::*;

    let bracket = OrderBracket {
        entry: BracketLeg {
            line: midas_chart::widget::PriceLine {
                price: 100.0,
                extent: midas_chart::widget::LineExtent::FullWidth,
                stroke: midas_chart::widget::LineStroke {
                    color: [0.0, 0.0, 0.0, 1.0],
                    width: 1.0,
                    style: LineStyle::Solid,
                },
            },
            role: LegRole::Entry,
            projected_pnl: None,
            projected_pnl_pct: None,
        },
        take_profit: Some(BracketLeg {
            line: midas_chart::widget::PriceLine {
                price: 101.0,
                extent: midas_chart::widget::LineExtent::FullWidth,
                stroke: midas_chart::widget::LineStroke {
                    color: [0.0, 0.0, 0.0, 1.0],
                    width: 1.0,
                    style: LineStyle::Solid,
                },
            },
            role: LegRole::Entry,
            projected_pnl: None,
            projected_pnl_pct: None,
        }),
        stop_loss: Some(BracketLeg {
            line: midas_chart::widget::PriceLine {
                price: 99.5,
                extent: midas_chart::widget::LineExtent::FullWidth,
                stroke: midas_chart::widget::LineStroke {
                    color: [0.0, 0.0, 0.0, 1.0],
                    width: 1.0,
                    style: LineStyle::Solid,
                },
            },
            role: LegRole::Entry,
            projected_pnl: None,
            projected_pnl_pct: None,
        }),
        side: BracketSide::Long,
        status: BracketStatus::Draft,
        quantity: None,
        saved: false,
        filled_qty: None,
        entry_type: EntryType::Market,
        entry_stop_price: None,
        wrong_side_warning: false,
    };

    let mut state = OrderPanelState::default();
    sync_panel_from_bracket(&mut state, &bracket);
    assert_eq!(state.tp_value, "101.00");
    assert_eq!(state.sl_value, "99.50");
}

#[test]
fn sync_panel_from_bracket_clears_missing_legs() {
    use midas_chart::widget::level::LineStyle;
    use midas_chart::widget::order_bracket::*;

    let bracket = OrderBracket {
        entry: BracketLeg {
            line: midas_chart::widget::PriceLine {
                price: 100.0,
                extent: midas_chart::widget::LineExtent::FullWidth,
                stroke: midas_chart::widget::LineStroke {
                    color: [0.0, 0.0, 0.0, 1.0],
                    width: 1.0,
                    style: LineStyle::Solid,
                },
            },
            role: LegRole::Entry,
            projected_pnl: None,
            projected_pnl_pct: None,
        },
        take_profit: None,
        stop_loss: None,
        side: BracketSide::Long,
        status: BracketStatus::Draft,
        quantity: None,
        saved: false,
        filled_qty: None,
        entry_type: EntryType::Market,
        entry_stop_price: None,
        wrong_side_warning: false,
    };

    let mut state = OrderPanelState {
        tp_value: "old_value".to_string(),
        sl_value: "old_value".to_string(),
        ..Default::default()
    };
    sync_panel_from_bracket(&mut state, &bracket);
    assert!(state.tp_value.is_empty());
    assert!(state.sl_value.is_empty());
}

#[test]
fn sync_panel_from_bracket_limit_entry() {
    use midas_chart::widget::level::LineStyle;
    use midas_chart::widget::order_bracket::*;

    let bracket = OrderBracket {
        entry: BracketLeg {
            line: midas_chart::widget::PriceLine {
                price: 180.50,
                extent: midas_chart::widget::LineExtent::FullWidth,
                stroke: midas_chart::widget::LineStroke {
                    color: [0.0, 0.0, 0.0, 1.0],
                    width: 1.0,
                    style: LineStyle::Solid,
                },
            },
            role: LegRole::Entry,
            projected_pnl: None,
            projected_pnl_pct: None,
        },
        take_profit: None,
        stop_loss: None,
        side: BracketSide::Long,
        status: BracketStatus::Draft,
        quantity: None,
        saved: false,
        filled_qty: None,
        entry_type: EntryType::Limit,
        entry_stop_price: None,
        wrong_side_warning: false,
    };

    let mut state = OrderPanelState::default();
    sync_panel_from_bracket(&mut state, &bracket);
    assert_eq!(state.limit_price, "180.50");
}

// -- should_reposition tests --

#[test]
fn should_reposition_within_threshold() {
    assert!(!should_reposition(100.0, 100.0, Some(5.0)));
    assert!(!should_reposition(100.0, 104.0, Some(5.0)));
}

#[test]
fn should_reposition_beyond_threshold() {
    assert!(should_reposition(100.0, 106.0, Some(5.0)));
}

#[test]
fn should_reposition_fallback_to_5_pct() {
    // 6 > 5% of 100 = 5.0
    assert!(should_reposition(100.0, 106.0, None));
    // 4 < 5% of 100 = 5.0
    assert!(!should_reposition(100.0, 104.0, None));
}

// -- reposition_bracket tests --

#[test]
fn reposition_bracket_shifts_all_legs() {
    use midas_chart::widget::level::LineStyle;
    use midas_chart::widget::order_bracket::*;

    let mut bracket = OrderBracket {
        entry: BracketLeg {
            line: midas_chart::widget::PriceLine {
                price: 100.0,
                extent: midas_chart::widget::LineExtent::FullWidth,
                stroke: midas_chart::widget::LineStroke {
                    color: [0.0, 0.0, 0.0, 1.0],
                    width: 1.0,
                    style: LineStyle::Solid,
                },
            },
            role: LegRole::Entry,
            projected_pnl: None,
            projected_pnl_pct: None,
        },
        take_profit: Some(BracketLeg {
            line: midas_chart::widget::PriceLine {
                price: 102.0,
                extent: midas_chart::widget::LineExtent::FullWidth,
                stroke: midas_chart::widget::LineStroke {
                    color: [0.0, 0.0, 0.0, 1.0],
                    width: 1.0,
                    style: LineStyle::Solid,
                },
            },
            role: LegRole::Entry,
            projected_pnl: None,
            projected_pnl_pct: None,
        }),
        stop_loss: Some(BracketLeg {
            line: midas_chart::widget::PriceLine {
                price: 99.0,
                extent: midas_chart::widget::LineExtent::FullWidth,
                stroke: midas_chart::widget::LineStroke {
                    color: [0.0, 0.0, 0.0, 1.0],
                    width: 1.0,
                    style: LineStyle::Solid,
                },
            },
            role: LegRole::Entry,
            projected_pnl: None,
            projected_pnl_pct: None,
        }),
        side: BracketSide::Long,
        status: BracketStatus::Draft,
        quantity: None,
        saved: true,
        filled_qty: None,
        entry_type: EntryType::Market,
        entry_stop_price: None,
        wrong_side_warning: false,
    };

    reposition_bracket(&mut bracket, 110.0);
    assert!((bracket.entry.line.price - 110.0).abs() < f64::EPSILON);
    assert!((bracket.take_profit.unwrap().line.price - 112.0).abs() < f64::EPSILON);
    assert!((bracket.stop_loss.unwrap().line.price - 109.0).abs() < f64::EPSILON);
}

// ===========================================================================
// Slice 2: hydration from TickerOrderIntent
// ===========================================================================

mod hydration {
    use super::*;
    use crate::annotation_store::SymbolKey;
    use crate::ticker_order_intent::{EntryMemory, GatrAnchor, TickerOrderIntent};
    use midas_chart::widget::order_bracket::EntryType;
    use std::collections::HashMap;

    /// Build an intent with `(Buy, Stop)` populated with custom prices
    /// and `sl_enabled = false`. Used as the common fixture so
    /// compound-key tests can pivot to the untouched `(Sell, Limit)`
    /// bucket and verify default fallbacks.
    fn fixture_intent(symbol: &str) -> TickerOrderIntent {
        let mut entries = HashMap::new();
        entries.insert(
            (OrderSide::Buy, EntryType::Stop),
            EntryMemory {
                entry_price_or_offset: Some(123.45),
                quantity: Some(50.0),
                tp_enabled: true,
                tp_value: "125.00".to_string(),
                tp_mode: PriceInputMode::Absolute,
                sl_enabled: false, // user explicitly turned SL off here
                sl_value: "120.00".to_string(),
                sl_mode: PriceInputMode::Absolute,
                sl_type: StopLossType::Stop,
                sl_limit_value: String::new(),
            },
        );
        TickerOrderIntent {
            version: crate::ticker_order_intent::CURRENT_VERSION,
            symbol: SymbolKey::new(symbol),
            last_side: OrderSide::Buy,
            last_entry_type: EntryType::Stop,
            entries,
            gatr_anchor: GatrAnchor::default(),
            live_annotation_id: None,
            broker_order_id: None,
            pinned: false,
            updated_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn hydrate_populates_fields_for_last_compound_key() {
        let mut panel = OrderPanelState::default();
        let intent = fixture_intent("AAPL");

        panel.hydrate_from_intent(&intent, Some(124.00));

        assert_eq!(panel.symbol, "AAPL");
        assert_eq!(panel.side, OrderSide::Buy);
        assert_eq!(panel.entry_type, EntryType::Stop);
        assert_eq!(panel.quantity, "50");
        assert!(panel.tp_enabled);
        assert_eq!(panel.tp_value, "125.00");
        // Compound key had SL turned off — hydration must preserve that.
        assert!(!panel.sl_enabled);
        // `entry_price_or_offset` for a Stop entry routes to stop_price.
        assert_eq!(panel.stop_price, "123.45");
        assert_eq!(panel.last_price, Some(124.00));
        assert!(!panel.dirty);
    }

    #[test]
    fn hydrate_dirty_same_symbol_is_noop() {
        let mut panel = OrderPanelState::default();
        panel.symbol = "AAPL".to_string();
        panel.quantity = "999".to_string();
        panel.dirty = true;

        let intent = fixture_intent("AAPL");
        panel.hydrate_from_intent(&intent, Some(124.00));

        // Panel state was preserved — dirty guard fired.
        assert_eq!(panel.quantity, "999");
        assert!(panel.dirty);
    }

    #[test]
    fn hydrate_dirty_different_symbol_proceeds() {
        let mut panel = OrderPanelState::default();
        panel.symbol = "MSFT".to_string();
        panel.quantity = "999".to_string();
        panel.dirty = true;

        let intent = fixture_intent("AAPL");
        panel.hydrate_from_intent(&intent, Some(124.00));

        // Cross-ticker hydration bypasses the dirty guard.
        assert_eq!(panel.symbol, "AAPL");
        assert_eq!(panel.quantity, "50");
        assert!(!panel.dirty);
    }

    #[test]
    fn hydrate_fresh_panel_keeps_regression_defaults_when_no_intent() {
        // Regression check: without any hydration call, the default
        // panel still has `sl_enabled = false` from its own Default.
        // This preserves existing behaviour for panels created before
        // any intent row exists.
        let panel = OrderPanelState::default();
        assert!(!panel.sl_enabled);
        assert!(!panel.dirty);
        assert_eq!(panel.quantity, "100");
    }

    #[test]
    fn rehydrate_compound_key_falls_back_to_defaults_with_sl_on() {
        let mut panel = OrderPanelState::default();
        let intent = fixture_intent("AAPL");
        // First land on the populated compound.
        panel.hydrate_from_intent(&intent, Some(124.00));
        assert!(!panel.sl_enabled);

        // Switch to an untouched bucket. The fallback `EntryMemory::default()`
        // has `sl_enabled = true` per the SL-on-by-default rule.
        panel.rehydrate_for_compound(&intent, OrderSide::Sell, EntryType::Limit);

        assert_eq!(panel.side, OrderSide::Sell);
        assert_eq!(panel.entry_type, EntryType::Limit);
        assert!(panel.sl_enabled, "untouched compound falls back to SL on");
        assert!(panel.tp_value.is_empty());
        assert!(panel.limit_price.is_empty());
        // Soft rehydrate does NOT touch the dirty flag.
        assert!(!panel.dirty);
    }

    #[test]
    fn rehydrate_compound_does_not_bump_dirty() {
        let mut panel = OrderPanelState::default();
        let intent = fixture_intent("AAPL");
        panel.hydrate_from_intent(&intent, Some(124.00));
        assert!(!panel.dirty);

        // Pretend the user was mid-edit.
        panel.dirty = true;
        panel.rehydrate_for_compound(&intent, OrderSide::Buy, EntryType::Limit);
        // `dirty` is explicitly untouched by the soft rehydrate.
        assert!(panel.dirty);
    }

    #[test]
    fn rehydrate_compound_within_same_side_lands_on_new_type_bucket() {
        let mut panel = OrderPanelState::default();
        // Seed intent with (Buy, Stop) AND (Buy, Limit) distinct.
        let mut intent = fixture_intent("AAPL");
        intent.entries.insert(
            (OrderSide::Buy, EntryType::Limit),
            EntryMemory {
                entry_price_or_offset: Some(200.00),
                quantity: Some(75.0),
                tp_enabled: false,
                tp_value: String::new(),
                tp_mode: PriceInputMode::Absolute,
                sl_enabled: true,
                sl_value: "195.00".to_string(),
                sl_mode: PriceInputMode::Absolute,
                sl_type: StopLossType::Stop,
                sl_limit_value: String::new(),
            },
        );

        panel.hydrate_from_intent(&intent, Some(124.00));
        // Land on (Buy, Stop) first.
        assert_eq!(panel.entry_type, EntryType::Stop);
        assert_eq!(panel.stop_price, "123.45");

        // User toggles entry type to Limit → rehydrate reads the
        // Limit bucket.
        panel.rehydrate_for_compound(&intent, OrderSide::Buy, EntryType::Limit);
        assert_eq!(panel.entry_type, EntryType::Limit);
        assert_eq!(panel.limit_price, "200.00");
        assert_eq!(panel.quantity, "75");
        assert!(panel.sl_enabled);
        assert_eq!(panel.sl_value, "195.00");
    }
}
