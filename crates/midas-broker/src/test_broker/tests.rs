use super::*;

fn make_broker() -> TestBroker {
    TestBroker::new(TestBrokerConfig::default())
}

/// Helper: count callbacks of a specific status string.
fn count_status(cbs: &[BrokerCallback], status_str: &str) -> usize {
    cbs.iter()
        .filter(|cb| {
            matches!(
                cb,
                BrokerCallback::OrderStatus { status, .. } if status == status_str
            )
        })
        .count()
}

fn count_executions(cbs: &[BrokerCallback]) -> usize {
    cbs.iter()
        .filter(|cb| matches!(cb, BrokerCallback::Execution { .. }))
        .count()
}

// ── 1. next_order_id ─────────────────────────────────────────────────

#[test]
fn test_next_order_id_increments() {
    let broker = make_broker();
    let id1 = broker.next_order_id();
    let id2 = broker.next_order_id();
    let id3 = broker.next_order_id();
    assert_eq!(id1, 1000);
    assert_eq!(id2, 1001);
    assert_eq!(id3, 1002);
}

// ── 2. Market order instant fill ─────────────────────────────────────

#[test]
fn test_place_market_order_instant_fill() {
    let broker = make_broker();
    broker.set_market_price("AAPL", 185.50);

    let id = broker.next_order_id();
    broker
        .place_order(
            id, "AAPL", "BUY", "MKT", 100.0, None, None, None, true, "DAY", false,
        )
        .unwrap();

    let cbs = broker.poll_callbacks();

    // Should have: Submitted, Execution, Filled (3 callbacks)
    assert_eq!(count_status(&cbs, "Submitted"), 1, "expected 1 Submitted");
    assert_eq!(count_executions(&cbs), 1, "expected 1 Execution");
    assert_eq!(count_status(&cbs, "Filled"), 1, "expected 1 Filled");

    // Verify execution details (BUY fills at ask = base + half_spread)
    let half_spread = 0.005; // default_spread=0.01 / 2
    let expected_fill = 185.50 + half_spread;
    let exec = cbs
        .iter()
        .find(|cb| matches!(cb, BrokerCallback::Execution { .. }))
        .unwrap();
    if let BrokerCallback::Execution { shares, price, .. } = exec {
        assert_eq!(*shares, 100.0);
        assert!(
            (*price - expected_fill).abs() < f64::EPSILON,
            "BUY market order should fill at ask ({expected_fill}), got {price}"
        );
    }

    // Verify filled status
    let filled = cbs
        .iter()
        .find(|cb| {
            matches!(
                cb,
                BrokerCallback::OrderStatus { status, .. } if status == "Filled"
            )
        })
        .unwrap();
    if let BrokerCallback::OrderStatus {
        filled: f,
        remaining,
        avg_fill_price,
        ..
    } = filled
    {
        assert_eq!(*f, 100.0);
        assert_eq!(*remaining, 0.0);
        assert!(
            (*avg_fill_price - expected_fill).abs() < f64::EPSILON,
            "avg_fill_price should be {expected_fill}, got {avg_fill_price}"
        );
    }
}

// ── 3. Bracket held until transmit ───────────────────────────────────

#[test]
fn test_bracket_held_until_transmit() {
    let broker = make_broker();
    broker.set_market_price("AAPL", 185.50);

    let parent_id = broker.next_order_id();
    let tp_id = broker.next_order_id();
    let sl_id = broker.next_order_id();

    // Parent: MKT BUY, transmit=false
    broker
        .place_order(
            parent_id, "AAPL", "BUY", "MKT", 100.0, None, None, None, false, "DAY", false,
        )
        .unwrap();
    assert!(
        broker.poll_callbacks().is_empty(),
        "no callbacks before transmit"
    );

    // TP: LMT SELL, transmit=false
    broker
        .place_order(
            tp_id,
            "AAPL",
            "SELL",
            "LMT",
            100.0,
            Some(192.0),
            None,
            Some(parent_id),
            false,
            "DAY",
            false,
        )
        .unwrap();
    assert!(
        broker.poll_callbacks().is_empty(),
        "no callbacks before transmit"
    );

    // SL: STP SELL, transmit=true -- activates the bracket
    broker
        .place_order(
            sl_id,
            "AAPL",
            "SELL",
            "STP",
            100.0,
            None,
            Some(182.0),
            Some(parent_id),
            true,
            "DAY",
            false,
        )
        .unwrap();

    let cbs = broker.poll_callbacks();
    assert!(
        !cbs.is_empty(),
        "callbacks should exist after transmit=true"
    );

    // Parent: Submitted + Execution + Filled
    let parent_submitted = cbs.iter().any(|cb| {
        matches!(
            cb,
            BrokerCallback::OrderStatus { ib_order_id, status, .. }
            if *ib_order_id == parent_id && status == "Submitted"
        )
    });
    assert!(parent_submitted, "parent should get Submitted");

    let parent_filled = cbs.iter().any(|cb| {
        matches!(
            cb,
            BrokerCallback::OrderStatus { ib_order_id, status, .. }
            if *ib_order_id == parent_id && status == "Filled"
        )
    });
    assert!(parent_filled, "parent should get Filled (MKT instant)");

    // Children should be activated
    let tp_activated = cbs.iter().any(|cb| {
        matches!(
            cb,
            BrokerCallback::OrderStatus { ib_order_id, .. } if *ib_order_id == tp_id
        )
    });
    assert!(tp_activated, "TP child should be activated");

    let sl_activated = cbs.iter().any(|cb| {
        matches!(
            cb,
            BrokerCallback::OrderStatus { ib_order_id, .. } if *ib_order_id == sl_id
        )
    });
    assert!(sl_activated, "SL child should be activated");
}

// ── 4. Bracket parent fill activates children ────────────────────────

#[test]
fn test_bracket_parent_fill_activates_children() {
    let broker = make_broker();
    broker.set_market_price("AAPL", 185.50);

    let parent_id = broker.next_order_id();
    let tp_id = broker.next_order_id();
    let sl_id = broker.next_order_id();

    // Place bracket
    broker
        .place_order(
            parent_id, "AAPL", "BUY", "MKT", 100.0, None, None, None, false, "DAY", false,
        )
        .unwrap();
    broker
        .place_order(
            tp_id,
            "AAPL",
            "SELL",
            "LMT",
            100.0,
            Some(192.0),
            None,
            Some(parent_id),
            false,
            "DAY",
            false,
        )
        .unwrap();
    broker
        .place_order(
            sl_id,
            "AAPL",
            "SELL",
            "STP",
            100.0,
            None,
            Some(182.0),
            Some(parent_id),
            true,
            "DAY",
            false,
        )
        .unwrap();

    let cbs = broker.poll_callbacks();

    // After parent fills (MKT instant), TP should be Submitted, SL should be PreSubmitted
    let tp_submitted = cbs.iter().any(|cb| {
        matches!(
            cb,
            BrokerCallback::OrderStatus { ib_order_id, status, .. }
            if *ib_order_id == tp_id && status == "Submitted"
        )
    });
    assert!(tp_submitted, "TP should get Submitted after parent fills");

    let sl_presubmitted = cbs.iter().any(|cb| {
        matches!(
            cb,
            BrokerCallback::OrderStatus { ib_order_id, status, .. }
            if *ib_order_id == sl_id && status == "PreSubmitted"
        )
    });
    assert!(
        sl_presubmitted,
        "SL should get PreSubmitted after parent fills"
    );
}

// ── 5. TP fill cancels SL (OCA) ─────────────────────────────────────

#[test]
fn test_bracket_tp_fill_cancels_sl() {
    let broker = make_broker();
    broker.set_market_price("AAPL", 185.50);

    let parent_id = broker.next_order_id();
    let tp_id = broker.next_order_id();
    let sl_id = broker.next_order_id();

    // Place and activate bracket
    broker
        .place_order(
            parent_id, "AAPL", "BUY", "MKT", 100.0, None, None, None, false, "DAY", false,
        )
        .unwrap();
    broker
        .place_order(
            tp_id,
            "AAPL",
            "SELL",
            "LMT",
            100.0,
            Some(192.0),
            None,
            Some(parent_id),
            false,
            "DAY",
            false,
        )
        .unwrap();
    broker
        .place_order(
            sl_id,
            "AAPL",
            "SELL",
            "STP",
            100.0,
            None,
            Some(182.0),
            Some(parent_id),
            true,
            "DAY",
            false,
        )
        .unwrap();

    // Drain bracket activation callbacks
    let _ = broker.poll_callbacks();

    // Simulate TP fill
    broker.simulate_fill(tp_id, 192.0, 100.0);

    let cbs = broker.poll_callbacks();

    // TP should be filled
    let tp_filled = cbs.iter().any(|cb| {
        matches!(
            cb,
            BrokerCallback::OrderStatus { ib_order_id, status, .. }
            if *ib_order_id == tp_id && status == "Filled"
        )
    });
    assert!(tp_filled, "TP should be Filled");

    // SL should be cancelled (OCA)
    let sl_cancelled = cbs.iter().any(|cb| {
        matches!(
            cb,
            BrokerCallback::OrderStatus { ib_order_id, status, .. }
            if *ib_order_id == sl_id && status == "Cancelled"
        )
    });
    assert!(sl_cancelled, "SL should be Cancelled via OCA when TP fills");
}

// ── 6. SL fill cancels TP (OCA) ─────────────────────────────────────

#[test]
fn test_bracket_sl_fill_cancels_tp() {
    let broker = make_broker();
    broker.set_market_price("AAPL", 185.50);

    let parent_id = broker.next_order_id();
    let tp_id = broker.next_order_id();
    let sl_id = broker.next_order_id();

    // Place and activate bracket
    broker
        .place_order(
            parent_id, "AAPL", "BUY", "MKT", 100.0, None, None, None, false, "DAY", false,
        )
        .unwrap();
    broker
        .place_order(
            tp_id,
            "AAPL",
            "SELL",
            "LMT",
            100.0,
            Some(192.0),
            None,
            Some(parent_id),
            false,
            "DAY",
            false,
        )
        .unwrap();
    broker
        .place_order(
            sl_id,
            "AAPL",
            "SELL",
            "STP",
            100.0,
            None,
            Some(182.0),
            Some(parent_id),
            true,
            "DAY",
            false,
        )
        .unwrap();

    // Drain bracket activation callbacks
    let _ = broker.poll_callbacks();

    // Simulate SL fill
    broker.simulate_fill(sl_id, 181.80, 100.0);

    let cbs = broker.poll_callbacks();

    // SL should be filled
    let sl_filled = cbs.iter().any(|cb| {
        matches!(
            cb,
            BrokerCallback::OrderStatus { ib_order_id, status, .. }
            if *ib_order_id == sl_id && status == "Filled"
        )
    });
    assert!(sl_filled, "SL should be Filled");

    // TP should be cancelled (OCA)
    let tp_cancelled = cbs.iter().any(|cb| {
        matches!(
            cb,
            BrokerCallback::OrderStatus { ib_order_id, status, .. }
            if *ib_order_id == tp_id && status == "Cancelled"
        )
    });
    assert!(tp_cancelled, "TP should be Cancelled via OCA when SL fills");
}

// ── 7. Parent cancel cascades to children ────────────────────────────

#[test]
fn test_bracket_parent_cancel_cancels_children() {
    let broker = make_broker();
    broker.set_market_price("AAPL", 185.50);

    let parent_id = broker.next_order_id();
    let tp_id = broker.next_order_id();
    let sl_id = broker.next_order_id();

    // Place bracket with LMT parent (so it doesn't auto-fill)
    broker
        .place_order(
            parent_id,
            "AAPL",
            "BUY",
            "LMT",
            100.0,
            Some(180.0),
            None,
            None,
            false,
            "DAY",
            false,
        )
        .unwrap();
    broker
        .place_order(
            tp_id,
            "AAPL",
            "SELL",
            "LMT",
            100.0,
            Some(192.0),
            None,
            Some(parent_id),
            false,
            "DAY",
            false,
        )
        .unwrap();
    broker
        .place_order(
            sl_id,
            "AAPL",
            "SELL",
            "STP",
            100.0,
            None,
            Some(182.0),
            Some(parent_id),
            true,
            "DAY",
            false,
        )
        .unwrap();

    // Drain bracket activation callbacks
    let _ = broker.poll_callbacks();

    // Cancel parent
    broker.cancel_order(parent_id).unwrap();

    let cbs = broker.poll_callbacks();

    let parent_cancelled = cbs.iter().any(|cb| {
        matches!(
            cb,
            BrokerCallback::OrderStatus { ib_order_id, status, .. }
            if *ib_order_id == parent_id && status == "Cancelled"
        )
    });
    assert!(parent_cancelled, "parent should be Cancelled");

    let tp_cancelled = cbs.iter().any(|cb| {
        matches!(
            cb,
            BrokerCallback::OrderStatus { ib_order_id, status, .. }
            if *ib_order_id == tp_id && status == "Cancelled"
        )
    });
    assert!(
        tp_cancelled,
        "TP should be Cancelled when parent is cancelled"
    );

    let sl_cancelled = cbs.iter().any(|cb| {
        matches!(
            cb,
            BrokerCallback::OrderStatus { ib_order_id, status, .. }
            if *ib_order_id == sl_id && status == "Cancelled"
        )
    });
    assert!(
        sl_cancelled,
        "SL should be Cancelled when parent is cancelled"
    );
}

// ── 8. Manual simulate_fill ──────────────────────────────────────────

#[test]
fn test_simulate_fill_manual() {
    let broker = make_broker();
    broker.set_market_price("AAPL", 185.50);

    let id = broker.next_order_id();
    // Place a limit order (won't auto-fill)
    broker
        .place_order(
            id,
            "AAPL",
            "BUY",
            "LMT",
            50.0,
            Some(180.0),
            None,
            None,
            true,
            "DAY",
            false,
        )
        .unwrap();

    // Drain activation callbacks
    let _ = broker.poll_callbacks();

    // Manually fill at a specific price
    broker.simulate_fill(id, 179.50, 50.0);

    let cbs = broker.poll_callbacks();

    // Should have Execution + Filled
    assert_eq!(count_executions(&cbs), 1, "expected 1 Execution callback");
    assert_eq!(
        count_status(&cbs, "Filled"),
        1,
        "expected 1 Filled callback"
    );

    // Verify fill price
    let exec = cbs
        .iter()
        .find(|cb| matches!(cb, BrokerCallback::Execution { .. }))
        .unwrap();
    if let BrokerCallback::Execution { shares, price, .. } = exec {
        assert_eq!(*shares, 50.0);
        assert_eq!(*price, 179.50);
    }
}

// ── Phase 3: Partial fill tranches ──────────────────────────────────

#[test]
fn test_partial_fill_tranches() {
    let config = TestBrokerConfig {
        partial_fill_threshold: 100.0,
        partial_fill_tranches: 3,
        ..Default::default()
    };
    let broker = TestBroker::new(config);
    broker.set_market_price("AAPL", 185.50);

    let id = broker.next_order_id();
    broker
        .place_order(
            id, "AAPL", "BUY", "MKT", 300.0, None, None, None, true, "DAY", false,
        )
        .unwrap();

    let cbs = broker.poll_callbacks();

    // Should have: 1 Submitted (activation) + 3 Executions + 2 intermediate "PartiallyFilled" + 1 "Filled"
    let exec_count = count_executions(&cbs);
    assert_eq!(
        exec_count, 3,
        "expected 3 Execution callbacks for 3 tranches"
    );

    // The final callback should be Filled
    assert_eq!(count_status(&cbs, "Filled"), 1, "expected exactly 1 Filled");

    // 1 Submitted from activation
    assert_eq!(
        count_status(&cbs, "Submitted"),
        1,
        "expected 1 Submitted (activation)"
    );

    // 2 PartiallyFilled from intermediate tranches
    assert_eq!(
        count_status(&cbs, "PartiallyFilled"),
        2,
        "expected 2 PartiallyFilled for intermediate tranches"
    );

    // Verify each execution has 100 shares
    let exec_shares: Vec<f64> = cbs
        .iter()
        .filter_map(|cb| match cb {
            BrokerCallback::Execution { shares, .. } => Some(*shares),
            _ => None,
        })
        .collect();
    assert_eq!(exec_shares.len(), 3);
    assert!((exec_shares[0] - 100.0).abs() < f64::EPSILON);
    assert!((exec_shares[1] - 100.0).abs() < f64::EPSILON);
    assert!((exec_shares[2] - 100.0).abs() < f64::EPSILON);
}

// ── Phase 5: Position tracking ──────────────────────────────────────

#[test]
fn test_position_long_buy() {
    let broker = make_broker();
    broker.set_market_price("AAPL", 185.50);

    let id = broker.next_order_id();
    broker
        .place_order(
            id, "AAPL", "BUY", "MKT", 100.0, None, None, None, true, "DAY", false,
        )
        .unwrap();
    let _ = broker.poll_callbacks();

    let positions = broker.positions();
    assert_eq!(positions.len(), 1);
    let (symbol, qty, avg_cost) = &positions[0];
    assert_eq!(symbol, "AAPL");
    assert!(
        (qty - 100.0).abs() < f64::EPSILON,
        "expected +100 shares, got {qty}"
    );
    // BUY fills at ask = 185.50 + 0.005 (half of default spread 0.01)
    let expected_avg = 185.505;
    assert!(
        (avg_cost - expected_avg).abs() < f64::EPSILON,
        "expected avg_cost {expected_avg}, got {avg_cost}"
    );
}

#[test]
fn test_position_close_sell() {
    let broker = make_broker();
    broker.set_market_price("AAPL", 185.50);

    // Open long position
    let id1 = broker.next_order_id();
    broker
        .place_order(
            id1, "AAPL", "BUY", "MKT", 100.0, None, None, None, true, "DAY", false,
        )
        .unwrap();
    let _ = broker.poll_callbacks();

    // Close position by selling
    let id2 = broker.next_order_id();
    broker
        .place_order(
            id2, "AAPL", "SELL", "MKT", 100.0, None, None, None, true, "DAY", false,
        )
        .unwrap();
    let _ = broker.poll_callbacks();

    // Position should be flat
    let positions = broker.positions();
    assert!(
        positions.is_empty(),
        "expected no open positions after close, got {positions:?}"
    );
}

#[test]
fn test_account_cash_decreases_on_buy() {
    let broker = make_broker();
    let initial_cash = broker.cash_balance();
    assert!(
        (initial_cash - 100_000.0).abs() < f64::EPSILON,
        "expected 100k initial cash"
    );

    broker.set_market_price("AAPL", 185.50);

    let id = broker.next_order_id();
    broker
        .place_order(
            id, "AAPL", "BUY", "MKT", 100.0, None, None, None, true, "DAY", false,
        )
        .unwrap();
    let _ = broker.poll_callbacks();

    let cash_after = broker.cash_balance();
    // BUY fills at ask = 185.50 + 0.005 (half of default spread 0.01)
    let fill_price = 185.505;
    let expected_cost = 100.0 * fill_price; // notional
    let expected_commission = 100.0 * 0.005; // commission
    let expected_cash = initial_cash - expected_cost - expected_commission;

    assert!(
        (cash_after - expected_cash).abs() < 0.01,
        "expected cash {expected_cash:.2}, got {cash_after:.2}"
    );
}

// ── Phase 6: Error injection ────────────────────────────────────────

#[test]
fn test_rejection_by_configuration() {
    let config = TestBrokerConfig {
        rejection_rate: 0.5, // every 2nd order rejected
        ..Default::default()
    };
    let broker = TestBroker::new(config);
    broker.set_market_price("AAPL", 185.50);

    // Place 4 orders; every 2nd should be rejected
    let mut rejected_count = 0;
    let mut accepted_count = 0;

    for _ in 0..4 {
        let id = broker.next_order_id();
        broker
            .place_order(
                id, "AAPL", "BUY", "MKT", 10.0, None, None, None, true, "DAY", false,
            )
            .unwrap();

        let cbs = broker.poll_callbacks();
        let has_rejection = cbs
            .iter()
            .any(|cb| matches!(cb, BrokerCallback::OrderRejected { .. }));
        let has_fill = cbs.iter().any(|cb| {
            matches!(
                cb,
                BrokerCallback::OrderStatus { status, .. } if status == "Filled"
            )
        });

        if has_rejection {
            rejected_count += 1;
        }
        if has_fill {
            accepted_count += 1;
        }
    }

    assert!(
        rejected_count > 0,
        "expected at least 1 rejection with rate=0.5"
    );
    assert!(
        accepted_count > 0,
        "expected at least 1 accepted order with rate=0.5"
    );
    assert_eq!(
        rejected_count + accepted_count,
        4,
        "all orders should be either rejected or filled"
    );
}

#[test]
fn test_disconnect_reconnect() {
    let broker = make_broker();
    assert!(broker.is_connected(), "should start connected");

    // Disconnect
    broker.simulate_disconnect();
    assert!(
        !broker.is_connected(),
        "should be disconnected after simulate_disconnect"
    );

    let cbs = broker.poll_callbacks();
    let disconnected = cbs.iter().any(|cb| {
        matches!(
            cb,
            BrokerCallback::ConnectionStatus {
                connected: false,
                ..
            }
        )
    });
    assert!(disconnected, "should get ConnectionStatus(false) callback");

    // Reconnect
    broker.simulate_reconnect();
    assert!(
        broker.is_connected(),
        "should be connected after simulate_reconnect"
    );

    let cbs = broker.poll_callbacks();
    let reconnected = cbs.iter().any(|cb| {
        matches!(
            cb,
            BrokerCallback::ConnectionStatus {
                connected: true,
                server_version: Some(176)
            }
        )
    });
    assert!(
        reconnected,
        "should get ConnectionStatus(true, 176) callback"
    );
}

// ── Phase 2: Limit order fills ──────────────────────────────────────

#[test]
fn test_limit_buy_fills_at_price() {
    let broker = make_broker();
    broker.set_market_price("AAPL", 185.50);

    let id = broker.next_order_id();
    broker
        .place_order(
            id,
            "AAPL",
            "BUY",
            "LMT",
            100.0,
            Some(180.0),
            None,
            None,
            true,
            "DAY",
            false,
        )
        .unwrap();

    let cbs = broker.poll_callbacks();
    assert_eq!(count_status(&cbs, "Submitted"), 1);
    assert_eq!(count_executions(&cbs), 0, "should NOT fill at 185.50");

    broker.set_market_price("AAPL", 180.0);
    let cbs = broker.poll_callbacks();
    assert_eq!(count_executions(&cbs), 1, "expected fill at limit");
    assert_eq!(count_status(&cbs, "Filled"), 1);

    let exec = cbs
        .iter()
        .find(|cb| matches!(cb, BrokerCallback::Execution { .. }))
        .unwrap();
    if let BrokerCallback::Execution { shares, price, .. } = exec {
        assert_eq!(*shares, 100.0);
        assert_eq!(*price, 180.0, "limit buy fills at limit price");
    }
}

#[test]
fn test_limit_sell_fills_at_price() {
    let broker = make_broker();
    broker.set_market_price("AAPL", 185.50);

    let id = broker.next_order_id();
    broker
        .place_order(
            id,
            "AAPL",
            "SELL",
            "LMT",
            100.0,
            Some(192.0),
            None,
            None,
            true,
            "DAY",
            false,
        )
        .unwrap();

    let cbs = broker.poll_callbacks();
    assert_eq!(count_status(&cbs, "Submitted"), 1);
    assert_eq!(count_executions(&cbs), 0, "should NOT fill at 185.50");

    broker.set_market_price("AAPL", 190.0);
    let cbs = broker.poll_callbacks();
    assert_eq!(count_executions(&cbs), 0, "should NOT fill at 190");

    broker.set_market_price("AAPL", 193.0);
    let cbs = broker.poll_callbacks();
    assert_eq!(
        count_executions(&cbs),
        1,
        "expected fill when price crosses"
    );
    assert_eq!(count_status(&cbs, "Filled"), 1);

    let exec = cbs
        .iter()
        .find(|cb| matches!(cb, BrokerCallback::Execution { .. }))
        .unwrap();
    if let BrokerCallback::Execution { shares, price, .. } = exec {
        assert_eq!(*shares, 100.0);
        // SELL LMT fills at better-of limit and market: max(192.0, 193.0) = 193.0
        assert_eq!(*price, 193.0, "limit sell fills at better (higher) price");
    }
}

#[test]
fn test_limit_buy_no_fill_when_price_above() {
    let broker = make_broker();
    broker.set_market_price("AAPL", 185.50);

    let id = broker.next_order_id();
    broker
        .place_order(
            id,
            "AAPL",
            "BUY",
            "LMT",
            100.0,
            Some(180.0),
            None,
            None,
            true,
            "DAY",
            false,
        )
        .unwrap();
    let _ = broker.poll_callbacks();

    broker.set_market_price("AAPL", 181.0);
    let cbs = broker.poll_callbacks();
    assert_eq!(
        count_executions(&cbs),
        0,
        "should NOT fill at 181 when limit is 180"
    );
}

// ── Phase 2: Stop order triggers ────────────────────────────────────

#[test]
fn test_stop_triggers_at_price() {
    let broker = make_broker();
    broker.set_market_price("AAPL", 185.50);

    let parent_id = broker.next_order_id();
    let tp_id = broker.next_order_id();
    let sl_id = broker.next_order_id();

    broker
        .place_order(
            parent_id, "AAPL", "BUY", "MKT", 100.0, None, None, None, false, "DAY", false,
        )
        .unwrap();
    broker
        .place_order(
            tp_id,
            "AAPL",
            "SELL",
            "LMT",
            100.0,
            Some(192.0),
            None,
            Some(parent_id),
            false,
            "DAY",
            false,
        )
        .unwrap();
    broker
        .place_order(
            sl_id,
            "AAPL",
            "SELL",
            "STP",
            100.0,
            None,
            Some(182.0),
            Some(parent_id),
            true,
            "DAY",
            false,
        )
        .unwrap();
    let _ = broker.poll_callbacks();

    {
        let inner = broker.inner.lock();
        assert_eq!(
            inner.orders.get(&sl_id).unwrap().status,
            SimOrderStatus::Triggered
        );
    }

    broker.set_market_price("AAPL", 183.0);
    let cbs = broker.poll_callbacks();
    assert_eq!(count_executions(&cbs), 0, "should NOT trigger at 183");

    broker.set_market_price("AAPL", 181.50);
    let cbs = broker.poll_callbacks();

    assert!(
        cbs.iter().any(|cb| matches!(
            cb,
            BrokerCallback::OrderStatus { ib_order_id, status, .. }
            if *ib_order_id == sl_id && status == "Submitted"
        )),
        "SL should transition to Submitted on trigger"
    );

    assert!(
        cbs.iter().any(|cb| matches!(
            cb,
            BrokerCallback::Execution { ib_order_id, .. } if *ib_order_id == sl_id
        )),
        "SL should have an execution"
    );

    assert!(
        cbs.iter().any(|cb| matches!(
            cb,
            BrokerCallback::OrderStatus { ib_order_id, status, .. }
            if *ib_order_id == sl_id && status == "Filled"
        )),
        "SL should be Filled"
    );

    let exec = cbs
        .iter()
        .find(|cb| {
            matches!(
                cb,
                BrokerCallback::Execution { ib_order_id, .. } if *ib_order_id == sl_id
            )
        })
        .unwrap();
    if let BrokerCallback::Execution { price, shares, .. } = exec {
        // SELL STP fills at bid = market - half_spread = 181.50 - 0.005
        let expected = 181.50 - 0.005;
        assert!(
            (*price - expected).abs() < f64::EPSILON,
            "SELL stop fills at bid ({expected}), got {price}"
        );
        assert_eq!(*shares, 100.0);
    }

    assert!(
        cbs.iter().any(|cb| matches!(
            cb,
            BrokerCallback::OrderStatus { ib_order_id, status, .. }
            if *ib_order_id == tp_id && status == "Cancelled"
        )),
        "TP should be OCA-cancelled when SL fills"
    );
}

// ── Phase 2: set_market_price triggers fills ────────────────────────

#[test]
fn test_set_market_price_triggers_fills() {
    let broker = make_broker();
    broker.set_market_price("AAPL", 185.50);

    let id = broker.next_order_id();
    broker
        .place_order(
            id,
            "AAPL",
            "BUY",
            "LMT",
            50.0,
            Some(183.0),
            None,
            None,
            true,
            "DAY",
            false,
        )
        .unwrap();
    let _ = broker.poll_callbacks();

    broker.set_market_price("AAPL", 182.0);
    let cbs = broker.poll_callbacks();

    assert_eq!(
        count_executions(&cbs),
        1,
        "limit should fill when price crosses"
    );
    assert_eq!(count_status(&cbs, "Filled"), 1);

    let exec = cbs
        .iter()
        .find(|cb| matches!(cb, BrokerCallback::Execution { .. }))
        .unwrap();
    if let BrokerCallback::Execution { price, shares, .. } = exec {
        // BUY LMT fills at better-of limit and market: min(183.0, 182.0) = 182.0
        assert_eq!(*price, 182.0, "should fill at better (lower) price for BUY");
        assert_eq!(*shares, 50.0);
    }
}

#[test]
fn test_set_market_price_triggers_stop_in_bracket() {
    let broker = make_broker();
    broker.set_market_price("MSFT", 400.0);

    let parent_id = broker.next_order_id();
    let tp_id = broker.next_order_id();
    let sl_id = broker.next_order_id();

    broker
        .place_order(
            parent_id, "MSFT", "BUY", "MKT", 50.0, None, None, None, false, "DAY", false,
        )
        .unwrap();
    broker
        .place_order(
            tp_id,
            "MSFT",
            "SELL",
            "LMT",
            50.0,
            Some(420.0),
            None,
            Some(parent_id),
            false,
            "DAY",
            false,
        )
        .unwrap();
    broker
        .place_order(
            sl_id,
            "MSFT",
            "SELL",
            "STP",
            50.0,
            None,
            Some(390.0),
            Some(parent_id),
            true,
            "DAY",
            false,
        )
        .unwrap();
    let _ = broker.poll_callbacks();

    broker.set_market_price("MSFT", 389.0);
    let cbs = broker.poll_callbacks();

    assert!(
        cbs.iter().any(|cb| matches!(
            cb,
            BrokerCallback::OrderStatus { ib_order_id, status, .. }
            if *ib_order_id == sl_id && status == "Filled"
        )),
        "SL should fill"
    );

    assert!(
        cbs.iter().any(|cb| matches!(
            cb,
            BrokerCallback::OrderStatus { ib_order_id, status, .. }
            if *ib_order_id == tp_id && status == "Cancelled"
        )),
        "TP should be OCA-cancelled"
    );
}

#[test]
fn test_set_market_price_triggers_limit_fill_in_bracket() {
    let broker = make_broker();
    broker.set_market_price("AAPL", 185.50);

    let parent_id = broker.next_order_id();
    let tp_id = broker.next_order_id();
    let sl_id = broker.next_order_id();

    broker
        .place_order(
            parent_id, "AAPL", "BUY", "MKT", 100.0, None, None, None, false, "DAY", false,
        )
        .unwrap();
    broker
        .place_order(
            tp_id,
            "AAPL",
            "SELL",
            "LMT",
            100.0,
            Some(192.0),
            None,
            Some(parent_id),
            false,
            "DAY",
            false,
        )
        .unwrap();
    broker
        .place_order(
            sl_id,
            "AAPL",
            "SELL",
            "STP",
            100.0,
            None,
            Some(182.0),
            Some(parent_id),
            true,
            "DAY",
            false,
        )
        .unwrap();
    let _ = broker.poll_callbacks();

    broker.set_market_price("AAPL", 193.0);
    let cbs = broker.poll_callbacks();

    assert!(
        cbs.iter().any(|cb| matches!(
            cb,
            BrokerCallback::OrderStatus { ib_order_id, status, .. }
            if *ib_order_id == tp_id && status == "Filled"
        )),
        "TP should fill when price crosses limit"
    );

    let exec = cbs
        .iter()
        .find(|cb| {
            matches!(
                cb,
                BrokerCallback::Execution { ib_order_id, .. } if *ib_order_id == tp_id
            )
        })
        .unwrap();
    if let BrokerCallback::Execution { price, .. } = exec {
        // SELL LMT fills at better-of limit and market: max(192.0, 193.0) = 193.0
        assert_eq!(*price, 193.0, "TP fills at better (higher) price for SELL");
    }

    assert!(
        cbs.iter().any(|cb| matches!(
            cb,
            BrokerCallback::OrderStatus { ib_order_id, status, .. }
            if *ib_order_id == sl_id && status == "Cancelled"
        )),
        "SL should be OCA-cancelled when TP fills"
    );
}

// ── Phase 2: Stop-limit orders ──────────────────────────────────────

#[test]
fn test_stop_limit_triggers_and_fills() {
    let broker = make_broker();
    broker.set_market_price("AAPL", 185.50);

    let parent_id = broker.next_order_id();
    let tp_id = broker.next_order_id();
    let sl_id = broker.next_order_id();

    broker
        .place_order(
            parent_id, "AAPL", "BUY", "MKT", 100.0, None, None, None, false, "DAY", false,
        )
        .unwrap();
    broker
        .place_order(
            tp_id,
            "AAPL",
            "SELL",
            "LMT",
            100.0,
            Some(192.0),
            None,
            Some(parent_id),
            false,
            "DAY",
            false,
        )
        .unwrap();
    broker
        .place_order(
            sl_id,
            "AAPL",
            "SELL",
            "STP LMT",
            100.0,
            Some(181.50),
            Some(182.0),
            Some(parent_id),
            true,
            "DAY",
            false,
        )
        .unwrap();
    let _ = broker.poll_callbacks();

    broker.set_market_price("AAPL", 181.80);
    let cbs = broker.poll_callbacks();

    assert!(
        cbs.iter().any(|cb| matches!(
            cb,
            BrokerCallback::OrderStatus { ib_order_id, status, .. }
            if *ib_order_id == sl_id && status == "Filled"
        )),
        "STP LMT should fill when both conditions met"
    );

    let exec = cbs
        .iter()
        .find(|cb| {
            matches!(
                cb,
                BrokerCallback::Execution { ib_order_id, .. } if *ib_order_id == sl_id
            )
        })
        .unwrap();
    if let BrokerCallback::Execution { price, .. } = exec {
        assert_eq!(*price, 181.50, "STP LMT fills at limit price");
    }
}

#[test]
fn test_stop_limit_triggers_but_gaps_through() {
    let broker = make_broker();
    broker.set_market_price("AAPL", 185.50);

    let parent_id = broker.next_order_id();
    let tp_id = broker.next_order_id();
    let sl_id = broker.next_order_id();

    broker
        .place_order(
            parent_id, "AAPL", "BUY", "MKT", 100.0, None, None, None, false, "DAY", false,
        )
        .unwrap();
    broker
        .place_order(
            tp_id,
            "AAPL",
            "SELL",
            "LMT",
            100.0,
            Some(192.0),
            None,
            Some(parent_id),
            false,
            "DAY",
            false,
        )
        .unwrap();
    broker
        .place_order(
            sl_id,
            "AAPL",
            "SELL",
            "STP LMT",
            100.0,
            Some(181.50),
            Some(182.0),
            Some(parent_id),
            true,
            "DAY",
            false,
        )
        .unwrap();
    let _ = broker.poll_callbacks();

    // Price gaps through: stop triggers but limit NOT met (180 < 181.50).
    broker.set_market_price("AAPL", 180.0);
    let cbs = broker.poll_callbacks();

    assert!(
        cbs.iter().any(|cb| matches!(
            cb,
            BrokerCallback::OrderStatus { ib_order_id, status, .. }
            if *ib_order_id == sl_id && status == "Submitted"
        )),
        "STP LMT should trigger to Submitted"
    );

    assert!(
        !cbs.iter().any(|cb| matches!(
            cb,
            BrokerCallback::OrderStatus { ib_order_id, status, .. }
            if *ib_order_id == sl_id && status == "Filled"
        )),
        "STP LMT should NOT fill when price gaps through"
    );

    let inner = broker.inner.lock();
    assert_eq!(
        inner.orders.get(&sl_id).unwrap().status,
        SimOrderStatus::Working,
        "STP LMT should be Working after trigger"
    );
}

// ── Phase 2: Edge cases ─────────────────────────────────────────────

#[test]
fn test_filled_limit_not_retriggered() {
    let broker = make_broker();
    broker.set_market_price("AAPL", 185.50);

    let id = broker.next_order_id();
    broker
        .place_order(
            id,
            "AAPL",
            "BUY",
            "LMT",
            100.0,
            Some(183.0),
            None,
            None,
            true,
            "DAY",
            false,
        )
        .unwrap();
    let _ = broker.poll_callbacks();

    broker.set_market_price("AAPL", 182.0);
    let cbs = broker.poll_callbacks();
    assert_eq!(count_status(&cbs, "Filled"), 1);

    broker.set_market_price("AAPL", 181.0);
    let cbs = broker.poll_callbacks();
    assert!(
        cbs.is_empty(),
        "filled order should not generate more callbacks"
    );
}

#[test]
fn test_stop_buy_triggers_at_price() {
    let broker = make_broker();
    broker.set_market_price("AAPL", 185.50);

    let parent_id = broker.next_order_id();
    let tp_id = broker.next_order_id();
    let sl_id = broker.next_order_id();

    // Short bracket: MKT SELL parent, LMT BUY TP, STP BUY SL
    broker
        .place_order(
            parent_id, "AAPL", "SELL", "MKT", 100.0, None, None, None, false, "DAY", false,
        )
        .unwrap();
    broker
        .place_order(
            tp_id,
            "AAPL",
            "BUY",
            "LMT",
            100.0,
            Some(180.0),
            None,
            Some(parent_id),
            false,
            "DAY",
            false,
        )
        .unwrap();
    broker
        .place_order(
            sl_id,
            "AAPL",
            "BUY",
            "STP",
            100.0,
            None,
            Some(190.0),
            Some(parent_id),
            true,
            "DAY",
            false,
        )
        .unwrap();
    let _ = broker.poll_callbacks();

    broker.set_market_price("AAPL", 191.0);
    let cbs = broker.poll_callbacks();

    assert!(
        cbs.iter().any(|cb| matches!(
            cb,
            BrokerCallback::OrderStatus { ib_order_id, status, .. }
            if *ib_order_id == sl_id && status == "Filled"
        )),
        "BUY STP should fill when price rises above stop"
    );

    let exec = cbs
        .iter()
        .find(|cb| {
            matches!(
                cb,
                BrokerCallback::Execution { ib_order_id, .. } if *ib_order_id == sl_id
            )
        })
        .unwrap();
    if let BrokerCallback::Execution { price, .. } = exec {
        // BUY STP fills at ask = market + half_spread = 191.0 + 0.005
        let expected = 191.0 + 0.005;
        assert!(
            (*price - expected).abs() < f64::EPSILON,
            "BUY stop fills at ask ({expected}), got {price}"
        );
    }

    assert!(
        cbs.iter().any(|cb| matches!(
            cb,
            BrokerCallback::OrderStatus { ib_order_id, status, .. }
            if *ib_order_id == tp_id && status == "Cancelled"
        )),
        "TP should be OCA-cancelled when SL fills"
    );
}

// ── Phase 4: Tick generation ───────────────────────────────────────

#[test]
fn test_subscribe_produces_initial_tick() {
    let broker = make_broker();
    broker.subscribe_market_data("AAPL", 265598);

    let cbs = broker.poll_callbacks();
    assert!(
        !cbs.is_empty(),
        "subscribe should produce at least one callback"
    );

    let tick = cbs
        .iter()
        .find(|cb| matches!(cb, BrokerCallback::Tick { .. }));
    assert!(tick.is_some(), "subscribe should produce a Tick callback");

    if let Some(BrokerCallback::Tick {
        symbol,
        con_id,
        bid,
        ask,
        last,
        volume,
    }) = tick
    {
        assert_eq!(symbol, "AAPL");
        assert_eq!(*con_id, 265598);
        assert!(bid.is_some(), "bid should be set");
        assert!(ask.is_some(), "ask should be set");
        assert!(last.is_some(), "last should be set");
        assert!(volume.is_some(), "volume should be set");
        // Bid < last < ask (spread symmetry)
        let b = bid.unwrap();
        let a = ask.unwrap();
        let l = last.unwrap();
        assert!(b < l, "bid ({b}) should be less than last ({l})");
        assert!(l < a, "last ({l}) should be less than ask ({a})");
    }
}

#[test]
fn test_unsubscribe_stops_ticks() {
    let broker = make_broker();
    broker.subscribe_market_data("AAPL", 265598);
    let _ = broker.poll_callbacks(); // drain initial tick

    // generate_tick should work while subscribed
    let tick = broker.generate_tick("AAPL");
    assert!(
        tick.is_some(),
        "generate_tick should return Some while subscribed"
    );

    // Unsubscribe
    broker.unsubscribe_market_data("AAPL");

    // generate_tick should return None after unsubscribe
    let tick = broker.generate_tick("AAPL");
    assert!(
        tick.is_none(),
        "generate_tick should return None after unsubscribe"
    );
}

#[test]
fn test_generate_tick_for_subscribed() {
    let broker = make_broker();
    broker.set_market_price("MSFT", 400.0);
    broker.subscribe_market_data("MSFT", 272093);
    let _ = broker.poll_callbacks(); // drain initial tick

    let tick = broker.generate_tick("MSFT");
    assert!(
        tick.is_some(),
        "generate_tick should return Some for subscribed symbol"
    );

    if let Some(BrokerCallback::Tick {
        symbol,
        bid,
        ask,
        last,
        volume,
        ..
    }) = tick
    {
        assert_eq!(symbol, "MSFT");
        assert_eq!(last.unwrap(), 400.0, "last should match set market price");
        let spread = broker.config.default_spread;
        assert!(
            (bid.unwrap() - (400.0 - spread / 2.0)).abs() < f64::EPSILON,
            "bid should be last - spread/2"
        );
        assert!(
            (ask.unwrap() - (400.0 + spread / 2.0)).abs() < f64::EPSILON,
            "ask should be last + spread/2"
        );
        assert_eq!(volume.unwrap(), 100);
    }

    // Non-subscribed symbol returns None
    assert!(
        broker.generate_tick("GOOG").is_none(),
        "non-subscribed symbol should return None"
    );
}

#[test]
fn test_auto_tick_triggers_stop_loss_fill() {
    // Phase 4 acceptance test: subscribe to AAPL, create bracket with SL,
    // move price below SL, verify that set_market_price triggers the SL fill,
    // and that generate_auto_ticks produces tick callbacks for subscribed symbols.
    let broker = make_broker();
    broker.set_market_price("AAPL", 185.50);
    broker.subscribe_market_data("AAPL", 265598);
    let _ = broker.poll_callbacks(); // drain initial tick

    // Create bracket: BUY MKT parent, SELL LMT TP @ 192, SELL STP SL @ 182
    let parent_id = broker.next_order_id();
    let tp_id = broker.next_order_id();
    let sl_id = broker.next_order_id();

    broker
        .place_order(
            parent_id, "AAPL", "BUY", "MKT", 100.0, None, None, None, false, "DAY", false,
        )
        .unwrap();
    broker
        .place_order(
            tp_id,
            "AAPL",
            "SELL",
            "LMT",
            100.0,
            Some(192.0),
            None,
            Some(parent_id),
            false,
            "DAY",
            false,
        )
        .unwrap();
    broker
        .place_order(
            sl_id,
            "AAPL",
            "SELL",
            "STP",
            100.0,
            None,
            Some(182.0),
            Some(parent_id),
            true,
            "DAY",
            false,
        )
        .unwrap();
    let _ = broker.poll_callbacks(); // drain bracket activation

    // Move price below stop loss level — this triggers SL via set_market_price
    broker.set_market_price("AAPL", 181.0);

    // Also generate auto-ticks explicitly (simulates what the engine poll loop does)
    broker.generate_auto_ticks();

    let cbs = broker.poll_callbacks();

    // SL should have been triggered and filled
    let sl_filled = cbs.iter().any(|cb| {
        matches!(
            cb,
            BrokerCallback::OrderStatus { ib_order_id, status, .. }
            if *ib_order_id == sl_id && status == "Filled"
        )
    });
    assert!(
        sl_filled,
        "SL should be Filled after price drops below stop"
    );

    // TP should be OCA-cancelled
    let tp_cancelled = cbs.iter().any(|cb| {
        matches!(
            cb,
            BrokerCallback::OrderStatus { ib_order_id, status, .. }
            if *ib_order_id == tp_id && status == "Cancelled"
        )
    });
    assert!(tp_cancelled, "TP should be Cancelled via OCA when SL fills");

    // Verify that auto-ticks were generated for the subscribed symbol
    let has_tick = cbs.iter().any(|cb| {
        matches!(
            cb,
            BrokerCallback::Tick { symbol, .. } if symbol == "AAPL"
        )
    });
    assert!(
        has_tick,
        "auto-tick should be generated for subscribed AAPL"
    );
}
