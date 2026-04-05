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

    let bracket = create_bracket_annotation(
        BracketSide::Long,
        185.0,
        Some(192.0),
        Some(182.0),
        100.0,
    );
    assert_eq!(bracket.side, BracketSide::Long);
    assert_eq!(bracket.status, BracketStatus::Pending);
    assert!((bracket.entry.price - 185.0).abs() < f64::EPSILON);
    assert!(bracket.take_profit.is_some());
    assert!((bracket.take_profit.unwrap().price - 192.0).abs() < f64::EPSILON);
    assert!(bracket.stop_loss.is_some());
    assert!((bracket.stop_loss.unwrap().price - 182.0).abs() < f64::EPSILON);
    assert_eq!(bracket.quantity, Some(100.0));
}

#[test]
fn create_bracket_short_no_tp() {
    use midas_chart::widget::order_bracket::*;

    let bracket = create_bracket_annotation(
        BracketSide::Short,
        185.0,
        None,
        Some(188.0),
        50.0,
    );
    assert_eq!(bracket.side, BracketSide::Short);
    assert_eq!(bracket.status, BracketStatus::Pending);
    assert!(bracket.take_profit.is_none());
    assert!(bracket.stop_loss.is_some());
    assert_eq!(bracket.quantity, Some(50.0));
}

#[test]
fn create_bracket_no_legs() {
    use midas_chart::widget::order_bracket::*;

    let bracket = create_bracket_annotation(
        BracketSide::Long,
        185.0,
        None,
        None,
        200.0,
    );
    assert!(bracket.take_profit.is_none());
    assert!(bracket.stop_loss.is_none());
    assert!((bracket.entry.price - 185.0).abs() < f64::EPSILON);
}

// -- map_lifecycle_to_chart_status tests --

#[test]
fn lifecycle_submitted_maps_to_pending() {
    use midas_chart::widget::order_bracket::BracketStatus;
    assert_eq!(map_lifecycle_to_chart_status("Submitted"), BracketStatus::Pending);
}

#[test]
fn lifecycle_entry_filled_maps_to_active() {
    use midas_chart::widget::order_bracket::BracketStatus;
    assert_eq!(map_lifecycle_to_chart_status("EntryFilled"), BracketStatus::Active);
}

#[test]
fn lifecycle_take_profit_hit_maps_to_closed() {
    use midas_chart::widget::order_bracket::BracketStatus;
    assert_eq!(map_lifecycle_to_chart_status("TakeProfitHit"), BracketStatus::Closed);
}

#[test]
fn lifecycle_stop_loss_hit_maps_to_closed() {
    use midas_chart::widget::order_bracket::BracketStatus;
    assert_eq!(map_lifecycle_to_chart_status("StopLossHit"), BracketStatus::Closed);
}

#[test]
fn lifecycle_cancelled_maps_to_cancelled() {
    use midas_chart::widget::order_bracket::BracketStatus;
    assert_eq!(map_lifecycle_to_chart_status("Cancelled"), BracketStatus::Cancelled);
}

#[test]
fn lifecycle_rejected_maps_to_cancelled() {
    use midas_chart::widget::order_bracket::BracketStatus;
    assert_eq!(map_lifecycle_to_chart_status("Rejected"), BracketStatus::Cancelled);
}

#[test]
fn lifecycle_unknown_defaults_to_pending() {
    use midas_chart::widget::order_bracket::BracketStatus;
    assert_eq!(map_lifecycle_to_chart_status("SomethingNew"), BracketStatus::Pending);
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
        errors.iter().any(|(f, msg)| f == "quantity" && msg.contains("not a number")),
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
        errors.iter().any(|(f, msg)| f == "tp" && msg.contains("not a number")),
        "expected 'not a number' error for TP, got: {errors:?}"
    );
    // Should not double-report with generic "Invalid TP price"
    let tp_errors: Vec<_> = errors.iter().filter(|(f, _)| f == "tp").collect();
    assert_eq!(tp_errors.len(), 1, "TP should have exactly one error, got: {tp_errors:?}");
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
        errors.iter().any(|(f, msg)| f == "sl" && msg.contains("not a number")),
        "expected 'not a number' error for SL, got: {errors:?}"
    );
    let sl_errors: Vec<_> = errors.iter().filter(|(f, _)| f == "sl").collect();
    assert_eq!(sl_errors.len(), 1, "SL should have exactly one error, got: {sl_errors:?}");
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
