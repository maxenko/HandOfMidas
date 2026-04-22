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

// Legacy `BrokerClient` trait is `#[deprecated]` pending slice 9;
// this module implements it. Silence the warning module-wide until then.
#![allow(deprecated)]

use parking_lot::Mutex;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use serde::Deserialize;

/// Standard three-shift xorshift64 RNG. Advances `state` in place and returns
/// the new value. Never produces 0 unless seeded with 0 (caller guarantees a
/// non-zero seed).
fn xorshift64(state: &mut u64) -> u64 {
    let mut x = *state;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    *state = x;
    x
}

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

fn default_tick_drift_bps() -> f64 {
    10.0
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

    /// Peak per-tick drift in basis points of the previous price
    /// (default 10.0 = ±0.10%). Applied uniformly in [-drift, +drift].
    /// Set to 0.0 for frozen prices.
    #[serde(default = "default_tick_drift_bps")]
    pub tick_drift_bps: f64,

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
            tick_drift_bps: default_tick_drift_bps(),
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

    /// Xorshift64 state for random price drift.
    rng_state: u64,
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
        // Seed the xorshift64 RNG from the wall clock. xorshift64 cannot
        // advance from a 0 state, so fall back to a fixed non-zero constant.
        let seed_ns = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        let rng_state = if seed_ns == 0 {
            0x9E3779B97F4A7C15
        } else {
            seed_ns
        };
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
                rng_state,
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
    fn execute_fill(&self, inner: &mut TestBrokerInner, ib_order_id: i32, shares: f64, price: f64) {
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

        let pos = inner
            .positions
            .entry(symbol.to_string())
            .or_insert(PositionState {
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
    fn fill_market_order_inner(&self, inner: &mut TestBrokerInner, ib_order_id: i32) {
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
        let parent_is_mkt = inner
            .orders
            .get(&parent_id)
            .map(|o| o.order_type == "MKT")
            .unwrap_or(false);

        if parent_is_mkt && self.config.fill_timing == "instant" {
            self.fill_market_order_inner(inner, parent_id);
        }

        // 3. Activate children
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
                o.symbol == symbol && o.status == SimOrderStatus::Working && o.order_type == "LMT"
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
                        "BUY" => limit.min(new_price),  // BUY gets the lower (better) price
                        "SELL" => limit.max(new_price), // SELL gets the higher (better) price
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
    fn check_stop_triggers_inner(&self, inner: &mut TestBrokerInner, symbol: &str, new_price: f64) {
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
        Some(Self::make_market_data_callback(
            0, symbol, base_price, spread, 100,
        ))
    }

    /// Generate ticks for all subscribed symbols and enqueue them as callbacks.
    /// Also updates market prices, which drives limit/stop fill checks.
    pub fn generate_auto_ticks(&self) {
        let mut inner = self.inner.lock();
        let symbols: Vec<String> = inner.subscriptions.iter().cloned().collect();
        for symbol in &symbols {
            let price = Self::get_or_seed_price_inner(&mut inner, symbol);
            let spread = self.config.default_spread;
            inner.callbacks.push_back(Self::make_market_data_callback(
                0, symbol, price, spread, 100,
            ));
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
            n > 0 && inner.order_count.is_multiple_of(n)
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
            inner
                .bracket_links
                .entry(pid)
                .or_default()
                .push(ib_order_id);
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

        inner.callbacks.push_back(Self::make_market_data_callback(
            con_id, symbol, price, spread, 0,
        ));
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

        // Phase 4: auto-tick generation based on elapsed time.
        // Each tick applies a uniform random drift in [-tick_drift_bps, +tick_drift_bps]
        // basis points of the previous price, then re-runs limit/stop checks so
        // pending orders actually trigger as the simulated market moves.
        if self.config.tick_interval_ms > 0 && !inner.subscriptions.is_empty() {
            let elapsed = inner.last_tick_time.elapsed();
            let interval = std::time::Duration::from_millis(self.config.tick_interval_ms);
            if elapsed >= interval {
                inner.last_tick_time = Instant::now();
                let symbols: Vec<String> = inner.subscriptions.iter().cloned().collect();
                let spread = self.config.default_spread;
                let drift_bps = self.config.tick_drift_bps;
                for symbol in &symbols {
                    // Seed if absent, then draw a drifted price.
                    let prev_price = Self::get_or_seed_price_inner(&mut inner, symbol);
                    let new_price = if drift_bps > 0.0 {
                        // xorshift64 → signed i64 → f64 in [-1.0, 1.0]
                        let r = xorshift64(&mut inner.rng_state) as i64;
                        let rand_unit = r as f64 / i64::MAX as f64;
                        let drift = (drift_bps / 10_000.0) * rand_unit * prev_price;
                        prev_price + drift
                    } else {
                        prev_price
                    };
                    inner.market_prices.insert(symbol.clone(), new_price);

                    // Emit the tick using the new price.
                    inner.callbacks.push_back(Self::make_market_data_callback(
                        0, symbol, new_price, spread, 100,
                    ));

                    // Run the same fill/trigger checks set_market_price runs so
                    // pending limit/stop orders react to simulated drift.
                    self.check_limit_fills_inner(&mut inner, symbol, new_price);
                    self.check_stop_triggers_inner(&mut inner, symbol, new_price);
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

#[cfg(test)]
mod tests;
