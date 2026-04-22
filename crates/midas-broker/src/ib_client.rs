//! Live Interactive Brokers adapter implementing [`BrokerClient`].
//!
//! Wraps `ibapi::Client` (async) and translates between our broker trait and
//! the TWS API. The client runs on the tokio runtime and queues callbacks
//! for the engine to poll via [`BrokerClient::poll_callbacks`].

// Legacy `BrokerClient` trait is `#[deprecated]` pending slice 9.
#![allow(deprecated)]

use std::sync::{Arc, Mutex};

use tokio::runtime::Handle;

use crate::client::{
    AccountSummary, BrokerCallback, BrokerClient, CancelOrderResult, PlaceOrderResult,
    PositionRecord,
};

// ══════════════════════════════════════════════════════════════════════════
// IbClient
// ══════════════════════════════════════════════════════════════════════════

/// Live IB adapter implementing [`BrokerClient`].
///
/// Holds an `ibapi::Client` behind `Arc<Mutex<..>>` for interior mutability
/// (the `BrokerClient` trait uses `&self`) and a callback queue that the
/// engine drains each poll cycle.
pub struct IbClient {
    /// The underlying ibapi async client (set on connect, cleared on disconnect).
    inner: Arc<Mutex<Option<Arc<ibapi::Client>>>>,
    /// Pending callbacks queued by background tasks for the engine to drain.
    callbacks: Arc<Mutex<Vec<BrokerCallback>>>,
    /// Connection address (host:port).
    address: String,
    /// TWS API client ID.
    client_id: i32,
    /// Tokio runtime handle for sync→async bridge.
    rt: Handle,
}

impl IbClient {
    /// Create an IB client that will connect to the given address.
    ///
    /// The actual TCP connection is deferred to [`BrokerClient::connect`].
    pub fn new(host: &str, port: u16, client_id: i32) -> Self {
        Self {
            inner: Arc::new(Mutex::new(None)),
            callbacks: Arc::new(Mutex::new(Vec::new())),
            address: format!("{host}:{port}"),
            client_id,
            rt: Handle::current(),
        }
    }

    fn push_callback(&self, cb: BrokerCallback) {
        self.callbacks
            .lock()
            .expect("callback mutex poisoned")
            .push(cb);
    }

    /// Get a clone of the inner client Arc, if connected.
    fn client(&self) -> Option<Arc<ibapi::Client>> {
        self.inner.lock().expect("inner mutex poisoned").clone()
    }

    /// Build an `ibapi::orders::Order` from our order parameters.
    #[expect(clippy::too_many_arguments, reason = "mirrors IB order field set")]
    fn build_order(
        action: &str,
        order_type: &str,
        quantity: f64,
        limit_price: Option<f64>,
        stop_price: Option<f64>,
        parent_id: Option<i32>,
        transmit: bool,
        tif: &str,
        outside_rth: bool,
    ) -> ibapi::orders::Order {
        let action = match action.to_uppercase().as_str() {
            "BUY" => ibapi::orders::Action::Buy,
            _ => ibapi::orders::Action::Sell,
        };
        let tif = match tif.to_uppercase().as_str() {
            "GTC" => ibapi::orders::TimeInForce::GoodTilCanceled,
            "IOC" => ibapi::orders::TimeInForce::ImmediateOrCancel,
            "GTD" => ibapi::orders::TimeInForce::GoodTilDate,
            "OPG" => ibapi::orders::TimeInForce::OnOpen,
            _ => ibapi::orders::TimeInForce::Day,
        };
        let mut order = ibapi::orders::Order {
            action,
            total_quantity: quantity,
            order_type: order_type.to_string(),
            limit_price,
            aux_price: stop_price,
            transmit,
            outside_rth,
            tif,
            ..Default::default()
        };
        if let Some(pid) = parent_id {
            order.parent_id = pid;
        }
        order
    }

    /// Spawn a background task to monitor order updates from a `place_order`
    /// subscription and queue callbacks.
    fn spawn_order_monitor(&self, mut sub: ibapi::client::Subscription<ibapi::orders::PlaceOrder>) {
        let callbacks = Arc::clone(&self.callbacks);
        self.rt.spawn(async move {
            while let Some(update) = sub.next().await {
                match update {
                    Ok(ibapi::orders::PlaceOrder::OrderStatus(status)) => {
                        callbacks
                            .lock()
                            .expect("mutex")
                            .push(BrokerCallback::OrderStatus {
                                ib_order_id: status.order_id,
                                status: status.status.clone(),
                                filled: status.filled,
                                remaining: status.remaining,
                                avg_fill_price: status.average_fill_price,
                            });
                    }
                    Ok(ibapi::orders::PlaceOrder::ExecutionData(exec_data)) => {
                        let exec = &exec_data.execution;
                        callbacks
                            .lock()
                            .expect("mutex")
                            .push(BrokerCallback::Execution {
                                ib_order_id: exec.order_id,
                                exec_id: exec.execution_id.clone(),
                                shares: exec.shares,
                                price: exec.price,
                                commission: 0.0,
                                side: exec.side.clone(),
                            });
                    }
                    Ok(_) => {} // CommissionReport, OpenOrder, Message
                    Err(e) => {
                        tracing::error!("IB order subscription error: {e}");
                        break;
                    }
                }
            }
        });
    }

    /// Spawn a background task to monitor the global order update stream.
    fn spawn_order_update_stream(&self, client: Arc<ibapi::Client>) {
        let callbacks = Arc::clone(&self.callbacks);
        self.rt.spawn(async move {
            match client.order_update_stream().await {
                Ok(mut sub) => {
                    while let Some(update) = sub.next().await {
                        match update {
                            Ok(ibapi::orders::OrderUpdate::OrderStatus(status)) => {
                                callbacks.lock().expect("mutex").push(
                                    BrokerCallback::OrderStatus {
                                        ib_order_id: status.order_id,
                                        status: status.status.clone(),
                                        filled: status.filled,
                                        remaining: status.remaining,
                                        avg_fill_price: status.average_fill_price,
                                    },
                                );
                            }
                            Ok(ibapi::orders::OrderUpdate::ExecutionData(exec_data)) => {
                                let exec = &exec_data.execution;
                                callbacks
                                    .lock()
                                    .expect("mutex")
                                    .push(BrokerCallback::Execution {
                                        ib_order_id: exec.order_id,
                                        exec_id: exec.execution_id.clone(),
                                        shares: exec.shares,
                                        price: exec.price,
                                        commission: 0.0,
                                        side: exec.side.clone(),
                                    });
                            }
                            Ok(_) => {}
                            Err(e) => {
                                tracing::error!("IB order update stream error: {e}");
                                break;
                            }
                        }
                    }
                    callbacks
                        .lock()
                        .expect("mutex")
                        .push(BrokerCallback::ConnectionStatus {
                            connected: false,
                            server_version: None,
                        });
                }
                Err(e) => {
                    tracing::error!("Failed to start order update stream: {e}");
                }
            }
        });
    }
}

// ══════════════════════════════════════════════════════════════════════════
// BrokerClient implementation
// ══════════════════════════════════════════════════════════════════════════

impl BrokerClient for IbClient {
    fn next_order_id(&self) -> i32 {
        self.client().map(|c| c.next_order_id()).unwrap_or(0)
    }

    fn place_order(
        &self,
        order_id: i32,
        symbol: &str,
        action: &str,
        order_type: &str,
        quantity: f64,
        limit_price: Option<f64>,
        stop_price: Option<f64>,
        parent_id: Option<i32>,
        transmit: bool,
        tif: &str,
        outside_rth: bool,
    ) -> Result<PlaceOrderResult, String> {
        let client = self.client().ok_or("not connected to IB")?;
        let contract = ibapi::contracts::Contract::stock(symbol).build();
        let order = Self::build_order(
            action,
            order_type,
            quantity,
            limit_price,
            stop_price,
            parent_id,
            transmit,
            tif,
            outside_rth,
        );

        let result = self
            .rt
            .block_on(async { client.place_order(order_id, &contract, &order).await });

        match result {
            Ok(sub) => {
                self.spawn_order_monitor(sub);
                Ok(PlaceOrderResult {
                    ib_order_id: order_id,
                })
            }
            Err(e) => {
                self.push_callback(BrokerCallback::OrderRejected {
                    ib_order_id: order_id,
                    reason: e.to_string(),
                });
                Err(format!("IB place_order failed: {e}"))
            }
        }
    }

    fn cancel_order(&self, ib_order_id: i32) -> Result<CancelOrderResult, String> {
        let client = self.client().ok_or("not connected to IB")?;
        let callbacks = Arc::clone(&self.callbacks);

        self.rt.block_on(async {
            match client.cancel_order(ib_order_id, "").await {
                Ok(mut sub) => {
                    tokio::spawn(async move {
                        while let Some(update) = sub.next().await {
                            if let Ok(ibapi::orders::CancelOrder::OrderStatus(status)) = update {
                                callbacks.lock().expect("mutex").push(
                                    BrokerCallback::OrderStatus {
                                        ib_order_id: status.order_id,
                                        status: status.status,
                                        filled: status.filled,
                                        remaining: status.remaining,
                                        avg_fill_price: status.average_fill_price,
                                    },
                                );
                            }
                        }
                    });
                    Ok(CancelOrderResult { ib_order_id })
                }
                Err(e) => Err(format!("IB cancel_order failed: {e}")),
            }
        })
    }

    fn name(&self) -> &str {
        "InteractiveBrokers"
    }

    fn connect(&self) -> Result<i32, String> {
        let address = self.address.clone();
        let client_id = self.client_id;

        let client = self
            .rt
            .block_on(async { ibapi::Client::connect(&address, client_id).await })
            .map_err(|e| format!("IB connection failed: {e}"))?;

        let server_version = client.server_version();
        let client = Arc::new(client);

        self.spawn_order_update_stream(Arc::clone(&client));

        *self.inner.lock().expect("inner mutex poisoned") = Some(client);

        self.push_callback(BrokerCallback::ConnectionStatus {
            connected: true,
            server_version: Some(server_version),
        });

        Ok(server_version)
    }

    fn disconnect(&self) {
        *self.inner.lock().expect("inner mutex poisoned") = None;

        self.push_callback(BrokerCallback::ConnectionStatus {
            connected: false,
            server_version: None,
        });
    }

    fn is_connected(&self) -> bool {
        self.client().is_some_and(|c| c.is_connected())
    }

    fn subscribe_market_data(&self, symbol: &str, _con_id: i32) {
        let Some(client) = self.client() else { return };
        let callbacks = Arc::clone(&self.callbacks);
        let symbol = symbol.to_string();

        self.rt.spawn(async move {
            let contract = ibapi::contracts::Contract::stock(&symbol).build();
            match client
                .realtime_bars(
                    &contract,
                    ibapi::market_data::realtime::BarSize::Sec5,
                    ibapi::market_data::realtime::WhatToShow::Trades,
                    ibapi::market_data::TradingHours::Regular,
                )
                .await
            {
                Ok(mut sub) => {
                    while let Some(update) = sub.next().await {
                        match update {
                            Ok(bar) => {
                                callbacks
                                    .lock()
                                    .expect("mutex")
                                    .push(BrokerCallback::BarClosed {
                                        symbol: symbol.clone(),
                                        timestamp: bar.date.unix_timestamp(),
                                        open: bar.open,
                                        high: bar.high,
                                        low: bar.low,
                                        close: bar.close,
                                        volume: bar.volume as i64,
                                    });
                            }
                            Err(e) => {
                                tracing::error!("IB realtime bar error for {symbol}: {e}");
                                break;
                            }
                        }
                    }
                }
                Err(e) => {
                    tracing::error!("IB realtime_bars subscription failed for {symbol}: {e}");
                }
            }
        });
    }

    fn unsubscribe_market_data(&self, _symbol: &str) {
        // ibapi subscriptions are cancelled by dropping the Subscription.
        tracing::debug!("unsubscribe_market_data: subscription dropped on cleanup");
    }

    fn request_positions(&self) -> Vec<PositionRecord> {
        let Some(client) = self.client() else {
            return Vec::new();
        };

        self.rt.block_on(async {
            match client.positions().await {
                Ok(mut sub) => {
                    let mut positions = Vec::new();
                    while let Some(update) = sub.next().await {
                        match update {
                            Ok(ibapi::accounts::PositionUpdate::Position(pos)) => {
                                positions.push(PositionRecord {
                                    symbol: pos.contract.symbol.to_string(),
                                    quantity: pos.position,
                                    avg_cost: pos.average_cost,
                                });
                            }
                            Ok(ibapi::accounts::PositionUpdate::PositionEnd) => break,
                            Err(e) => {
                                tracing::error!("IB positions error: {e}");
                                break;
                            }
                        }
                    }
                    positions
                }
                Err(e) => {
                    tracing::error!("IB positions request failed: {e}");
                    Vec::new()
                }
            }
        })
    }

    fn request_account_summary(&self) -> AccountSummary {
        let Some(client) = self.client() else {
            return AccountSummary::default();
        };

        self.rt.block_on(async {
            let group = ibapi::accounts::types::AccountGroup("All".to_string());
            let tags = &[
                "NetLiquidation",
                "TotalCashValue",
                "UnrealizedPnL",
                "RealizedPnL",
            ];
            match client.account_summary(&group, tags).await {
                Ok(mut sub) => {
                    let mut summary = AccountSummary::default();
                    while let Some(update) = sub.next().await {
                        match update {
                            Ok(ibapi::accounts::AccountSummaryResult::Summary(s)) => {
                                match s.tag.as_str() {
                                    "TotalCashValue" => {
                                        summary.cash_balance = s.value.parse().unwrap_or(0.0);
                                    }
                                    "UnrealizedPnL" => {
                                        summary.unrealized_pnl = s.value.parse().unwrap_or(0.0);
                                    }
                                    "RealizedPnL" => {
                                        summary.realized_pnl = s.value.parse().unwrap_or(0.0);
                                    }
                                    _ => {}
                                }
                            }
                            Ok(ibapi::accounts::AccountSummaryResult::End) => break,
                            Err(e) => {
                                tracing::error!("IB account summary error: {e}");
                                break;
                            }
                        }
                    }
                    summary
                }
                Err(e) => {
                    tracing::error!("IB account summary request failed: {e}");
                    AccountSummary::default()
                }
            }
        })
    }

    fn poll_callbacks(&self) -> Vec<BrokerCallback> {
        let mut cbs = self.callbacks.lock().expect("callback mutex poisoned");
        std::mem::take(&mut *cbs)
    }
}

// IbClient is Send+Sync because all interior state is behind Arc<Mutex<..>>.
unsafe impl Send for IbClient {}
unsafe impl Sync for IbClient {}
