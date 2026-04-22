//! IB-faithful [`OrderClient`] sim backend.
//!
//! Ports the order-lifecycle logic from
//! [`TestBroker`](crate::test_broker::TestBroker) to the new trait
//! surface. The key architectural swap is the event transport: the
//! legacy path used `poll_callbacks` on a 10 ms drain loop; the new
//! path emits `OrderEvent` directly on a `broadcast::Sender` the
//! moment the lifecycle transition occurs.
//!
//! Behaviour preserved from the legacy broker:
//!
//! * Bracket activation rules (parent transmit-last, children
//!   PreSubmitted until parent fills).
//! * Market-order instant fill at `base_price ± spread/2`.
//! * Partial-fill tranches controlled by `partial_fill_threshold` /
//!   `partial_fill_tranches`.
//! * OCA (one-cancels-all): filling a child cancels its siblings.
//! * Deterministic rejection-rate knob.
//! * Price-triggered fills for LMT + STP orders on
//!   [`SimOrderClient::set_market_price`].

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use midas_broker_core::market_data::{ConnectionState, ErrorCode, SymbolKey};
use parking_lot::Mutex;
use tokio::sync::{broadcast, mpsc, watch};

use crate::client::PlaceOrderResult;
use crate::market_data_source::MarketDataSource;
use crate::order_client::{
    AccountEvent, CancelOrderEvent, CancelOrderStream, CompletedOrder, OpenOrder, OrderClient,
    OrderError, OrderEvent, OrderModify, OrderSpec, OrderType, PositionUpdate, Tif,
};
use crate::orders::state::OrderStatus;
use crate::orders::types::OrderAction;
use crate::sim::config::SimOrderConfig;
use crate::sim::market_data::SimMarketData;

/// Simulated order state (sim-internal).
#[derive(Debug, Clone, PartialEq, Eq)]
enum SimOrderStatus {
    Held,
    Working,
    Triggered,
    PartiallyFilled,
    Filled,
    Cancelled,
    Rejected,
}

impl SimOrderStatus {
    fn is_terminal(&self) -> bool {
        matches!(self, Self::Filled | Self::Cancelled | Self::Rejected)
    }
}

/// Simulated order record.
#[derive(Debug, Clone)]
struct SimulatedOrder {
    ib_order_id: i32,
    symbol: SymbolKey,
    action: OrderAction,
    order_type: OrderType,
    quantity: f64,
    limit_price: Option<f64>,
    stop_price: Option<f64>,
    parent_id: Option<i32>,
    /// Kept for parity with IB's transmit flag; the sim always
    /// activates immediately when `transmit == true` but the field
    /// lets tests inspect what was requested.
    #[allow(dead_code)]
    transmit: bool,
    status: SimOrderStatus,
    filled_qty: f64,
    avg_fill_price: f64,
    tif: Tif,
    outside_rth: bool,
}

#[derive(Debug, Clone)]
struct PositionState {
    /// Redundant with the hash-map key; kept so ad-hoc
    /// `.values().map(...)` iterations don't need the map reference.
    #[allow(dead_code)]
    symbol: String,
    quantity: f64,
    avg_cost: f64,
}

struct Inner {
    orders: HashMap<i32, SimulatedOrder>,
    bracket_links: HashMap<i32, Vec<i32>>,
    market_prices: HashMap<String, f64>,
    positions: HashMap<String, PositionState>,
    cash: f64,
    realized_pnl: f64,
    /// Pending cancel-stream tx's keyed by ib_order_id so a Cancelled
    /// broadcast can also flow back through the specific stream that
    /// requested it.
    cancel_streams: HashMap<i32, VecDeque<mpsc::Sender<CancelOrderEvent>>>,
    order_count: u64,
}

/// [`OrderClient`] implementation for the sim backend.
///
/// Optionally pairs with a [`SimMarketData`] so
/// [`Self::set_market_price`] drives both the order engine and the
/// downstream tick emitter in tests that exercise end-to-end fill
/// flows.
pub struct SimOrderClient {
    config: SimOrderConfig,
    inner: Mutex<Inner>,
    next_id: AtomicI32,
    exec_counter: AtomicI32,
    order_tx: broadcast::Sender<OrderEvent>,
    position_tx: broadcast::Sender<PositionUpdate>,
    account_tx: broadcast::Sender<AccountEvent>,
    /// Watches the market-data backend's ConnectionState so
    /// `next_order_id` can block until `Ready`.
    conn_state_rx: Mutex<Option<watch::Receiver<ConnectionState>>>,
    /// Non-owning weak ref to the sim market-data backend for
    /// price-lookup parity with the legacy broker. `None` when the
    /// caller wires the order client in isolation.
    market: Option<Arc<SimMarketData>>,
    /// Tracks symbols we've notified on for position-event fan-out.
    #[allow(dead_code)]
    symbols_seen: Mutex<HashSet<String>>,
}

impl SimOrderClient {
    /// Build a new sim order client.
    ///
    /// Pass `None` for `market` when the test only exercises the order
    /// client in isolation; the market-price lookups then fall back to
    /// `100.0` until [`Self::set_market_price`] is called.
    pub fn new(config: SimOrderConfig, market: Option<Arc<SimMarketData>>) -> Arc<Self> {
        let (order_tx, _) = broadcast::channel(8192);
        let (position_tx, _) = broadcast::channel(256);
        let (account_tx, _) = broadcast::channel(256);
        let conn_state_rx = market.as_ref().map(|m| m.connection_state());
        let next_id_seed = config.next_order_id_seed;
        let initial_cash = config.initial_cash;
        Arc::new(Self {
            config,
            inner: Mutex::new(Inner {
                orders: HashMap::new(),
                bracket_links: HashMap::new(),
                market_prices: HashMap::new(),
                positions: HashMap::new(),
                cash: initial_cash,
                realized_pnl: 0.0,
                cancel_streams: HashMap::new(),
                order_count: 0,
            }),
            next_id: AtomicI32::new(next_id_seed),
            exec_counter: AtomicI32::new(1),
            order_tx,
            position_tx,
            account_tx,
            conn_state_rx: Mutex::new(conn_state_rx),
            market,
            symbols_seen: Mutex::new(HashSet::new()),
        })
    }

    fn next_exec_id(&self) -> String {
        let n = self.exec_counter.fetch_add(1, Ordering::SeqCst);
        format!("SIM.{n}")
    }

    /// Read the current market price for `symbol`. Uses the attached
    /// [`SimMarketData`]'s live state when available, falling back to
    /// the local `market_prices` cache.
    fn get_market_price(&self, inner: &Inner, symbol: &SymbolKey) -> f64 {
        if let Some(m) = &self.market {
            if let Some(state) = m.symbol_state.get(symbol) {
                return state.market_price;
            }
        }
        inner
            .market_prices
            .get(&symbol.symbol)
            .copied()
            .unwrap_or(100.0)
    }

    fn market_fill_price(&self, base_price: f64, action: OrderAction) -> f64 {
        let half = self.config.default_spread / 2.0;
        match action {
            OrderAction::Buy => base_price + half,
            OrderAction::Sell => base_price - half,
        }
    }

    /// Execute a fill on an order. Emits `ExecutionDetails`,
    /// `Commission`, and `StatusChanged` events per M-19. Supports
    /// partial-fill tranches.
    fn execute_fill(&self, inner: &mut Inner, ib_order_id: i32, shares: f64, price: f64) {
        let mut order = match inner.orders.remove(&ib_order_id) {
            Some(o) => o,
            None => return,
        };

        let remaining = order.quantity - order.filled_qty;
        if remaining <= 0.0 {
            inner.orders.insert(ib_order_id, order);
            return;
        }
        let shares = shares.min(remaining);

        let tranches = if self.config.partial_fill_threshold > 0.0
            && shares > self.config.partial_fill_threshold
            && self.config.partial_fill_tranches > 1
        {
            self.config.partial_fill_tranches
        } else {
            1
        };

        let tranche_size = shares / tranches as f64;
        let mut shares_remaining_to_fill = shares;

        for i in 0..tranches {
            let is_last = i == tranches - 1;
            let fill_shares = if is_last {
                shares_remaining_to_fill
            } else {
                tranche_size.min(shares_remaining_to_fill)
            };
            shares_remaining_to_fill -= fill_shares;

            let commission = (fill_shares * self.config.commission_per_share).max(0.0);
            let exec_id = self.next_exec_id();

            let prev_filled = order.filled_qty;
            order.filled_qty += fill_shares;
            order.avg_fill_price = if prev_filled == 0.0 {
                price
            } else {
                (order.avg_fill_price * prev_filled + price * fill_shares) / order.filled_qty
            };

            let order_remaining = order.quantity - order.filled_qty;

            // 1. ExecutionDetails (M-19).
            let _ = self.order_tx.send(OrderEvent::ExecutionDetails {
                ib_order_id: order.ib_order_id,
                exec_id: exec_id.clone(),
                shares: fill_shares,
                price,
            });

            // 2. Commission (M-19) — correlated by exec_id.
            let realized = self.apply_position_on_fill(
                inner,
                &order.symbol.symbol,
                order.action,
                fill_shares,
                price,
                commission,
            );
            let _ = self.order_tx.send(OrderEvent::Commission {
                exec_id,
                commission,
                realized_pnl: realized,
                currency: "USD".to_string(),
            });

            // 3. StatusChanged (Partial or Filled).
            let new_status = if order_remaining <= 0.0 && is_last {
                OrderStatus::Filled
            } else if order.filled_qty > 0.0 {
                OrderStatus::PartiallyFilled
            } else {
                OrderStatus::Submitted
            };
            let _ = self.order_tx.send(OrderEvent::StatusChanged {
                ib_order_id: order.ib_order_id,
                status: new_status,
                filled: order.filled_qty,
                remaining: order_remaining.max(0.0),
                avg_fill_price: order.avg_fill_price,
            });

            // Position fan-out (parity with legacy path).
            let pos_snapshot = inner
                .positions
                .get(&order.symbol.symbol)
                .map(|p| (p.quantity, p.avg_cost))
                .unwrap_or((0.0, 0.0));
            let _ = self.position_tx.send(PositionUpdate {
                account: "SIM".to_string(),
                symbol: order.symbol.symbol.clone(),
                con_id: order.symbol.contract_id,
                quantity: pos_snapshot.0,
                avg_cost: pos_snapshot.1,
            });
        }

        if order.quantity - order.filled_qty <= 0.0 {
            order.status = SimOrderStatus::Filled;
        } else if order.filled_qty > 0.0 {
            order.status = SimOrderStatus::PartiallyFilled;
        }

        inner.orders.insert(ib_order_id, order);
    }

    /// Apply a fill to positions and cash; return realized PnL if the
    /// fill reduces or flips the position (Commission payload).
    fn apply_position_on_fill(
        &self,
        inner: &mut Inner,
        symbol: &str,
        action: OrderAction,
        shares: f64,
        price: f64,
        commission: f64,
    ) -> Option<f64> {
        let notional = shares * price;
        match action {
            OrderAction::Buy => inner.cash -= notional + commission,
            OrderAction::Sell => inner.cash += notional - commission,
        }
        let signed_shares = match action {
            OrderAction::Buy => shares,
            OrderAction::Sell => -shares,
        };
        let pos = inner
            .positions
            .entry(symbol.to_string())
            .or_insert(PositionState {
                symbol: symbol.to_string(),
                quantity: 0.0,
                avg_cost: 0.0,
            });

        let mut realized = None;
        if pos.quantity.signum() == signed_shares.signum() || pos.quantity == 0.0 {
            let total_cost = pos.avg_cost * pos.quantity.abs() + price * shares;
            pos.quantity += signed_shares;
            if pos.quantity.abs() > 0.0 {
                pos.avg_cost = total_cost / pos.quantity.abs();
            }
        } else {
            let close_qty = shares.min(pos.quantity.abs());
            let r = (price - pos.avg_cost) * close_qty * pos.quantity.signum();
            let close_ratio = close_qty / shares;
            let close_commission = close_ratio * commission;
            let net = r - close_commission;
            inner.realized_pnl += net;
            realized = Some(net);

            let prev_quantity = pos.quantity;
            pos.quantity += signed_shares;
            if pos.quantity.abs() < f64::EPSILON {
                pos.quantity = 0.0;
                pos.avg_cost = 0.0;
            } else if pos.quantity.signum() != prev_quantity.signum() {
                pos.avg_cost = price;
            }
        }
        realized
    }

    fn fill_market_order(&self, inner: &mut Inner, ib_order_id: i32) {
        let (symbol, action) = match inner.orders.get(&ib_order_id) {
            Some(o) => (o.symbol.clone(), o.action),
            None => return,
        };
        let base = self.get_market_price(inner, &symbol);
        let price = self.market_fill_price(base, action);
        let qty = match inner.orders.get(&ib_order_id) {
            Some(o) => o.quantity,
            None => return,
        };
        self.execute_fill(inner, ib_order_id, qty, price);
    }

    fn cancel_order_inner(
        inner: &mut Inner,
        ib_order_id: i32,
        order_tx: &broadcast::Sender<OrderEvent>,
    ) {
        if let Some(order) = inner.orders.get_mut(&ib_order_id) {
            if order.status.is_terminal() {
                return;
            }
            order.status = SimOrderStatus::Cancelled;
            let _ = order_tx.send(OrderEvent::StatusChanged {
                ib_order_id,
                status: OrderStatus::Cancelled,
                filled: order.filled_qty,
                remaining: order.quantity - order.filled_qty,
                avg_fill_price: order.avg_fill_price,
            });
            let _ = order_tx.send(OrderEvent::Cancelled { ib_order_id });
        }
        // Flush any pending cancel-stream listeners.
        if let Some(streams) = inner.cancel_streams.remove(&ib_order_id) {
            for tx in streams {
                let _ = tx.try_send(CancelOrderEvent::Cancelled { ib_order_id });
            }
        }
    }

    fn check_oca_inner(
        inner: &mut Inner,
        filled_order_id: i32,
        order_tx: &broadcast::Sender<OrderEvent>,
    ) {
        let parent_id = match inner.orders.get(&filled_order_id) {
            Some(o) => o.parent_id,
            None => return,
        };
        let parent_id = match parent_id {
            Some(p) => p,
            None => return,
        };
        let siblings = match inner.bracket_links.get(&parent_id) {
            Some(c) => c.clone(),
            None => return,
        };
        for s in siblings {
            if s != filled_order_id {
                Self::cancel_order_inner(inner, s, order_tx);
            }
        }
    }

    fn activate_bracket(&self, inner: &mut Inner, parent_id: i32) {
        // Activate parent.
        match inner.orders.get_mut(&parent_id) {
            Some(p) if p.status == SimOrderStatus::Held => {
                p.status = SimOrderStatus::Working;
                let qty = p.quantity;
                let _ = self.order_tx.send(OrderEvent::Submitted {
                    ib_order_id: parent_id,
                });
                let _ = self.order_tx.send(OrderEvent::StatusChanged {
                    ib_order_id: parent_id,
                    status: OrderStatus::Submitted,
                    filled: 0.0,
                    remaining: qty,
                    avg_fill_price: 0.0,
                });
            }
            _ => return,
        }

        // If parent is MKT, fill immediately (instant mode).
        let parent_is_mkt = inner
            .orders
            .get(&parent_id)
            .map(|o| o.order_type == OrderType::Market)
            .unwrap_or(false);

        if parent_is_mkt && self.config.fill_timing == "instant" {
            self.fill_market_order(inner, parent_id);
        }

        // Activate children.
        let children = inner
            .bracket_links
            .get(&parent_id)
            .cloned()
            .unwrap_or_default();
        let parent_filled = inner
            .orders
            .get(&parent_id)
            .map(|o| o.status == SimOrderStatus::Filled)
            .unwrap_or(false);

        for child_id in &children {
            if let Some(child) = inner.orders.get_mut(child_id) {
                if child.status != SimOrderStatus::Held {
                    continue;
                }
                if parent_filled {
                    match child.order_type {
                        OrderType::Stop | OrderType::StopLimit => {
                            child.status = SimOrderStatus::Triggered;
                            let _ = self.order_tx.send(OrderEvent::StatusChanged {
                                ib_order_id: *child_id,
                                status: OrderStatus::PreSubmitted,
                                filled: 0.0,
                                remaining: child.quantity,
                                avg_fill_price: 0.0,
                            });
                        }
                        _ => {
                            child.status = SimOrderStatus::Working;
                            let _ = self.order_tx.send(OrderEvent::Submitted {
                                ib_order_id: *child_id,
                            });
                            let _ = self.order_tx.send(OrderEvent::StatusChanged {
                                ib_order_id: *child_id,
                                status: OrderStatus::Submitted,
                                filled: 0.0,
                                remaining: child.quantity,
                                avg_fill_price: 0.0,
                            });
                        }
                    }
                } else {
                    child.status = SimOrderStatus::Triggered;
                    let _ = self.order_tx.send(OrderEvent::StatusChanged {
                        ib_order_id: *child_id,
                        status: OrderStatus::PreSubmitted,
                        filled: 0.0,
                        remaining: child.quantity,
                        avg_fill_price: 0.0,
                    });
                }
            }
        }
    }

    fn check_limit_fills(&self, inner: &mut Inner, symbol: &str, new_price: f64) {
        let to_fill: Vec<(i32, f64)> = inner
            .orders
            .values()
            .filter(|o| {
                o.symbol.symbol == symbol
                    && o.status == SimOrderStatus::Working
                    && o.order_type == OrderType::Limit
            })
            .filter_map(|o| {
                let limit = o.limit_price?;
                let should = match o.action {
                    OrderAction::Buy => new_price <= limit,
                    OrderAction::Sell => new_price >= limit,
                };
                if should {
                    let fill = match o.action {
                        OrderAction::Buy => limit.min(new_price),
                        OrderAction::Sell => limit.max(new_price),
                    };
                    Some((o.ib_order_id, fill))
                } else {
                    None
                }
            })
            .collect();
        for (id, fill) in &to_fill {
            let remaining = match inner.orders.get(id) {
                Some(o) => o.quantity - o.filled_qty,
                None => continue,
            };
            self.execute_fill(inner, *id, remaining, *fill);
        }
        for (id, _) in to_fill {
            Self::check_oca_inner(inner, id, &self.order_tx);
        }
    }

    fn check_stop_triggers(&self, inner: &mut Inner, symbol: &str, new_price: f64) {
        let triggered: Vec<i32> = inner
            .orders
            .values()
            .filter(|o| {
                o.symbol.symbol == symbol
                    && o.status == SimOrderStatus::Triggered
                    && (o.order_type == OrderType::Stop || o.order_type == OrderType::StopLimit)
            })
            .filter_map(|o| {
                let stop = o.stop_price?;
                let hit = match o.action {
                    OrderAction::Sell => new_price <= stop,
                    OrderAction::Buy => new_price >= stop,
                };
                if hit {
                    Some(o.ib_order_id)
                } else {
                    None
                }
            })
            .collect();
        let mut filled_ids = Vec::new();
        for id in triggered {
            let (order_type, quantity, filled_qty, limit_price, action) =
                match inner.orders.get(&id) {
                    Some(o) => (
                        o.order_type,
                        o.quantity,
                        o.filled_qty,
                        o.limit_price,
                        o.action,
                    ),
                    None => continue,
                };
            if let Some(o) = inner.orders.get_mut(&id) {
                o.status = SimOrderStatus::Working;
            }
            let _ = self.order_tx.send(OrderEvent::StatusChanged {
                ib_order_id: id,
                status: OrderStatus::Submitted,
                filled: filled_qty,
                remaining: quantity - filled_qty,
                avg_fill_price: 0.0,
            });
            let remaining = quantity - filled_qty;
            if order_type == OrderType::Stop {
                let fill = self.market_fill_price(new_price, action);
                self.execute_fill(inner, id, remaining, fill);
                filled_ids.push(id);
            } else {
                let should = match (action, limit_price) {
                    (OrderAction::Buy, Some(limit)) => new_price <= limit,
                    (OrderAction::Sell, Some(limit)) => new_price >= limit,
                    _ => false,
                };
                if let (true, Some(fill)) = (should, limit_price) {
                    self.execute_fill(inner, id, remaining, fill);
                    filled_ids.push(id);
                }
            }
        }
        for id in filled_ids {
            Self::check_oca_inner(inner, id, &self.order_tx);
        }
    }

    /// Set the market price for a symbol, driving limit and stop
    /// triggers. Parity with
    /// [`TestBroker::set_market_price`](crate::test_broker::TestBroker::set_market_price).
    pub fn set_market_price(&self, symbol: &str, price: f64) {
        let mut inner = self.inner.lock();
        inner.market_prices.insert(symbol.to_string(), price);
        self.check_limit_fills(&mut inner, symbol, price);
        self.check_stop_triggers(&mut inner, symbol, price);
    }

    /// Current cash balance.
    pub fn cash_balance(&self) -> f64 {
        self.inner.lock().cash
    }

    /// Cumulative realised P&L.
    pub fn realized_pnl(&self) -> f64 {
        self.inner.lock().realized_pnl
    }
}

#[async_trait]
impl OrderClient for SimOrderClient {
    async fn next_order_id(&self) -> Result<i32, OrderError> {
        // Block until the paired market-data backend (if any) reaches
        // `Ready`. The first caller after construction may have to wait
        // ~100 ms (farm-up delay); subsequent callers fall through
        // instantly.
        let rx_opt = self.conn_state_rx.lock().as_ref().cloned();
        if let Some(mut rx) = rx_opt {
            loop {
                if matches!(*rx.borrow(), ConnectionState::Ready) {
                    break;
                }
                if rx.changed().await.is_err() {
                    return Err(OrderError::Disconnected);
                }
            }
        }
        Ok(self.next_id.fetch_add(1, Ordering::SeqCst))
    }

    async fn place_order(&self, spec: OrderSpec) -> Result<PlaceOrderResult, OrderError> {
        let mut inner = self.inner.lock();
        let ib_order_id = spec.ib_order_id;

        // Modify path: order already exists.
        if let Some(existing) = inner.orders.get_mut(&ib_order_id) {
            if let Some(l) = spec.limit_price {
                existing.limit_price = Some(l);
            }
            if let Some(s) = spec.stop_price {
                existing.stop_price = Some(s);
            }
            let status = match existing.status {
                SimOrderStatus::Working => OrderStatus::Submitted,
                SimOrderStatus::Triggered => OrderStatus::PreSubmitted,
                _ => OrderStatus::Submitted,
            };
            let filled = existing.filled_qty;
            let remaining = existing.quantity - existing.filled_qty;
            let avg = existing.avg_fill_price;
            let _ = self.order_tx.send(OrderEvent::StatusChanged {
                ib_order_id,
                status,
                filled,
                remaining,
                avg_fill_price: avg,
            });
            return Ok(PlaceOrderResult { ib_order_id });
        }

        inner.order_count += 1;
        let should_reject = if self.config.rejection_rate > 0.0 {
            let n = (1.0 / self.config.rejection_rate).round() as u64;
            n > 0 && inner.order_count.is_multiple_of(n)
        } else {
            false
        };

        let sim_order = SimulatedOrder {
            ib_order_id,
            symbol: spec.symbol.clone(),
            action: spec.action,
            order_type: spec.order_type,
            quantity: spec.quantity,
            limit_price: spec.limit_price,
            stop_price: spec.stop_price,
            parent_id: spec.parent_id,
            transmit: spec.transmit,
            status: if should_reject {
                SimOrderStatus::Rejected
            } else {
                SimOrderStatus::Held
            },
            filled_qty: 0.0,
            avg_fill_price: 0.0,
            tif: spec.tif,
            outside_rth: spec.outside_rth,
        };
        inner.orders.insert(ib_order_id, sim_order);

        if should_reject {
            let _ = self.order_tx.send(OrderEvent::Rejected {
                ib_order_id,
                reason: "Order rejected by sim (error injection)".to_string(),
            });
            return Ok(PlaceOrderResult { ib_order_id });
        }

        if let Some(pid) = spec.parent_id {
            inner
                .bracket_links
                .entry(pid)
                .or_default()
                .push(ib_order_id);
        }

        if spec.transmit {
            let activate_parent = spec.parent_id.unwrap_or(ib_order_id);
            self.activate_bracket(&mut inner, activate_parent);
        }

        Ok(PlaceOrderResult { ib_order_id })
    }

    async fn cancel_order(
        &self,
        ib_order_id: i32,
        _manual_cancel_time: Option<DateTime<Utc>>,
    ) -> Result<CancelOrderStream, OrderError> {
        let (tx, rx) = mpsc::channel::<CancelOrderEvent>(8);

        let mut inner = self.inner.lock();
        match inner.orders.get(&ib_order_id) {
            None => {
                // IB returns an error for unknown orders.
                let _ = tx.try_send(CancelOrderEvent::Error {
                    ib_order_id,
                    code: ErrorCode::OrderCancelNotFound,
                    message: format!("order {ib_order_id} not found"),
                });
                let cancel = Box::new(|| {});
                return Ok(CancelOrderStream::new(rx, cancel));
            }
            Some(o) if o.status.is_terminal() => {
                // IB silently accepts cancel for filled/cancelled
                // orders. Emit a synthetic Cancelled for caller parity.
                let _ = tx.try_send(CancelOrderEvent::Cancelled { ib_order_id });
                drop(inner);
                let cancel = Box::new(|| {});
                return Ok(CancelOrderStream::new(rx, cancel));
            }
            _ => {}
        }

        // Ack the cancel.
        let _ = tx.try_send(CancelOrderEvent::Submitted { ib_order_id });
        inner
            .cancel_streams
            .entry(ib_order_id)
            .or_default()
            .push_back(tx.clone());

        Self::cancel_order_inner(&mut inner, ib_order_id, &self.order_tx);

        if let Some(children) = inner.bracket_links.get(&ib_order_id).cloned() {
            for c in children {
                Self::cancel_order_inner(&mut inner, c, &self.order_tx);
            }
        }

        // Keep the stream alive so late Cancelled events flow through;
        // sim emits the Cancelled directly in cancel_order_inner.
        let cancel = Box::new(|| {});
        Ok(CancelOrderStream::new(rx, cancel))
    }

    async fn modify_order(&self, ib_order_id: i32, spec: OrderModify) -> Result<(), OrderError> {
        let mut inner = self.inner.lock();
        let order = inner
            .orders
            .get_mut(&ib_order_id)
            .ok_or(OrderError::NotFound(ib_order_id))?;
        if let Some(q) = spec.quantity {
            order.quantity = q;
        }
        if let Some(l) = spec.limit_price {
            order.limit_price = Some(l);
        }
        if let Some(s) = spec.stop_price {
            order.stop_price = Some(s);
        }
        if let Some(tif) = spec.tif {
            order.tif = tif;
        }
        if let Some(outside) = spec.outside_rth {
            order.outside_rth = outside;
        }
        Ok(())
    }

    async fn open_orders(&self) -> Result<Vec<OpenOrder>, OrderError> {
        let inner = self.inner.lock();
        let out = inner
            .orders
            .values()
            .filter(|o| !o.status.is_terminal())
            .map(|o| OpenOrder {
                ib_order_id: o.ib_order_id,
                perm_id: None,
                symbol: o.symbol.clone(),
                action: o.action,
                order_type: o.order_type,
                quantity: o.quantity,
                limit_price: o.limit_price,
                stop_price: o.stop_price,
                tif: o.tif,
                status: map_status(&o.status),
                filled: o.filled_qty,
                remaining: o.quantity - o.filled_qty,
                avg_fill_price: if o.avg_fill_price > 0.0 {
                    Some(o.avg_fill_price)
                } else {
                    None
                },
                parent_id: o.parent_id,
            })
            .collect();
        Ok(out)
    }

    async fn completed_orders(&self) -> Result<Vec<CompletedOrder>, OrderError> {
        let inner = self.inner.lock();
        let out = inner
            .orders
            .values()
            .filter(|o| o.status.is_terminal())
            .map(|o| CompletedOrder {
                ib_order_id: o.ib_order_id,
                perm_id: None,
                symbol: o.symbol.clone(),
                action: o.action,
                order_type: o.order_type,
                quantity: o.quantity,
                filled: o.filled_qty,
                avg_fill_price: if o.avg_fill_price > 0.0 {
                    Some(o.avg_fill_price)
                } else {
                    None
                },
                status: map_status(&o.status),
                completed_at: Some(Utc::now()),
            })
            .collect();
        Ok(out)
    }

    fn order_events(&self) -> broadcast::Receiver<OrderEvent> {
        self.order_tx.subscribe()
    }

    fn position_events(&self) -> broadcast::Receiver<PositionUpdate> {
        self.position_tx.subscribe()
    }

    fn account_events(&self) -> broadcast::Receiver<AccountEvent> {
        self.account_tx.subscribe()
    }

    fn name(&self) -> &str {
        "sim"
    }
}

fn map_status(s: &SimOrderStatus) -> OrderStatus {
    match s {
        SimOrderStatus::Held => OrderStatus::PreSubmitted,
        SimOrderStatus::Working => OrderStatus::Submitted,
        SimOrderStatus::Triggered => OrderStatus::PreSubmitted,
        SimOrderStatus::PartiallyFilled => OrderStatus::PartiallyFilled,
        SimOrderStatus::Filled => OrderStatus::Filled,
        SimOrderStatus::Cancelled => OrderStatus::Cancelled,
        SimOrderStatus::Rejected => OrderStatus::Rejected,
    }
}
