use super::*;
use crate::db::BrokerDb;

/// Helper: create a minimal `OrderRow` with required fields filled in.
fn make_order(local_id: &str, status: &str, symbol: &str) -> OrderRow {
    OrderRow {
        local_id: local_id.to_string(),
        ib_order_id: None,
        ib_perm_id: None,
        status: status.to_string(),
        symbol: symbol.to_string(),
        sec_type: "STK".to_string(),
        exchange: "SMART".to_string(),
        currency: "USD".to_string(),
        con_id: None,
        action: "BUY".to_string(),
        order_type: "LMT".to_string(),
        quantity: 100.0,
        filled_qty: 0.0,
        remaining_qty: 100.0,
        limit_price: Some(150.0),
        stop_price: None,
        trail_amount: None,
        trail_percent: None,
        tif: "DAY".to_string(),
        parent_id: None,
        oca_group: None,
        bracket_role: None,
        strategy: None,
        tags: None,
        algo_strategy: None,
        algo_params: None,
        outside_rth: false,
        avg_fill_price: None,
        last_fill_price: None,
        commission: None,
        activation_count: 0,
        last_activated_at: None,
        last_deactivated_at: None,
        created_at: "2026-03-25T12:00:00Z".to_string(),
        updated_at: "2026-03-25T12:00:00Z".to_string(),
    }
}

#[test]
fn test_insert_and_get_order() {
    let db = BrokerDb::open_in_memory().unwrap();
    let conn = db.conn().lock().unwrap();

    let order = make_order("order-001", "Draft", "AAPL");
    insert_order(&conn, &order).unwrap();

    let fetched = get_order(&conn, "order-001").unwrap().expect("order should exist");

    assert_eq!(fetched.local_id, "order-001");
    assert_eq!(fetched.status, "Draft");
    assert_eq!(fetched.symbol, "AAPL");
    assert_eq!(fetched.quantity, 100.0);
    assert_eq!(fetched.limit_price, Some(150.0));
    assert!(!fetched.outside_rth);
    assert_eq!(fetched.tif, "DAY");
    assert_eq!(fetched.action, "BUY");
    assert_eq!(fetched.order_type, "LMT");
    assert_eq!(fetched.filled_qty, 0.0);
    assert_eq!(fetched.remaining_qty, 100.0);
}

#[test]
fn test_get_order_not_found() {
    let db = BrokerDb::open_in_memory().unwrap();
    let conn = db.conn().lock().unwrap();

    let result = get_order(&conn, "nonexistent").unwrap();
    assert!(result.is_none());
}

#[test]
fn test_update_order_status() {
    let db = BrokerDb::open_in_memory().unwrap();
    let conn = db.conn().lock().unwrap();

    let order = make_order("order-002", "Draft", "MSFT");
    insert_order(&conn, &order).unwrap();

    let updated = update_order_status(&conn, "order-002", "Submitted", "2026-03-25T12:01:00Z")
        .unwrap();
    assert!(updated);

    let fetched = get_order(&conn, "order-002").unwrap().unwrap();
    assert_eq!(fetched.status, "Submitted");
    assert_eq!(fetched.updated_at, "2026-03-25T12:01:00Z");
}

#[test]
fn test_update_order_status_not_found() {
    let db = BrokerDb::open_in_memory().unwrap();
    let conn = db.conn().lock().unwrap();

    let updated =
        update_order_status(&conn, "nonexistent", "Submitted", "2026-03-25T12:01:00Z")
            .unwrap();
    assert!(!updated);
}

#[test]
fn test_write_audit() {
    let db = BrokerDb::open_in_memory().unwrap();
    let conn = db.conn().lock().unwrap();

    write_audit(
        &conn,
        "order-003",
        "Draft",
        "Submitted",
        Some(r#"{"reason":"user click"}"#),
        "user",
    )
    .unwrap();

    let mut stmt = conn
        .prepare(
            "SELECT order_local_id, from_status, to_status, details, source
             FROM order_audit WHERE order_local_id = ?1",
        )
        .unwrap();

    let row = stmt
        .query_row(params!["order-003"], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, String>(4)?,
            ))
        })
        .unwrap();

    assert_eq!(row.0, "order-003");
    assert_eq!(row.1, "Draft");
    assert_eq!(row.2, "Submitted");
    assert_eq!(row.3, Some(r#"{"reason":"user click"}"#.to_string()));
    assert_eq!(row.4, "user");
}

#[test]
fn test_insert_fill_idempotent() {
    let db = BrokerDb::open_in_memory().unwrap();
    let conn = db.conn().lock().unwrap();

    let fill = FillRow {
        order_local_id: "order-004".to_string(),
        ib_exec_id: "exec-abc-123".to_string(),
        timestamp: "2026-03-25T12:05:00Z".to_string(),
        shares: 50.0,
        price: 151.25,
        commission: Some(1.0),
        exchange: Some("ARCA".to_string()),
        side: "BOT".to_string(),
    };

    // First insert succeeds.
    insert_fill(&conn, &fill).unwrap();

    // Second insert with same ib_exec_id should silently do nothing.
    insert_fill(&conn, &fill).unwrap();

    // Only one row should exist.
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM fills WHERE ib_exec_id = ?1",
            params!["exec-abc-123"],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);
}

#[test]
fn test_get_orders_by_parent_id() {
    let db = BrokerDb::open_in_memory().unwrap();
    let conn = db.conn().lock().unwrap();

    // Insert parent
    let mut parent = make_order("parent-001", "Filled", "AAPL");
    parent.bracket_role = Some("PARENT".to_string());
    insert_order(&conn, &parent).unwrap();

    // Insert children
    let mut tp = make_order("tp-001", "Submitted", "AAPL");
    tp.parent_id = Some("parent-001".to_string());
    tp.bracket_role = Some("TAKE_PROFIT".to_string());
    insert_order(&conn, &tp).unwrap();

    let mut sl = make_order("sl-001", "Submitted", "AAPL");
    sl.parent_id = Some("parent-001".to_string());
    sl.bracket_role = Some("STOP_LOSS".to_string());
    insert_order(&conn, &sl).unwrap();

    // Unrelated order
    insert_order(&conn, &make_order("other-001", "Draft", "MSFT")).unwrap();

    let children = get_orders_by_parent_id(&conn, "parent-001").unwrap();
    assert_eq!(children.len(), 2);
    let ids: Vec<&str> = children.iter().map(|o| o.local_id.as_str()).collect();
    assert!(ids.contains(&"tp-001"));
    assert!(ids.contains(&"sl-001"));
}

#[test]
fn test_bracket_role_stored_as_text() {
    let db = BrokerDb::open_in_memory().unwrap();
    let conn = db.conn().lock().unwrap();

    let mut order = make_order("br-001", "Inactive", "AAPL");
    order.bracket_role = Some("TAKE_PROFIT".to_string());
    insert_order(&conn, &order).unwrap();

    let fetched = get_order(&conn, "br-001").unwrap().unwrap();
    assert_eq!(fetched.bracket_role, Some("TAKE_PROFIT".to_string()));
}

#[test]
fn test_get_orders_by_status() {
    let db = BrokerDb::open_in_memory().unwrap();
    let conn = db.conn().lock().unwrap();

    insert_order(&conn, &make_order("o1", "Draft", "AAPL")).unwrap();
    insert_order(&conn, &make_order("o2", "Submitted", "MSFT")).unwrap();
    insert_order(&conn, &make_order("o3", "Draft", "GOOG")).unwrap();
    insert_order(&conn, &make_order("o4", "Filled", "TSLA")).unwrap();

    let drafts = get_orders_by_status(&conn, "Draft").unwrap();
    assert_eq!(drafts.len(), 2);
    let ids: Vec<&str> = drafts.iter().map(|o| o.local_id.as_str()).collect();
    assert!(ids.contains(&"o1"));
    assert!(ids.contains(&"o3"));

    let submitted = get_orders_by_status(&conn, "Submitted").unwrap();
    assert_eq!(submitted.len(), 1);
    assert_eq!(submitted[0].local_id, "o2");

    let cancelled = get_orders_by_status(&conn, "Cancelled").unwrap();
    assert!(cancelled.is_empty());
}

// -----------------------------------------------------------------------
// Conversion layer tests
// -----------------------------------------------------------------------

#[test]
fn test_order_row_to_local_round_trip() {
    use crate::orders::types::{OrderAction, OrderKind};

    // Build a LocalOrder with many fields populated.
    let mut order =
        LocalOrder::new_draft("AAPL", OrderAction::Buy, OrderKind::Limit, 100.0);
    order.limit_price = Some(185.50);
    order.ib_order_id = Some(42);
    order.ib_perm_id = Some(123456);
    order.con_id = Some(265598);
    order.sec_type = SecurityType::Stock;
    order.exchange = "SMART".to_string();
    order.currency = "USD".to_string();
    order.tif = TimeInForce::Gtc;
    order.outside_rth = true;
    order.bracket_role = Some(BracketRole::Parent);
    order.parent_id = Some(Uuid::now_v7());
    order.oca_group = Some("oca-1".to_string());
    order.strategy = Some("momentum_scalp".to_string());
    order.tags = vec!["fast".to_string(), "earnings".to_string()];
    order.algo_strategy = Some("Adaptive".to_string());
    order.algo_params = Some(serde_json::json!({"adaptivePriority": "Normal"}));
    order.filled_qty = 25.0;
    order.remaining_qty = 75.0;
    order.avg_fill_price = Some(185.30);
    order.last_fill_price = Some(185.25);
    order.commission = Some(1.50);
    order.activation_count = 2;
    order.status = OrderStatus::Inactive;

    // Round-trip: LocalOrder -> OrderRow -> LocalOrder
    let row = local_to_order_row(&order);
    let restored = order_row_to_local(&row).expect("round-trip conversion should succeed");

    // Identity
    assert_eq!(restored.id, order.id);
    assert_eq!(restored.ib_order_id, order.ib_order_id);
    assert_eq!(restored.ib_perm_id, order.ib_perm_id);

    // Contract
    assert_eq!(restored.symbol, order.symbol);
    assert_eq!(restored.con_id, order.con_id);
    assert_eq!(restored.sec_type, order.sec_type);
    assert_eq!(restored.exchange, order.exchange);
    assert_eq!(restored.currency, order.currency);

    // Order params
    assert_eq!(restored.action, order.action);
    assert_eq!(restored.order_type, order.order_type);
    assert_eq!(restored.quantity, order.quantity);
    assert_eq!(restored.limit_price, order.limit_price);
    assert_eq!(restored.stop_price, order.stop_price);
    assert_eq!(restored.trail_amount, order.trail_amount);
    assert_eq!(restored.trail_percent, order.trail_percent);
    assert_eq!(restored.tif, order.tif);

    // State & grouping
    assert_eq!(restored.status, order.status);
    assert_eq!(restored.parent_id, order.parent_id);
    assert_eq!(restored.oca_group, order.oca_group);
    assert_eq!(restored.bracket_role, order.bracket_role);
    assert_eq!(restored.strategy, order.strategy);
    assert_eq!(restored.tags, order.tags);

    // Algo
    assert_eq!(restored.algo_strategy, order.algo_strategy);
    assert_eq!(restored.algo_params, order.algo_params);

    // Execution
    assert_eq!(restored.outside_rth, order.outside_rth);
    assert_eq!(restored.filled_qty, order.filled_qty);
    assert_eq!(restored.remaining_qty, order.remaining_qty);
    assert_eq!(restored.avg_fill_price, order.avg_fill_price);
    assert_eq!(restored.last_fill_price, order.last_fill_price);
    assert_eq!(restored.commission, order.commission);

    // Activation
    assert_eq!(restored.activation_count, order.activation_count);

    // Timestamps: compare to the second (rfc3339 round-trip may truncate sub-second)
    assert_eq!(
        restored.created_at.timestamp(),
        order.created_at.timestamp()
    );
    assert_eq!(
        restored.updated_at.timestamp(),
        order.updated_at.timestamp()
    );
}

#[test]
fn test_order_row_to_local_hard_fail_on_bad_status() {
    let row = make_order(
        "019577a0-0000-7000-8000-000000000001",
        "BOGUS_STATUS",
        "AAPL",
    );
    let result = order_row_to_local(&row);
    assert!(result.is_err(), "should fail on invalid status string");
    let err = result.unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("status"),
        "error message should reference the 'status' field: {msg}"
    );
}

#[test]
fn test_order_row_to_local_soft_fail_on_bad_bracket_role() {
    let mut row = make_order(
        "019577a0-0000-7000-8000-000000000002",
        "Inactive",
        "AAPL",
    );
    row.bracket_role = Some("NOT_A_ROLE".to_string());

    let result = order_row_to_local(&row);
    assert!(
        result.is_ok(),
        "should succeed with bad bracket_role (soft fail)"
    );
    let order = result.unwrap();
    assert_eq!(
        order.bracket_role, None,
        "bad bracket_role should default to None"
    );
}

#[test]
fn test_persist_and_transition_bracket() {
    use crate::orders::types::{OrderAction, OrderKind};

    let db = BrokerDb::open_in_memory().unwrap();
    let conn = db.conn().lock().unwrap();

    // Build a bracket group with parent + TP + SL, all Inactive.
    let mut parent =
        LocalOrder::new_draft("AAPL", OrderAction::Buy, OrderKind::Market, 100.0);
    parent.status = OrderStatus::Inactive;
    let parent_id = parent.id;

    let mut tp =
        LocalOrder::new_draft("AAPL", OrderAction::Sell, OrderKind::Limit, 100.0);
    tp.status = OrderStatus::Inactive;
    tp.parent_id = Some(parent_id);
    tp.bracket_role = Some(BracketRole::TakeProfit);
    tp.limit_price = Some(195.0);

    let mut sl =
        LocalOrder::new_draft("AAPL", OrderAction::Sell, OrderKind::Stop, 100.0);
    sl.status = OrderStatus::Inactive;
    sl.parent_id = Some(parent_id);
    sl.bracket_role = Some(BracketRole::StopLoss);
    sl.stop_price = Some(180.0);

    let mut group = BracketGroup {
        parent,
        take_profit: Some(tp),
        stop_loss: Some(sl),
    };

    // Persist and transition.
    persist_and_transition_to_pending_submit(&conn, &mut group).unwrap();

    // Verify in-memory status was updated.
    assert_eq!(group.parent.status, OrderStatus::PendingSubmit);
    assert_eq!(
        group.take_profit.as_ref().unwrap().status,
        OrderStatus::PendingSubmit
    );
    assert_eq!(
        group.stop_loss.as_ref().unwrap().status,
        OrderStatus::PendingSubmit
    );

    // Verify all three rows exist in the DB with PendingSubmit status.
    for leg in group.legs() {
        let fetched = get_order(&conn, &leg.id.to_string())
            .unwrap()
            .expect("leg should be persisted");
        assert_eq!(
            fetched.status, "PendingSubmit",
            "DB row should have been transitioned to PendingSubmit"
        );
    }

    // Verify audit rows were written for all legs.
    let audit_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM order_audit WHERE to_status = 'PendingSubmit'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(audit_count, 3, "should have 3 audit rows (parent + TP + SL)");
}
