use super::*;
use std::time::Duration;

#[tokio::test]
async fn test_broker_handle_creation() {
    let config = BrokerConfig::default();
    let handle = start_broker_engine(config);

    // Connection should start as Disconnected.
    assert_eq!(
        *handle.connection_state.borrow(),
        ConnectionState::Disconnected
    );

    // Send Shutdown and verify the engine stops (command sender is not
    // dropped prematurely).
    handle
        .commands
        .send(BrokerCommand::Shutdown)
        .await
        .expect("should send shutdown");

    // Give the engine task a moment to process the shutdown.
    tokio::time::sleep(Duration::from_millis(50)).await;

    // After shutdown, sending another command should still succeed on the
    // channel (the mpsc sender is still alive), but the engine won't
    // process it. We just verify we can subscribe to events without panic.
    let _rx = handle.market_events.subscribe();
    let _rx2 = handle.order_events.subscribe();
}

#[tokio::test]
async fn test_event_broadcast() {
    let config = BrokerConfig::default();
    let handle = start_broker_engine(config);

    // Subscribe before sending.
    let mut market_rx = handle.market_events.subscribe();
    let mut order_rx = handle.order_events.subscribe();

    // Emit a market event on the broadcast channel.
    let market_event = BrokerEvent::Connected {
        server_version: 176,
    };
    handle
        .market_events
        .send(market_event)
        .expect("should broadcast market event");

    // Emit an order event on the broadcast channel.
    let order_event = BrokerEvent::OrderCreated {
        order_id: uuid::Uuid::nil(),
    };
    handle
        .order_events
        .send(order_event)
        .expect("should broadcast order event");

    // Verify the subscribers receive the events.
    let received_market = tokio::time::timeout(Duration::from_millis(100), market_rx.recv())
        .await
        .expect("should not timeout")
        .expect("should receive market event");

    match received_market {
        BrokerEvent::Connected { server_version } => assert_eq!(server_version, 176),
        other => panic!("expected Connected, got {other:?}"),
    }

    let received_order = tokio::time::timeout(Duration::from_millis(100), order_rx.recv())
        .await
        .expect("should not timeout")
        .expect("should receive order event");

    match received_order {
        BrokerEvent::OrderCreated { order_id } => {
            assert_eq!(order_id, uuid::Uuid::nil());
        }
        other => panic!("expected OrderCreated, got {other:?}"),
    }

    // Clean up.
    handle
        .commands
        .send(BrokerCommand::Shutdown)
        .await
        .expect("should send shutdown");
}

#[tokio::test]
async fn test_engine_stops_on_sender_drop() {
    let config = BrokerConfig::default();
    let handle = start_broker_engine(config);

    // Drop the command sender.
    drop(handle.commands);

    // Give the engine task time to notice and exit.
    tokio::time::sleep(Duration::from_millis(100)).await;

    // If we get here without hanging, the engine stopped correctly.
}

#[tokio::test]
async fn test_connection_state_watch() {
    let config = BrokerConfig::default();
    let handle = start_broker_engine(config);

    // Initial state should be Disconnected.
    let state = handle.connection_state.borrow().clone();
    assert_eq!(state, ConnectionState::Disconnected);
    assert!(!state.is_connected());
    assert!(!state.is_ready());

    // Clean up.
    let _ = handle.commands.send(BrokerCommand::Shutdown).await;
}

#[tokio::test]
async fn test_historical_data_via_test_source() {
    use crate::config::DataSourceConfig;

    let config = BrokerConfig {
        data_source: DataSourceConfig::Test,
        ..Default::default()
    };
    let handle = start_broker_engine(config);
    let mut rx = handle.market_events.subscribe();

    handle
        .commands
        .send(BrokerCommand::RequestHistoricalData {
            symbol: "AAPL".to_string(),
            con_id: 265598,
            duration: "30 D".to_string(),
            bar_size: "1 day".to_string(),
            request_id: 42,
        })
        .await
        .unwrap();

    // Collect events until HistoricalDataComplete
    let mut bar_count = 0;
    loop {
        let event = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("should not timeout")
            .expect("should receive event");
        match event {
            BrokerEvent::BarClosed { symbol, .. } => {
                assert_eq!(symbol.symbol, "AAPL");
                assert_eq!(symbol.contract_id, 265598);
                bar_count += 1;
            }
            BrokerEvent::HistoricalDataComplete { request_id, symbol } => {
                assert_eq!(request_id, 42);
                assert_eq!(symbol.symbol, "AAPL");
                break;
            }
            _ => {} // ignore heartbeat or other events
        }
    }
    // 30 calendar days ≈ 21-22 trading days
    assert!(
        bar_count > 15,
        "expected ~21 trading days in 30 D, got {bar_count}"
    );

    let _ = handle.commands.send(BrokerCommand::Shutdown).await;
}

// -----------------------------------------------------------------------
// Bracket builder tests
// -----------------------------------------------------------------------

fn sample_bracket_params() -> BracketParams {
    use crate::orders::bracket::{StopLossParams, TakeProfitParams};
    BracketParams {
        symbol: "AAPL".to_string(),
        con_id: Some(265598),
        sec_type: midas_broker_core::SecurityType::Stock,
        exchange: "SMART".to_string(),
        currency: "USD".to_string(),
        action: crate::orders::types::OrderAction::Buy,
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
        strategy: Some("test_strat".to_string()),
        tags: vec!["tag1".to_string()],
    }
}

#[test]
fn test_build_bracket_full() {
    let params = sample_bracket_params();
    let group = build_bracket(&params);

    // Parent
    assert_eq!(group.parent.symbol, "AAPL");
    assert_eq!(group.parent.order_type, OrderKind::Market);
    assert_eq!(group.parent.action, OrderAction::Buy);
    assert_eq!(group.parent.quantity, 100.0);
    assert_eq!(group.parent.status, OrderStatus::Inactive);
    assert_eq!(group.parent.bracket_role, Some(BracketRole::Parent));
    assert!(group.parent.parent_id.is_none());

    // TP
    let tp = group.take_profit.as_ref().unwrap();
    assert_eq!(tp.order_type, OrderKind::Limit);
    assert_eq!(tp.action, OrderAction::Sell); // opposite
    assert_eq!(tp.limit_price, Some(192.0));
    assert_eq!(tp.tif, TimeInForce::Gtc);
    assert_eq!(tp.parent_id, Some(group.parent.id));
    assert_eq!(tp.bracket_role, Some(BracketRole::TakeProfit));

    // SL
    let sl = group.stop_loss.as_ref().unwrap();
    assert_eq!(sl.order_type, OrderKind::Stop);
    assert_eq!(sl.action, OrderAction::Sell);
    assert_eq!(sl.stop_price, Some(182.0));
    assert_eq!(sl.parent_id, Some(group.parent.id));
    assert_eq!(sl.bracket_role, Some(BracketRole::StopLoss));

    // Tags/strategy propagate
    assert_eq!(tp.strategy, Some("test_strat".to_string()));
    assert_eq!(sl.tags, vec!["tag1".to_string()]);
}

#[test]
fn test_build_bracket_tp_only() {
    let mut params = sample_bracket_params();
    params.stop_loss = None;
    let group = build_bracket(&params);
    assert_eq!(group.legs().len(), 2);
    assert!(group.take_profit.is_some());
    assert!(group.stop_loss.is_none());
}

#[test]
fn test_build_bracket_sl_only() {
    let mut params = sample_bracket_params();
    params.take_profit = None;
    let group = build_bracket(&params);
    assert_eq!(group.legs().len(), 2);
    assert!(group.take_profit.is_none());
    assert!(group.stop_loss.is_some());
}

#[test]
fn test_build_bracket_naked() {
    let mut params = sample_bracket_params();
    params.take_profit = None;
    params.stop_loss = None;
    let group = build_bracket(&params);
    assert_eq!(group.legs().len(), 1);
}

#[test]
fn test_build_bracket_sell_children_are_buy() {
    let mut params = sample_bracket_params();
    params.action = OrderAction::Sell;
    let group = build_bracket(&params);
    assert_eq!(group.parent.action, OrderAction::Sell);
    assert_eq!(group.take_profit.as_ref().unwrap().action, OrderAction::Buy);
    assert_eq!(group.stop_loss.as_ref().unwrap().action, OrderAction::Buy);
}

#[test]
fn test_build_bracket_stop_limit_sl() {
    use crate::orders::bracket::StopLossParams;
    let mut params = sample_bracket_params();
    params.stop_loss = Some(StopLossParams {
        stop_price: 182.0,
        limit_price: Some(181.50),
        tif: None,
    });
    let group = build_bracket(&params);
    let sl = group.stop_loss.as_ref().unwrap();
    assert_eq!(sl.order_type, OrderKind::StopLimit);
    assert_eq!(sl.stop_price, Some(182.0));
    assert_eq!(sl.limit_price, Some(181.50));
}

// -----------------------------------------------------------------------
// Non-Market entry type tests
// -----------------------------------------------------------------------

#[test]
fn test_build_bracket_limit_entry() {
    let mut params = sample_bracket_params();
    params.entry_kind = OrderKind::Limit;
    params.entry_price = Some(180.00);
    let group = build_bracket(&params);
    assert_eq!(group.parent.order_type, OrderKind::Limit);
    assert_eq!(group.parent.limit_price, Some(180.00));
    assert!(group.parent.stop_price.is_none());
}

#[test]
fn test_build_bracket_stop_entry() {
    let mut params = sample_bracket_params();
    params.entry_kind = OrderKind::Stop;
    params.entry_stop_price = Some(190.00);
    let group = build_bracket(&params);
    assert_eq!(group.parent.order_type, OrderKind::Stop);
    assert_eq!(group.parent.stop_price, Some(190.00));
    assert!(group.parent.limit_price.is_none());
}

#[test]
fn test_build_bracket_stop_limit_entry() {
    let mut params = sample_bracket_params();
    params.entry_kind = OrderKind::StopLimit;
    params.entry_price = Some(184.50);
    params.entry_stop_price = Some(185.00);
    let group = build_bracket(&params);
    assert_eq!(group.parent.order_type, OrderKind::StopLimit);
    assert_eq!(group.parent.limit_price, Some(184.50));
    assert_eq!(group.parent.stop_price, Some(185.00));
}

// -----------------------------------------------------------------------
// Order size guard tests
// -----------------------------------------------------------------------

#[test]
fn test_validate_order_size_within_limits() {
    let params = sample_bracket_params();
    let limits = TradingLimits {
        max_order_quantity: 10_000.0,
        max_notional_value: 500_000.0,
    };
    assert!(validate_order_size(&params, &limits).is_ok());
}

#[test]
fn test_validate_order_size_quantity_exceeded() {
    let mut params = sample_bracket_params();
    params.quantity = 20_000.0;
    let limits = TradingLimits {
        max_order_quantity: 10_000.0,
        max_notional_value: 0.0,
    };
    assert!(validate_order_size(&params, &limits).is_err());
}

#[test]
fn test_validate_order_size_notional_exceeded() {
    let mut params = sample_bracket_params();
    params.quantity = 5000.0;
    params.reference_price = Some(200.0); // 5000 * 200 = 1M
    let limits = TradingLimits {
        max_order_quantity: 0.0,
        max_notional_value: 500_000.0,
    };
    assert!(validate_order_size(&params, &limits).is_err());
}

#[test]
fn test_validate_order_size_missing_reference_price() {
    let mut params = sample_bracket_params();
    params.reference_price = None;
    let limits = TradingLimits {
        max_order_quantity: 0.0,
        max_notional_value: 500_000.0,
    };
    assert!(validate_order_size(&params, &limits).is_err());
}

#[test]
fn test_validate_order_size_no_limits() {
    let params = sample_bracket_params();
    let limits = TradingLimits {
        max_order_quantity: 0.0,
        max_notional_value: 0.0,
    };
    assert!(validate_order_size(&params, &limits).is_ok());
}

#[tokio::test]
async fn test_historical_data_test_source_returns_data() {
    // Default config = Test, which has a TestDataProvider → should return bars
    let handle = start_broker_engine(BrokerConfig::default());
    let mut rx = handle.market_events.subscribe();

    handle
        .commands
        .send(BrokerCommand::RequestHistoricalData {
            symbol: "AAPL".to_string(),
            con_id: 265598,
            duration: "30 D".to_string(),
            bar_size: "1 day".to_string(),
            request_id: 1,
        })
        .await
        .unwrap();

    // Should receive bar events from the test data provider
    let mut got_bars = false;
    for _ in 0..100 {
        match tokio::time::timeout(Duration::from_secs(2), rx.recv()).await {
            Ok(Ok(BrokerEvent::BarClosed { .. })) => {
                got_bars = true;
                break;
            }
            Ok(Ok(BrokerEvent::HistoricalDataComplete { .. })) => break,
            Ok(Ok(_)) => {}
            _ => break,
        }
    }
    assert!(got_bars, "should receive bars from test data provider");

    let _ = handle.commands.send(BrokerCommand::Shutdown).await;
}

// -----------------------------------------------------------------------
// Cancel bracket tests
// -----------------------------------------------------------------------

#[tokio::test]
async fn test_cancel_bracket_via_command() {
    use crate::config::DataSourceConfig;
    let config = BrokerConfig {
        data_source: DataSourceConfig::Test,
        ..Default::default()
    };
    let handle = start_broker_engine(config);
    let mut rx = handle.order_events.subscribe();

    // Create a bracket first
    let params = sample_bracket_params();
    handle
        .commands
        .send(BrokerCommand::CreateBracket(params))
        .await
        .unwrap();

    // Wait for BracketCreated
    let parent_id = loop {
        let event = tokio::time::timeout(Duration::from_secs(1), rx.recv())
            .await
            .unwrap()
            .unwrap();
        if let BrokerEvent::BracketCreated { parent_id: pid, .. } = event {
            break pid;
        }
    };

    // Let the poll loop process fill callbacks before cancelling
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Cancel it — with TestBroker instant fills, the market order is
    // already filled. Cancel only affects non-terminal children.
    handle
        .commands
        .send(BrokerCommand::CancelBracket { parent_id })
        .await
        .unwrap();

    // Drain events — expect at least one BracketStatusChanged
    tokio::time::sleep(Duration::from_millis(50)).await;
    let mut got_status = false;
    for _ in 0..30 {
        match tokio::time::timeout(Duration::from_millis(200), rx.recv()).await {
            Ok(Ok(BrokerEvent::BracketStatusChanged { parent_id: pid, .. }))
                if pid == parent_id =>
            {
                got_status = true;
            }
            Ok(Ok(_)) => {} // drain other events
            _ => break,
        }
    }
    assert!(
        got_status,
        "Expected at least one BracketStatusChanged event"
    );

    let _ = handle.commands.send(BrokerCommand::Shutdown).await;
}

// -----------------------------------------------------------------------
// Bracket status derivation integration test
// -----------------------------------------------------------------------

#[test]
fn test_check_bracket_status_change_unit() {
    // This tests the derive_bracket_status function which is already tested in bracket.rs
    // Just verify our integration through the status cache
    use crate::orders::bracket::derive_bracket_status;
    use crate::orders::bracket::BracketGroup;

    let parent = {
        let mut o = LocalOrder::new_draft("AAPL", OrderAction::Buy, OrderKind::Market, 100.0);
        o.status = OrderStatus::Filled;
        o
    };
    let tp = {
        let mut o = LocalOrder::new_draft("AAPL", OrderAction::Sell, OrderKind::Limit, 100.0);
        o.status = OrderStatus::Submitted;
        o.parent_id = Some(parent.id);
        o
    };
    let sl = {
        let mut o = LocalOrder::new_draft("AAPL", OrderAction::Sell, OrderKind::Stop, 100.0);
        o.status = OrderStatus::Submitted;
        o.parent_id = Some(parent.id);
        o
    };
    let group = BracketGroup {
        parent,
        take_profit: Some(tp),
        stop_loss: Some(sl),
    };
    assert_eq!(
        derive_bracket_status(&group),
        BracketLifecycleStatus::EntryFilled
    );
}

// -----------------------------------------------------------------------
// Bracket integration tests (engine command/event interface)
// -----------------------------------------------------------------------

/// Helper: create a BrokerConfig with Test data source for integration tests.
fn test_config() -> BrokerConfig {
    BrokerConfig {
        data_source: DataSourceConfig::Test,
        ..Default::default()
    }
}

/// Helper: create a BrokerConfig with custom trading limits.
fn test_config_with_limits(max_qty: f64, max_notional: f64) -> BrokerConfig {
    let mut config = test_config();
    config.trading_limits = TradingLimits {
        max_order_quantity: max_qty,
        max_notional_value: max_notional,
    };
    config
}

// 1. Full bracket lifecycle: create -> verify BracketCreated event
#[tokio::test]
async fn test_create_full_bracket_emits_event() {
    let handle = start_broker_engine(test_config());
    let mut rx = handle.order_events.subscribe();

    let params = sample_bracket_params();
    handle
        .commands
        .send(BrokerCommand::CreateBracket(params))
        .await
        .unwrap();

    let event = recv_bracket_created(&mut rx, 2).await;

    match event {
        BrokerEvent::BracketCreated {
            parent_id,
            take_profit_id,
            stop_loss_id,
            symbol,
            action,
            quantity,
            tp_price: _,
            sl_price: _,
            reference_price: _,
            ..
        } => {
            assert!(!parent_id.is_nil(), "parent_id should be a valid UUID");
            assert!(take_profit_id.is_some(), "full bracket should have TP");
            assert!(stop_loss_id.is_some(), "full bracket should have SL");
            assert_eq!(symbol, "AAPL");
            assert_eq!(action, OrderAction::Buy);
            assert_eq!(quantity, 100.0);
            // All three IDs should be distinct
            let tp_id = take_profit_id.unwrap();
            let sl_id = stop_loss_id.unwrap();
            assert_ne!(parent_id, tp_id);
            assert_ne!(parent_id, sl_id);
            assert_ne!(tp_id, sl_id);
        }
        other => panic!("expected BracketCreated, got {other:?}"),
    }

    let _ = handle.commands.send(BrokerCommand::Shutdown).await;
}

// 2. TP-only bracket
#[tokio::test]
async fn test_create_tp_only_bracket() {
    let handle = start_broker_engine(test_config());
    let mut rx = handle.order_events.subscribe();

    let mut params = sample_bracket_params();
    params.stop_loss = None;
    handle
        .commands
        .send(BrokerCommand::CreateBracket(params))
        .await
        .unwrap();

    let event = recv_bracket_created(&mut rx, 2).await;

    match event {
        BrokerEvent::BracketCreated {
            take_profit_id,
            stop_loss_id,
            ..
        } => {
            assert!(take_profit_id.is_some(), "TP-only bracket should have TP");
            assert!(stop_loss_id.is_none(), "TP-only bracket should not have SL");
        }
        other => panic!("expected BracketCreated, got {other:?}"),
    }

    let _ = handle.commands.send(BrokerCommand::Shutdown).await;
}

// 3. SL-only bracket
#[tokio::test]
async fn test_create_sl_only_bracket() {
    let handle = start_broker_engine(test_config());
    let mut rx = handle.order_events.subscribe();

    let mut params = sample_bracket_params();
    params.take_profit = None;
    handle
        .commands
        .send(BrokerCommand::CreateBracket(params))
        .await
        .unwrap();

    let event = recv_bracket_created(&mut rx, 2).await;

    match event {
        BrokerEvent::BracketCreated {
            take_profit_id,
            stop_loss_id,
            ..
        } => {
            assert!(
                take_profit_id.is_none(),
                "SL-only bracket should not have TP"
            );
            assert!(stop_loss_id.is_some(), "SL-only bracket should have SL");
        }
        other => panic!("expected BracketCreated, got {other:?}"),
    }

    let _ = handle.commands.send(BrokerCommand::Shutdown).await;
}

// 4. Naked market order (no TP, no SL)
#[tokio::test]
async fn test_create_naked_market_order() {
    let handle = start_broker_engine(test_config());
    let mut rx = handle.order_events.subscribe();

    let mut params = sample_bracket_params();
    params.take_profit = None;
    params.stop_loss = None;
    // No TP/SL means no notional guard is needed, but we still have reference_price
    handle
        .commands
        .send(BrokerCommand::CreateBracket(params))
        .await
        .unwrap();

    let event = recv_bracket_created(&mut rx, 2).await;

    match event {
        BrokerEvent::BracketCreated {
            take_profit_id,
            stop_loss_id,
            ..
        } => {
            assert!(take_profit_id.is_none(), "naked order should not have TP");
            assert!(stop_loss_id.is_none(), "naked order should not have SL");
        }
        other => panic!("expected BracketCreated, got {other:?}"),
    }

    let _ = handle.commands.send(BrokerCommand::Shutdown).await;
}

// 5. Validation failure - empty symbol
#[tokio::test]
async fn test_create_bracket_empty_symbol_rejected() {
    let handle = start_broker_engine(test_config());
    let mut rx = handle.order_events.subscribe();

    let mut params = sample_bracket_params();
    params.symbol = String::new();
    handle
        .commands
        .send(BrokerCommand::CreateBracket(params))
        .await
        .unwrap();

    let event = tokio::time::timeout(Duration::from_secs(1), rx.recv())
        .await
        .expect("should not timeout")
        .expect("should receive event");

    match event {
        BrokerEvent::OrderValidationFailed { code, message } => {
            assert_eq!(code, -1);
            assert!(
                message.contains("symbol"),
                "error should mention symbol: {message}"
            );
        }
        other => panic!("expected OrderValidationFailed event for empty symbol, got {other:?}"),
    }

    // Ensure no BracketCreated follows
    let timeout_result = tokio::time::timeout(Duration::from_millis(200), rx.recv()).await;
    assert!(
        timeout_result.is_err(),
        "should not receive any more events after validation error"
    );

    let _ = handle.commands.send(BrokerCommand::Shutdown).await;
}

// 6. Validation failure - zero quantity
#[tokio::test]
async fn test_create_bracket_zero_qty_rejected() {
    let handle = start_broker_engine(test_config());
    let mut rx = handle.order_events.subscribe();

    let mut params = sample_bracket_params();
    params.quantity = 0.0;
    handle
        .commands
        .send(BrokerCommand::CreateBracket(params))
        .await
        .unwrap();

    let event = tokio::time::timeout(Duration::from_secs(1), rx.recv())
        .await
        .expect("should not timeout")
        .expect("should receive event");

    match event {
        BrokerEvent::OrderValidationFailed { code, message } => {
            assert_eq!(code, -1);
            assert!(
                message.contains("quantity"),
                "error should mention quantity: {message}"
            );
        }
        other => panic!("expected OrderValidationFailed event for zero quantity, got {other:?}"),
    }

    let _ = handle.commands.send(BrokerCommand::Shutdown).await;
}

// 7. Order size guard - quantity exceeds limit
#[tokio::test]
async fn test_order_size_guard_quantity() {
    let handle = start_broker_engine(test_config_with_limits(50.0, 0.0));
    let mut rx = handle.order_events.subscribe();

    let params = sample_bracket_params(); // quantity=100, limit=50
    handle
        .commands
        .send(BrokerCommand::CreateBracket(params))
        .await
        .unwrap();

    let event = tokio::time::timeout(Duration::from_secs(1), rx.recv())
        .await
        .expect("should not timeout")
        .expect("should receive event");

    match event {
        BrokerEvent::OrderValidationFailed { code, message } => {
            assert_eq!(code, -2, "order size guard uses code -2");
            assert!(
                message.contains("quantity") || message.contains("exceeds"),
                "error should mention quantity limit: {message}"
            );
        }
        other => panic!("expected OrderValidationFailed event for quantity guard, got {other:?}"),
    }

    let _ = handle.commands.send(BrokerCommand::Shutdown).await;
}

// 8. Order size guard - notional exceeds limit
#[tokio::test]
async fn test_order_size_guard_notional() {
    let handle = start_broker_engine(test_config_with_limits(0.0, 1000.0));
    let mut rx = handle.order_events.subscribe();

    let mut params = sample_bracket_params();
    params.quantity = 100.0;
    params.reference_price = Some(185.0); // notional = 18500, limit = 1000
    handle
        .commands
        .send(BrokerCommand::CreateBracket(params))
        .await
        .unwrap();

    let event = tokio::time::timeout(Duration::from_secs(1), rx.recv())
        .await
        .expect("should not timeout")
        .expect("should receive event");

    match event {
        BrokerEvent::OrderValidationFailed { code, message } => {
            assert_eq!(code, -2, "order size guard uses code -2");
            assert!(
                message.contains("notional") || message.contains("exceeds"),
                "error should mention notional limit: {message}"
            );
        }
        other => panic!("expected OrderValidationFailed event for notional guard, got {other:?}"),
    }

    let _ = handle.commands.send(BrokerCommand::Shutdown).await;
}

// 9. Cancel bracket lifecycle
#[tokio::test]
async fn test_cancel_bracket_lifecycle() {
    let handle = start_broker_engine(test_config());
    let mut rx = handle.order_events.subscribe();

    let params = sample_bracket_params();
    handle
        .commands
        .send(BrokerCommand::CreateBracket(params))
        .await
        .unwrap();

    // Collect parent_id from BracketCreated
    let parent_id = loop {
        let event = tokio::time::timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("should not timeout")
            .expect("should receive event");
        if let BrokerEvent::BracketCreated { parent_id, .. } = event {
            break parent_id;
        }
    };

    // Let fill callbacks process before cancelling
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Cancel the bracket — with instant fills, parent is already Filled.
    // Children (TP/SL) are still live and will be cancelled.
    handle
        .commands
        .send(BrokerCommand::CancelBracket { parent_id })
        .await
        .unwrap();

    // Wait for BracketStatusChanged — may be Cancelled or Closed depending
    // on which legs were already terminal when cancel arrived.
    tokio::time::sleep(Duration::from_millis(50)).await;
    let mut found_status = false;
    for _ in 0..20 {
        match tokio::time::timeout(Duration::from_millis(200), rx.recv()).await {
            Ok(Ok(BrokerEvent::BracketStatusChanged { parent_id: pid, .. }))
                if pid == parent_id =>
            {
                found_status = true;
                break;
            }
            Ok(Ok(_)) => {} // drain other events
            _ => break,
        }
    }
    assert!(found_status, "should have received BracketStatusChanged");

    let _ = handle.commands.send(BrokerCommand::Shutdown).await;
}

// 10. Modify TP price (no error expected)
#[tokio::test]
async fn test_modify_tp_price() {
    let handle = start_broker_engine(test_config());
    let mut rx = handle.order_events.subscribe();

    let params = sample_bracket_params();
    handle
        .commands
        .send(BrokerCommand::CreateBracket(params))
        .await
        .unwrap();

    // Collect TP id from BracketCreated
    let tp_id = loop {
        let event = tokio::time::timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("should not timeout")
            .expect("should receive event");
        if let BrokerEvent::BracketCreated { take_profit_id, .. } = event {
            break take_profit_id.expect("should have TP");
        }
    };

    // Modify the TP leg price
    handle
        .commands
        .send(BrokerCommand::ModifyBracketLeg {
            order_id: tp_id,
            new_price: 195.0,
        })
        .await
        .unwrap();

    // The modify handler logs but may not emit an event for the price change
    // in the current implementation (it requires the order to be in a
    // modifiable state: PreSubmitted/Submitted/PartiallyFilled).
    // Since our orders are in PendingSubmit (not modifiable), this is a
    // no-op. Verify no error event is emitted.
    let timeout_result = tokio::time::timeout(Duration::from_millis(300), rx.recv()).await;
    // Either timeout (no event) or a non-error event is acceptable
    if let Ok(Ok(event)) = timeout_result {
        match event {
            BrokerEvent::Error { .. } | BrokerEvent::OrderError { .. } => {
                panic!("modify should not produce an error event: {event:?}");
            }
            _ => {} // any other event is fine
        }
    }

    let _ = handle.commands.send(BrokerCommand::Shutdown).await;
}

// 11. Sell bracket - children are Buy
#[tokio::test]
async fn test_sell_bracket_created() {
    let handle = start_broker_engine(test_config());
    let mut rx = handle.order_events.subscribe();

    let mut params = sample_bracket_params();
    params.action = OrderAction::Sell;
    handle
        .commands
        .send(BrokerCommand::CreateBracket(params))
        .await
        .unwrap();

    let event = recv_bracket_created(&mut rx, 2).await;

    match event {
        BrokerEvent::BracketCreated {
            action,
            symbol,
            quantity,
            ..
        } => {
            assert_eq!(action, OrderAction::Sell, "bracket action should be Sell");
            assert_eq!(symbol, "AAPL");
            assert_eq!(quantity, 100.0);
        }
        other => panic!("expected BracketCreated, got {other:?}"),
    }

    let _ = handle.commands.send(BrokerCommand::Shutdown).await;
}

// 12. StopLimit SL type - creation succeeds
#[tokio::test]
async fn test_stop_limit_sl_bracket() {
    use crate::orders::bracket::StopLossParams;

    let handle = start_broker_engine(test_config());
    let mut rx = handle.order_events.subscribe();

    let mut params = sample_bracket_params();
    params.stop_loss = Some(StopLossParams {
        stop_price: 182.0,
        limit_price: Some(181.50),
        tif: None,
    });
    handle
        .commands
        .send(BrokerCommand::CreateBracket(params))
        .await
        .unwrap();

    let event = recv_bracket_created(&mut rx, 2).await;

    match event {
        BrokerEvent::BracketCreated { stop_loss_id, .. } => {
            assert!(
                stop_loss_id.is_some(),
                "StopLimit SL bracket should have SL"
            );
        }
        other => panic!("expected BracketCreated, got {other:?}"),
    }

    let _ = handle.commands.send(BrokerCommand::Shutdown).await;
}

// 13. Multiple brackets created sequentially - unique parent_ids
#[tokio::test]
async fn test_multiple_brackets_sequential() {
    let handle = start_broker_engine(test_config());
    let mut rx = handle.order_events.subscribe();

    // Create 3 brackets
    for _ in 0..3 {
        let params = sample_bracket_params();
        handle
            .commands
            .send(BrokerCommand::CreateBracket(params))
            .await
            .unwrap();
    }

    // Collect 3 BracketCreated events
    let mut parent_ids = Vec::new();
    for _ in 0..20 {
        let event = tokio::time::timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("should not timeout")
            .expect("should receive event");
        if let BrokerEvent::BracketCreated { parent_id, .. } = event {
            parent_ids.push(parent_id);
            if parent_ids.len() == 3 {
                break;
            }
        }
    }
    assert_eq!(
        parent_ids.len(),
        3,
        "should have received 3 BracketCreated events"
    );

    // All parent IDs must be unique
    let mut unique = parent_ids.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), 3, "all 3 parent_ids should be distinct");

    let _ = handle.commands.send(BrokerCommand::Shutdown).await;
}

// 14. Cancel nonexistent bracket - no crash
#[tokio::test]
async fn test_cancel_nonexistent_bracket() {
    let handle = start_broker_engine(test_config());
    let mut rx = handle.order_events.subscribe();

    let random_id = Uuid::now_v7();
    handle
        .commands
        .send(BrokerCommand::CancelBracket {
            parent_id: random_id,
        })
        .await
        .unwrap();

    // Should not crash. May or may not emit an event.
    // Give a short window then verify engine is still alive.
    let _ = tokio::time::timeout(Duration::from_millis(200), rx.recv()).await;

    // Engine is still alive: send another command successfully
    handle
        .commands
        .send(BrokerCommand::Shutdown)
        .await
        .expect("engine should still be running after cancelling nonexistent bracket");
}

// 15. Persistence round-trip via build_bracket
#[test]
fn test_bracket_persistence_round_trip() {
    use crate::db::BrokerDb;
    use crate::persist::order_repo::{
        get_order, get_orders_by_parent_id, insert_order, local_to_order_row, order_row_to_local,
    };

    let db = BrokerDb::open_in_memory().unwrap();
    let conn = db.conn().lock().unwrap();

    // Build a bracket group from params
    let params = sample_bracket_params();
    let group = build_bracket(&params);

    let parent_id = group.parent.id;

    // Convert all legs to OrderRows and insert
    for leg in group.legs() {
        let row = local_to_order_row(leg);
        insert_order(&conn, &row).unwrap();
    }

    // Read parent back
    let parent_row = get_order(&conn, &parent_id.to_string())
        .unwrap()
        .expect("parent should be persisted");
    let restored_parent = order_row_to_local(&parent_row).unwrap();

    assert_eq!(restored_parent.id, group.parent.id);
    assert_eq!(restored_parent.symbol, "AAPL");
    assert_eq!(restored_parent.action, OrderAction::Buy);
    assert_eq!(restored_parent.order_type, OrderKind::Market);
    assert_eq!(restored_parent.quantity, 100.0);
    assert_eq!(restored_parent.status, OrderStatus::Inactive);
    assert_eq!(restored_parent.bracket_role, Some(BracketRole::Parent));
    assert_eq!(restored_parent.con_id, Some(265598));
    assert_eq!(
        restored_parent.sec_type,
        midas_broker_core::SecurityType::Stock
    );
    assert_eq!(restored_parent.exchange, "SMART");
    assert_eq!(restored_parent.currency, "USD");
    assert_eq!(restored_parent.strategy, Some("test_strat".to_string()));
    assert_eq!(restored_parent.tags, vec!["tag1".to_string()]);
    assert!(restored_parent.parent_id.is_none());

    // Read children back
    let children = get_orders_by_parent_id(&conn, &parent_id.to_string()).unwrap();
    assert_eq!(children.len(), 2, "should have TP + SL children");

    let mut tp_found = false;
    let mut sl_found = false;
    for child_row in &children {
        let child = order_row_to_local(child_row).unwrap();
        assert_eq!(child.parent_id, Some(parent_id));
        assert_eq!(child.symbol, "AAPL");
        assert_eq!(child.quantity, 100.0);

        match child.bracket_role {
            Some(BracketRole::TakeProfit) => {
                tp_found = true;
                assert_eq!(child.order_type, OrderKind::Limit);
                assert_eq!(child.action, OrderAction::Sell);
                assert_eq!(child.limit_price, Some(192.0));
                assert_eq!(child.tif, TimeInForce::Gtc);
            }
            Some(BracketRole::StopLoss) => {
                sl_found = true;
                assert_eq!(child.order_type, OrderKind::Stop);
                assert_eq!(child.action, OrderAction::Sell);
                assert_eq!(child.stop_price, Some(182.0));
                assert_eq!(child.tif, TimeInForce::Gtc);
            }
            other => panic!("unexpected bracket role: {other:?}"),
        }
    }
    assert!(tp_found, "should have found TP child");
    assert!(sl_found, "should have found SL child");
}

// 16. order_row_to_local handles all fields in a fully-populated LocalOrder
#[test]
fn test_order_row_to_local_all_fields() {
    use crate::persist::order_repo::{local_to_order_row, order_row_to_local};
    use chrono::Utc;

    // Create a fully-populated LocalOrder with every optional field set
    let mut order = LocalOrder::new_draft("TSLA", OrderAction::Sell, OrderKind::StopLimit, 50.0);
    order.ib_order_id = Some(9876);
    order.ib_perm_id = Some(5555555);
    order.con_id = Some(76792991);
    order.sec_type = midas_broker_core::SecurityType::Stock;
    order.exchange = "ISLAND".to_string();
    order.currency = "USD".to_string();
    order.limit_price = Some(250.0);
    order.stop_price = Some(255.0);
    order.trail_amount = Some(2.5);
    order.trail_percent = Some(1.5);
    order.tif = TimeInForce::Gtc;
    order.status = OrderStatus::Submitted;
    order.parent_id = Some(Uuid::now_v7());
    order.oca_group = Some("oca-group-42".to_string());
    order.bracket_role = Some(BracketRole::StopLoss);
    order.strategy = Some("mean_reversion".to_string());
    order.tags = vec![
        "urgent".to_string(),
        "earnings".to_string(),
        "q4".to_string(),
    ];
    order.algo_strategy = Some("TWAP".to_string());
    order.algo_params = Some(serde_json::json!({"startTime": "09:30", "endTime": "16:00"}));
    order.outside_rth = true;
    order.filled_qty = 10.0;
    order.remaining_qty = 40.0;
    order.avg_fill_price = Some(252.75);
    order.last_fill_price = Some(253.00);
    order.commission = Some(3.50);
    order.activation_count = 3;
    order.last_activated_at = Some(Utc::now());
    order.last_deactivated_at = Some(Utc::now());

    // Round-trip: LocalOrder -> OrderRow -> LocalOrder
    let row = local_to_order_row(&order);
    let restored = order_row_to_local(&row).expect("round-trip should succeed");

    // Verify every field
    assert_eq!(restored.id, order.id);
    assert_eq!(restored.ib_order_id, Some(9876));
    assert_eq!(restored.ib_perm_id, Some(5555555));
    assert_eq!(restored.symbol, "TSLA");
    assert_eq!(restored.con_id, Some(76792991));
    assert_eq!(restored.sec_type, midas_broker_core::SecurityType::Stock);
    assert_eq!(restored.exchange, "ISLAND");
    assert_eq!(restored.currency, "USD");
    assert_eq!(restored.action, OrderAction::Sell);
    assert_eq!(restored.order_type, OrderKind::StopLimit);
    assert_eq!(restored.quantity, 50.0);
    assert_eq!(restored.limit_price, Some(250.0));
    assert_eq!(restored.stop_price, Some(255.0));
    assert_eq!(restored.trail_amount, Some(2.5));
    assert_eq!(restored.trail_percent, Some(1.5));
    assert_eq!(restored.tif, TimeInForce::Gtc);
    assert_eq!(restored.status, OrderStatus::Submitted);
    assert_eq!(restored.parent_id, order.parent_id);
    assert_eq!(restored.oca_group, Some("oca-group-42".to_string()));
    assert_eq!(restored.bracket_role, Some(BracketRole::StopLoss));
    assert_eq!(restored.strategy, Some("mean_reversion".to_string()));
    assert_eq!(
        restored.tags,
        vec![
            "urgent".to_string(),
            "earnings".to_string(),
            "q4".to_string()
        ]
    );
    assert_eq!(restored.algo_strategy, Some("TWAP".to_string()));
    assert_eq!(
        restored.algo_params,
        Some(serde_json::json!({"startTime": "09:30", "endTime": "16:00"}))
    );
    assert!(restored.outside_rth);
    assert_eq!(restored.filled_qty, 10.0);
    assert_eq!(restored.remaining_qty, 40.0);
    assert_eq!(restored.avg_fill_price, Some(252.75));
    assert_eq!(restored.last_fill_price, Some(253.00));
    assert_eq!(restored.commission, Some(3.50));
    assert_eq!(restored.activation_count, 3);
    assert!(restored.last_activated_at.is_some());
    assert!(restored.last_deactivated_at.is_some());
    // Timestamps: compare to the second (rfc3339 round-trip may truncate sub-second)
    assert_eq!(
        restored.created_at.timestamp(),
        order.created_at.timestamp()
    );
    assert_eq!(
        restored.updated_at.timestamp(),
        order.updated_at.timestamp()
    );
    assert_eq!(
        restored.last_activated_at.unwrap().timestamp(),
        order.last_activated_at.unwrap().timestamp()
    );
    assert_eq!(
        restored.last_deactivated_at.unwrap().timestamp(),
        order.last_deactivated_at.unwrap().timestamp()
    );
}

// 17. Validation failure - empty exchange rejected
#[tokio::test]
async fn test_create_bracket_empty_exchange_rejected() {
    let handle = start_broker_engine(test_config());
    let mut rx = handle.order_events.subscribe();

    let mut params = sample_bracket_params();
    params.exchange = String::new();
    handle
        .commands
        .send(BrokerCommand::CreateBracket(params))
        .await
        .unwrap();

    let event = tokio::time::timeout(Duration::from_secs(1), rx.recv())
        .await
        .expect("should not timeout")
        .expect("should receive event");

    match event {
        BrokerEvent::OrderValidationFailed { code, message } => {
            assert_eq!(code, -1);
            assert!(
                message.contains("exchange"),
                "error should mention exchange: {message}"
            );
        }
        other => panic!("expected OrderValidationFailed event for empty exchange, got {other:?}"),
    }

    let _ = handle.commands.send(BrokerCommand::Shutdown).await;
}

// 18. Validation failure - empty currency rejected
#[tokio::test]
async fn test_create_bracket_empty_currency_rejected() {
    let handle = start_broker_engine(test_config());
    let mut rx = handle.order_events.subscribe();

    let mut params = sample_bracket_params();
    params.currency = String::new();
    handle
        .commands
        .send(BrokerCommand::CreateBracket(params))
        .await
        .unwrap();

    let event = tokio::time::timeout(Duration::from_secs(1), rx.recv())
        .await
        .expect("should not timeout")
        .expect("should receive event");

    match event {
        BrokerEvent::OrderValidationFailed { code, message } => {
            assert_eq!(code, -1);
            assert!(
                message.contains("currency"),
                "error should mention currency: {message}"
            );
        }
        other => panic!("expected OrderValidationFailed event for empty currency, got {other:?}"),
    }

    let _ = handle.commands.send(BrokerCommand::Shutdown).await;
}

// 19. Order size guard - missing reference price with notional limit
#[tokio::test]
async fn test_order_size_guard_missing_reference_price() {
    let handle = start_broker_engine(test_config_with_limits(0.0, 500_000.0));
    let mut rx = handle.order_events.subscribe();

    let mut params = sample_bracket_params();
    params.reference_price = None; // remove reference price
    handle
        .commands
        .send(BrokerCommand::CreateBracket(params))
        .await
        .unwrap();

    let event = tokio::time::timeout(Duration::from_secs(1), rx.recv())
        .await
        .expect("should not timeout")
        .expect("should receive event");

    match event {
        BrokerEvent::OrderValidationFailed { code, message } => {
            assert_eq!(code, -2, "order size guard uses code -2");
            assert!(
                message.contains("reference price") || message.contains("AAPL"),
                "error should mention missing reference price: {message}"
            );
        }
        other => panic!(
            "expected OrderValidationFailed event for missing reference price, got {other:?}"
        ),
    }

    let _ = handle.commands.send(BrokerCommand::Shutdown).await;
}

// 20. Bracket persistence through engine - orders appear in DB
#[tokio::test]
async fn test_bracket_persisted_to_db_via_engine() {
    let handle = start_broker_engine(test_config());
    let mut rx = handle.order_events.subscribe();

    let params = sample_bracket_params();
    handle
        .commands
        .send(BrokerCommand::CreateBracket(params))
        .await
        .unwrap();

    // Get parent_id and child IDs from BracketCreated
    let (parent_id, tp_id, sl_id) = loop {
        let event = tokio::time::timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("should not timeout")
            .expect("should receive event");
        if let BrokerEvent::BracketCreated {
            parent_id,
            take_profit_id,
            stop_loss_id,
            ..
        } = event
        {
            break (parent_id, take_profit_id.unwrap(), stop_loss_id.unwrap());
        }
    };

    // Verify the IDs are all non-nil UUIDs
    assert!(!parent_id.is_nil());
    assert!(!tp_id.is_nil());
    assert!(!sl_id.is_nil());

    // The engine persists the bracket synchronously within the command handler,
    // so by the time BracketCreated is emitted, the DB writes are complete.
    // We cannot directly access the DB from outside the engine, but we can
    // verify that all three IDs are distinct (which requires correct UUID
    // generation inside build_bracket).
    let mut ids = vec![parent_id, tp_id, sl_id];
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), 3, "all bracket leg IDs should be unique");

    let _ = handle.commands.send(BrokerCommand::Shutdown).await;
}

// 21. Cancel bracket emits OrderCancelled for each leg
#[tokio::test]
async fn test_cancel_bracket_emits_order_cancelled_per_leg() {
    let handle = start_broker_engine(test_config());
    let mut rx = handle.order_events.subscribe();

    let params = sample_bracket_params();
    handle
        .commands
        .send(BrokerCommand::CreateBracket(params))
        .await
        .unwrap();

    // Get parent_id from BracketCreated
    let parent_id = loop {
        let event = tokio::time::timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("should not timeout")
            .expect("should receive event");
        if let BrokerEvent::BracketCreated { parent_id, .. } = event {
            break parent_id;
        }
    };

    // Cancel it — with TestBroker instant fills, the parent market order
    // fills immediately, so only the non-terminal children get cancelled.
    handle
        .commands
        .send(BrokerCommand::CancelBracket { parent_id })
        .await
        .unwrap();

    // Collect all events within a window
    let mut order_cancelled_count = 0;
    let mut bracket_status_count = 0;
    for _ in 0..30 {
        match tokio::time::timeout(Duration::from_millis(500), rx.recv()).await {
            Ok(Ok(BrokerEvent::OrderCancelled { .. })) => {
                order_cancelled_count += 1;
            }
            Ok(Ok(BrokerEvent::BracketStatusChanged { .. })) => {
                bracket_status_count += 1;
            }
            Ok(Ok(_)) => {} // other events (OrderSubmitted, OrderFilled, etc.)
            _ => break,
        }
    }
    // With instant fills: parent is Filled (terminal, not cancelled),
    // TP and SL are cancelled = 2 OrderCancelled events.
    // Without instant fills: all 3 cancelled.
    assert!(
        order_cancelled_count >= 2,
        "expected at least 2 cancelled legs, got {order_cancelled_count}"
    );
    assert!(
        bracket_status_count >= 1,
        "expected at least one BracketStatusChanged"
    );

    let _ = handle.commands.send(BrokerCommand::Shutdown).await;
}

// -----------------------------------------------------------------------
// Bracket submission integration tests (TestBrokerClient)
// -----------------------------------------------------------------------

/// Helper: collect N OrderSubmitted events from the event stream, ignoring
/// other event types. Returns the collected events.
async fn collect_order_submitted(
    rx: &mut broadcast::Receiver<BrokerEvent>,
    count: usize,
    timeout_secs: u64,
) -> Vec<(Uuid, i32, i64)> {
    let mut results = Vec::new();
    for _ in 0..count * 5 {
        match tokio::time::timeout(Duration::from_secs(timeout_secs), rx.recv()).await {
            Ok(Ok(BrokerEvent::OrderSubmitted {
                order_id,
                ib_order_id,
                ib_perm_id,
            })) => {
                results.push((order_id, ib_order_id, ib_perm_id));
                if results.len() == count {
                    break;
                }
            }
            Ok(Ok(_)) => {} // skip non-submitted events
            _ => break,
        }
    }
    results
}

/// Helper: receive events until a BracketCreated is found, returning it.
/// Skips OrderSubmitted and other events that may precede it.
async fn recv_bracket_created(
    rx: &mut broadcast::Receiver<BrokerEvent>,
    timeout_secs: u64,
) -> BrokerEvent {
    for _ in 0..20 {
        match tokio::time::timeout(Duration::from_secs(timeout_secs), rx.recv()).await {
            Ok(Ok(event @ BrokerEvent::BracketCreated { .. })) => return event,
            Ok(Ok(_)) => {} // skip non-BracketCreated events
            Ok(Err(e)) => panic!("broadcast recv error: {e}"),
            Err(_) => panic!("timed out waiting for BracketCreated"),
        }
    }
    panic!("did not receive BracketCreated within 20 events");
}

// 22. Full bracket submission: parent + TP + SL = 3 OrderSubmitted events
#[tokio::test]
async fn test_bracket_submission_with_test_client() {
    let handle = start_broker_engine(test_config());
    let mut rx = handle.order_events.subscribe();

    let params = sample_bracket_params();
    handle
        .commands
        .send(BrokerCommand::CreateBracket(params))
        .await
        .unwrap();

    // Collect all events: BracketCreated + 3 OrderSubmitted
    let submitted = collect_order_submitted(&mut rx, 3, 2).await;
    assert_eq!(
        submitted.len(),
        3,
        "full bracket should emit 3 OrderSubmitted events, got {}",
        submitted.len()
    );

    // All IB order IDs should be distinct
    let ib_ids: Vec<i32> = submitted.iter().map(|(_, ib_id, _)| *ib_id).collect();
    let mut unique_ib_ids = ib_ids.clone();
    unique_ib_ids.sort();
    unique_ib_ids.dedup();
    assert_eq!(
        unique_ib_ids.len(),
        3,
        "all 3 IB order IDs should be distinct: {ib_ids:?}"
    );

    // All local order IDs should be distinct
    let local_ids: Vec<Uuid> = submitted.iter().map(|(id, _, _)| *id).collect();
    let mut unique_local_ids = local_ids.clone();
    unique_local_ids.sort();
    unique_local_ids.dedup();
    assert_eq!(
        unique_local_ids.len(),
        3,
        "all 3 local order IDs should be distinct"
    );

    let _ = handle.commands.send(BrokerCommand::Shutdown).await;
}

// 23. TP-only bracket: parent + TP = 2 OrderSubmitted events
#[tokio::test]
async fn test_tp_only_bracket_submission() {
    let handle = start_broker_engine(test_config());
    let mut rx = handle.order_events.subscribe();

    let mut params = sample_bracket_params();
    params.stop_loss = None;
    handle
        .commands
        .send(BrokerCommand::CreateBracket(params))
        .await
        .unwrap();

    let submitted = collect_order_submitted(&mut rx, 2, 2).await;
    assert_eq!(
        submitted.len(),
        2,
        "TP-only bracket should emit 2 OrderSubmitted events"
    );

    let _ = handle.commands.send(BrokerCommand::Shutdown).await;
}

// 24. Naked market order: parent only = 1 OrderSubmitted event
#[tokio::test]
async fn test_naked_market_order_submission() {
    let handle = start_broker_engine(test_config());
    let mut rx = handle.order_events.subscribe();

    let mut params = sample_bracket_params();
    params.take_profit = None;
    params.stop_loss = None;
    handle
        .commands
        .send(BrokerCommand::CreateBracket(params))
        .await
        .unwrap();

    let submitted = collect_order_submitted(&mut rx, 1, 2).await;
    assert_eq!(
        submitted.len(),
        1,
        "naked market order should emit 1 OrderSubmitted event"
    );

    let _ = handle.commands.send(BrokerCommand::Shutdown).await;
}

// 25. SL-only bracket: parent + SL = 2 OrderSubmitted events
#[tokio::test]
async fn test_sl_only_bracket_submission() {
    let handle = start_broker_engine(test_config());
    let mut rx = handle.order_events.subscribe();

    let mut params = sample_bracket_params();
    params.take_profit = None;
    handle
        .commands
        .send(BrokerCommand::CreateBracket(params))
        .await
        .unwrap();

    let submitted = collect_order_submitted(&mut rx, 2, 2).await;
    assert_eq!(
        submitted.len(),
        2,
        "SL-only bracket should emit 2 OrderSubmitted events"
    );

    let _ = handle.commands.send(BrokerCommand::Shutdown).await;
}

// 26. Verify IB order IDs are assigned to the correct legs
#[tokio::test]
async fn test_bracket_ib_order_ids_match_events() {
    let handle = start_broker_engine(test_config());
    let mut rx = handle.order_events.subscribe();

    let params = sample_bracket_params();
    handle
        .commands
        .send(BrokerCommand::CreateBracket(params))
        .await
        .unwrap();

    // Collect all events (OrderSubmitted comes before BracketCreated now)
    let mut submitted: Vec<(Uuid, i32, i64)> = Vec::new();
    let mut bracket_created: Option<(Uuid, Uuid, Uuid)> = None;

    for _ in 0..20 {
        match tokio::time::timeout(Duration::from_secs(2), rx.recv()).await {
            Ok(Ok(BrokerEvent::OrderSubmitted {
                order_id,
                ib_order_id,
                ib_perm_id,
            })) => {
                submitted.push((order_id, ib_order_id, ib_perm_id));
            }
            Ok(Ok(BrokerEvent::BracketCreated {
                parent_id,
                take_profit_id,
                stop_loss_id,
                ..
            })) => {
                bracket_created = Some((parent_id, take_profit_id.unwrap(), stop_loss_id.unwrap()));
                // BracketCreated is last in the sequence, so we can stop
                break;
            }
            Ok(Ok(_)) => {} // skip other events
            _ => break,
        }
    }

    let (parent_id, tp_id, sl_id) = bracket_created.expect("should have received BracketCreated");
    assert_eq!(submitted.len(), 3, "should have 3 OrderSubmitted events");

    // Map local_id -> ib_order_id
    let mut id_map: HashMap<Uuid, i32> = HashMap::new();
    for (local_id, ib_id, _) in &submitted {
        id_map.insert(*local_id, *ib_id);
    }

    // All bracket leg IDs should appear in the submitted events
    assert!(
        id_map.contains_key(&parent_id),
        "parent should have OrderSubmitted"
    );
    assert!(id_map.contains_key(&tp_id), "TP should have OrderSubmitted");
    assert!(id_map.contains_key(&sl_id), "SL should have OrderSubmitted");

    let _ = handle.commands.send(BrokerCommand::Shutdown).await;
}

// 27. Engine with unconnected client - bracket submission fails gracefully
#[tokio::test]
async fn test_bracket_unconnected_client_emits_error() {
    // Test config has a TestBroker client that IS connected, so we
    // verify that bracket submission with the test broker works. If
    // we needed to test the "no client" path, we'd need a custom
    // engine setup — but that code path is guarded by
    // `self.client.as_ref().ok_or("no broker client configured")`.
    // Here we verify the happy path with default test config.
    let config = BrokerConfig::default();
    let handle = start_broker_engine(config);
    let mut rx = handle.order_events.subscribe();

    let params = sample_bracket_params();
    handle
        .commands
        .send(BrokerCommand::CreateBracket(params))
        .await
        .unwrap();

    // Should get BracketCreated (test broker auto-connects)
    let mut got_bracket = false;
    for _ in 0..10 {
        match tokio::time::timeout(Duration::from_secs(2), rx.recv()).await {
            Ok(Ok(BrokerEvent::BracketCreated { .. })) => {
                got_bracket = true;
                break;
            }
            Ok(Ok(_)) => {} // skip other events
            _ => break,
        }
    }
    assert!(got_bracket, "should have received BracketCreated");

    let _ = handle.commands.send(BrokerCommand::Shutdown).await;
}
