//! Adapter between `midas-broker::BrokerHandle` and the desktop workspace.
//!
//! `BrokerBridge` wraps the broker engine's channel handles and translates
//! between the desktop mirror types (`midas_core::broker::*`) and the broker
//! engine types (`midas_broker::*`).

use std::hash::{Hash, Hasher};

use async_trait::async_trait;
use iced::futures::SinkExt;
use tokio::sync::{broadcast, mpsc, watch};

use midas_broker::{BrokerCommand, BrokerEvent, BrokerHandle};

use crate::app::Message;

// ══════════════════════════════════════════════════════════════════════════════
// BrokerBridge
// ══════════════════════════════════════════════════════════════════════════════

/// Desktop-side wrapper around the broker engine's channel handles.
///
/// All methods are non-blocking: they send commands over an mpsc channel
/// and return immediately. Results arrive as `BrokerEvent`s on the
/// broadcast channels.
pub struct BrokerBridge {
    /// Command sender to the broker engine.
    commands: mpsc::Sender<BrokerCommand>,
    /// Order event broadcast sender (kept alive to subscribe new receivers).
    order_events: broadcast::Sender<BrokerEvent>,
    /// Connection state watcher.
    connection_state: watch::Receiver<midas_broker::ConnectionState>,
    /// Display name for the UI.
    name: String,
}

impl BrokerBridge {
    /// Create a bridge from a `BrokerHandle` returned by `start_broker_engine`.
    pub fn new(handle: BrokerHandle, name: impl Into<String>) -> Self {
        Self {
            commands: handle.commands,
            order_events: handle.order_events,
            connection_state: handle.connection_state,
            name: name.into(),
        }
    }

    /// Send a command to the broker engine (non-blocking).
    ///
    /// Returns `Err` only if the engine task has been dropped (shutdown).
    pub fn send_command(&self, cmd: BrokerCommand) -> Result<(), String> {
        self.commands
            .try_send(cmd)
            .map_err(|e| format!("broker command send failed: {e}"))
    }

    /// Send `BrokerCommand::Connect` to initiate the connection.
    pub fn connect(&self) -> Result<(), String> {
        self.send_command(BrokerCommand::Connect)
    }

    /// Send `BrokerCommand::CreateMarketBracket` with translated params.
    pub fn create_market_bracket(
        &self,
        params: midas_core::broker::MarketBracketParams,
    ) -> Result<(), String> {
        let broker_params = translate_bracket_params(params);
        self.send_command(BrokerCommand::CreateMarketBracket(broker_params))
    }

    /// Send `BrokerCommand::CancelBracket`.
    pub fn cancel_bracket(&self, parent_id: uuid::Uuid) -> Result<(), String> {
        self.send_command(BrokerCommand::CancelBracket { parent_id })
    }

    /// Send `BrokerCommand::ModifyBracketLeg`.
    pub fn modify_bracket_leg(&self, order_id: uuid::Uuid, new_price: f64) -> Result<(), String> {
        self.send_command(BrokerCommand::ModifyBracketLeg {
            order_id,
            new_price,
        })
    }

    /// Whether the broker engine reports a connected state.
    pub fn is_engine_connected(&self) -> bool {
        self.connection_state.borrow().is_connected()
    }

    /// Gracefully shut down the broker engine.
    pub fn shutdown(&self) -> Result<(), String> {
        self.send_command(BrokerCommand::Shutdown)
    }

    /// Create a `BrokerEventSource` for use with `Subscription::run_with`.
    pub fn event_source(&self) -> BrokerEventSource {
        BrokerEventSource {
            sender: self.order_events.clone(),
        }
    }

    /// Create a `BrokerConnSource` for the connection state subscription.
    pub fn conn_source(&self) -> BrokerConnSource {
        BrokerConnSource {
            receiver: self.connection_state.clone(),
        }
    }
}

// ── OrderBroker trait implementation (provider) ──────────────────────────────

#[async_trait]
impl midas_core::provider::OrderBroker for BrokerBridge {
    fn name(&self) -> &str {
        &self.name
    }

    fn is_connected(&self) -> bool {
        self.is_engine_connected()
    }

    fn connection_state(&self) -> midas_core::provider::ConnectionState {
        translate_connection_state(&self.connection_state.borrow())
    }
}

// ══════════════════════════════════════════════════════════════════════════════
// Subscription helpers
// ══════════════════════════════════════════════════════════════════════════════

/// Wrapper around `broadcast::Sender<BrokerEvent>` that implements `Hash`
/// so it can be used with `Subscription::run_with`.
///
/// The hash is a constant (we only ever have one broker subscription),
/// which tells iced to keep a single instance alive.
#[derive(Clone)]
pub struct BrokerEventSource {
    pub sender: broadcast::Sender<BrokerEvent>,
}

impl Hash for BrokerEventSource {
    fn hash<H: Hasher>(&self, state: &mut H) {
        "broker-event-source".hash(state);
    }
}

/// Wrapper around `watch::Receiver<ConnectionState>` that implements `Hash`
/// for `Subscription::run_with`.
#[derive(Clone)]
pub struct BrokerConnSource {
    pub receiver: watch::Receiver<midas_broker::ConnectionState>,
}

impl Hash for BrokerConnSource {
    fn hash<H: Hasher>(&self, state: &mut H) {
        "broker-conn-source".hash(state);
    }
}

/// Build a stream of broker events for iced subscription.
///
/// This is a `fn` pointer (not a closure) as required by `Subscription::run_with`.
pub fn broker_event_stream(
    source: &BrokerEventSource,
) -> impl iced::futures::Stream<Item = Message> {
    let sender = source.sender.clone();
    iced::stream::channel(256, async move |mut output| {
        let mut rx = sender.subscribe();
        loop {
            match rx.recv().await {
                Ok(event) => {
                    if output
                        .send(Message::BrokerEventReceived(Box::new(event)))
                        .await
                        .is_err()
                    {
                        tracing::trace!("Broker event subscription output closed");
                        break;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!("Broker event subscription lagged by {n} events");
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    tracing::info!("Broker event channel closed, subscription ending");
                    std::future::pending::<()>().await;
                    break;
                }
            }
        }
    })
}

/// Build a stream of connection state changes for iced subscription.
pub fn broker_conn_stream(source: &BrokerConnSource) -> impl iced::futures::Stream<Item = Message> {
    let mut conn_rx = source.receiver.clone();
    iced::stream::channel(16, async move |mut output| {
        loop {
            if conn_rx.changed().await.is_err() {
                // Sender dropped — engine shut down.
                std::future::pending::<()>().await;
                break;
            }
            let state_str = conn_rx.borrow_and_update().to_string();
            if output
                .send(Message::BrokerConnectionChanged(state_str))
                .await
                .is_err()
            {
                tracing::trace!("Broker connection subscription output closed");
                break;
            }
        }
    })
}

// ══════════════════════════════════════════════════════════════════════════════
// Type translation functions
// ══════════════════════════════════════════════════════════════════════════════

/// Translate desktop `MarketBracketParams` to broker engine
/// `midas_broker::MarketBracketParams`.
fn translate_bracket_params(
    p: midas_core::broker::MarketBracketParams,
) -> midas_broker::MarketBracketParams {
    midas_broker::MarketBracketParams {
        symbol: p.symbol,
        con_id: p.con_id,
        sec_type: translate_security_type(p.sec_type),
        exchange: p.exchange,
        currency: p.currency,
        action: translate_order_action(p.action),
        quantity: p.quantity,
        outside_rth: p.outside_rth,
        take_profit: p.take_profit.map(|tp| midas_broker::TakeProfitParams {
            price: tp.price,
            tif: tp.tif.map(translate_tif),
        }),
        stop_loss: p.stop_loss.map(|sl| midas_broker::StopLossParams {
            stop_price: sl.stop_price,
            limit_price: sl.limit_price,
            tif: sl.tif.map(translate_tif),
        }),
        reference_price: p.reference_price,
        strategy: p.strategy,
        tags: p.tags,
    }
}

/// Desktop `SecurityType` -> broker `SecurityType`.
fn translate_security_type(st: midas_core::SecurityType) -> midas_broker::SecurityType {
    match st {
        midas_core::SecurityType::Stock => midas_broker::SecurityType::Stock,
        midas_core::SecurityType::Option => midas_broker::SecurityType::Option,
        midas_core::SecurityType::Future => midas_broker::SecurityType::Future,
        midas_core::SecurityType::Forex => midas_broker::SecurityType::Forex,
    }
}

/// Desktop `OrderAction` -> broker `OrderAction`.
fn translate_order_action(a: midas_core::broker::OrderAction) -> midas_broker::OrderAction {
    match a {
        midas_core::broker::OrderAction::Buy => midas_broker::OrderAction::Buy,
        midas_core::broker::OrderAction::Sell => midas_broker::OrderAction::Sell,
    }
}

/// Desktop `TimeInForce` -> broker `TimeInForce`.
fn translate_tif(tif: midas_core::broker::TimeInForce) -> midas_broker::TimeInForce {
    match tif {
        midas_core::broker::TimeInForce::Day => midas_broker::TimeInForce::Day,
        midas_core::broker::TimeInForce::Gtc => midas_broker::TimeInForce::Gtc,
        midas_core::broker::TimeInForce::Ioc => midas_broker::TimeInForce::Ioc,
        midas_core::broker::TimeInForce::Gtd => midas_broker::TimeInForce::Gtd,
        midas_core::broker::TimeInForce::Opg => midas_broker::TimeInForce::Opg,
    }
}

/// Broker engine `ConnectionState` -> desktop `ConnectionState`.
fn translate_connection_state(
    cs: &midas_broker::ConnectionState,
) -> midas_core::provider::ConnectionState {
    match cs {
        midas_broker::ConnectionState::Disconnected => {
            midas_core::provider::ConnectionState::Disconnected
        }
        midas_broker::ConnectionState::Connecting => {
            midas_core::provider::ConnectionState::Connecting
        }
        midas_broker::ConnectionState::Connected { server_version } => {
            midas_core::provider::ConnectionState::Connected {
                server_version: *server_version,
            }
        }
        midas_broker::ConnectionState::Ready => midas_core::provider::ConnectionState::Ready,
        midas_broker::ConnectionState::Reconnecting { attempt } => {
            midas_core::provider::ConnectionState::Reconnecting { attempt: *attempt }
        }
    }
}

/// Translate a broker `OrderAction` to a chart `BracketSide`.
pub fn translate_action_to_side(
    action: &midas_broker::OrderAction,
) -> midas_chart::widget::order_bracket::BracketSide {
    match action {
        midas_broker::OrderAction::Buy => midas_chart::widget::order_bracket::BracketSide::Long,
        midas_broker::OrderAction::Sell => midas_chart::widget::order_bracket::BracketSide::Short,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn translate_order_action_roundtrip() {
        let buy = translate_order_action(midas_core::broker::OrderAction::Buy);
        assert_eq!(buy, midas_broker::OrderAction::Buy);
        let sell = translate_order_action(midas_core::broker::OrderAction::Sell);
        assert_eq!(sell, midas_broker::OrderAction::Sell);
    }

    #[test]
    fn translate_action_to_side_mapping() {
        use midas_broker::OrderAction;
        use midas_chart::widget::order_bracket::BracketSide;

        assert_eq!(
            translate_action_to_side(&OrderAction::Buy),
            BracketSide::Long
        );
        assert_eq!(
            translate_action_to_side(&OrderAction::Sell),
            BracketSide::Short
        );
    }
}
