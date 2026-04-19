//! Runner-side abstraction over "an engine that a scenario can drive".
//!
//! Wave 3 Part B ships two implementations:
//!
//! - [`super::mock_engine::MockEngine`] — the in-memory stand-in that Stage 06
//!   originally shipped against.
//! - [`RealScenarioEngine`] — wires the Stage-03/04/05 components into the
//!   Wave-3 [`crate::engine::orchestrator::OrchestratedEngine`] and exposes
//!   the same surface the runner consumes.
//!
//! The trait is intentionally behavioural, not structural: it bundles the
//! runner-side operations so [`crate::scenario::runner::ScenarioRunner`] can
//! be parameterised over either impl. The existing `.expected.jsonl`
//! recording surface lives here unchanged — the trait exposes the shared
//! `MockCmd` projection that we serialise, even when the real engine also
//! produces richer `EngineEvent`s internally.

use std::sync::Arc;
use std::time::Duration;

use crate::engine::clock::{Clock, VirtualInstant};
use crate::engine::orchestrator::{build_engine, OrchestratedEngine};
use crate::engine::types::{EngineCmd, OrderId, OrderStatusCode, SessionId};
use crate::market_data::MarketDataEngine;
use crate::orders::OrderSimulator;
use crate::quirks::QuirksConfig;

use super::expr::{
    OrderSnapshot, OrderStatusName, PositionSnapshot, ScenarioQuery, SessionMetrics,
};
use super::mock_engine::{MockCmd, MockEngine, MOCK_FILL_DELAY};

/// The runner's view of an engine. Both [`MockEngine`] and
/// [`RealScenarioEngine`] implement this.
///
/// # Invariants
///
/// - `next_order_id` must be monotonic.
/// - `record` must push to the same log that `outgoing` returns.
/// - `attach_order_ref` must rename the most-recently-placed order so the
///   scenario YAML's `order_ref` stays in sync with the engine's internal
///   order id.
pub trait ScenarioEngine: ScenarioQuery {
    /// Mutable clone of the engine handle — scenario runner keeps one for
    /// driving, the calling test keeps a separate one for introspection.
    /// Both share the underlying state through `Arc`.
    fn clone_handle(&self) -> Box<dyn ScenarioEngine>;

    fn seed_price(&self, symbol: &str, price: f64);
    fn tick_duration_to(&self, now: VirtualInstant);
    fn accept(&self, cmd: EngineCmd) -> Option<MockCmd>;
    fn record(&self, cmd: MockCmd);
    fn advance_fills(&self, now: VirtualInstant);
    fn next_fill_deadline(&self) -> Option<VirtualInstant>;
    fn next_order_id(&self) -> OrderId;
    fn attach_order_ref(&self, order_ref: &str);
    fn cancel_by_ref(&self, order_ref: &str) -> bool;
    fn outgoing(&self) -> Vec<MockCmd>;
}

// ---------------------------------------------------------------------------
// MockEngine impl — trivial forwarders.
// ---------------------------------------------------------------------------

impl ScenarioEngine for MockEngine {
    fn clone_handle(&self) -> Box<dyn ScenarioEngine> {
        Box::new(self.clone())
    }

    fn seed_price(&self, symbol: &str, price: f64) {
        MockEngine::seed_price(self, symbol, price)
    }

    fn tick_duration_to(&self, now: VirtualInstant) {
        MockEngine::tick_duration_to(self, now)
    }

    fn accept(&self, cmd: EngineCmd) -> Option<MockCmd> {
        MockEngine::accept(self, cmd)
    }

    fn record(&self, cmd: MockCmd) {
        MockEngine::record(self, cmd)
    }

    fn advance_fills(&self, now: VirtualInstant) {
        MockEngine::advance_fills(self, now)
    }

    fn next_fill_deadline(&self) -> Option<VirtualInstant> {
        MockEngine::next_fill_deadline(self)
    }

    fn next_order_id(&self) -> OrderId {
        MockEngine::next_order_id(self)
    }

    fn attach_order_ref(&self, order_ref: &str) {
        MockEngine::attach_order_ref(self, order_ref)
    }

    fn cancel_by_ref(&self, order_ref: &str) -> bool {
        MockEngine::cancel_by_ref(self, order_ref)
    }

    fn outgoing(&self) -> Vec<MockCmd> {
        MockEngine::outgoing(self)
    }
}

// ---------------------------------------------------------------------------
// RealScenarioEngine — drives the Wave-3 orchestrated engine.
// ---------------------------------------------------------------------------

/// Scenario-runner-facing wrapper around [`OrchestratedEngine`].
///
/// The real engine is single-owner (it holds an `mpsc::Receiver`), but the
/// scenario runner wants the same `Clone`-friendly handle shape
/// [`MockEngine`] offers. We back the engine with a `Mutex` + `Arc` so the
/// runner, the test harness, and the `ScenarioQuery` path can all reach the
/// same state.
///
/// ## Determinism strategy
///
/// The real synthetic market-data engine is seeded-deterministic but does
/// not guarantee that any particular order fills within a scenario's
/// virtual duration (tick rate depends on the preset and Hawkes excitement).
/// To keep scenario recordings stable we mirror [`MockEngine`]'s
/// auto-fill contract: every `PlaceOrder` is scheduled to fill after
/// [`super::mock_engine::MOCK_FILL_DELAY`] of virtual time, regardless of
/// what the synthetic market data does in parallel. The real order
/// simulator still processes the snapshot-driven path (so Wave-4 scenarios
/// can opt out of the shortcut), but the scenario DSL's expected output
/// stays a function of the YAML, not the Hawkes RNG state.
pub struct RealScenarioEngine {
    inner: Arc<std::sync::Mutex<RealScenarioState>>,
    /// Handle to the underlying clock — for queries that need `now`.
    clock: Arc<dyn Clock>,
}

struct RealScenarioState {
    engine: OrchestratedEngine,
    outgoing: Vec<MockCmd>,
    /// Mirror of MockEngine's "seeded" prices, used for auto-fill pricing.
    prices: std::collections::BTreeMap<String, f64>,
    /// Tracks the order_ref given to each order_id.
    order_refs: std::collections::BTreeMap<OrderId, String>,
    /// Reverse map: ref → order_id.
    ref_to_id: std::collections::BTreeMap<String, OrderId>,
    /// Scripted fill deadlines (virtual time). Orders complete at this
    /// point regardless of the synthetic market-data state.
    fill_at: std::collections::BTreeMap<OrderId, VirtualInstant>,
    /// Orders we've already auto-filled, to avoid double-filling.
    auto_filled: std::collections::BTreeSet<OrderId>,
    /// The most-recently-placed order — receives the next
    /// [`attach_order_ref`] call.
    last_placed: Option<OrderId>,
    /// Synthesised order id counter (matches MockEngine for recording parity).
    next_order_id: i32,
}

impl RealScenarioEngine {
    /// Build a fresh real-engine-backed scenario runtime. Spins up the
    /// orchestrator with the given market-data engine + quirk config.
    pub fn new(
        clock: Arc<dyn Clock>,
        market_data: Box<dyn MarketDataEngine>,
        quirks: &QuirksConfig,
        seed: u64,
    ) -> Self {
        let (mut engine, _cmd_tx, _event_rx) =
            build_engine(Arc::clone(&clock), market_data, quirks, seed);
        // Pre-register session 0 so DSL references to `session[0]` work
        // before any `inject_disconnect` / `StartApi` arrives. Matches
        // MockEngine semantics for the canonical scenarios.
        engine.handle_command(EngineCmd::StartApi {
            session: SessionId(0),
            client_id: 0,
        });
        let inner = RealScenarioState {
            engine,
            outgoing: Vec::new(),
            prices: std::collections::BTreeMap::new(),
            order_refs: std::collections::BTreeMap::new(),
            ref_to_id: std::collections::BTreeMap::new(),
            fill_at: std::collections::BTreeMap::new(),
            auto_filled: std::collections::BTreeSet::new(),
            last_placed: None,
            next_order_id: 1000,
        };
        Self {
            inner: Arc::new(std::sync::Mutex::new(inner)),
            clock,
        }
    }

    /// Consume `self` and return the underlying handle the runner keeps.
    pub fn handle(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
            clock: Arc::clone(&self.clock),
        }
    }

    fn with_state<R>(&self, f: impl FnOnce(&mut RealScenarioState) -> R) -> R {
        let mut s = self.inner.lock().expect("real scenario state poisoned");
        f(&mut s)
    }

    fn record_into(&self, cmd: MockCmd) {
        self.with_state(|s| s.outgoing.push(cmd));
    }

    fn project_cmd(cmd: &EngineCmd) -> Option<MockCmd> {
        // Use MockEngine's projection for stable recording format.
        super::mock_engine::project(cmd)
    }
}

impl ScenarioQuery for RealScenarioEngine {
    fn orders(&self) -> Vec<OrderSnapshot> {
        self.with_state(|s| {
            s.engine
                .orders
                .orders()
                .values()
                .map(|o| {
                    let order_ref = s
                        .order_refs
                        .get(&o.order_id)
                        .cloned()
                        .unwrap_or_else(|| format!("{}/{}", o.account, o.order_id.0));
                    OrderSnapshot {
                        order_ref,
                        symbol: o.contract.symbol().to_string(),
                        side: match o.side {
                            crate::engine::types::Side::Buy => "buy".into(),
                            crate::engine::types::Side::Sell => "sell".into(),
                        },
                        quantity: o.total_qty,
                        filled_qty: o.filled_qty,
                        remaining_qty: o.remaining_qty,
                        status: match o.status {
                            OrderStatusCode::ApiPending => OrderStatusName::ApiPending,
                            OrderStatusCode::PendingSubmit => OrderStatusName::PendingSubmit,
                            OrderStatusCode::PreSubmitted => OrderStatusName::PreSubmitted,
                            OrderStatusCode::Submitted => OrderStatusName::Submitted,
                            OrderStatusCode::Filled => OrderStatusName::Filled,
                            OrderStatusCode::PartiallyFilled => OrderStatusName::PartiallyFilled,
                            OrderStatusCode::Cancelled => OrderStatusName::Cancelled,
                            OrderStatusCode::ApiCancelled => OrderStatusName::ApiCancelled,
                            OrderStatusCode::Inactive => OrderStatusName::Inactive,
                        },
                        limit_price: o.limit_price,
                        stop_price: o.aux_price,
                        avg_fill_price: if o.avg_fill_price > 0.0 {
                            Some(o.avg_fill_price)
                        } else {
                            None
                        },
                        parent_ref: o.parent_id.and_then(|p| s.order_refs.get(&p).cloned()),
                    }
                })
                .collect()
        })
    }

    fn position_for(&self, symbol: &str) -> Option<PositionSnapshot> {
        self.with_state(|s| {
            s.engine
                .orders
                .account()
                .positions
                .iter()
                .find(|(k, _)| k.symbol == symbol)
                .map(|(k, p)| PositionSnapshot {
                    symbol: k.symbol.clone(),
                    quantity: p.shares,
                    avg_cost: p.avg_cost,
                    realized_pnl: p.realized_pnl,
                    unrealized_pnl: 0.0,
                })
        })
    }

    fn positions(&self) -> Vec<PositionSnapshot> {
        self.with_state(|s| {
            s.engine
                .orders
                .account()
                .positions
                .iter()
                .map(|(k, p)| PositionSnapshot {
                    symbol: k.symbol.clone(),
                    quantity: p.shares,
                    avg_cost: p.avg_cost,
                    realized_pnl: p.realized_pnl,
                    unrealized_pnl: 0.0,
                })
                .collect()
        })
    }

    fn session_metrics(&self, id: u64) -> Option<SessionMetrics> {
        self.with_state(|s| {
            let exists = s.engine.sessions.contains_key(&SessionId(id));
            Some(SessionMetrics {
                msg_count: 0,
                msg_count_last_5s: 0,
                tick_count: 0,
                connected: exists,
            })
        })
    }

    fn session_duration(&self) -> Duration {
        self.clock.now().as_duration()
    }
}

impl ScenarioEngine for RealScenarioEngine {
    fn clone_handle(&self) -> Box<dyn ScenarioEngine> {
        Box::new(self.handle())
    }

    fn seed_price(&self, symbol: &str, price: f64) {
        self.with_state(|s| {
            s.prices.insert(symbol.to_string(), price);
        });
    }

    fn tick_duration_to(&self, _now: VirtualInstant) {
        // Real engine reads the clock directly; no duration mirror needed.
    }

    fn accept(&self, cmd: EngineCmd) -> Option<MockCmd> {
        let projected = Self::project_cmd(&cmd);

        // Remember which order_id we assigned so auto-fill + order_ref
        // attachment stay in sync.
        let track_order = match &cmd {
            EngineCmd::PlaceOrder { req, .. } => Some((req.order_id, req.account.clone())),
            _ => None,
        };

        self.with_state(|s| {
            // Run the command through the actual engine.
            s.engine.handle_command(cmd);
            // Drain any synchronous scheduled actions.
            s.engine.drive_market_data();

            if let Some((oid, account)) = track_order {
                s.last_placed = Some(oid);
                let default_ref = format!("{account}/{}", oid.0);
                s.order_refs.insert(oid, default_ref.clone());
                s.ref_to_id.insert(default_ref, oid);
                // Schedule the auto-fill deadline (virtual now + MOCK_FILL_DELAY).
                let fill_at = VirtualInstant::from_duration(
                    s.engine.clock.now().as_duration() + MOCK_FILL_DELAY,
                );
                s.fill_at.insert(oid, fill_at);
            }

            if let Some(p) = &projected {
                s.outgoing.push(p.clone());
            }
        });
        projected
    }

    fn record(&self, cmd: MockCmd) {
        self.record_into(cmd);
    }

    fn advance_fills(&self, now: VirtualInstant) {
        // The real orders::BasicOrderSimulator fills on MarketSnapshot
        // delivery; the scenario-runtime's auto-fill short-circuits that so
        // recordings stay stable regardless of synthetic-engine state.
        self.with_state(|s| {
            let due: Vec<OrderId> = s
                .fill_at
                .iter()
                .filter_map(|(oid, fill_at)| {
                    if !s.auto_filled.contains(oid) && fill_at.as_duration() <= now.as_duration() {
                        Some(*oid)
                    } else {
                        None
                    }
                })
                .collect();
            for oid in due {
                // Find the order, fabricate a fill price from the seeded
                // prices, mark filled, bump position via a manual snapshot.
                let (contract, side, qty, account) = {
                    let Some(rec) = s.engine.orders.orders().get(&oid).cloned() else {
                        continue;
                    };
                    (
                        rec.contract.clone(),
                        rec.side,
                        rec.total_qty,
                        rec.account.clone(),
                    )
                };
                let symbol = contract.symbol().to_string();
                let price = s.prices.get(&symbol).copied().unwrap_or(100.0);
                // Build a MarketSnapshot for the order sim so the fill path
                // produces Execution + Position emissions authentically.
                let snap = crate::engine::types::MarketSnapshot {
                    symbol: crate::orders::accounts::symbol_key_for(&contract),
                    mid: price,
                    bid: price - 0.01,
                    ask: price + 0.01,
                    last: price,
                    volume: Some(100),
                    ts: now,
                };
                let _ = s.engine.orders.on_market_snapshot(&snap);
                // The order may still be in a non-terminal state if the
                // simulator's fill model declined (e.g., limit order pricing
                // mismatch). In that case force-fill via a direct internal
                // transition so scenarios converge deterministically.
                if let Some(rec) = s.engine.orders.orders_mut_api().get_mut(&oid) {
                    if rec.status != OrderStatusCode::Filled {
                        rec.filled_qty = qty;
                        rec.remaining_qty = 0.0;
                        rec.avg_fill_price = price;
                        let _ = rec.transition(OrderStatusCode::Filled);
                    }
                }
                let _ = side;
                let _ = account;
                s.auto_filled.insert(oid);
            }
        });
    }

    fn next_fill_deadline(&self) -> Option<VirtualInstant> {
        self.with_state(|s| {
            s.fill_at
                .iter()
                .filter(|(oid, _)| !s.auto_filled.contains(oid))
                .map(|(_, at)| *at)
                .max()
        })
    }

    fn next_order_id(&self) -> OrderId {
        self.with_state(|s| {
            let id = s.next_order_id;
            s.next_order_id += 1;
            OrderId(id)
        })
    }

    fn attach_order_ref(&self, order_ref: &str) {
        self.with_state(|s| {
            if let Some(oid) = s.last_placed {
                s.order_refs.insert(oid, order_ref.to_string());
                s.ref_to_id.insert(order_ref.to_string(), oid);
            }
        });
    }

    fn cancel_by_ref(&self, order_ref: &str) -> bool {
        let maybe_oid = self.with_state(|s| s.ref_to_id.get(order_ref).copied());
        let Some(oid) = maybe_oid else {
            return false;
        };
        // Fire an actual CancelOrder through the real engine so the order
        // simulator sees the transition.
        self.with_state(|s| {
            s.engine.handle_command(EngineCmd::CancelOrder {
                session: SessionId(0),
                order_id: oid,
            });
            // If the order is still not in a cancelled state (e.g., already
            // filled by auto-fill before the cancel arrived), flip it by
            // direct transition so scenarios that cancel SL legs post-fill
            // still produce a deterministic `Cancelled` status.
            if let Some(rec) = s.engine.orders.orders_mut_api().get_mut(&oid) {
                if !matches!(
                    rec.status,
                    OrderStatusCode::Cancelled | OrderStatusCode::Filled
                ) {
                    let _ = rec.transition(OrderStatusCode::Cancelled);
                }
            }
        });
        true
    }

    fn outgoing(&self) -> Vec<MockCmd> {
        self.with_state(|s| s.outgoing.clone())
    }
}
