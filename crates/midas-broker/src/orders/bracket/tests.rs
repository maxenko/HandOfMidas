use super::*;
use crate::orders::types::{OrderAction, OrderKind};

// -- helpers ----------------------------------------------------------

fn make_order(status: OrderStatus) -> LocalOrder {
    let mut o = LocalOrder::new_draft("AAPL", OrderAction::Buy, OrderKind::Market, 100.0);
    o.status = status;
    o
}

fn make_bracket(
    parent_status: OrderStatus,
    tp_status: Option<OrderStatus>,
    sl_status: Option<OrderStatus>,
) -> BracketGroup {
    let parent = make_order(parent_status);
    let parent_id = parent.id;
    BracketGroup {
        parent,
        take_profit: tp_status.map(|s| {
            let mut o = LocalOrder::new_draft("AAPL", OrderAction::Sell, OrderKind::Limit, 100.0);
            o.status = s;
            o.parent_id = Some(parent_id);
            o
        }),
        stop_loss: sl_status.map(|s| {
            let mut o = LocalOrder::new_draft("AAPL", OrderAction::Sell, OrderKind::Stop, 100.0);
            o.status = s;
            o.parent_id = Some(parent_id);
            o
        }),
    }
}

fn sample_params() -> BracketParams {
    BracketParams {
        symbol: "AAPL".to_string(),
        con_id: Some(265598),
        sec_type: SecurityType::Stock,
        exchange: "SMART".to_string(),
        currency: "USD".to_string(),
        action: OrderAction::Buy,
        quantity: 100.0,
        outside_rth: false,
        entry_kind: OrderKind::Market,
        entry_price: None,
        entry_stop_price: None,
        take_profit: Some(TakeProfitParams {
            price: 192.0,
            tif: None,
        }),
        stop_loss: Some(StopLossParams {
            stop_price: 182.0,
            limit_price: None,
            tif: None,
        }),
        reference_price: Some(185.50),
        strategy: None,
        tags: Vec::new(),
    }
}

// -- BracketLifecycleStatus Display/FromStr ---------------------------

#[test]
fn lifecycle_status_display_fromstr_round_trip() {
    let all = [
        BracketLifecycleStatus::Submitted,
        BracketLifecycleStatus::EntryFilled,
        BracketLifecycleStatus::TakeProfitHit,
        BracketLifecycleStatus::StopLossHit,
        BracketLifecycleStatus::Cancelled,
        BracketLifecycleStatus::Rejected,
        BracketLifecycleStatus::Error,
        BracketLifecycleStatus::Closed,
    ];
    for status in all {
        let s = status.to_string();
        let parsed: BracketLifecycleStatus = s.parse().unwrap();
        assert_eq!(parsed, status, "round-trip failed for {status}");
    }
}

#[test]
fn lifecycle_status_parse_unknown_fails() {
    assert!("Bogus".parse::<BracketLifecycleStatus>().is_err());
}

#[test]
fn lifecycle_status_serde_round_trip() {
    let status = BracketLifecycleStatus::TakeProfitHit;
    let json = serde_json::to_string(&status).unwrap();
    let restored: BracketLifecycleStatus = serde_json::from_str(&json).unwrap();
    assert_eq!(restored, status);
}

// -- Validation -------------------------------------------------------

#[test]
fn valid_full_bracket() {
    assert!(validate_bracket(&sample_params()).is_ok());
}

#[test]
fn valid_tp_only() {
    let mut p = sample_params();
    p.stop_loss = None;
    assert!(validate_bracket(&p).is_ok());
}

#[test]
fn valid_sl_only() {
    let mut p = sample_params();
    p.take_profit = None;
    assert!(validate_bracket(&p).is_ok());
}

#[test]
fn valid_naked_market() {
    let mut p = sample_params();
    p.take_profit = None;
    p.stop_loss = None;
    assert!(validate_bracket(&p).is_ok());
}

#[test]
fn reject_empty_symbol() {
    let mut p = sample_params();
    p.symbol = String::new();
    let errs = validate_bracket(&p).unwrap_err();
    assert!(errs.contains(&ValidationError::MissingSymbol));
}

#[test]
fn reject_empty_exchange() {
    let mut p = sample_params();
    p.exchange = String::new();
    let errs = validate_bracket(&p).unwrap_err();
    assert!(errs.contains(&ValidationError::MissingExchange));
}

#[test]
fn reject_empty_currency() {
    let mut p = sample_params();
    p.currency = String::new();
    let errs = validate_bracket(&p).unwrap_err();
    assert!(errs.contains(&ValidationError::MissingCurrency));
}

#[test]
fn reject_zero_quantity() {
    let mut p = sample_params();
    p.quantity = 0.0;
    let errs = validate_bracket(&p).unwrap_err();
    assert!(errs.contains(&ValidationError::InvalidQuantity));
}

#[test]
fn reject_negative_quantity() {
    let mut p = sample_params();
    p.quantity = -100.0;
    let errs = validate_bracket(&p).unwrap_err();
    assert!(errs.contains(&ValidationError::InvalidQuantity));
}

#[test]
fn reject_negative_tp_price() {
    let mut p = sample_params();
    p.take_profit = Some(TakeProfitParams {
        price: -10.0,
        tif: None,
    });
    let errs = validate_bracket(&p).unwrap_err();
    assert!(errs.contains(&ValidationError::InvalidPrice("take_profit")));
}

#[test]
fn reject_negative_sl_price() {
    let mut p = sample_params();
    p.stop_loss = Some(StopLossParams {
        stop_price: -10.0,
        limit_price: None,
        tif: None,
    });
    let errs = validate_bracket(&p).unwrap_err();
    assert!(errs.contains(&ValidationError::InvalidPrice("stop_loss")));
}

#[test]
fn reject_nan_quantity() {
    let mut p = sample_params();
    p.quantity = f64::NAN;
    let errs = validate_bracket(&p).unwrap_err();
    assert!(errs.contains(&ValidationError::InvalidQuantity));
}

#[test]
fn reject_infinity_quantity() {
    let mut p = sample_params();
    p.quantity = f64::INFINITY;
    let errs = validate_bracket(&p).unwrap_err();
    assert!(errs.contains(&ValidationError::InvalidQuantity));
}

#[test]
fn reject_nan_tp_price() {
    let mut p = sample_params();
    p.take_profit = Some(TakeProfitParams {
        price: f64::NAN,
        tif: None,
    });
    let errs = validate_bracket(&p).unwrap_err();
    assert!(errs.contains(&ValidationError::InvalidPrice("take_profit")));
}

#[test]
fn reject_infinity_sl_stop_price() {
    let mut p = sample_params();
    p.stop_loss = Some(StopLossParams {
        stop_price: f64::INFINITY,
        limit_price: None,
        tif: None,
    });
    let errs = validate_bracket(&p).unwrap_err();
    assert!(errs.contains(&ValidationError::InvalidPrice("stop_loss")));
}

#[test]
fn reject_nan_sl_limit_price() {
    let mut p = sample_params();
    p.stop_loss = Some(StopLossParams {
        stop_price: 182.0,
        limit_price: Some(f64::NAN),
        tif: None,
    });
    let errs = validate_bracket(&p).unwrap_err();
    assert!(errs.contains(&ValidationError::InvalidPrice("stop_loss_limit")));
}

// -- Directional Validation -------------------------------------------

#[test]
fn buy_tp_above_entry() {
    let w = check_bracket_direction(OrderAction::Buy, 185.0, Some(192.0), None);
    assert!(w.is_empty());
}

#[test]
fn buy_tp_below_entry_warns() {
    let w = check_bracket_direction(OrderAction::Buy, 185.0, Some(180.0), None);
    assert_eq!(w.len(), 1);
    assert!(matches!(w[0], DirectionWarning::TpBelowReference { .. }));
}

#[test]
fn buy_sl_below_entry() {
    let w = check_bracket_direction(OrderAction::Buy, 185.0, None, Some(182.0));
    assert!(w.is_empty());
}

#[test]
fn buy_sl_above_entry_warns() {
    let w = check_bracket_direction(OrderAction::Buy, 185.0, None, Some(190.0));
    assert_eq!(w.len(), 1);
    assert!(matches!(w[0], DirectionWarning::SlAboveReference { .. }));
}

#[test]
fn sell_tp_below_entry() {
    let w = check_bracket_direction(OrderAction::Sell, 185.0, Some(178.0), None);
    assert!(w.is_empty());
}

#[test]
fn sell_tp_above_entry_warns() {
    let w = check_bracket_direction(OrderAction::Sell, 185.0, Some(190.0), None);
    assert_eq!(w.len(), 1);
    assert!(matches!(w[0], DirectionWarning::TpAboveReference { .. }));
}

#[test]
fn sell_sl_above_entry() {
    let w = check_bracket_direction(OrderAction::Sell, 185.0, None, Some(190.0));
    assert!(w.is_empty());
}

#[test]
fn sell_sl_below_entry_warns() {
    let w = check_bracket_direction(OrderAction::Sell, 185.0, None, Some(180.0));
    assert_eq!(w.len(), 1);
    assert!(matches!(w[0], DirectionWarning::SlBelowReference { .. }));
}

// -- BracketGroup -----------------------------------------------------

#[test]
fn legs_count_full() {
    let g = make_bracket(
        OrderStatus::Inactive,
        Some(OrderStatus::Inactive),
        Some(OrderStatus::Inactive),
    );
    assert_eq!(g.legs().len(), 3);
}

#[test]
fn legs_count_tp_only() {
    let g = make_bracket(OrderStatus::Inactive, Some(OrderStatus::Inactive), None);
    assert_eq!(g.legs().len(), 2);
}

#[test]
fn legs_count_sl_only() {
    let g = make_bracket(OrderStatus::Inactive, None, Some(OrderStatus::Inactive));
    assert_eq!(g.legs().len(), 2);
}

#[test]
fn can_activate_all_inactive() {
    let g = make_bracket(
        OrderStatus::Inactive,
        Some(OrderStatus::Inactive),
        Some(OrderStatus::Inactive),
    );
    assert!(g.can_activate());
}

#[test]
fn can_activate_one_error() {
    let g = make_bracket(
        OrderStatus::Error,
        Some(OrderStatus::Inactive),
        Some(OrderStatus::Inactive),
    );
    assert!(g.can_activate());
}

#[test]
fn cannot_activate_one_submitted() {
    let g = make_bracket(
        OrderStatus::Submitted,
        Some(OrderStatus::Inactive),
        Some(OrderStatus::Inactive),
    );
    assert!(!g.can_activate());
}

#[test]
fn is_active_when_parent_filled() {
    let g = make_bracket(
        OrderStatus::Filled,
        Some(OrderStatus::Submitted),
        Some(OrderStatus::Submitted),
    );
    assert!(g.is_active());
}

#[test]
fn is_closed_all_terminal() {
    let g = make_bracket(
        OrderStatus::Filled,
        Some(OrderStatus::Filled),
        Some(OrderStatus::Cancelled),
    );
    assert!(g.is_closed());
}

#[test]
fn not_closed_child_live() {
    let g = make_bracket(
        OrderStatus::Filled,
        Some(OrderStatus::Submitted),
        Some(OrderStatus::Submitted),
    );
    assert!(!g.is_closed());
}

// -- derive_bracket_status --------------------------------------------

#[test]
fn derive_parent_pending() {
    let g = make_bracket(
        OrderStatus::PendingSubmit,
        Some(OrderStatus::PendingSubmit),
        Some(OrderStatus::PendingSubmit),
    );
    assert_eq!(derive_bracket_status(&g), BracketLifecycleStatus::Submitted);
}

#[test]
fn derive_parent_submitted() {
    let g = make_bracket(
        OrderStatus::Submitted,
        Some(OrderStatus::PreSubmitted),
        Some(OrderStatus::PreSubmitted),
    );
    assert_eq!(derive_bracket_status(&g), BracketLifecycleStatus::Submitted);
}

#[test]
fn derive_parent_filled_children_live() {
    let g = make_bracket(
        OrderStatus::Filled,
        Some(OrderStatus::Submitted),
        Some(OrderStatus::Submitted),
    );
    assert_eq!(
        derive_bracket_status(&g),
        BracketLifecycleStatus::EntryFilled
    );
}

#[test]
fn derive_tp_filled() {
    let g = make_bracket(
        OrderStatus::Filled,
        Some(OrderStatus::Filled),
        Some(OrderStatus::Cancelled),
    );
    assert_eq!(
        derive_bracket_status(&g),
        BracketLifecycleStatus::TakeProfitHit
    );
}

#[test]
fn derive_sl_filled() {
    let g = make_bracket(
        OrderStatus::Filled,
        Some(OrderStatus::Cancelled),
        Some(OrderStatus::Filled),
    );
    assert_eq!(
        derive_bracket_status(&g),
        BracketLifecycleStatus::StopLossHit
    );
}

#[test]
fn derive_parent_cancelled() {
    let g = make_bracket(
        OrderStatus::Cancelled,
        Some(OrderStatus::Cancelled),
        Some(OrderStatus::Cancelled),
    );
    assert_eq!(derive_bracket_status(&g), BracketLifecycleStatus::Cancelled);
}

#[test]
fn derive_parent_rejected() {
    let g = make_bracket(
        OrderStatus::Rejected,
        Some(OrderStatus::Rejected),
        Some(OrderStatus::Rejected),
    );
    assert_eq!(derive_bracket_status(&g), BracketLifecycleStatus::Rejected);
}

#[test]
fn derive_parent_error() {
    let g = make_bracket(
        OrderStatus::Error,
        Some(OrderStatus::Inactive),
        Some(OrderStatus::Inactive),
    );
    assert_eq!(derive_bracket_status(&g), BracketLifecycleStatus::Error);
}

#[test]
fn derive_child_error() {
    let g = make_bracket(
        OrderStatus::Filled,
        Some(OrderStatus::Error),
        Some(OrderStatus::Submitted),
    );
    assert_eq!(derive_bracket_status(&g), BracketLifecycleStatus::Error);
}

#[test]
fn derive_all_closed() {
    let g = make_bracket(
        OrderStatus::Filled,
        Some(OrderStatus::Cancelled),
        Some(OrderStatus::Cancelled),
    );
    assert_eq!(derive_bracket_status(&g), BracketLifecycleStatus::Closed);
}

#[test]
fn derive_unexpected_parent_status_does_not_panic() {
    let g = make_bracket(OrderStatus::Filled, None, None);
    assert_eq!(derive_bracket_status(&g), BracketLifecycleStatus::Closed);
}
