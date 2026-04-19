//! `MockEngine` — minimal stand-in for the real engine during Stage 06 runner
//! development.
//!
//! Implements [`ScenarioQuery`] + consumes [`EngineCmd`]s from an mpsc
//! channel. Each consumed command mutates the mock's internal state just
//! enough that subsequent `when:`-clause evaluations observe the effect:
//!
//! - `PlaceOrder` → registers an order in `orders` with
//!   `status = PreSubmitted` + auto-fill after `MOCK_FILL_DELAY_MS` virtual ms.
//! - `CancelOrder` → marks the order `Cancelled`.
//! - `SubscribeMarketData` → records the subscription.
//! - `InjectPriceJump` → mutates the price state.
//! - Every other command is logged without side-effect (scenarios still
//!   exercise the `EngineCmd` encoding, and Stages 03/04/05 Wave 2 will add
//!   the real behaviour).
//!
//! Deterministic: no RNG, no real clock. Fill timing is a constant offset
//! from command receipt, applied by the [`crate::scenario::runner`] which
//! owns the clock.
//!
//! When the real engine lands, the runner swaps `MockEngine` for the live
//! engine without changing fixture YAML or expression grammar.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::engine::clock::VirtualInstant;
use crate::engine::types::{EngineCmd, OrderId};

use super::expr::{
    OrderSnapshot, OrderStatusName, PositionSnapshot, ScenarioQuery, SessionMetrics,
};

/// Mock order record.
#[derive(Clone, Debug)]
struct MockOrder {
    order_ref: String,
    symbol: String,
    side: String,
    quantity: f64,
    filled_qty: f64,
    status: OrderStatusName,
    limit_price: Option<f64>,
    stop_price: Option<f64>,
    avg_fill_price: Option<f64>,
    parent_ref: Option<String>,
    fill_at: VirtualInstant,
}

/// Mock state — owned behind `Arc<Mutex<..>>` so the runner can read
/// snapshots concurrently with submitting commands.
#[derive(Default)]
pub struct MockState {
    orders: Vec<MockOrder>,
    positions: BTreeMap<String, PositionSnapshot>,
    prices: BTreeMap<String, f64>,
    sessions: BTreeMap<u64, SessionMetrics>,
    subscriptions: Vec<String>,
    outgoing_cmds: Vec<MockCmd>,
    duration: Duration,
    session_count: u64,
    next_order_id: i32,
}

/// Projection of a received `EngineCmd` — captured for `.expected.jsonl`
/// recording + regression replay. Held as a side-channel so tests can
/// inspect the sequence cheaply without needing serde on `EngineCmd` itself.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum MockCmd {
    SubscribeMarketData {
        symbol: String,
        mode: String,
    },
    UnsubscribeMarketData {
        symbol: String,
    },
    PlaceOrder {
        order_ref: String,
        symbol: String,
        side: String,
        quantity: f64,
        order_kind: String,
    },
    CancelOrder {
        order_ref: String,
    },
    InjectDisconnect {
        session_id: String,
        reason: String,
    },
    InjectFarmOutage {
        code: i32,
        farms: Vec<String>,
    },
    InjectFarmRestore {
        code: i32,
        farms: Vec<String>,
    },
    InjectPacingViolation {
        session_id: String,
    },
    InjectLag {
        session_id: String,
        duration: String,
    },
    InjectBadFrame {
        session_id: String,
        bytes_hex: String,
    },
    InjectPriceJump {
        symbol: String,
        magnitude_pct: f64,
    },
    InjectGap {
        symbol: String,
        from: f64,
        to: f64,
    },
    InjectHalt {
        symbol: String,
        duration: String,
    },
    InjectBurst {
        symbols: Vec<String>,
        multiplier: f64,
        duration: String,
    },
    InjectDuplicateOrderStatus {
        order_ref: String,
        count: u32,
    },
    InjectSlowCommissionReport {
        order_ref: String,
        delay: String,
    },
    InjectOutOfOrderEvents {
        emit_first: String,
        emit_second: String,
    },
    InjectDailyRestart,
    Sleep {
        duration: String,
    },
    SetClockMode {
        mode: String,
        multiplier: Option<f64>,
    },
    Assert {
        cond: String,
        passed: bool,
        message: Option<String>,
    },
    AssertClientReceived {
        session_id: String,
        message: String,
    },
    AssertClientEventOrder {
        session_id: String,
        sequence: Vec<String>,
    },
    ScenarioCompleted,
}

/// Handle used by the runner. Cheap to clone.
#[derive(Clone)]
pub struct MockEngine {
    state: Arc<Mutex<MockState>>,
}

/// Default fill delay applied to all placed orders — keeps scenarios
/// deterministic while giving `when:` clauses something to wait for.
pub const MOCK_FILL_DELAY: Duration = Duration::from_millis(500);

impl Default for MockEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl MockEngine {
    pub fn new() -> Self {
        let mut state = MockState::default();
        // Pre-register session 0 so scenarios that reference `session[0]`
        // without a `StartApi` command don't trip `PathResolve`.
        state.sessions.insert(
            0,
            SessionMetrics {
                msg_count: 0,
                msg_count_last_5s: 0,
                tick_count: 0,
                connected: true,
            },
        );
        state.session_count = 1;
        state.next_order_id = 1000;
        Self {
            state: Arc::new(Mutex::new(state)),
        }
    }

    /// Configure an initial price for `symbol` — called by the runner during
    /// scenario setup so `InjectPriceJump` has a base value.
    pub fn seed_price(&self, symbol: &str, price: f64) {
        let mut s = self.state.lock().expect("mock state poisoned");
        s.prices.insert(symbol.to_string(), price);
    }

    /// Record the runner's current virtual duration so
    /// `session_duration` paths reflect reality.
    pub fn tick_duration_to(&self, now: VirtualInstant) {
        let mut s = self.state.lock().expect("mock state poisoned");
        s.duration = now.as_duration();
    }

    /// Consume a command. Returns the projection used for recording, or
    /// `None` when the command is purely observational.
    pub fn accept(&self, cmd: EngineCmd) -> Option<MockCmd> {
        let mut s = self.state.lock().expect("mock state poisoned");
        let projected = project(&cmd);
        apply_to_state(&mut s, &cmd);
        if let Some(p) = &projected {
            s.outgoing_cmds.push(p.clone());
        }
        projected
    }

    /// Test-only access to the record of received projected commands.
    pub fn outgoing(&self) -> Vec<MockCmd> {
        self.state
            .lock()
            .expect("mock state poisoned")
            .outgoing_cmds
            .clone()
    }

    /// Push a scenario-local event (assert result, scenario completion) into
    /// the outgoing record.
    pub fn record(&self, cmd: MockCmd) {
        let mut s = self.state.lock().expect("mock state poisoned");
        s.outgoing_cmds.push(cmd);
    }

    /// Advance the auto-fill state machine — moves every order whose
    /// `fill_at <= now` into `Filled`. Called by the runner each time
    /// virtual time advances.
    pub fn advance_fills(&self, now: VirtualInstant) {
        let mut s = self.state.lock().expect("mock state poisoned");
        // Snapshot prices up-front so we don't hold a borrow on `s.prices`
        // while mutating `s.orders`.
        let prices = s.prices.clone();
        for order in s.orders.iter_mut() {
            if order.status == OrderStatusName::PreSubmitted
                && order.fill_at.as_duration() <= now.as_duration()
            {
                order.status = OrderStatusName::Filled;
                order.filled_qty = order.quantity;
                if order.avg_fill_price.is_none() {
                    // Favour the seeded price, falling back to limit/stop/constant.
                    let px = prices
                        .get(&order.symbol)
                        .copied()
                        .or(order.limit_price)
                        .or(order.stop_price)
                        .unwrap_or(100.0);
                    order.avg_fill_price = Some(px);
                }
            }
        }
        // Update position snapshots from filled orders.
        let mut by_sym: BTreeMap<String, PositionSnapshot> = BTreeMap::new();
        for o in &s.orders {
            if o.status == OrderStatusName::Filled {
                let sign = if o.side == "buy" { 1.0 } else { -1.0 };
                let entry = by_sym
                    .entry(o.symbol.clone())
                    .or_insert_with(|| PositionSnapshot {
                        symbol: o.symbol.clone(),
                        quantity: 0.0,
                        avg_cost: 0.0,
                        realized_pnl: 0.0,
                        unrealized_pnl: 0.0,
                    });
                entry.quantity += sign * o.filled_qty;
                entry.avg_cost = o.avg_fill_price.unwrap_or(0.0);
            }
        }
        s.positions = by_sym;
    }
}

fn apply_to_state(s: &mut MockState, cmd: &EngineCmd) {
    use EngineCmd::*;
    match cmd {
        PlaceOrder { req, .. } => {
            let order_ref = format!("{}/{}", req.account, req.order_id.0);
            let side = match req.side {
                crate::engine::types::Side::Buy => "buy",
                crate::engine::types::Side::Sell => "sell",
            };
            let fill_at = VirtualInstant::from_duration(s.duration + MOCK_FILL_DELAY);
            s.orders.push(MockOrder {
                order_ref,
                symbol: req.contract.symbol().to_string(),
                side: side.into(),
                quantity: req.total_quantity,
                filled_qty: 0.0,
                status: OrderStatusName::PreSubmitted,
                limit_price: req.limit_price,
                stop_price: req.aux_price,
                avg_fill_price: None,
                parent_ref: req.parent_id.map(|p| format!("{}", p.0)),
                fill_at,
            });
        }
        CancelOrder { order_id, .. } => {
            let search = order_id.0.to_string();
            for o in s.orders.iter_mut() {
                if o.order_ref.ends_with(&format!("/{search}")) {
                    o.status = OrderStatusName::Cancelled;
                }
            }
        }
        SubscribeMarketData { contract, .. } => {
            s.subscriptions.push(contract.symbol().to_string());
        }
        InjectPriceJump {
            symbol,
            magnitude_pct,
        } => {
            let entry = s.prices.entry(symbol.symbol.clone()).or_insert(100.0);
            *entry *= 1.0 + magnitude_pct / 100.0;
        }
        InjectGap { symbol, to, .. } => {
            s.prices.insert(symbol.symbol.clone(), *to);
        }
        InjectDisconnect { session, .. } => {
            if let Some(m) = s.sessions.get_mut(&session.0) {
                m.connected = false;
            }
        }
        InjectPacingViolation { session } => {
            // Real IB: emits error 100 then disconnects after a brief delay.
            // Mock: apply the disconnect synchronously.
            if let Some(m) = s.sessions.get_mut(&session.0) {
                m.connected = false;
            }
        }
        _ => {}
    }
}

/// Project an `EngineCmd` into a record-friendly `MockCmd`. Returns `None`
/// for commands that are internal-only (e.g. `Tick`). Crate-public so the
/// [`super::engine_adapter::RealScenarioEngine`] can reuse the same
/// projection and keep `.expected.jsonl` recordings byte-identical across
/// engine back-ends.
pub(crate) fn project(cmd: &EngineCmd) -> Option<MockCmd> {
    use EngineCmd::*;
    Some(match cmd {
        SubscribeMarketData { contract, mode, .. } => MockCmd::SubscribeMarketData {
            symbol: contract.symbol().to_string(),
            mode: mode_string(mode),
        },
        UnsubscribeMarketData { .. } => MockCmd::UnsubscribeMarketData {
            symbol: String::new(), // req_id keyed; real sim resolves upstream.
        },
        PlaceOrder { req, .. } => {
            let side = match req.side {
                crate::engine::types::Side::Buy => "buy",
                crate::engine::types::Side::Sell => "sell",
            };
            let kind = format!("{:?}", req.kind).to_lowercase();
            MockCmd::PlaceOrder {
                order_ref: format!("{}/{}", req.account, req.order_id.0),
                symbol: req.contract.symbol().to_string(),
                side: side.into(),
                quantity: req.total_quantity,
                order_kind: kind,
            }
        }
        CancelOrder { order_id, .. } => MockCmd::CancelOrder {
            order_ref: order_id.0.to_string(),
        },
        InjectDisconnect { session, reason } => MockCmd::InjectDisconnect {
            session_id: session.0.to_string(),
            reason: reason.clone(),
        },
        InjectFarmOutage { code, farms } => MockCmd::InjectFarmOutage {
            code: *code,
            farms: farms.clone(),
        },
        InjectFarmRestore { code, farms } => MockCmd::InjectFarmRestore {
            code: *code,
            farms: farms.clone(),
        },
        InjectPacingViolation { session } => MockCmd::InjectPacingViolation {
            session_id: session.0.to_string(),
        },
        InjectLag { session, duration } => MockCmd::InjectLag {
            session_id: session.0.to_string(),
            duration: format!("{}ms", duration.as_millis()),
        },
        InjectPriceJump {
            symbol,
            magnitude_pct,
        } => MockCmd::InjectPriceJump {
            symbol: symbol.symbol.clone(),
            magnitude_pct: *magnitude_pct,
        },
        InjectGap { symbol, from, to } => MockCmd::InjectGap {
            symbol: symbol.symbol.clone(),
            from: *from,
            to: *to,
        },
        InjectHalt { symbol, duration } => MockCmd::InjectHalt {
            symbol: symbol.symbol.clone(),
            duration: format!("{}ms", duration.as_millis()),
        },
        InjectBurst {
            symbols,
            multiplier,
            duration,
        } => MockCmd::InjectBurst {
            symbols: symbols.iter().map(|s| s.symbol.clone()).collect(),
            multiplier: *multiplier,
            duration: format!("{}ms", duration.as_millis()),
        },
        InjectDailyRestart => MockCmd::InjectDailyRestart,
        // Scenario-local verbs produce no EngineCmd, so Tick / LoadScenario /
        // DumpState etc. are ignored at the projection layer.
        _ => return None,
    })
}

fn mode_string(mode: &crate::engine::types::SubMode) -> String {
    use crate::engine::types::SubMode::*;
    match mode {
        StreamingL1 { .. } => "streaming_l1".into(),
        TickByTick { kind } => format!("tick_by_tick_{kind:?}").to_lowercase(),
        RealtimeBars5s => "realtime_bars_5s".into(),
        Historical(_) => "historical".into(),
    }
}

// ---------------------------------------------------------------------------
// ScenarioQuery — the read-only surface.
// ---------------------------------------------------------------------------

impl ScenarioQuery for MockEngine {
    fn orders(&self) -> Vec<OrderSnapshot> {
        let s = self.state.lock().expect("mock state poisoned");
        s.orders
            .iter()
            .map(|o| OrderSnapshot {
                order_ref: o.order_ref.clone(),
                symbol: o.symbol.clone(),
                side: o.side.clone(),
                quantity: o.quantity,
                filled_qty: o.filled_qty,
                remaining_qty: o.quantity - o.filled_qty,
                status: o.status,
                limit_price: o.limit_price,
                stop_price: o.stop_price,
                avg_fill_price: o.avg_fill_price,
                parent_ref: o.parent_ref.clone(),
            })
            .collect()
    }

    fn order_by_ref(&self, order_ref: &str) -> Option<OrderSnapshot> {
        // Default impl calls orders() — override is cheaper for point lookups.
        self.orders().into_iter().find(|o| o.order_ref == order_ref)
    }

    fn position_for(&self, symbol: &str) -> Option<PositionSnapshot> {
        self.state
            .lock()
            .expect("mock state poisoned")
            .positions
            .get(symbol)
            .cloned()
    }

    fn positions(&self) -> Vec<PositionSnapshot> {
        self.state
            .lock()
            .expect("mock state poisoned")
            .positions
            .values()
            .cloned()
            .collect()
    }

    fn session_metrics(&self, id: u64) -> Option<SessionMetrics> {
        self.state
            .lock()
            .expect("mock state poisoned")
            .sessions
            .get(&id)
            .cloned()
    }

    fn session_duration(&self) -> Duration {
        self.state.lock().expect("mock state poisoned").duration
    }
}

impl MockEngine {
    /// Allocate a fresh `OrderId` for the runner to attach to synthesised
    /// `PlaceOrderReq`s.
    pub fn next_order_id(&self) -> OrderId {
        let mut s = self.state.lock().expect("mock state poisoned");
        let id = s.next_order_id;
        s.next_order_id += 1;
        OrderId(id)
    }

    /// Attach a user-supplied `order_ref` to the most-recently placed order.
    /// Called by the runner directly after forwarding a `PlaceOrder` command
    /// because `EngineCmd::PlaceOrder` has no `order_ref` slot today.
    pub fn attach_order_ref(&self, order_ref: &str) {
        let mut s = self.state.lock().expect("mock state poisoned");
        if let Some(last) = s.orders.last_mut() {
            last.order_ref = order_ref.to_string();
        }
    }

    /// Latest deadline at which a still-`PreSubmitted` order wants to fill.
    /// Returns `None` when every order is already in a terminal state.
    pub fn next_fill_deadline(&self) -> Option<VirtualInstant> {
        let s = self.state.lock().expect("mock state poisoned");
        s.orders
            .iter()
            .filter(|o| o.status == OrderStatusName::PreSubmitted)
            .map(|o| o.fill_at)
            .max()
    }

    /// Count of outgoing commands captured so far.
    pub fn cmd_count(&self) -> usize {
        self.state
            .lock()
            .expect("mock state poisoned")
            .outgoing_cmds
            .len()
    }

    /// Cancel an order by its string `order_ref` (runner-side lookup).
    pub fn cancel_by_ref(&self, order_ref: &str) -> bool {
        let mut s = self.state.lock().expect("mock state poisoned");
        for o in s.orders.iter_mut() {
            if o.order_ref == order_ref {
                o.status = OrderStatusName::Cancelled;
                return true;
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::types::{PlaceOrderReq, Side};
    use midas_broker_core::ContractSpec;

    fn spec(symbol: &str) -> ContractSpec {
        ContractSpec::Stock {
            symbol: symbol.into(),
            exchange: "SMART".into(),
            currency: "USD".into(),
        }
    }

    #[test]
    fn place_order_registers_pre_submitted() {
        let eng = MockEngine::new();
        let req = PlaceOrderReq {
            order_id: eng.next_order_id(),
            contract: spec("AAPL"),
            side: Side::Buy,
            total_quantity: 100.0,
            kind: crate::engine::types::OrderKind::Market,
            limit_price: None,
            aux_price: None,
            tif: "DAY".into(),
            account: "DU1".into(),
            parent_id: None,
            oca_group: None,
            transmit: true,
        };
        eng.accept(EngineCmd::PlaceOrder {
            session: crate::engine::types::SessionId(0),
            req,
        });
        eng.attach_order_ref("smoke-1");
        let orders = eng.orders();
        assert_eq!(orders.len(), 1);
        assert_eq!(orders[0].order_ref, "smoke-1");
        assert_eq!(orders[0].status, OrderStatusName::PreSubmitted);
    }

    #[test]
    fn advance_fills_fills_when_deadline_reached() {
        let eng = MockEngine::new();
        let req = PlaceOrderReq {
            order_id: eng.next_order_id(),
            contract: spec("AAPL"),
            side: Side::Buy,
            total_quantity: 50.0,
            kind: crate::engine::types::OrderKind::Market,
            limit_price: None,
            aux_price: None,
            tif: "DAY".into(),
            account: "DU1".into(),
            parent_id: None,
            oca_group: None,
            transmit: true,
        };
        eng.seed_price("AAPL", 175.0);
        eng.accept(EngineCmd::PlaceOrder {
            session: crate::engine::types::SessionId(0),
            req,
        });
        eng.attach_order_ref("o1");
        // Before deadline — still pre-submitted.
        eng.advance_fills(VirtualInstant::from_millis(100));
        assert_eq!(eng.orders()[0].status, OrderStatusName::PreSubmitted);
        // After deadline — filled.
        eng.advance_fills(VirtualInstant::from_millis(1_000));
        let o = &eng.orders()[0];
        assert_eq!(o.status, OrderStatusName::Filled);
        assert_eq!(o.filled_qty, 50.0);
        assert_eq!(o.avg_fill_price, Some(175.0));
    }

    #[test]
    fn position_computed_from_filled_orders() {
        let eng = MockEngine::new();
        eng.seed_price("AAPL", 175.0);
        let req = PlaceOrderReq {
            order_id: eng.next_order_id(),
            contract: spec("AAPL"),
            side: Side::Buy,
            total_quantity: 100.0,
            kind: crate::engine::types::OrderKind::Market,
            limit_price: None,
            aux_price: None,
            tif: "DAY".into(),
            account: "DU1".into(),
            parent_id: None,
            oca_group: None,
            transmit: true,
        };
        eng.accept(EngineCmd::PlaceOrder {
            session: crate::engine::types::SessionId(0),
            req,
        });
        eng.attach_order_ref("o1");
        eng.advance_fills(VirtualInstant::from_millis(1_000));
        let p = eng.position_for("AAPL").expect("position");
        assert_eq!(p.quantity, 100.0);
    }

    #[test]
    fn cancel_marks_order() {
        let eng = MockEngine::new();
        let req = PlaceOrderReq {
            order_id: eng.next_order_id(),
            contract: spec("AAPL"),
            side: Side::Buy,
            total_quantity: 50.0,
            kind: crate::engine::types::OrderKind::Limit,
            limit_price: Some(170.0),
            aux_price: None,
            tif: "DAY".into(),
            account: "DU1".into(),
            parent_id: None,
            oca_group: None,
            transmit: true,
        };
        eng.accept(EngineCmd::PlaceOrder {
            session: crate::engine::types::SessionId(0),
            req,
        });
        eng.attach_order_ref("o1");
        assert!(eng.cancel_by_ref("o1"));
        assert_eq!(eng.orders()[0].status, OrderStatusName::Cancelled);
    }
}
