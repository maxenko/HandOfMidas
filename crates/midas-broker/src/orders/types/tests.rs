use super::*;

// -----------------------------------------------------------------------
// OrderAction
// -----------------------------------------------------------------------

#[test]
fn order_action_display_fromstr() {
    assert_eq!(OrderAction::Buy.to_string(), "BUY");
    assert_eq!(OrderAction::Sell.to_string(), "SELL");
    assert_eq!("BUY".parse::<OrderAction>().unwrap(), OrderAction::Buy);
    assert_eq!("SELL".parse::<OrderAction>().unwrap(), OrderAction::Sell);
    assert!("buy".parse::<OrderAction>().is_err());
}

// -----------------------------------------------------------------------
// OrderKind
// -----------------------------------------------------------------------

#[test]
fn order_kind_display_fromstr() {
    let cases = [
        (OrderKind::Market, "MKT"),
        (OrderKind::Limit, "LMT"),
        (OrderKind::Stop, "STP"),
        (OrderKind::StopLimit, "STP LMT"),
        (OrderKind::TrailingStop, "TRAIL"),
    ];
    for (kind, s) in cases {
        assert_eq!(kind.to_string(), s);
        assert_eq!(s.parse::<OrderKind>().unwrap(), kind);
    }
    assert!("MARKET".parse::<OrderKind>().is_err());
}

// -----------------------------------------------------------------------
// TimeInForce
// -----------------------------------------------------------------------

#[test]
fn time_in_force_display_fromstr() {
    let cases = [
        (TimeInForce::Day, "DAY"),
        (TimeInForce::Gtc, "GTC"),
        (TimeInForce::Ioc, "IOC"),
        (TimeInForce::Gtd, "GTD"),
        (TimeInForce::Opg, "OPG"),
    ];
    for (tif, s) in cases {
        assert_eq!(tif.to_string(), s);
        assert_eq!(s.parse::<TimeInForce>().unwrap(), tif);
    }
    assert!("gtc".parse::<TimeInForce>().is_err());
}

// -----------------------------------------------------------------------
// LocalOrder::new_draft
// -----------------------------------------------------------------------

#[test]
fn new_draft_defaults() {
    let order = LocalOrder::new_draft("AAPL", OrderAction::Buy, OrderKind::Limit, 100.0);

    assert_eq!(order.symbol, "AAPL");
    assert_eq!(order.action, OrderAction::Buy);
    assert_eq!(order.order_type, OrderKind::Limit);
    assert_eq!(order.quantity, 100.0);
    assert_eq!(order.remaining_qty, 100.0);
    assert_eq!(order.filled_qty, 0.0);
    assert_eq!(order.status, OrderStatus::Draft);
    assert_eq!(order.sec_type, SecurityType::Stock);
    assert_eq!(order.exchange, "SMART");
    assert_eq!(order.currency, "USD");
    assert_eq!(order.tif, TimeInForce::Day);
    assert!(!order.outside_rth);
    assert!(order.ib_order_id.is_none());
    assert!(order.limit_price.is_none());
    assert_eq!(order.activation_count, 0);
    assert!(order.tags.is_empty());
    // UUIDv7 should have version nibble = 7.
    assert_eq!(order.id.get_version_num(), 7);
}

#[test]
fn new_draft_timestamps_are_recent() {
    let before = Utc::now();
    let order = LocalOrder::new_draft("SPY", OrderAction::Sell, OrderKind::Market, 50.0);
    let after = Utc::now();

    assert!(order.created_at >= before && order.created_at <= after);
    assert_eq!(order.created_at, order.updated_at);
}

#[test]
fn local_order_serde_round_trip() {
    let order = LocalOrder::new_draft("TSLA", OrderAction::Buy, OrderKind::StopLimit, 10.0);
    let json = serde_json::to_string(&order).unwrap();
    let restored: LocalOrder = serde_json::from_str(&json).unwrap();
    assert_eq!(restored.id, order.id);
    assert_eq!(restored.symbol, "TSLA");
    assert_eq!(restored.order_type, OrderKind::StopLimit);
    assert_eq!(restored.status, OrderStatus::Draft);
}

// -----------------------------------------------------------------------
// BracketRole
// -----------------------------------------------------------------------

#[test]
fn bracket_role_display_parent() {
    assert_eq!(BracketRole::Parent.to_string(), "PARENT");
}

#[test]
fn bracket_role_display_take_profit() {
    assert_eq!(BracketRole::TakeProfit.to_string(), "TAKE_PROFIT");
}

#[test]
fn bracket_role_display_stop_loss() {
    assert_eq!(BracketRole::StopLoss.to_string(), "STOP_LOSS");
}

#[test]
fn bracket_role_parse_parent() {
    assert_eq!("PARENT".parse::<BracketRole>().unwrap(), BracketRole::Parent);
}

#[test]
fn bracket_role_parse_take_profit() {
    assert_eq!(
        "TAKE_PROFIT".parse::<BracketRole>().unwrap(),
        BracketRole::TakeProfit
    );
}

#[test]
fn bracket_role_parse_legacy_profit() {
    assert_eq!("PROFIT".parse::<BracketRole>().unwrap(), BracketRole::TakeProfit);
}

#[test]
fn bracket_role_parse_legacy_stop() {
    assert_eq!("STOP".parse::<BracketRole>().unwrap(), BracketRole::StopLoss);
}

#[test]
fn bracket_role_parse_lowercase_parent() {
    assert_eq!("parent".parse::<BracketRole>().unwrap(), BracketRole::Parent);
}

#[test]
fn bracket_role_parse_lowercase_take_profit() {
    assert_eq!(
        "take_profit".parse::<BracketRole>().unwrap(),
        BracketRole::TakeProfit
    );
}

#[test]
fn bracket_role_parse_lowercase_stop_loss() {
    assert_eq!(
        "stop_loss".parse::<BracketRole>().unwrap(),
        BracketRole::StopLoss
    );
}

#[test]
fn bracket_role_parse_unknown_fails() {
    assert!("UNKNOWN".parse::<BracketRole>().is_err());
}

#[test]
fn bracket_role_serde_round_trip() {
    let role = BracketRole::TakeProfit;
    let json = serde_json::to_string(&role).unwrap();
    let restored: BracketRole = serde_json::from_str(&json).unwrap();
    assert_eq!(restored, role);
}

#[test]
fn bracket_role_serde_matches_display() {
    assert_eq!(
        serde_json::to_string(&BracketRole::Parent).unwrap(),
        "\"PARENT\""
    );
    assert_eq!(
        serde_json::to_string(&BracketRole::TakeProfit).unwrap(),
        "\"TAKE_PROFIT\""
    );
    assert_eq!(
        serde_json::to_string(&BracketRole::StopLoss).unwrap(),
        "\"STOP_LOSS\""
    );
}
