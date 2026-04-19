//! Engine orchestrator — wires Stage 03 (market data) + Stage 04 (orders) +
//! Stage 05 (quirks) into the single `Engine` actor whose shell Stage 01
//! froze.
//!
//! Wave 3 Part A implementation of `plan/ib-sim/01-architecture.md`
//! §"Engine actor shape".
//!
//! The actor owns four cross-cutting concerns:
//!
//! 1. **Sessions**: per-connection state records (`SessionState`) keyed by
//!    `SessionId`. Each holds an outbound broadcast sender for wire frames.
//! 2. **Subscriptions**: live market-data subscriptions keyed by `SubKey`.
//!    We remember each subscription's `ContractSpec` because the market-
//!    data engine emits by [`SubKey`] but the order simulator consumes
//!    `MarketSnapshot`s keyed by [`midas_broker_core::SymbolKey`] derived
//!    from the contract — the two hashes differ (see
//!    `orders::accounts::symbol_key_for` vs. `scenario::injector`'s
//!    `synth_contract_id`), so the orchestrator owns the translation.
//! 3. **Order execution**: placed orders flow through the quirk guard, then
//!    into the order simulator. The simulator emits `OrderEmission`s which
//!    we broadcast to all sessions that reference the order.
//! 4. **Scheduling**: periodic market-data ticks and scripted Pattern-B fill
//!    sequences live in the [`EventScheduler`].
//!
//! ## Cadence
//!
//! Every command / scheduled-action cycle ends with a call to
//! [`Engine::drive_market_data`] which advances the market-data engine to
//! `now`, projects emissions into `MarketSnapshot`s, and walks them through
//! the order simulator. This keeps fills in lockstep with the market's
//! virtual time without a dedicated side task.
//!
//! ## ScenarioQuery
//!
//! The [`Engine`] implements [`crate::scenario::expr::ScenarioQuery`]. DSL
//! `when:` / `assert` clauses read through this trait so the same scenario
//! YAML runs under either `MockEngine` or the real engine.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::Duration;

use midas_broker_core::{ContractSpec, SymbolKey};

use crate::engine::clock::{Clock, VirtualInstant};
use crate::engine::scheduler::{EngineAction, EventScheduler};
use crate::engine::state::SessionState;
use crate::engine::types::{
    EngineCmd, EngineEvent, EngineSnapshot, HistoricalReq, MarketDataType, MarketEmission,
    MarketSnapshot, OpenOrderSummary, OrderEmission, OrderId, OrderKind, OrderStatus,
    OrderStatusCode, PlaceOrderReq, QuirkCounters, QuirkViolation, ReqId, SessionId,
    SessionSummary, Side, SubKey, SubMode, SubscriptionSummary, TickType, ViolationAction,
};
use crate::market_data::MarketDataEngine;
use crate::orders::accounts::symbol_key_for;
use crate::orders::BasicOrderSimulator;
use crate::orders::OrderSimulator;
use crate::quirks::{
    error_codes, CompositeQuirkGuard, QuirkCheckCtx, QuirkCheckKind, QuirkGuard, QuirksConfig,
};

use tokio::sync::{broadcast, mpsc};
use tracing::{debug, trace, warn};

// Channel capacities — mirror the constants in `engine::mod`.
pub use crate::engine::{ENGINE_CMD_CHANNEL_CAP, ENGINE_EVENT_CHANNEL_CAP};

/// State we track per live market-data subscription. The contract spec lives
/// here so fills keyed by `symbol_key_for(contract)` can be resolved from a
/// `SubKey`.
#[derive(Clone, Debug)]
pub struct SubState {
    pub contract: ContractSpec,
    pub mode: SubMode,
    pub data_type: MarketDataType,
}

// ---------------------------------------------------------------------------
// Orchestrator entry points
// ---------------------------------------------------------------------------

/// Build a production-shaped Engine using the given market-data engine and a
/// fresh `BasicOrderSimulator` + `CompositeQuirkGuard`.
#[allow(clippy::too_many_arguments)]
pub fn build_engine(
    clock: Arc<dyn Clock>,
    market_data: Box<dyn MarketDataEngine>,
    quirks_cfg: &QuirksConfig,
    seed: u64,
) -> (
    OrchestratedEngine,
    mpsc::Sender<EngineCmd>,
    broadcast::Receiver<EngineEvent>,
) {
    let (cmd_tx, cmd_rx) = mpsc::channel(ENGINE_CMD_CHANNEL_CAP);
    let (event_tx, event_rx) = broadcast::channel(ENGINE_EVENT_CHANNEL_CAP);
    let orders = BasicOrderSimulator::with_seed(Arc::clone(&clock), seed);
    let quirks = CompositeQuirkGuard::from_config(Arc::clone(&clock), quirks_cfg);
    let engine = OrchestratedEngine {
        clock,
        scheduler: EventScheduler::new(),
        market_data,
        orders,
        quirks,
        sessions: BTreeMap::new(),
        subscriptions: HashMap::new(),
        contract_to_sym: HashMap::new(),
        order_sessions: BTreeMap::new(),
        order_contracts: BTreeMap::new(),
        quirk_counters: QuirkCounters::default(),
        command_rx: cmd_rx,
        event_tx,
        pending_emissions: Vec::new(),
    };
    (engine, cmd_tx, event_rx)
}

/// The orchestrated engine actor. Parallel implementation of
/// [`crate::engine::Engine`] that keeps generic bounds out of the public
/// facade while allowing Wave 3's concrete component wiring.
///
/// The public [`crate::engine::Engine`] still carries `Box<dyn>`s for the
/// market-data + quirk guard slots; this struct narrows the quirks slot to
/// the concrete composite because the actor calls `forget_session` /
/// `release_l1` which are not on the trait.
pub struct OrchestratedEngine {
    pub clock: Arc<dyn Clock>,
    pub scheduler: EventScheduler,
    pub market_data: Box<dyn MarketDataEngine>,
    pub orders: BasicOrderSimulator,
    pub quirks: CompositeQuirkGuard,
    /// Per-session state.
    pub sessions: BTreeMap<SessionId, SessionState>,
    /// Active subscriptions, keyed by `(session, req_id, symbol)`. HashMap
    /// because `SubKey`'s central-type freeze omits `Ord`; snapshot
    /// projection sorts deterministically on the way out.
    pub subscriptions: HashMap<SubKey, SubState>,
    /// Cache: `contract_id (from SubKey::symbol)` → `symbol_key_for(contract)`
    /// so emissions translate to snapshots in O(1).
    pub contract_to_sym: HashMap<i32, SymbolKey>,
    /// Which session owns which order.
    pub order_sessions: BTreeMap<OrderId, SessionId>,
    /// Contract we booked the order against — used when projecting fills
    /// back to per-session snapshot summaries.
    pub order_contracts: BTreeMap<OrderId, ContractSpec>,
    pub quirk_counters: QuirkCounters,
    pub command_rx: mpsc::Receiver<EngineCmd>,
    pub event_tx: broadcast::Sender<EngineEvent>,
    /// Emissions gathered during the current command cycle, flushed at the
    /// end of [`Engine::drive_market_data`]. Public for test-harness use.
    pub pending_emissions: Vec<MarketEmission>,
}

impl OrchestratedEngine {
    /// Run forever. Exits when the command channel closes. Wave 3 integration
    /// does not hit this path — scenarios drive the engine explicitly through
    /// [`Self::tick`].
    pub async fn run(&mut self) {
        loop {
            let next_deadline = self.scheduler.peek_deadline();
            tokio::select! {
                maybe_cmd = self.command_rx.recv() => {
                    match maybe_cmd {
                        Some(cmd) => { self.handle_command(cmd); }
                        None => {
                            debug!("engine: command channel closed");
                            break;
                        }
                    }
                }
                _ = async {
                    match next_deadline {
                        Some(d) => self.clock.sleep_until(d).await,
                        None => std::future::pending::<()>().await,
                    }
                } => {
                    let now = self.clock.now();
                    while let Some(action) = self.scheduler.pop_if_due(now) {
                        self.handle_scheduled(action);
                    }
                }
            }
            self.drive_market_data();
        }
    }

    /// Tick the engine once: drain the command queue, fire due scheduled
    /// actions, drive market data. Used by tests + scenario runners that
    /// control the clock directly.
    pub fn tick(&mut self) {
        while let Ok(cmd) = self.command_rx.try_recv() {
            self.handle_command(cmd);
        }
        let now = self.clock.now();
        while let Some(action) = self.scheduler.pop_if_due(now) {
            self.handle_scheduled(action);
        }
        self.drive_market_data();
    }

    /// Advance the market-data engine to `now`, project each emission into a
    /// `MarketSnapshot`, feed snapshots to the order simulator, and publish
    /// all resulting emissions as `EngineEvent`s. Also drains any deferred
    /// order emissions (Wave-4 slow-commission-report path) whose deadlines
    /// have come due.
    pub fn drive_market_data(&mut self) {
        let now = self.clock.now();
        let emissions = self.market_data.step(now);
        for em in emissions {
            if let Some(snap) = self.project_snapshot(&em, now) {
                let order_events = self.orders.on_market_snapshot(&snap);
                for oe in order_events {
                    self.record_order_emission(&oe);
                }
            }
            self.pending_emissions.push(em);
        }
        // Drain any deferred (slow-commission) emissions that became due
        // since the last tick. Uses the `OrderSimulator` trait method so
        // test doubles can participate in the same path.
        let due = <BasicOrderSimulator as OrderSimulator>::drain_due(&mut self.orders, now);
        for oe in due {
            self.record_order_emission(&oe);
        }
    }

    fn project_snapshot(&self, em: &MarketEmission, now: VirtualInstant) -> Option<MarketSnapshot> {
        let (key, price) = match em {
            MarketEmission::TickPrice {
                key,
                price,
                tick: TickType::Last | TickType::Bid | TickType::Ask | TickType::MarkPrice,
                ..
            } => (key, *price),
            _ => return None,
        };
        // Prefer the order-sim-compatible SymbolKey we stashed at subscribe
        // time; fall back to the market-data one if we somehow missed it.
        let symbol = self
            .contract_to_sym
            .get(&key.symbol.contract_id)
            .cloned()
            .unwrap_or_else(|| key.symbol.clone());
        let md_snap = self.market_data.snapshot(&key.symbol);
        let bid = md_snap.as_ref().map(|s| s.bid).unwrap_or(price - 0.01);
        let ask = md_snap.as_ref().map(|s| s.ask).unwrap_or(price + 0.01);
        Some(MarketSnapshot {
            symbol,
            mid: (bid + ask) / 2.0,
            bid,
            ask,
            last: price,
            volume: md_snap.and_then(|s| s.volume),
            ts: now,
        })
    }

    // ------------------------------------------------------------------
    // Command dispatch
    // ------------------------------------------------------------------

    /// Route a single command to the right handler.
    pub fn handle_command(&mut self, cmd: EngineCmd) {
        match cmd {
            EngineCmd::StartApi { session, client_id } => {
                self.on_start_api(session, client_id);
            }
            EngineCmd::PlaceOrder { session, req } => {
                self.on_place_order(session, req);
            }
            EngineCmd::CancelOrder { session, order_id } => {
                self.on_cancel_order(session, order_id);
            }
            EngineCmd::SubscribeMarketData {
                session,
                req_id,
                contract,
                mode,
            } => {
                self.on_subscribe_market_data(session, req_id, contract, mode);
            }
            EngineCmd::UnsubscribeMarketData { session, req_id } => {
                self.on_unsubscribe_market_data(session, req_id);
            }
            EngineCmd::ReqContractData { .. }
            | EngineCmd::ReqPositions { .. }
            | EngineCmd::ReqAccountSummary { .. }
            | EngineCmd::ReqAccountData { .. }
            | EngineCmd::ReqExecutions { .. }
            | EngineCmd::ReqGlobalCancel { .. }
            | EngineCmd::ReqCurrentTime { .. }
            | EngineCmd::ReqIds { .. }
            | EngineCmd::ReqMarketDataType { .. } => {
                // Wave 3 keeps these as no-ops — the scenario DSL never
                // drives them and Wave 4 (Stage 09) wires them into
                // midas-broker's bootstrap path.
            }
            EngineCmd::ReqHistoricalData {
                session,
                req_id,
                req,
            } => {
                self.on_req_historical_data(session, req_id, &req);
            }
            EngineCmd::ReqRealTimeBars { .. } => {
                // Wave 3 stub: real-time 5s bars are an adjunct to streaming
                // L1. The synthetic engine produces them via its own SubMode
                // branch; we don't route through quirks here.
            }
            EngineCmd::InjectDisconnect { session, reason } => {
                self.on_inject_disconnect(session, reason);
            }
            EngineCmd::InjectLag { session, duration } => {
                debug!(session = ?session, ?duration, "inject_lag recorded (no-op in Wave 3)");
            }
            EngineCmd::InjectPacingViolation { session } => {
                self.on_inject_pacing_violation(session);
            }
            EngineCmd::InjectFarmOutage { code, farms } => {
                self.emit_event(EngineEvent::FarmStatusChanged { code, farms });
            }
            EngineCmd::InjectFarmRestore { code, farms } => {
                self.emit_event(EngineEvent::FarmStatusChanged { code, farms });
            }
            EngineCmd::InjectPriceJump {
                symbol,
                magnitude_pct,
            } => {
                self.on_inject_price_jump(&symbol, magnitude_pct);
            }
            EngineCmd::InjectGap { symbol, from, to } => {
                self.on_inject_gap(&symbol, from, to);
            }
            EngineCmd::InjectHalt { symbol, duration } => {
                self.on_inject_halt(&symbol, duration);
            }
            EngineCmd::InjectBurst {
                symbols,
                multiplier,
                duration,
            } => {
                self.on_inject_burst(&symbols, multiplier, duration);
            }
            EngineCmd::InjectDailyRestart => {
                self.on_inject_daily_restart();
            }
            EngineCmd::LoadScenario(_) => {
                // Scenarios are loaded by the runner, not forwarded to the
                // engine. Wave 4 may surface this if the control plane
                // wants to trigger mid-run scenario swap.
            }
            EngineCmd::DumpState { reply } => {
                let _ = reply.send(self.snapshot());
            }
            EngineCmd::Tick(_) => {
                // Scheduler tick is handled by the outer loop; a direct Tick
                // command is a no-op.
            }
        }
    }

    /// Dispatch a scheduled action popped from the priority queue.
    pub fn handle_scheduled(&mut self, action: EngineAction) {
        match action {
            EngineAction::EmitTick { .. } => {
                // Wave 3: synthetic engine emits on its own step cadence,
                // so explicit EmitTick scheduling is unused. Keep the arm
                // so Wave 4 can add scripted ticks without breaking the
                // central-types freeze.
            }
            EngineAction::EmitFillPatternStep { .. } => {
                // Pattern-B fills are emitted in-line by the order simulator
                // today. The scheduler path is reserved for the
                // "Slow-Commission-Report" quirk (Wave 4).
            }
            EngineAction::EmitFarmStatus { code, farm, up } => {
                let farms = vec![farm];
                let _ = up;
                self.emit_event(EngineEvent::FarmStatusChanged { code, farms });
            }
            EngineAction::EmitDailyRestart => {
                self.on_inject_daily_restart();
            }
            EngineAction::DeliverHistoricalBatch { .. } => {
                // Wave 3 does not yet emit historical batches on a timer;
                // the scenario DSL exercises historical-pacing quirks only.
            }
            EngineAction::Deferred(payload) => {
                trace!(description = %payload.description, "deferred action fired (no-op)");
            }
        }
    }

    // ------------------------------------------------------------------
    // Command handlers
    // ------------------------------------------------------------------

    fn on_start_api(&mut self, session: SessionId, client_id: i32) {
        let session_state = self.sessions.entry(session).or_insert_with(|| {
            let (tx, _rx) = broadcast::channel(64);
            SessionState::new(session, client_id, String::new(), self.clock.now(), tx)
        });
        session_state.client_id = client_id;
        self.emit_event(EngineEvent::Connected { session, client_id });
    }

    fn on_place_order(&mut self, session: SessionId, req: PlaceOrderReq) {
        let ctx = QuirkCheckCtx {
            session,
            req_id: None,
            kind: QuirkCheckKind::OrderPlace,
        };
        if let Err(violation) = self.quirks.check(ctx) {
            self.handle_violation(session, violation);
            return;
        }

        let order_id = req.order_id;
        let contract = req.contract.clone();
        let events = self.orders.place(req);
        self.order_sessions.insert(order_id, session);
        self.order_contracts.insert(order_id, contract);
        // Register session's order list for global-cancel scoping.
        if let Some(s) = self.sessions.get_mut(&session) {
            s.owned_orders.insert(order_id);
        }
        for e in &events {
            self.record_order_emission(e);
        }
        self.emit_event(EngineEvent::OrderPlaced { session, order_id });
    }

    fn on_cancel_order(&mut self, session: SessionId, order_id: OrderId) {
        let events = self.orders.cancel(order_id);
        for e in &events {
            self.record_order_emission(e);
        }
        self.emit_event(EngineEvent::OrderCancelled { session, order_id });
    }

    fn on_subscribe_market_data(
        &mut self,
        session: SessionId,
        req_id: ReqId,
        contract: ContractSpec,
        mode: SubMode,
    ) {
        let symbol = self.market_data_symbol_key(&contract);
        let key = SubKey {
            session,
            req_id,
            symbol: symbol.clone(),
        };

        let kind = match &mode {
            SubMode::StreamingL1 { .. } => QuirkCheckKind::L1Subscribe { symbol: &symbol },
            SubMode::TickByTick { .. } => QuirkCheckKind::TickByTickSubscribe { symbol: &symbol },
            SubMode::Historical(req) => QuirkCheckKind::HistoricalRequest { req },
            SubMode::RealtimeBars5s => QuirkCheckKind::L1Subscribe { symbol: &symbol },
        };
        let ctx = QuirkCheckCtx {
            session,
            req_id: Some(req_id),
            kind,
        };
        if let Err(violation) = self.quirks.check(ctx) {
            self.handle_violation(session, violation);
            return;
        }

        if let Err(e) = self.market_data.subscribe(key.clone(), mode.clone()) {
            warn!(error = %e, "subscribe failed");
            // No dedicated Quirk variant for subscription errors beyond
            // the ones above; emit an UnknownContract-like event.
            self.handle_violation(
                session,
                QuirkViolation::UnknownContract {
                    code: error_codes::NO_SECURITY_DEF,
                    message: e.to_string(),
                    req_id,
                },
            );
            return;
        }

        // Remember contract + symbol mapping so fills resolve back.
        self.contract_to_sym
            .insert(symbol.contract_id, symbol_key_for(&contract));
        self.subscriptions.insert(
            key.clone(),
            SubState {
                contract,
                mode,
                data_type: MarketDataType::Live,
            },
        );
        if let Some(s) = self.sessions.get_mut(&session) {
            s.streaming_reqs.insert(req_id);
        }
        self.emit_event(EngineEvent::MarketDataSubscribed {
            session,
            req_id,
            symbol,
        });
    }

    fn on_unsubscribe_market_data(&mut self, session: SessionId, req_id: ReqId) {
        let key_opt = self
            .subscriptions
            .keys()
            .find(|k| k.session == session && k.req_id == req_id)
            .cloned();
        if let Some(key) = key_opt {
            self.market_data.unsubscribe(&key);
            self.subscriptions.remove(&key);
            self.quirks.release_l1(session, req_id);
        }
        if let Some(s) = self.sessions.get_mut(&session) {
            s.streaming_reqs.remove(&req_id);
        }
        self.emit_event(EngineEvent::MarketDataUnsubscribed { session, req_id });
    }

    fn on_req_historical_data(&mut self, session: SessionId, req_id: ReqId, req: &HistoricalReq) {
        let ctx = QuirkCheckCtx {
            session,
            req_id: Some(req_id),
            kind: QuirkCheckKind::HistoricalRequest { req },
        };
        if let Err(violation) = self.quirks.check(ctx) {
            self.handle_violation(session, violation);
        }
    }

    fn on_inject_disconnect(&mut self, session: SessionId, reason: String) {
        if self.sessions.remove(&session).is_some() {
            self.quirks.forget_session(session);
        }
        self.emit_event(EngineEvent::Disconnected { session, reason });
    }

    fn on_inject_pacing_violation(&mut self, session: SessionId) {
        let violation = QuirkViolation::RateLimit {
            code: error_codes::MSG_RATE_EXCEEDED,
            message: error_codes::message(error_codes::MSG_RATE_EXCEEDED).into(),
            action: ViolationAction::DisconnectAfterError,
        };
        self.handle_violation(session, violation);
    }

    fn on_inject_price_jump(&mut self, symbol: &SymbolKey, magnitude_pct: f64) {
        let now = self.clock.now();
        if let Err(e) = self.market_data.inject_jump(symbol, magnitude_pct, now) {
            warn!(
                symbol = ?symbol,
                error = %e,
                "inject_price_jump: market-data engine rejected perturbation",
            );
        }
    }

    fn on_inject_gap(&mut self, symbol: &SymbolKey, from: f64, to: f64) {
        let now = self.clock.now();
        if let Err(e) = self.market_data.inject_gap(symbol, from, to, now) {
            warn!(
                symbol = ?symbol,
                error = %e,
                "inject_gap: market-data engine rejected perturbation",
            );
        }
    }

    fn on_inject_halt(&mut self, symbol: &SymbolKey, duration: Duration) {
        let now = self.clock.now();
        if let Err(e) = self.market_data.inject_halt(symbol, duration, now) {
            warn!(
                symbol = ?symbol,
                error = %e,
                "inject_halt: market-data engine rejected perturbation",
            );
        }
    }

    fn on_inject_burst(&mut self, symbols: &[SymbolKey], multiplier: f64, duration: Duration) {
        let now = self.clock.now();
        if let Err(e) = self
            .market_data
            .inject_burst(symbols, multiplier, duration, now)
        {
            warn!(
                symbols = ?symbols,
                error = %e,
                "inject_burst: market-data engine rejected perturbation",
            );
        }
    }

    fn on_inject_daily_restart(&mut self) {
        // Drop every session + clear quirk bookkeeping. Farm-status event
        // carries code 1300 to mirror real IB.
        let sessions: Vec<SessionId> = self.sessions.keys().copied().collect();
        for s in sessions {
            self.quirks.forget_session(s);
            self.sessions.remove(&s);
            self.emit_event(EngineEvent::Disconnected {
                session: s,
                reason: "TWS daily restart".into(),
            });
        }
        self.emit_event(EngineEvent::FarmStatusChanged {
            code: error_codes::TWS_DAILY_RESTART,
            farms: vec!["tws".into()],
        });
    }

    fn handle_violation(&mut self, session: SessionId, violation: QuirkViolation) {
        match &violation {
            QuirkViolation::RateLimit { action, .. } => {
                self.quirk_counters.rate_limit_triggers += 1;
                self.emit_event(EngineEvent::QuirkTriggered {
                    session,
                    violation: violation.clone(),
                });
                if matches!(action, ViolationAction::DisconnectAfterError) {
                    self.on_inject_disconnect(session, "pacing violation".into());
                }
            }
            QuirkViolation::LineLimit { .. } => {
                self.quirk_counters.line_limit_triggers += 1;
                self.emit_event(EngineEvent::QuirkTriggered { session, violation });
            }
            QuirkViolation::HistoricalPacing { .. } => {
                self.quirk_counters.historical_pacing_triggers += 1;
                self.emit_event(EngineEvent::QuirkTriggered { session, violation });
            }
            QuirkViolation::TickByTickLimit { .. } => {
                self.quirk_counters.tick_by_tick_triggers += 1;
                self.emit_event(EngineEvent::QuirkTriggered { session, violation });
            }
            _ => {
                self.emit_event(EngineEvent::QuirkTriggered { session, violation });
            }
        }
    }

    /// Translate a ContractSpec into the SymbolKey the market-data engine
    /// expects. Synthetic / replay engines both use a deterministic hash on
    /// the symbol string.
    fn market_data_symbol_key(&self, contract: &ContractSpec) -> SymbolKey {
        let sym = contract.symbol();
        // djb2 — matches the injector's `synth_contract_id`.
        let mut hash = 5381i32;
        for b in sym.bytes() {
            hash = hash.wrapping_mul(33).wrapping_add(b as i32);
        }
        let contract_id = (hash ^ 0x5f5f5f5f).unsigned_abs() as i32;
        SymbolKey {
            contract_id,
            symbol: sym.to_string(),
        }
    }

    fn emit_event(&self, event: EngineEvent) {
        // Fan-out failure is not fatal — no receivers just means nobody
        // cares about engine events (e.g. standalone tests).
        let _ = self.event_tx.send(event);
    }

    fn record_order_emission(&mut self, emission: &OrderEmission) {
        if let OrderEmission::Execution(e) = emission {
            self.emit_event(EngineEvent::FillObserved {
                order_id: e.order_id,
                price: e.price,
                shares: e.shares,
            });
        }
    }

    /// Schedule a deferred action (public for scenario-driver tests).
    pub fn schedule_after(&mut self, delay: Duration, action: EngineAction) {
        let deadline = self.clock.now().saturating_add(delay);
        self.scheduler.schedule(deadline, action);
    }

    // ------------------------------------------------------------------
    // Snapshot projection — feeds `/control/dump` + ScenarioQuery impl.
    // ------------------------------------------------------------------

    pub fn snapshot(&self) -> EngineSnapshot {
        let sessions = self
            .sessions
            .values()
            .map(|s| SessionSummary {
                session: s.id,
                client_id: s.client_id,
                peer: s.peer_addr.clone(),
                connected_at: Some(s.connected_at),
                msgs_in: s.msgs_in,
                msgs_out: s.msgs_out,
            })
            .collect();

        let open_orders = self
            .orders
            .orders()
            .values()
            .filter(|o| !o.is_filled() && !o.is_cancelled())
            .map(|o| OpenOrderSummary {
                order_id: o.order_id,
                symbol: self.order_contracts.get(&o.order_id).map(symbol_key_for),
                side: Some(o.side),
                kind: Some(o.kind),
                status: Some(o.status),
                remaining: o.remaining_qty,
            })
            .collect();

        let mut active_subscriptions: Vec<SubscriptionSummary> = self
            .subscriptions
            .keys()
            .map(|k| SubscriptionSummary {
                session: k.session,
                req_id: k.req_id,
                symbol: Some(k.symbol.clone()),
            })
            .collect();
        active_subscriptions.sort_by_key(|s| (s.session.0, s.req_id.0));

        EngineSnapshot {
            now: Some(self.clock.now()),
            sessions,
            open_orders,
            active_subscriptions,
            scheduler_queue_depth: self.scheduler.len(),
            quirks: self.quirk_counters.clone(),
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers + tests
// ---------------------------------------------------------------------------

/// Find the session id that placed `order_id`. Returns `None` if the order
/// was placed outside the actor (e.g. through a test shim) or already
/// forgotten.
impl OrchestratedEngine {
    pub fn session_for_order(&self, order_id: OrderId) -> Option<SessionId> {
        self.order_sessions.get(&order_id).copied()
    }

    pub fn owns_contract(&self, order_id: OrderId) -> Option<&ContractSpec> {
        self.order_contracts.get(&order_id)
    }
}

// Re-export Side/OrderKind helpers so callers importing the orchestrator
// don't need a second `use`.
pub use crate::engine::types::{OrderKind as OrderKind2, Side as OrderSide2};

// ---------------------------------------------------------------------------
// ScenarioQuery direct impl (Wave 4)
// ---------------------------------------------------------------------------
//
// `MarketDataEngine` is `Send + Sync` since Wave 4 (the default impls are
// single-threaded state machines with no interior mutability — declaring
// `Sync` is purely a bound-level change). That makes `OrchestratedEngine`
// `Send + Sync` too, so it can implement [`crate::scenario::expr::ScenarioQuery`]
// directly without any `Arc<Mutex<_>>` wrapper.
//
// The adapter in `scenario::engine_adapter` still wraps the engine in a
// `Mutex` because it needs interior mutability for the `ScenarioEngine`
// trait's `&self`-mutating methods (`accept`, `advance_fills`, …). But its
// `ScenarioQuery` impl delegates to the direct one here, keeping a single
// source of truth.

impl crate::scenario::expr::ScenarioQuery for OrchestratedEngine {
    fn orders(&self) -> Vec<crate::scenario::expr::OrderSnapshot> {
        use crate::scenario::expr::OrderSnapshot;
        self.orders
            .orders()
            .values()
            .map(|o| OrderSnapshot {
                order_ref: format!("{}/{}", o.account, o.order_id.0),
                symbol: o.contract.symbol().to_string(),
                side: match o.side {
                    Side::Buy => "buy".into(),
                    Side::Sell => "sell".into(),
                },
                quantity: o.total_qty,
                filled_qty: o.filled_qty,
                remaining_qty: o.remaining_qty,
                status: status_code_to_name(o.status),
                limit_price: o.limit_price,
                stop_price: o.aux_price,
                avg_fill_price: if o.avg_fill_price > 0.0 {
                    Some(o.avg_fill_price)
                } else {
                    None
                },
                parent_ref: o.parent_id.map(|p| format!("{}/{}", o.account, p.0)),
            })
            .collect()
    }

    fn position_for(&self, symbol: &str) -> Option<crate::scenario::expr::PositionSnapshot> {
        use crate::scenario::expr::PositionSnapshot;
        self.orders
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
    }

    fn positions(&self) -> Vec<crate::scenario::expr::PositionSnapshot> {
        project_positions(self)
    }

    fn session_metrics(&self, id: u64) -> Option<crate::scenario::expr::SessionMetrics> {
        use crate::scenario::expr::SessionMetrics;
        let connected = self.sessions.contains_key(&SessionId(id));
        Some(SessionMetrics {
            msg_count: 0,
            msg_count_last_5s: 0,
            tick_count: 0,
            connected,
        })
    }

    fn session_duration(&self) -> Duration {
        self.clock.now().as_duration()
    }
}

// Compile-time assertion: `OrchestratedEngine` is `Send + Sync`. Catches
// regressions if a future field adds a `!Send` / `!Sync` type and quietly
// breaks the direct `ScenarioQuery` impl (which requires `Self: Send + Sync`
// via the trait's super-trait bound).
#[allow(dead_code)]
const _: fn() = || {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<OrchestratedEngine>();
};

// ---------------------------------------------------------------------------
// ScenarioQuery projection helpers
// ---------------------------------------------------------------------------
//
// These helpers predate the direct `impl` above; they remain public because
// Wave-4 integration tests and the `RealScenarioEngine` adapter reuse them
// when they need to project orders with custom `order_ref` naming (the
// direct impl uses a deterministic `account/order_id` scheme, but the
// adapter lets scenarios supply their own `order_ref`).

/// Translate the engine's [`crate::orders::state_machine::OrderRecord`]s into
/// the DSL-facing [`crate::scenario::expr::OrderSnapshot`] projection.
pub fn project_orders_for_query(
    engine: &OrchestratedEngine,
    order_ref_of: impl Fn(OrderId) -> String,
    parent_ref_of: impl Fn(OrderId) -> Option<String>,
) -> Vec<crate::scenario::expr::OrderSnapshot> {
    use crate::scenario::expr::OrderSnapshot;
    engine
        .orders
        .orders()
        .values()
        .map(|o| OrderSnapshot {
            order_ref: order_ref_of(o.order_id),
            symbol: o.contract.symbol().to_string(),
            side: match o.side {
                Side::Buy => "buy".into(),
                Side::Sell => "sell".into(),
            },
            quantity: o.total_qty,
            filled_qty: o.filled_qty,
            remaining_qty: o.remaining_qty,
            status: status_code_to_name(o.status),
            limit_price: o.limit_price,
            stop_price: o.aux_price,
            avg_fill_price: if o.avg_fill_price > 0.0 {
                Some(o.avg_fill_price)
            } else {
                None
            },
            parent_ref: o.parent_id.and_then(&parent_ref_of),
        })
        .collect()
}

/// Project all account positions into the DSL's [`PositionSnapshot`] shape.
pub fn project_positions(
    engine: &OrchestratedEngine,
) -> Vec<crate::scenario::expr::PositionSnapshot> {
    use crate::scenario::expr::PositionSnapshot;
    engine
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
}

/// Convenience: quick-build a market order on the fly. Used by the scenario
/// adapter when auto-filling simulated orders.
pub fn build_market_order_req(
    order_id: OrderId,
    contract: ContractSpec,
    side: Side,
    quantity: f64,
    account: String,
) -> PlaceOrderReq {
    PlaceOrderReq {
        order_id,
        contract,
        side,
        total_quantity: quantity,
        kind: OrderKind::Market,
        limit_price: None,
        aux_price: None,
        tif: "DAY".into(),
        account,
        parent_id: None,
        oca_group: None,
        transmit: true,
    }
}

/// Helper: translate an `OrderStatus` emission to the scenario-layer
/// `OrderStatusName`. Kept in the orchestrator because the real engine is
/// the sole producer of `OrderEmission::OrderStatus` in the Wave 3 path.
pub fn status_code_to_name(code: OrderStatusCode) -> crate::scenario::expr::OrderStatusName {
    use crate::scenario::expr::OrderStatusName as N;
    match code {
        OrderStatusCode::ApiPending => N::ApiPending,
        OrderStatusCode::PendingSubmit => N::PendingSubmit,
        OrderStatusCode::PreSubmitted => N::PreSubmitted,
        OrderStatusCode::Submitted => N::Submitted,
        OrderStatusCode::Filled => N::Filled,
        OrderStatusCode::PartiallyFilled => N::PartiallyFilled,
        OrderStatusCode::Cancelled => N::Cancelled,
        OrderStatusCode::ApiCancelled => N::ApiCancelled,
        OrderStatusCode::Inactive => N::Inactive,
    }
}

// `OrderStatus` not referenced above — re-export to keep the symbol live in
// public docs in case Wave 4 adds richer order summaries.
#[allow(dead_code)]
pub(crate) type _EnsureOrderStatus = OrderStatus;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::clock::VirtualClock;
    use crate::market_data::generator::{SymbolPreset, SyntheticEngine};
    use midas_broker_core::ContractSpec;

    fn stock(sym: &str) -> ContractSpec {
        ContractSpec::Stock {
            symbol: sym.into(),
            exchange: "SMART".into(),
            currency: "USD".into(),
        }
    }

    fn build(
        clock: Arc<VirtualClock>,
    ) -> (
        OrchestratedEngine,
        mpsc::Sender<EngineCmd>,
        broadcast::Receiver<EngineEvent>,
    ) {
        let mut synth = SyntheticEngine::new(42);
        synth.register(
            SymbolKey {
                contract_id: 1,
                symbol: "AAPL".into(),
            },
            SymbolPreset::Liquid,
            175.0,
        );
        let md: Box<dyn MarketDataEngine> = Box::new(synth);
        build_engine(clock as Arc<dyn Clock>, md, &QuirksConfig::default(), 42)
    }

    #[tokio::test]
    async fn start_api_emits_connected() {
        let clock = VirtualClock::shared();
        let (mut eng, _tx, mut rx) = build(clock);
        eng.handle_command(EngineCmd::StartApi {
            session: SessionId(1),
            client_id: 1,
        });
        let ev = rx.recv().await.expect("event");
        assert!(matches!(ev, EngineEvent::Connected { .. }));
    }

    #[tokio::test]
    async fn place_order_routes_through_orders() {
        let clock = VirtualClock::shared();
        let (mut eng, _tx, mut rx) = build(clock);
        eng.handle_command(EngineCmd::StartApi {
            session: SessionId(1),
            client_id: 1,
        });
        let _ = rx.recv().await;
        let req = build_market_order_req(OrderId(1), stock("AAPL"), Side::Buy, 100.0, "DU1".into());
        eng.handle_command(EngineCmd::PlaceOrder {
            session: SessionId(1),
            req,
        });
        assert_eq!(eng.order_sessions.get(&OrderId(1)), Some(&SessionId(1)));
        // We should have published an OrderPlaced event.
        let mut saw_placed = false;
        for _ in 0..4 {
            match rx.try_recv() {
                Ok(EngineEvent::OrderPlaced {
                    order_id: OrderId(1),
                    ..
                }) => {
                    saw_placed = true;
                    break;
                }
                Ok(_) => continue,
                Err(_) => break,
            }
        }
        assert!(saw_placed, "expected OrderPlaced event");
    }

    #[tokio::test]
    async fn pacing_violation_disconnects_session() {
        let clock = VirtualClock::shared();
        let (mut eng, _tx, _rx) = build(clock);
        eng.handle_command(EngineCmd::StartApi {
            session: SessionId(1),
            client_id: 1,
        });
        eng.handle_command(EngineCmd::InjectPacingViolation {
            session: SessionId(1),
        });
        assert!(!eng.sessions.contains_key(&SessionId(1)));
        assert_eq!(eng.quirk_counters.rate_limit_triggers, 1);
    }

    #[tokio::test]
    async fn subscribe_then_unsubscribe_releases_line() {
        let clock = VirtualClock::shared();
        let (mut eng, _tx, _rx) = build(clock);
        eng.handle_command(EngineCmd::StartApi {
            session: SessionId(1),
            client_id: 1,
        });
        eng.handle_command(EngineCmd::SubscribeMarketData {
            session: SessionId(1),
            req_id: ReqId(1),
            contract: stock("AAPL"),
            mode: SubMode::StreamingL1 {
                snapshot: false,
                regulatory_snapshot: false,
            },
        });
        assert_eq!(eng.subscriptions.len(), 1);
        eng.handle_command(EngineCmd::UnsubscribeMarketData {
            session: SessionId(1),
            req_id: ReqId(1),
        });
        assert_eq!(eng.subscriptions.len(), 0);
    }

    #[tokio::test]
    async fn daily_restart_drops_all_sessions() {
        let clock = VirtualClock::shared();
        let (mut eng, _tx, _rx) = build(clock);
        eng.handle_command(EngineCmd::StartApi {
            session: SessionId(1),
            client_id: 1,
        });
        eng.handle_command(EngineCmd::StartApi {
            session: SessionId(2),
            client_id: 2,
        });
        eng.handle_command(EngineCmd::InjectDailyRestart);
        assert!(eng.sessions.is_empty());
    }
}
