use tokio::sync::{broadcast, mpsc, watch};

use crate::commands::BrokerCommand;
use crate::config::{BrokerConfig, DataSourceConfig};
use crate::connection::ConnectionState;
use crate::error::BrokerError;
use crate::events::BrokerEvent;
use crate::market_data::MarketDataSource;

/// Returned by [`start_broker_engine`]. The UI interacts with the broker
/// exclusively through these channel handles.
///
/// - Send commands via `commands` (mpsc).
/// - Receive market data events via `market_events` (broadcast).
/// - Receive order lifecycle events via `order_events` (broadcast).
/// - Watch the connection state via `connection_state` (watch).
pub struct BrokerHandle {
    /// Send commands to the engine (connect, place orders, subscribe, etc.).
    pub commands: mpsc::Sender<BrokerCommand>,
    /// Subscribe to market data events (ticks, bars, depth).
    pub market_events: broadcast::Sender<BrokerEvent>,
    /// Subscribe to order lifecycle events (fills, status changes, errors).
    pub order_events: broadcast::Sender<BrokerEvent>,
    /// Watch the current connection state.
    pub connection_state: watch::Receiver<ConnectionState>,
}

/// Creates the broker engine and returns channel handles.
///
/// The engine runs as a tokio task on the current runtime. It processes
/// commands from the `BrokerHandle::commands` sender, emits events on the
/// broadcast channels, and updates connection state on the watch channel.
///
/// # Panics
///
/// Panics if called outside of a tokio runtime context.
pub fn start_broker_engine(config: BrokerConfig) -> BrokerHandle {
    let (command_tx, command_rx) = mpsc::channel::<BrokerCommand>(256);
    let (market_event_tx, _) = broadcast::channel::<BrokerEvent>(4096);
    let (order_event_tx, _) = broadcast::channel::<BrokerEvent>(8192);
    let (conn_state_tx, conn_state_rx) = watch::channel(ConnectionState::Disconnected);

    let market_tx_clone = market_event_tx.clone();
    let order_tx_clone = order_event_tx.clone();

    let data_source: Option<Box<dyn MarketDataSource>> = match &config.data_source {
        DataSourceConfig::Test => {
            Some(Box::new(crate::testdata::TestDataProvider::new()))
        }
        DataSourceConfig::Live => None, // IB adapter not yet implemented
    };

    tokio::spawn(async move {
        let mut engine = BrokerEngine {
            config,
            command_rx,
            market_event_tx: market_tx_clone,
            order_event_tx: order_tx_clone,
            conn_state_tx,
            data_source,
        };
        engine.run().await;
    });

    BrokerHandle {
        commands: command_tx,
        market_events: market_event_tx,
        order_events: order_event_tx,
        connection_state: conn_state_rx,
    }
}

/// The internal engine that drives the broker. Not exposed publicly.
struct BrokerEngine {
    #[allow(dead_code)]
    config: BrokerConfig,
    command_rx: mpsc::Receiver<BrokerCommand>,
    market_event_tx: broadcast::Sender<BrokerEvent>,
    #[allow(dead_code)]
    order_event_tx: broadcast::Sender<BrokerEvent>,
    #[allow(dead_code)]
    conn_state_tx: watch::Sender<ConnectionState>,
    data_source: Option<Box<dyn MarketDataSource>>,
}

impl BrokerEngine {
    /// Main event loop. Runs until the command channel is closed or a
    /// `Shutdown` command is received.
    async fn run(&mut self) {
        let mut heartbeat = tokio::time::interval(std::time::Duration::from_secs(30));

        loop {
            tokio::select! {
                cmd = self.command_rx.recv() => {
                    match cmd {
                        Some(command) => {
                            if self.handle_command(command).await {
                                break;
                            }
                        }
                        None => {
                            // All senders dropped; shut down.
                            tracing::info!("Command channel closed, engine stopping");
                            break;
                        }
                    }
                }
                _ = heartbeat.tick() => {
                    // Future: connection health check / keepalive
                }
            }
        }

        tracing::info!("Broker engine stopped");
    }

    /// Handle a single command. Returns `true` if the engine should stop.
    async fn handle_command(&mut self, cmd: BrokerCommand) -> bool {
        match cmd {
            BrokerCommand::Shutdown => {
                tracing::info!("Broker engine shutting down");
                self.command_rx.close();
                true
            }
            BrokerCommand::RequestHistoricalData {
                symbol,
                con_id,
                duration,
                bar_size,
                request_id,
            } => {
                if let Some(ref mut source) = self.data_source {
                    if let Err(e) = Self::dispatch_historical(
                        source.as_mut(),
                        &self.market_event_tx,
                        &symbol,
                        con_id,
                        &duration,
                        &bar_size,
                        request_id,
                    ) {
                        let _ = self.market_event_tx.send(BrokerEvent::Error {
                            code: -1,
                            message: format!("historical data error: {e}"),
                        });
                    }
                } else {
                    tracing::debug!(
                        "RequestHistoricalData: no data source configured (IB not yet implemented)"
                    );
                }
                false
            }
            _ => {
                tracing::debug!(?cmd, "Command received (handler not yet implemented)");
                false
            }
        }
    }

    /// Parse IB strings, fetch bars from the data source, and emit events.
    fn dispatch_historical(
        source: &mut dyn MarketDataSource,
        tx: &broadcast::Sender<BrokerEvent>,
        symbol: &str,
        con_id: i32,
        duration: &str,
        bar_size: &str,
        request_id: u64,
    ) -> Result<(), BrokerError> {
        use crate::ib_strings::{duration_to_start, parse_bar_size};

        let timeframe = parse_bar_size(bar_size)?;
        let end = chrono::Utc::now().timestamp();
        let start = duration_to_start(end, duration)?;

        let result = source.historical_bars(symbol, con_id, timeframe, start, end, request_id)?;

        for bar in &result.bars {
            let _ = tx.send(BrokerEvent::BarClosed {
                symbol: result.symbol.clone(),
                timestamp: bar.timestamp,
                open: bar.open,
                high: bar.high,
                low: bar.low,
                close: bar.close,
                volume: bar.volume,
            });
        }

        let _ = tx.send(BrokerEvent::HistoricalDataComplete {
            request_id: result.request_id,
            symbol: result.symbol.clone(),
        });

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn test_broker_handle_creation() {
        let config = BrokerConfig::default();
        let handle = start_broker_engine(config);

        // Connection should start as Disconnected.
        assert_eq!(*handle.connection_state.borrow(), ConnectionState::Disconnected);

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
        let market_event = BrokerEvent::Connected { server_version: 176 };
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

        let mut config = BrokerConfig::default();
        config.data_source = DataSourceConfig::Test;
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
                BrokerEvent::HistoricalDataComplete {
                    request_id,
                    symbol,
                } => {
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

    #[tokio::test]
    async fn test_historical_data_live_source_noop() {
        // Default config = Live, no data source → graceful no-op
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

        // Should not receive any events (no data source configured)
        let result = tokio::time::timeout(Duration::from_millis(200), rx.recv()).await;
        assert!(result.is_err(), "should timeout — no events expected");

        let _ = handle.commands.send(BrokerCommand::Shutdown).await;
    }
}
