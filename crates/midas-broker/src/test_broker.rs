//! Full-simulation test broker for integration and E2E testing.
//!
//! Unlike [`TestBrokerClient`](crate::client::TestBrokerClient) which simply
//! records orders, `TestBroker` simulates the IB order lifecycle: bracket
//! activation, market-order fills, OCA cancellation, and callback generation.
//!
//! # Example
//!
//! ```ignore
//! use midas_broker::test_broker::{TestBroker, TestBrokerConfig};
//! use midas_broker::client::BrokerClient;
//!
//! let broker = TestBroker::new(TestBrokerConfig::default());
//! let id = broker.next_order_id();
//! broker.place_order(id, "AAPL", "BUY", "MKT", 100.0, None, None, None, true, "DAY", false).unwrap();
//! let cbs = broker.poll_callbacks();
//! // cbs contains: Submitted, Execution, Filled
//! ```

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use parking_lot::Mutex;
use std::time::Instant;

use serde::Deserialize;

use crate::client::{BrokerCallback, BrokerClient, CancelOrderResult, PlaceOrderResult};
use crate::testdata::TestDataProvider;

// ══════════════════════════════════════════════════════════════════════════════
// Configuration
// ══════════════════════════════════════════════════════════════════════════════

fn default_fill_timing() -> String {
    "instant".to_string()
}

fn default_fill_price_model() -> String {
    "market".to_string()
}

fn default_initial_cash() -> f64 {
    100_000.0
}

fn default_commission_per_share() -> f64 {
    0.005
}

fn default_auto_connect() -> bool {
    true
}

fn default_partial_fill_tranches() -> u32 {
    1
}

fn default_default_spread() -> f64 {
    0.01
}

/// Configuration for the test broker simulation engine.
#[derive(Debug, Clone, Deserialize)]
pub struct TestBrokerConfig {
    /// Fill timing mode: "instant" (default), "delayed", or "price_triggered".
    #[serde(default = "default_fill_timing")]
    pub fill_timing: String,

    /// Fill price model: "exact", "slippage", or "market" (default).
    #[serde(default = "default_fill_price_model")]
    pub fill_price_model: String,

    /// Maximum slippage in basis points (only for "slippage" model).
    #[serde(default)]
    pub max_slippage_bps: f64,

    /// Starting cash balance.
    #[serde(default = "default_initial_cash")]
    pub initial_cash: f64,

    /// Commission per share (USD).
    #[serde(default = "default_commission_per_share")]
    pub commission_per_share: f64,

    /// Auto-connect on engine start.
    #[serde(default = "default_auto_connect")]
    pub auto_connect: bool,

    // ── Phase 3: Partial fills ──────────────────────────────────────────

    /// Minimum order quantity to trigger partial fills (0.0 = disabled).
    #[serde(default)]
    pub partial_fill_threshold: f64,

    /// Number of tranches to split a large fill into (default 1 = single fill).
    #[serde(default = "default_partial_fill_tranches")]
    pub partial_fill_tranches: u32,

    // ── Phase 4: Tick generation ─────────────────────────────────────────

    /// Interval in milliseconds between auto-tick generation (0 = no auto ticks).
    #[serde(default)]
    pub tick_interval_ms: u64,

    /// Default bid-ask spread for generated ticks (in dollars, default $0.01).
    #[serde(default = "default_default_spread")]
    pub default_spread: f64,

    // ── Phase 6: Error injection ────────────────────────────────────────

    /// Fraction of orders to reject deterministically (0.0 = none).
    /// Uses order count modulo: every Nth order is rejected where N = 1/rate.
    #[serde(default)]
    pub rejection_rate: f64,
}

impl Default for TestBrokerConfig {
    fn default() -> Self {
        Self {
            fill_timing: default_fill_timing(),
            fill_price_model: default_fill_price_model(),
            max_slippage_bps: 0.0,
            initial_cash: default_initial_cash(),
            commission_per_share: default_commission_per_share(),
            auto_connect: default_auto_connect(),
            partial_fill_threshold: 0.0,
            partial_fill_tranches: 1,
            tick_interval_ms: 0,
            default_spread: default_default_spread(),
            rejection_rate: 0.0,
        }
    }
}

// ══════════════════════════════════════════════════════════════════════════════
// SimulatedOrder / SimOrderStatus
// ══════════════════════════════════════════════════════════════════════════════

/// Status of a simulated order in the test broker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SimOrderStatus {
    /// Queued but not transmitted (transmit=false, waiting for bracket completion).
    Held,
    /// Transmitted, working at simulated exchange.
    Working,
    /// Stop/conditional order waiting for trigger (maps to IB "PreSubmitted").
    Triggered,
    /// Some shares filled, more remaining.
    PartiallyFilled,
    /// Completely filled (terminal).
    Filled,
    /// Cancelled (terminal).
    Cancelled,
    /// Rejected by the broker (terminal).
    Rejected,
}

impl SimOrderStatus {
    /// Whether this status is terminal (no further transitions).
    fn is_terminal(&self) -> bool {
        matches!(
            self,
            SimOrderStatus::Filled | SimOrderStatus::Cancelled | SimOrderStatus::Rejected
        )
    }
}

/// A simulated order tracked by the test broker.
#[derive(Debug, Clone)]
pub struct SimulatedOrder {
    pub ib_order_id: i32,
    pub symbol: String,
    pub action: String,
    pub order_type: String,
    pub quantity: f64,
    pub limit_price: Option<f64>,
    pub stop_price: Option<f64>,
    pub parent_id: Option<i32>,
    pub transmit: bool,
    pub status: SimOrderStatus,
    pub filled_qty: f64,
    pub avg_fill_price: f64,
    /// Time-in-force qualifier (e.g. "DAY", "GTC").
    pub tif: String,
    /// Whether order can execute outside regular trading hours.
    pub outside_rth: bool,
}

// ══════════════════════════════════════════════════════════════════════════════
// PositionState (Phase 5)
// ══════════════════════════════════════════════════════════════════════════════

/// Tracked position for a single symbol.
#[derive(Debug, Clone)]
struct PositionState {
    symbol: String,
    /// Positive = long, negative = short.
    quantity: f64,
    /// Weighted average entry price.
    avg_cost: f64,
}

// ══════════════════════════════════════════════════════════════════════════════
// TestBrokerInner
// ══════════════════════════════════════════════════════════════════════════════

struct TestBrokerInner {
    /// All orders by IB order ID.
    orders: HashMap<i32, SimulatedOrder>,
    /// Parent IB order ID -> child IB order IDs.
    bracket_links: HashMap<i32, Vec<i32>>,
    /// Last known market price per symbol.
    market_prices: HashMap<String, f64>,
    /// Pending callbacks to be polled by the engine.
    callbacks: VecDeque<BrokerCallback>,
    /// Test data provider for price seeding.
    data_provider: TestDataProvider,

    // ── Phase 5: Position tracking ──────────────────────────────────────

    /// Current positions keyed by symbol.
    positions: HashMap<String, PositionState>,
    /// Available cash (starts at config.initial_cash).
    cash: f64,
    /// Cumulative realized P&L from closed positions.
    realized_pnl: f64,

    // ── Phase 4: Tick generation ─────────────────────────────────────────

    /// Symbols with active market data subscriptions.
    subscriptions: HashSet<String>,
    /// Last time auto-ticks were generated (for interval gating).
    last_tick_time: Instant,

    // ── Phase 6: Error injection ────────────────────────────────────────

    /// Running count of orders placed (used for deterministic rejection).
    order_count: u64,
}

// ══════════════════════════════════════════════════════════════════════════════
// TestBroker
// ══════════════════════════════════════════════════════════════════════════════

/// Full-simulation broker for integration and E2E testing.
///
/// Simulates the IB order lifecycle: bracket activation, market-order fills,
/// OCA cancellation, and callback generation. All state is behind a single
/// `Mutex<TestBrokerInner>` to eliminate deadlock risk.
pub struct TestBroker {
    config: TestBrokerConfig,
    inner: Mutex<TestBrokerInner>,
    next_id: AtomicI32,
    connected: AtomicBool,
    exec_counter: AtomicI32,
}

impl TestBroker {
    /// Create a new test broker with the given configuration.
    pub fn new(config: TestBrokerConfig) -> Self {
        let auto_connect = config.auto_connect;
        let initial_cash = config.initial_cash;
        Self {
            config,
            inner: Mutex::new(TestBrokerInner {
                orders: HashMap::new(),
                bracket_links: HashMap::new(),
                market_prices: HashMap::new(),
                callbacks: VecDeque::new(),
                data_provider: TestDataProvider::new(),
                positions: HashMap::new(),
                cash: initial_cash,
                realized_pnl: 0.0,
                subscriptions: HashSet::new(),
                last_tick_time: Instant::now(),
                order_count: 0,
            }),
            next_id: AtomicI32::new(1000),
            connected: AtomicBool::new(auto_connect),
            exec_counter: AtomicI32::new(1),
        }
    }

    /// Generate the next unique execution ID.
    fn next_exec_id(&self) -> String {
        let n = self.exec_counter.fetch_add(1, Ordering::SeqCst);
        format!("TEST.{n}")
    }

    /// Get or seed a market price for a symbol from TestDataProvider.
    /// Must be called with the lock already held.
    fn get_or_seed_price_inner(inner: &mut TestBrokerInner, symbol: &str) -> f64 {
        if let Some(&price) = inner.market_prices.get(symbol) {
            return price;
        }
        let bars = inner.data_provider.daily_bars(symbol);
        let price = bars.last().map(|b| b.close).unwrap_or(100.0);
        inner.market_prices.insert(symbol.to_string(), price);
        price
    }

    /// Execute a fill on an order: update fill state, push Execution + OrderStatus callbacks.
    ///
    /// Supports partial fill tranches (Phase 3): when the order quantity exceeds
    /// `partial_fill_threshold` and `partial_fill_tranches > 1`, the fill is
    /// split into multiple Execution callbacks.
    ///
    /// Updates position and account state on each tranche (Phase 5).
    ///
    /// To avoid simultaneous mutable borrows of `inner.orders` and `inner.callbacks`,
    /// this method temporarily removes the order from the map, mutates it, then
    /// reinserts it.
    fn execute_fill(
        &self,
        inner: &mut TestBrokerInner,
        ib_order_id: i32,
        shares: f64,
        price: f64,
    ) {
        let mut order = match inner.orders.remove(&ib_order_id) {
            Some(o) => o,
            None => return,
        };

        // Overfill guard: clamp shares to remaining quantity
        let remaining = order.quantity - order.filled_qty;
        if remaining <= 0.0 {
            // Already fully filled; reinsert and bail out
            inner.orders.insert(ib_order_id, order);
            return;
        }
        let shares = shares.min(remaining);

        // Determine how many tranches to split the fill into (Phase 3).
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
            // Last tranche gets any remainder to avoid floating-point drift.
            let fill_shares = if is_last {
                shares_remaining_to_fill
            } else {
                tranche_size.min(shares_remaining_to_fill)
            };
            shares_remaining_to_fill -= fill_shares;

            let commission = (fill_shares * self.config.commission_per_share).max(0.0);
            let exec_id = self.next_exec_id();

            // Update order fill state
            let prev_filled = order.filled_qty;
            order.filled_qty += fill_shares;
            order.avg_fill_price = if prev_filled == 0.0 {
                price
            } else {
                (order.avg_fill_price * prev_filled + price * fill_shares) / order.filled_qty
            };

            let order_remaining = order.quantity - order.filled_qty;

            // Execution callback
            inner.callbacks.push_back(BrokerCallback::Execution {
                ib_order_id: order.ib_order_id,
                exec_id,
                shares: fill_shares,
                price,
                commission,
                side: order.action.clone(),
            });

            // OrderStatus: "Filled" when complete, "PartiallyFilled" when partially done
            let status_str = if order_remaining <= 0.0 && is_last {
                "Filled"
            } else if order.filled_qty > 0.0 {
                "PartiallyFilled"
            } else {
                "Submitted"
            };
            inner.callbacks.push_back(BrokerCallback::OrderStatus {
                ib_order_id: order.ib_order_id,
                status: status_str.to_string(),
                filled: order.filled_qty,
                remaining: order_remaining.max(0.0),
                avg_fill_price: order.avg_fill_price,
            });

            // Phase 5: Update position on each tranche
            Self::update_position_on_fill(
                inner,
                &order.symbol,
                &order.action,
                fill_shares,
                price,
                commission,
            );
        }

        if order.quantity - order.filled_qty <= 0.0 {
            order.status = SimOrderStatus::Filled;
        } else if order.filled_qty > 0.0 {
            order.status = SimOrderStatus::PartiallyFilled;
        }

        // Reinsert the order
        inner.orders.insert(ib_order_id, order);
    }

    /// Update position and account state after a fill. Called with lock held.
    fn update_position_on_fill(
        inner: &mut TestBrokerInner,
        symbol: &str,
        action: &str,
        shares: f64,
        price: f64,
        commission: f64,
    ) {
        let notional = shares * price;

        // Update cash: BUY decreases, SELL increases, minus commission
        match action {
            "BUY" => inner.cash -= notional + commission,
            "SELL" => inner.cash += notional - commission,
            _ => return,
        }

        let signed_shares: f64 = match action {
            "BUY" => shares,
            "SELL" => -shares,
            _ => return,
        };

        let pos = inner.positions.entry(symbol.to_string()).or_insert(PositionState {
            symbol: symbol.to_string(),
            quantity: 0.0,
            avg_cost: 0.0,
        });

        if pos.quantity.signum() == signed_shares.signum() || pos.quantity == 0.0 {
            // Adding to position (or opening new): weighted average cost
            let total_cost = pos.avg_cost * pos.quantity.abs() + price * shares;
            pos.quantity += signed_shares;
            if pos.quantity.abs() > 0.0 {
                pos.avg_cost = total_cost / pos.quantity.abs();
            }
        } else {
            // Reducing or flipping position: track realized P&L (net of closing commission)
            let close_qty = shares.min(pos.quantity.abs());
            let realized = (price - pos.avg_cost) * close_qty * pos.quantity.signum();
            let close_ratio = close_qty / shares;
            let close_commission = close_ratio * commission;
            inner.realized_pnl += realized - close_commission;

            let prev_quantity = pos.quantity;
            pos.quantity += signed_shares;

            if pos.quantity.abs() < f64::EPSILON {
                // Flat
                pos.quantity = 0.0;
                pos.avg_cost = 0.0;
            } else if pos.quantity.signum() != prev_quantity.signum() {
                // Flipped direction: avg_cost becomes the fill price
                pos.avg_cost = price;
            }
            // If still same direction (partial close), avg_cost stays unchanged
        }
    }

    /// Compute the fill price for a market order, applying the configured
    /// bid-ask spread: BUY fills at the ask (base + half spread), SELL fills
    /// at the bid (base - half spread).
    fn market_fill_price(&self, base_price: f64, action: &str) -> f64 {
        let half_spread = self.config.default_spread / 2.0;
        if action == "BUY" {
            base_price + half_spread
        } else {
            base_price - half_spread
        }
    }

    /// Fill a market order immediately. Must be called with the lock already held.
    fn fill_market_order_inner(
        &self,
        inner: &mut TestBrokerInner,
        ib_order_id: i32,
    ) {
        let (symbol, action) = match inner.orders.get(&ib_order_id) {
            Some(o) => (o.symbol.clone(), o.action.clone()),
            None => return,
        };
        let base_price = Self::get_or_seed_price_inner(inner, &symbol);
        let fill_price = self.market_fill_price(base_price, &action);
        let quantity = match inner.orders.get(&ib_order_id) {
            Some(o) => o.quantity,
            None => return,
        };

        self.execute_fill(inner, ib_order_id, quantity, fill_price);
    }

    /// Cancel a single order. Must be called with the lock already held.
    fn cancel_order_inner(inner: &mut TestBrokerInner, ib_order_id: i32) {
        if let Some(order) = inner.orders.get_mut(&ib_order_id) {
            if order.status.is_terminal() {
                return;
            }
            order.status = SimOrderStatus::Cancelled;
            inner.callbacks.push_back(BrokerCallback::OrderStatus {
                ib_order_id,
                status: "Cancelled".to_string(),
                filled: order.filled_qty,
                remaining: order.quantity - order.filled_qty,
                avg_fill_price: order.avg_fill_price,
            });
        }
    }

    /// Check OCA: when a bracket child fills, cancel its siblings.
    /// Must be called with the lock already held.
    fn check_oca_inner(inner: &mut TestBrokerInner, filled_order_id: i32) {
        // Find the parent of the filled order
        let parent_id = match inner.orders.get(&filled_order_id) {
            Some(order) => order.parent_id,
            None => return,
        };

        let parent_id = match parent_id {
            Some(pid) => pid,
            None => return, // not a child order, no OCA
        };

        // Get sibling IDs from bracket_links
        let siblings = match inner.bracket_links.get(&parent_id) {
            Some(children) => children.clone(),
            None => return,
        };

        // Cancel all siblings that are not the filled order
        for sibling_id in siblings {
            if sibling_id != filled_order_id {
                Self::cancel_order_inner(inner, sibling_id);
            }
        }
    }

    /// Activate a bracket when transmit=true arrives.
    /// Must be called with the lock already held.
    fn activate_bracket(&self, inner: &mut TestBrokerInner, parent_id: i32) {
        // 1. Activate parent
        match inner.orders.get_mut(&parent_id) {
            Some(parent) if parent.status == SimOrderStatus::Held => {
                parent.status = SimOrderStatus::Working;
                let qty = parent.quantity;
                // Push Submitted callback for parent
                inner.callbacks.push_back(BrokerCallback::OrderStatus {
                    ib_order_id: parent_id,
                    status: "Submitted".to_string(),
                    filled: 0.0,
                    remaining: qty,
                    avg_fill_price: 0.0,
                });
            }
            _ => return, // already activated or not found
        }

        // 2. If parent is MKT, fill it immediately (instant mode)
        let parent_is_mkt = inner.orders.get(&parent_id)
            .map(|o| o.order_type == "MKT")
            .unwrap_or(false);

        if parent_is_mkt && self.config.fill_timing == "instant" {
            self.fill_market_order_inner(inner, parent_id);
        }

        // 3. Activate children
        let children = inner.bracket_links.get(&parent_id).cloned().unwrap_or_default();
        let parent_filled = inner.orders.get(&parent_id)
            .map(|o| o.status == SimOrderStatus::Filled)
            .unwrap_or(false);

        for child_id in &children {
            if let Some(child) = inner.orders.get_mut(child_id) {
                if child.status != SimOrderStatus::Held {
                    continue;
                }

                if parent_filled {
                    // Parent already filled: activate children based on type
                    match child.order_type.as_str() {
                        "STP" | "STP LMT" => {
                            // Stop orders stay as PreSubmitted (waiting for trigger)
                            child.status = SimOrderStatus::Triggered;
                            inner.callbacks.push_back(BrokerCallback::OrderStatus {
                                ib_order_id: *child_id,
                                status: "PreSubmitted".to_string(),
                                filled: 0.0,
                                remaining: child.quantity,
                                avg_fill_price: 0.0,
                            });
                        }
                        _ => {
                            // LMT and other orders go to Submitted (working at exchange)
                            child.status = SimOrderStatus::Working;
                            inner.callbacks.push_back(BrokerCallback::OrderStatus {
                                ib_order_id: *child_id,
                                status: "Submitted".to_string(),
                                filled: 0.0,
                                remaining: child.quantity,
                                avg_fill_price: 0.0,
                            });
                        }
                    }
                } else {
                    // Parent not filled yet: children go to PreSubmitted
                    child.status = SimOrderStatus::Triggered;
                    inner.callbacks.push_back(BrokerCallback::OrderStatus {
                        ib_order_id: *child_id,
                        status: "PreSubmitted".to_string(),
                        filled: 0.0,
                        remaining: child.quantity,
                        avg_fill_price: 0.0,
                    });
                }
            }
        }
    }

    /// Manually trigger a fill for a specific order.
    /// Used in integration tests to drive the bracket lifecycle.
    pub fn simulate_fill(&self, ib_order_id: i32, price: f64, quantity: f64) {
        let mut inner = self.inner.lock();

        // Execute fill
        self.execute_fill(&mut inner, ib_order_id, quantity, price);

        // Check bracket OCA after fill
        Self::check_oca_inner(&mut inner, ib_order_id);
    }

    /// Check all working limit orders for a symbol and fill those whose price
    /// condition is met. Must be called with the lock already held.
    ///
    /// - BUY LMT: fills when `market_price <= limit_price`
    /// - SELL LMT: fills when `market_price >= limit_price`
    ///
    /// Fills execute at the *limit price* (not market), reflecting price improvement.
    fn check_limit_fills_inner(&self, inner: &mut TestBrokerInner, symbol: &str, new_price: f64) {
        // Collect order IDs that should fill (avoids borrow conflict during mutation).
        // Fill price is the better-of limit and market: BUY gets min, SELL gets max.
        let to_fill: Vec<(i32, f64)> = inner
            .orders
            .values()
            .filter(|o| {
                o.symbol == symbol
                    && o.status == SimOrderStatus::Working
                    && o.order_type == "LMT"
            })
            .filter_map(|o| {
                let limit = o.limit_price?;
                let should_fill = match o.action.as_str() {
                    "BUY" => new_price <= limit,
                    "SELL" => new_price >= limit,
                    _ => false,
                };
                if should_fill {
                    // Fill at the better price for the trader
                    let fill_price = match o.action.as_str() {
                        "BUY" => limit.min(new_price),   // BUY gets the lower (better) price
                        "SELL" => limit.max(new_price),   // SELL gets the higher (better) price
                        _ => limit,
                    };
                    Some((o.ib_order_id, fill_price))
                } else {
                    None
                }
            })
            .collect();

        for (ib_order_id, fill_price) in &to_fill {
            let remaining = match inner.orders.get(ib_order_id) {
                Some(o) => o.quantity - o.filled_qty,
                None => continue,
            };
            self.execute_fill(inner, *ib_order_id, remaining, *fill_price);
        }

        // OCA checks must happen after all fills so sibling state is up to date.
        for (ib_order_id, _) in to_fill {
            Self::check_oca_inner(inner, ib_order_id);
        }
    }

    /// Check all triggered (PreSubmitted) stop orders for a symbol and trigger
    /// those whose stop condition is met. Must be called with the lock already held.
    ///
    /// - SELL STP: triggers when `market_price <= stop_price`
    /// - BUY STP: triggers when `market_price >= stop_price`
    ///
    /// Plain STP orders: trigger -> Submitted callback -> fill at market price.
    /// STP LMT orders: trigger -> Submitted callback -> check limit; only fills
    /// if limit condition is also met (may remain unfilled if price gaps through).
    fn check_stop_triggers_inner(
        &self,
        inner: &mut TestBrokerInner,
        symbol: &str,
        new_price: f64,
    ) {
        // Phase 1: collect triggered order IDs.
        let triggered: Vec<i32> = inner
            .orders
            .values()
            .filter(|o| {
                o.symbol == symbol
                    && o.status == SimOrderStatus::Triggered
                    && (o.order_type == "STP" || o.order_type == "STP LMT")
            })
            .filter_map(|o| {
                let stop = o.stop_price?;
                let hit = match o.action.as_str() {
                    "SELL" => new_price <= stop,
                    "BUY" => new_price >= stop,
                    _ => false,
                };
                if hit {
                    Some(o.ib_order_id)
                } else {
                    None
                }
            })
            .collect();

        // Phase 2: process each triggered order.
        let mut filled_ids: Vec<i32> = Vec::new();

        for ib_order_id in triggered {
            let (order_type, quantity, filled_qty, limit_price, action) =
                match inner.orders.get(&ib_order_id) {
                    Some(o) => (
                        o.order_type.clone(),
                        o.quantity,
                        o.filled_qty,
                        o.limit_price,
                        o.action.clone(),
                    ),
                    None => continue,
                };

            // Transition: PreSubmitted -> Submitted (stop triggered, now a market/limit order).
            if let Some(order) = inner.orders.get_mut(&ib_order_id) {
                order.status = SimOrderStatus::Working;
            }
            inner.callbacks.push_back(BrokerCallback::OrderStatus {
                ib_order_id,
                status: "Submitted".to_string(),
                filled: filled_qty,
                remaining: quantity - filled_qty,
                avg_fill_price: 0.0,
            });

            let remaining = quantity - filled_qty;

            if order_type == "STP" {
                // Plain stop: fill immediately at market price (with spread).
                let fill_price = self.market_fill_price(new_price, &action);
                self.execute_fill(inner, ib_order_id, remaining, fill_price);
                filled_ids.push(ib_order_id);
            } else {
                // STP LMT: only fill if limit condition is also satisfied.
                let should_fill = match (action.as_str(), limit_price) {
                    ("BUY", Some(limit)) => new_price <= limit,
                    ("SELL", Some(limit)) => new_price >= limit,
                    _ => false,
                };
                if let (true, Some(fill_price)) = (should_fill, limit_price) {
                    self.execute_fill(inner, ib_order_id, remaining, fill_price);
                    filled_ids.push(ib_order_id);
                }
                // If limit condition NOT met, order stays Working (unfilled STP LMT).
            }
        }

        // OCA checks after all fills.
        for ib_order_id in filled_ids {
            Self::check_oca_inner(inner, ib_order_id);
        }
    }

    /// Set the market price for a symbol, triggering any pending limit/stop
    /// orders whose fill conditions are met.
    pub fn set_market_price(&self, symbol: &str, price: f64) {
        let mut inner = self.inner.lock();
        inner.market_prices.insert(symbol.to_string(), price);
        self.check_limit_fills_inner(&mut inner, symbol, price);
        self.check_stop_triggers_inner(&mut inner, symbol, price);
    }

    // ── Phase 5: Position & account queries ─────────────────────────────

    /// Current positions as `(symbol, quantity, avg_cost)` tuples.
    pub fn positions(&self) -> Vec<(String, f64, f64)> {
        let inner = self.inner.lock();
        inner
            .positions
            .values()
            .filter(|p| p.quantity.abs() > f64::EPSILON)
            .map(|p| (p.symbol.clone(), p.quantity, p.avg_cost))
            .collect()
    }

    /// Current cash balance.
    pub fn cash_balance(&self) -> f64 {
        let inner = self.inner.lock();
        inner.cash
    }

    /// Unrealized P&L across all open positions (mark-to-market).
    pub fn unrealized_pnl(&self) -> f64 {
        let inner = self.inner.lock();
        inner
            .positions
            .values()
            .map(|pos| {
                let market_price = inner
                    .market_prices
                    .get(&pos.symbol)
                    .copied()
                    .unwrap_or(pos.avg_cost);
                (market_price - pos.avg_cost) * pos.quantity
            })
            .sum()
    }

    // ── Phase 4: Market data subscription & tick generation ──────────────

    /// Build a `BrokerCallback::Tick` from a mid price and spread.
    fn make_market_data_callback(
        con_id: i32,
        symbol: &str,
        price: f64,
        spread: f64,
        volume: i64,
    ) -> BrokerCallback {
        BrokerCallback::Tick {
            symbol: symbol.to_string(),
            con_id,
            bid: Some(price - spread / 2.0),
            ask: Some(price + spread / 2.0),
            last: Some(price),
            volume: Some(volume),
        }
    }

    /// Subscribe to market data. Convenience wrapper around the trait method.
    pub fn subscribe(&self, symbol: &str, con_id: i32) {
        BrokerClient::subscribe_market_data(self, symbol, con_id);
    }

    /// Unsubscribe from market data. Convenience wrapper around the trait method.
    pub fn unsubscribe(&self, symbol: &str) {
        BrokerClient::unsubscribe_market_data(self, symbol);
    }

    /// Generate a synthetic tick for a subscribed symbol. Returns `None` if
    /// the symbol is not subscribed.
    pub fn generate_tick(&self, symbol: &str) -> Option<BrokerCallback> {
        let mut inner = self.inner.lock();
        if !inner.subscriptions.contains(symbol) {
            return None;
        }
        let base_price = Self::get_or_seed_price_inner(&mut inner, symbol);
        let spread = self.config.default_spread;
        Some(Self::make_market_data_callback(0, symbol, base_price, spread, 100))
    }

    /// Generate ticks for all subscribed symbols and enqueue them as callbacks.
    /// Also updates market prices, which drives limit/stop fill checks.
    pub fn generate_auto_ticks(&self) {
        let mut inner = self.inner.lock();
        let symbols: Vec<String> = inner.subscriptions.iter().cloned().collect();
        for symbol in &symbols {
            let price = Self::get_or_seed_price_inner(&mut inner, symbol);
            let spread = self.config.default_spread;
            inner
                .callbacks
                .push_back(Self::make_market_data_callback(0, symbol, price, spread, 100));
        }
        // Price-triggered fills are already handled by set_market_price
        // which is called externally. Auto-ticks just produce tick events.
    }

    // ── Phase 6: Error injection ────────────────────────────────────────

    /// Simulate a connection loss. Pending orders remain in their current state.
    pub fn simulate_disconnect(&self) {
        self.connected.store(false, Ordering::SeqCst);
        let mut inner = self.inner.lock();
        inner.callbacks.push_back(BrokerCallback::ConnectionStatus {
            connected: false,
            server_version: None,
        });
    }

    /// Simulate reconnection. Sets connected flag and pushes callback.
    pub fn simulate_reconnect(&self) {
        self.connected.store(true, Ordering::SeqCst);
        let mut inner = self.inner.lock();
        inner.callbacks.push_back(BrokerCallback::ConnectionStatus {
            connected: true,
            server_version: Some(176),
        });
    }
}

// ══════════════════════════════════════════════════════════════════════════════
// BrokerClient impl
// ══════════════════════════════════════════════════════════════════════════════

impl BrokerClient for TestBroker {
    fn next_order_id(&self) -> i32 {
        self.next_id.fetch_add(1, Ordering::SeqCst)
    }

    #[allow(clippy::too_many_arguments)]
    fn place_order(
        &self,
        ib_order_id: i32,
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
        let mut inner = self.inner.lock();

        // Modification: if order already exists, update prices
        if let Some(existing) = inner.orders.get_mut(&ib_order_id) {
            if let Some(new_limit) = limit_price {
                existing.limit_price = Some(new_limit);
            }
            if let Some(new_stop) = stop_price {
                existing.stop_price = Some(new_stop);
            }
            // Capture values before releasing the mutable borrow on `existing`
            let cb_status = match existing.status {
                SimOrderStatus::Working => "Submitted".to_string(),
                SimOrderStatus::Triggered => "PreSubmitted".to_string(),
                _ => "Submitted".to_string(),
            };
            let cb_filled = existing.filled_qty;
            let cb_remaining = existing.quantity - existing.filled_qty;
            let cb_avg = existing.avg_fill_price;
            // Emit OrderStatus callback so the engine sees the modification
            inner.callbacks.push_back(BrokerCallback::OrderStatus {
                ib_order_id,
                status: cb_status,
                filled: cb_filled,
                remaining: cb_remaining,
                avg_fill_price: cb_avg,
            });
            return Ok(PlaceOrderResult { ib_order_id });
        }

        // Phase 6: Deterministic rejection based on order count
        inner.order_count += 1;
        let should_reject = if self.config.rejection_rate > 0.0 {
            let n = (1.0 / self.config.rejection_rate).round() as u64;
            n > 0 && inner.order_count % n == 0
        } else {
            false
        };

        // New order — always insert so bracket children can detect terminal parent
        let order = SimulatedOrder {
            ib_order_id,
            symbol: symbol.to_string(),
            action: action.to_string(),
            order_type: order_type.to_string(),
            quantity,
            limit_price,
            stop_price,
            parent_id,
            transmit,
            status: if should_reject {
                SimOrderStatus::Rejected
            } else {
                SimOrderStatus::Held
            },
            filled_qty: 0.0,
            avg_fill_price: 0.0,
            tif: tif.to_string(),
            outside_rth,
        };
        inner.orders.insert(ib_order_id, order);

        if should_reject {
            inner.callbacks.push_back(BrokerCallback::OrderRejected {
                ib_order_id,
                reason: "Order rejected by test broker (error injection)".to_string(),
            });
            return Ok(PlaceOrderResult { ib_order_id });
        }

        // Register in bracket_links if this is a child order
        if let Some(pid) = parent_id {
            inner.bracket_links.entry(pid).or_default().push(ib_order_id);
        }

        // If transmit=true, activate the bracket
        if transmit {
            let activate_parent = parent_id.unwrap_or(ib_order_id);
            self.activate_bracket(&mut inner, activate_parent);
        }

        Ok(PlaceOrderResult { ib_order_id })
    }

    fn cancel_order(&self, ib_order_id: i32) -> Result<CancelOrderResult, String> {
        let mut inner = self.inner.lock();

        // Check order exists; silently accept cancel for terminal orders (matches IB behavior)
        match inner.orders.get(&ib_order_id) {
            None => return Err(format!("order {ib_order_id} not found")),
            Some(order) if order.status.is_terminal() => {
                // IB silently ignores cancel requests for filled/cancelled orders
                return Ok(CancelOrderResult { ib_order_id });
            }
            _ => {}
        }

        // Cancel the order itself
        Self::cancel_order_inner(&mut inner, ib_order_id);

        // Cancel all children if this is a parent
        if let Some(children) = inner.bracket_links.get(&ib_order_id).cloned() {
            for child_id in children {
                Self::cancel_order_inner(&mut inner, child_id);
            }
        }

        Ok(CancelOrderResult { ib_order_id })
    }

    fn subscribe_market_data(&self, symbol: &str, con_id: i32) {
        let mut inner = self.inner.lock();
        inner.subscriptions.insert(symbol.to_string());

        let price = Self::get_or_seed_price_inner(&mut inner, symbol);
        let spread = self.config.default_spread;

        inner
            .callbacks
            .push_back(Self::make_market_data_callback(con_id, symbol, price, spread, 0));
    }

    fn unsubscribe_market_data(&self, symbol: &str) {
        self.inner.lock().subscriptions.remove(symbol);
    }

    fn request_positions(&self) -> Vec<crate::client::PositionRecord> {
        let inner = self.inner.lock();
        inner
            .positions
            .values()
            .filter(|p| p.quantity.abs() > f64::EPSILON)
            .map(|p| crate::client::PositionRecord {
                symbol: p.symbol.clone(),
                quantity: p.quantity,
                avg_cost: p.avg_cost,
            })
            .collect()
    }

    fn request_account_summary(&self) -> crate::client::AccountSummary {
        let inner = self.inner.lock();
        let unrealized_pnl: f64 = inner
            .positions
            .values()
            .map(|pos| {
                let market_price = inner
                    .market_prices
                    .get(&pos.symbol)
                    .copied()
                    .unwrap_or(pos.avg_cost);
                (market_price - pos.avg_cost) * pos.quantity
            })
            .sum();
        crate::client::AccountSummary {
            cash_balance: inner.cash,
            unrealized_pnl,
            realized_pnl: inner.realized_pnl,
        }
    }

    fn poll_callbacks(&self) -> Vec<BrokerCallback> {
        let mut inner = self.inner.lock();

        // Phase 4: auto-tick generation based on elapsed time
        if self.config.tick_interval_ms > 0
            && !inner.subscriptions.is_empty()
        {
            let elapsed = inner.last_tick_time.elapsed();
            let interval = std::time::Duration::from_millis(self.config.tick_interval_ms);
            if elapsed >= interval {
                inner.last_tick_time = Instant::now();
                let symbols: Vec<String> = inner.subscriptions.iter().cloned().collect();
                for symbol in &symbols {
                    let price = Self::get_or_seed_price_inner(&mut inner, symbol);
                    let spread = self.config.default_spread;
                    inner
                        .callbacks
                        .push_back(Self::make_market_data_callback(0, symbol, price, spread, 100));
                }
            }
        }

        inner.callbacks.drain(..).collect()
    }

    fn name(&self) -> &str {
        "test-broker"
    }

    fn is_connected(&self) -> bool {
        self.connected.load(Ordering::SeqCst)
    }
}

// ══════════════════════════════════════════════════════════════════════════════
// Tests
// ══════════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    fn make_broker() -> TestBroker {
        TestBroker::new(TestBrokerConfig::default())
    }

    /// Helper: count callbacks of a specific status string.
    fn count_status(cbs: &[BrokerCallback], status_str: &str) -> usize {
        cbs.iter()
            .filter(|cb| matches!(
                cb,
                BrokerCallback::OrderStatus { status, .. } if status == status_str
            ))
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
            .place_order(id, "AAPL", "BUY", "MKT", 100.0, None, None, None, true, "DAY", false)
            .unwrap();

        let cbs = broker.poll_callbacks();

        // Should have: Submitted, Execution, Filled (3 callbacks)
        assert_eq!(count_status(&cbs, "Submitted"), 1, "expected 1 Submitted");
        assert_eq!(count_executions(&cbs), 1, "expected 1 Execution");
        assert_eq!(count_status(&cbs, "Filled"), 1, "expected 1 Filled");

        // Verify execution details (BUY fills at ask = base + half_spread)
        let half_spread = 0.005; // default_spread=0.01 / 2
        let expected_fill = 185.50 + half_spread;
        let exec = cbs.iter()
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
        let filled = cbs.iter()
            .find(|cb| matches!(
                cb,
                BrokerCallback::OrderStatus { status, .. } if status == "Filled"
            ))
            .unwrap();
        if let BrokerCallback::OrderStatus {
            filled: f, remaining, avg_fill_price, ..
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
            .place_order(parent_id, "AAPL", "BUY", "MKT", 100.0, None, None, None, false, "DAY", false)
            .unwrap();
        assert!(broker.poll_callbacks().is_empty(), "no callbacks before transmit");

        // TP: LMT SELL, transmit=false
        broker
            .place_order(
                tp_id, "AAPL", "SELL", "LMT", 100.0, Some(192.0), None,
                Some(parent_id), false, "DAY", false,
            )
            .unwrap();
        assert!(broker.poll_callbacks().is_empty(), "no callbacks before transmit");

        // SL: STP SELL, transmit=true -- activates the bracket
        broker
            .place_order(
                sl_id, "AAPL", "SELL", "STP", 100.0, None, Some(182.0),
                Some(parent_id), true, "DAY", false,
            )
            .unwrap();

        let cbs = broker.poll_callbacks();
        assert!(!cbs.is_empty(), "callbacks should exist after transmit=true");

        // Parent: Submitted + Execution + Filled
        let parent_submitted = cbs.iter().any(|cb| matches!(
            cb,
            BrokerCallback::OrderStatus { ib_order_id, status, .. }
            if *ib_order_id == parent_id && status == "Submitted"
        ));
        assert!(parent_submitted, "parent should get Submitted");

        let parent_filled = cbs.iter().any(|cb| matches!(
            cb,
            BrokerCallback::OrderStatus { ib_order_id, status, .. }
            if *ib_order_id == parent_id && status == "Filled"
        ));
        assert!(parent_filled, "parent should get Filled (MKT instant)");

        // Children should be activated
        let tp_activated = cbs.iter().any(|cb| matches!(
            cb,
            BrokerCallback::OrderStatus { ib_order_id, .. } if *ib_order_id == tp_id
        ));
        assert!(tp_activated, "TP child should be activated");

        let sl_activated = cbs.iter().any(|cb| matches!(
            cb,
            BrokerCallback::OrderStatus { ib_order_id, .. } if *ib_order_id == sl_id
        ));
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
            .place_order(parent_id, "AAPL", "BUY", "MKT", 100.0, None, None, None, false, "DAY", false)
            .unwrap();
        broker
            .place_order(
                tp_id, "AAPL", "SELL", "LMT", 100.0, Some(192.0), None,
                Some(parent_id), false, "DAY", false,
            )
            .unwrap();
        broker
            .place_order(
                sl_id, "AAPL", "SELL", "STP", 100.0, None, Some(182.0),
                Some(parent_id), true, "DAY", false,
            )
            .unwrap();

        let cbs = broker.poll_callbacks();

        // After parent fills (MKT instant), TP should be Submitted, SL should be PreSubmitted
        let tp_submitted = cbs.iter().any(|cb| matches!(
            cb,
            BrokerCallback::OrderStatus { ib_order_id, status, .. }
            if *ib_order_id == tp_id && status == "Submitted"
        ));
        assert!(tp_submitted, "TP should get Submitted after parent fills");

        let sl_presubmitted = cbs.iter().any(|cb| matches!(
            cb,
            BrokerCallback::OrderStatus { ib_order_id, status, .. }
            if *ib_order_id == sl_id && status == "PreSubmitted"
        ));
        assert!(sl_presubmitted, "SL should get PreSubmitted after parent fills");
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
            .place_order(parent_id, "AAPL", "BUY", "MKT", 100.0, None, None, None, false, "DAY", false)
            .unwrap();
        broker
            .place_order(
                tp_id, "AAPL", "SELL", "LMT", 100.0, Some(192.0), None,
                Some(parent_id), false, "DAY", false,
            )
            .unwrap();
        broker
            .place_order(
                sl_id, "AAPL", "SELL", "STP", 100.0, None, Some(182.0),
                Some(parent_id), true, "DAY", false,
            )
            .unwrap();

        // Drain bracket activation callbacks
        let _ = broker.poll_callbacks();

        // Simulate TP fill
        broker.simulate_fill(tp_id, 192.0, 100.0);

        let cbs = broker.poll_callbacks();

        // TP should be filled
        let tp_filled = cbs.iter().any(|cb| matches!(
            cb,
            BrokerCallback::OrderStatus { ib_order_id, status, .. }
            if *ib_order_id == tp_id && status == "Filled"
        ));
        assert!(tp_filled, "TP should be Filled");

        // SL should be cancelled (OCA)
        let sl_cancelled = cbs.iter().any(|cb| matches!(
            cb,
            BrokerCallback::OrderStatus { ib_order_id, status, .. }
            if *ib_order_id == sl_id && status == "Cancelled"
        ));
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
            .place_order(parent_id, "AAPL", "BUY", "MKT", 100.0, None, None, None, false, "DAY", false)
            .unwrap();
        broker
            .place_order(
                tp_id, "AAPL", "SELL", "LMT", 100.0, Some(192.0), None,
                Some(parent_id), false, "DAY", false,
            )
            .unwrap();
        broker
            .place_order(
                sl_id, "AAPL", "SELL", "STP", 100.0, None, Some(182.0),
                Some(parent_id), true, "DAY", false,
            )
            .unwrap();

        // Drain bracket activation callbacks
        let _ = broker.poll_callbacks();

        // Simulate SL fill
        broker.simulate_fill(sl_id, 181.80, 100.0);

        let cbs = broker.poll_callbacks();

        // SL should be filled
        let sl_filled = cbs.iter().any(|cb| matches!(
            cb,
            BrokerCallback::OrderStatus { ib_order_id, status, .. }
            if *ib_order_id == sl_id && status == "Filled"
        ));
        assert!(sl_filled, "SL should be Filled");

        // TP should be cancelled (OCA)
        let tp_cancelled = cbs.iter().any(|cb| matches!(
            cb,
            BrokerCallback::OrderStatus { ib_order_id, status, .. }
            if *ib_order_id == tp_id && status == "Cancelled"
        ));
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
                parent_id, "AAPL", "BUY", "LMT", 100.0, Some(180.0), None, None, false, "DAY", false,
            )
            .unwrap();
        broker
            .place_order(
                tp_id, "AAPL", "SELL", "LMT", 100.0, Some(192.0), None,
                Some(parent_id), false, "DAY", false,
            )
            .unwrap();
        broker
            .place_order(
                sl_id, "AAPL", "SELL", "STP", 100.0, None, Some(182.0),
                Some(parent_id), true, "DAY", false,
            )
            .unwrap();

        // Drain bracket activation callbacks
        let _ = broker.poll_callbacks();

        // Cancel parent
        broker.cancel_order(parent_id).unwrap();

        let cbs = broker.poll_callbacks();

        let parent_cancelled = cbs.iter().any(|cb| matches!(
            cb,
            BrokerCallback::OrderStatus { ib_order_id, status, .. }
            if *ib_order_id == parent_id && status == "Cancelled"
        ));
        assert!(parent_cancelled, "parent should be Cancelled");

        let tp_cancelled = cbs.iter().any(|cb| matches!(
            cb,
            BrokerCallback::OrderStatus { ib_order_id, status, .. }
            if *ib_order_id == tp_id && status == "Cancelled"
        ));
        assert!(tp_cancelled, "TP should be Cancelled when parent is cancelled");

        let sl_cancelled = cbs.iter().any(|cb| matches!(
            cb,
            BrokerCallback::OrderStatus { ib_order_id, status, .. }
            if *ib_order_id == sl_id && status == "Cancelled"
        ));
        assert!(sl_cancelled, "SL should be Cancelled when parent is cancelled");
    }

    // ── 8. Manual simulate_fill ──────────────────────────────────────────

    #[test]
    fn test_simulate_fill_manual() {
        let broker = make_broker();
        broker.set_market_price("AAPL", 185.50);

        let id = broker.next_order_id();
        // Place a limit order (won't auto-fill)
        broker
            .place_order(id, "AAPL", "BUY", "LMT", 50.0, Some(180.0), None, None, true, "DAY", false)
            .unwrap();

        // Drain activation callbacks
        let _ = broker.poll_callbacks();

        // Manually fill at a specific price
        broker.simulate_fill(id, 179.50, 50.0);

        let cbs = broker.poll_callbacks();

        // Should have Execution + Filled
        assert_eq!(count_executions(&cbs), 1, "expected 1 Execution callback");
        assert_eq!(count_status(&cbs, "Filled"), 1, "expected 1 Filled callback");

        // Verify fill price
        let exec = cbs.iter()
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
            .place_order(id, "AAPL", "BUY", "MKT", 300.0, None, None, None, true, "DAY", false)
            .unwrap();

        let cbs = broker.poll_callbacks();

        // Should have: 1 Submitted (activation) + 3 Executions + 2 intermediate "PartiallyFilled" + 1 "Filled"
        let exec_count = count_executions(&cbs);
        assert_eq!(exec_count, 3, "expected 3 Execution callbacks for 3 tranches");

        // The final callback should be Filled
        assert_eq!(count_status(&cbs, "Filled"), 1, "expected exactly 1 Filled");

        // 1 Submitted from activation
        assert_eq!(count_status(&cbs, "Submitted"), 1, "expected 1 Submitted (activation)");

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
            .place_order(id, "AAPL", "BUY", "MKT", 100.0, None, None, None, true, "DAY", false)
            .unwrap();
        let _ = broker.poll_callbacks();

        let positions = broker.positions();
        assert_eq!(positions.len(), 1);
        let (symbol, qty, avg_cost) = &positions[0];
        assert_eq!(symbol, "AAPL");
        assert!((qty - 100.0).abs() < f64::EPSILON, "expected +100 shares, got {qty}");
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
            .place_order(id1, "AAPL", "BUY", "MKT", 100.0, None, None, None, true, "DAY", false)
            .unwrap();
        let _ = broker.poll_callbacks();

        // Close position by selling
        let id2 = broker.next_order_id();
        broker
            .place_order(id2, "AAPL", "SELL", "MKT", 100.0, None, None, None, true, "DAY", false)
            .unwrap();
        let _ = broker.poll_callbacks();

        // Position should be flat
        let positions = broker.positions();
        assert!(positions.is_empty(), "expected no open positions after close, got {positions:?}");
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
            .place_order(id, "AAPL", "BUY", "MKT", 100.0, None, None, None, true, "DAY", false)
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
                .place_order(id, "AAPL", "BUY", "MKT", 10.0, None, None, None, true, "DAY", false)
                .unwrap();

            let cbs = broker.poll_callbacks();
            let has_rejection = cbs.iter().any(|cb| matches!(cb, BrokerCallback::OrderRejected { .. }));
            let has_fill = cbs.iter().any(|cb| matches!(
                cb,
                BrokerCallback::OrderStatus { status, .. } if status == "Filled"
            ));

            if has_rejection {
                rejected_count += 1;
            }
            if has_fill {
                accepted_count += 1;
            }
        }

        assert!(rejected_count > 0, "expected at least 1 rejection with rate=0.5");
        assert!(accepted_count > 0, "expected at least 1 accepted order with rate=0.5");
        assert_eq!(rejected_count + accepted_count, 4, "all orders should be either rejected or filled");
    }

    #[test]
    fn test_disconnect_reconnect() {
        let broker = make_broker();
        assert!(broker.is_connected(), "should start connected");

        // Disconnect
        broker.simulate_disconnect();
        assert!(!broker.is_connected(), "should be disconnected after simulate_disconnect");

        let cbs = broker.poll_callbacks();
        let disconnected = cbs.iter().any(|cb| matches!(
            cb,
            BrokerCallback::ConnectionStatus { connected: false, .. }
        ));
        assert!(disconnected, "should get ConnectionStatus(false) callback");

        // Reconnect
        broker.simulate_reconnect();
        assert!(broker.is_connected(), "should be connected after simulate_reconnect");

        let cbs = broker.poll_callbacks();
        let reconnected = cbs.iter().any(|cb| matches!(
            cb,
            BrokerCallback::ConnectionStatus { connected: true, server_version: Some(176) }
        ));
        assert!(reconnected, "should get ConnectionStatus(true, 176) callback");
    }

    // ── Phase 2: Limit order fills ──────────────────────────────────────

    #[test]
    fn test_limit_buy_fills_at_price() {
        let broker = make_broker();
        broker.set_market_price("AAPL", 185.50);

        let id = broker.next_order_id();
        broker
            .place_order(id, "AAPL", "BUY", "LMT", 100.0, Some(180.0), None, None, true, "DAY", false)
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
            .place_order(id, "AAPL", "SELL", "LMT", 100.0, Some(192.0), None, None, true, "DAY", false)
            .unwrap();

        let cbs = broker.poll_callbacks();
        assert_eq!(count_status(&cbs, "Submitted"), 1);
        assert_eq!(count_executions(&cbs), 0, "should NOT fill at 185.50");

        broker.set_market_price("AAPL", 190.0);
        let cbs = broker.poll_callbacks();
        assert_eq!(count_executions(&cbs), 0, "should NOT fill at 190");

        broker.set_market_price("AAPL", 193.0);
        let cbs = broker.poll_callbacks();
        assert_eq!(count_executions(&cbs), 1, "expected fill when price crosses");
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
            .place_order(id, "AAPL", "BUY", "LMT", 100.0, Some(180.0), None, None, true, "DAY", false)
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
                tp_id, "AAPL", "SELL", "LMT", 100.0, Some(192.0), None,
                Some(parent_id), false, "DAY", false,
            )
            .unwrap();
        broker
            .place_order(
                sl_id, "AAPL", "SELL", "STP", 100.0, None, Some(182.0),
                Some(parent_id), true, "DAY", false,
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
            .find(|cb| matches!(
                cb,
                BrokerCallback::Execution { ib_order_id, .. } if *ib_order_id == sl_id
            ))
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
            .place_order(id, "AAPL", "BUY", "LMT", 50.0, Some(183.0), None, None, true, "DAY", false)
            .unwrap();
        let _ = broker.poll_callbacks();

        broker.set_market_price("AAPL", 182.0);
        let cbs = broker.poll_callbacks();

        assert_eq!(count_executions(&cbs), 1, "limit should fill when price crosses");
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
                tp_id, "MSFT", "SELL", "LMT", 50.0, Some(420.0), None,
                Some(parent_id), false, "DAY", false,
            )
            .unwrap();
        broker
            .place_order(
                sl_id, "MSFT", "SELL", "STP", 50.0, None, Some(390.0),
                Some(parent_id), true, "DAY", false,
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
                tp_id, "AAPL", "SELL", "LMT", 100.0, Some(192.0), None,
                Some(parent_id), false, "DAY", false,
            )
            .unwrap();
        broker
            .place_order(
                sl_id, "AAPL", "SELL", "STP", 100.0, None, Some(182.0),
                Some(parent_id), true, "DAY", false,
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
            .find(|cb| matches!(
                cb,
                BrokerCallback::Execution { ib_order_id, .. } if *ib_order_id == tp_id
            ))
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
                tp_id, "AAPL", "SELL", "LMT", 100.0, Some(192.0), None,
                Some(parent_id), false, "DAY", false,
            )
            .unwrap();
        broker
            .place_order(
                sl_id, "AAPL", "SELL", "STP LMT", 100.0, Some(181.50), Some(182.0),
                Some(parent_id), true, "DAY", false,
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
            .find(|cb| matches!(
                cb,
                BrokerCallback::Execution { ib_order_id, .. } if *ib_order_id == sl_id
            ))
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
                tp_id, "AAPL", "SELL", "LMT", 100.0, Some(192.0), None,
                Some(parent_id), false, "DAY", false,
            )
            .unwrap();
        broker
            .place_order(
                sl_id, "AAPL", "SELL", "STP LMT", 100.0, Some(181.50), Some(182.0),
                Some(parent_id), true, "DAY", false,
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
            .place_order(id, "AAPL", "BUY", "LMT", 100.0, Some(183.0), None, None, true, "DAY", false)
            .unwrap();
        let _ = broker.poll_callbacks();

        broker.set_market_price("AAPL", 182.0);
        let cbs = broker.poll_callbacks();
        assert_eq!(count_status(&cbs, "Filled"), 1);

        broker.set_market_price("AAPL", 181.0);
        let cbs = broker.poll_callbacks();
        assert!(cbs.is_empty(), "filled order should not generate more callbacks");
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
                tp_id, "AAPL", "BUY", "LMT", 100.0, Some(180.0), None,
                Some(parent_id), false, "DAY", false,
            )
            .unwrap();
        broker
            .place_order(
                sl_id, "AAPL", "BUY", "STP", 100.0, None, Some(190.0),
                Some(parent_id), true, "DAY", false,
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
            .find(|cb| matches!(
                cb,
                BrokerCallback::Execution { ib_order_id, .. } if *ib_order_id == sl_id
            ))
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
        assert!(!cbs.is_empty(), "subscribe should produce at least one callback");

        let tick = cbs.iter().find(|cb| matches!(cb, BrokerCallback::Tick { .. }));
        assert!(tick.is_some(), "subscribe should produce a Tick callback");

        if let Some(BrokerCallback::Tick { symbol, con_id, bid, ask, last, volume }) = tick {
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
        assert!(tick.is_some(), "generate_tick should return Some while subscribed");

        // Unsubscribe
        broker.unsubscribe_market_data("AAPL");

        // generate_tick should return None after unsubscribe
        let tick = broker.generate_tick("AAPL");
        assert!(tick.is_none(), "generate_tick should return None after unsubscribe");
    }

    #[test]
    fn test_generate_tick_for_subscribed() {
        let broker = make_broker();
        broker.set_market_price("MSFT", 400.0);
        broker.subscribe_market_data("MSFT", 272093);
        let _ = broker.poll_callbacks(); // drain initial tick

        let tick = broker.generate_tick("MSFT");
        assert!(tick.is_some(), "generate_tick should return Some for subscribed symbol");

        if let Some(BrokerCallback::Tick { symbol, bid, ask, last, volume, .. }) = tick {
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
        assert!(broker.generate_tick("GOOG").is_none(), "non-subscribed symbol should return None");
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
            .place_order(parent_id, "AAPL", "BUY", "MKT", 100.0, None, None, None, false, "DAY", false)
            .unwrap();
        broker
            .place_order(
                tp_id, "AAPL", "SELL", "LMT", 100.0, Some(192.0), None,
                Some(parent_id), false, "DAY", false,
            )
            .unwrap();
        broker
            .place_order(
                sl_id, "AAPL", "SELL", "STP", 100.0, None, Some(182.0),
                Some(parent_id), true, "DAY", false,
            )
            .unwrap();
        let _ = broker.poll_callbacks(); // drain bracket activation

        // Move price below stop loss level — this triggers SL via set_market_price
        broker.set_market_price("AAPL", 181.0);

        // Also generate auto-ticks explicitly (simulates what the engine poll loop does)
        broker.generate_auto_ticks();

        let cbs = broker.poll_callbacks();

        // SL should have been triggered and filled
        let sl_filled = cbs.iter().any(|cb| matches!(
            cb,
            BrokerCallback::OrderStatus { ib_order_id, status, .. }
            if *ib_order_id == sl_id && status == "Filled"
        ));
        assert!(sl_filled, "SL should be Filled after price drops below stop");

        // TP should be OCA-cancelled
        let tp_cancelled = cbs.iter().any(|cb| matches!(
            cb,
            BrokerCallback::OrderStatus { ib_order_id, status, .. }
            if *ib_order_id == tp_id && status == "Cancelled"
        ));
        assert!(tp_cancelled, "TP should be Cancelled via OCA when SL fills");

        // Verify that auto-ticks were generated for the subscribed symbol
        let has_tick = cbs.iter().any(|cb| matches!(
            cb,
            BrokerCallback::Tick { symbol, .. } if symbol == "AAPL"
        ));
        assert!(has_tick, "auto-tick should be generated for subscribed AAPL");
    }
}
