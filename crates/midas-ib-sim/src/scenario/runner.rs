//! Scenario runner — the DSL's execution loop.
//!
//! Interleaves three schedules:
//!
//! - **fixed-time events**: `at: 00:00:05` style, sorted by virtual offset
//! - **anchor-relative events**: `after: <name>, delay: 2s` — deadline set
//!   the moment the anchor fires, not at scenario load
//! - **pattern-triggered events**: `when: <expr>` — re-evaluated every tick
//!   against the live query; fires on the first `true`
//!
//! On each loop iteration the runner picks the earliest deadline among the
//! three sets, advances the clock, runs the action, then repeats. When no
//! events remain, it runs the scenario-end `asserts:` block.
//!
//! `when:` clauses are cheap to re-eval because the expression language has
//! no side effects.
//!
//! ## Clocks
//!
//! The runner is clock-agnostic — it owns an `Arc<dyn Clock>`. Under the
//! virtual clock, the mock engine's state is deterministic and fills happen
//! at `MOCK_FILL_DELAY` after each `PlaceOrder`. Under the real clock the
//! runner still works but fills observe wall-clock delays from the real
//! engine (which lands in Stages 03/04/05).

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use tracing::{debug, warn};

use crate::engine::clock::{Clock, VirtualInstant};

use super::engine_adapter::ScenarioEngine;
use super::expr::{self, Expr};
use super::injector;
use super::mock_engine::{MockCmd, MockEngine, MOCK_FILL_DELAY};
use super::schema::{
    AssertArgs, AssertClientEventOrderArgs, AssertClientReceivedArgs, OrderKindArg, OrderSide,
    Scenario, ScenarioEvent, SessionSelector, Verb,
};

/// Runner diagnostics.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum RunnerError {
    #[error("bad timing string `{0}`: {1}")]
    BadTiming(String, String),
    #[error("expression parse error in `{src}`: {err}")]
    ExpressionParse {
        src: String,
        #[source]
        err: expr::ParseError,
    },
    #[error("expression eval error in `{src}`: {err}")]
    ExpressionEval {
        src: String,
        #[source]
        err: expr::EvalError,
    },
    #[error("unresolved anchor `{0}`")]
    UnknownAnchor(String),
    #[error("assert failed: {cond} — {message}")]
    AssertFailed { cond: String, message: String },
    #[error("scenario timed out")]
    Timeout,
}

/// Final report returned by [`ScenarioRunner::run`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ScenarioResult {
    pub steps_executed: u32,
    pub when_clauses_fired: u32,
    pub assert_count: u32,
    pub scenario_name: String,
}

/// A scheduled event waiting in the queue — fixed- or anchor-scheduled.
#[derive(Clone, Debug)]
struct TimedEvent {
    deadline: VirtualInstant,
    name: Option<String>,
    verb: Verb,
    seq: u32,
}

#[derive(Clone, Debug)]
struct WhenEvent {
    expr: Expr,
    source: String,
    name: Option<String>,
    verb: Verb,
}

#[derive(Clone, Debug)]
struct PendingAfter {
    anchor: String,
    delay: Duration,
    name: Option<String>,
    verb: Verb,
}

// ---------------------------------------------------------------------------
// ScenarioRunner
// ---------------------------------------------------------------------------

/// The scenario runner. Drives a loaded [`Scenario`] against any
/// [`ScenarioEngine`] (either [`MockEngine`] or the Wave-3
/// [`super::engine_adapter::RealScenarioEngine`]) under a chosen [`Clock`].
///
/// Generic over the engine type so the same runner binary serves both
/// back-ends without dynamic dispatch in the hot `when:`-evaluation path.
pub struct ScenarioRunner<E: ScenarioEngine = MockEngine> {
    scenario: Scenario,
    clock: Arc<dyn Clock>,
    engine: E,
    named_anchors: BTreeMap<String, VirtualInstant>,
    fixed: Vec<TimedEvent>,
    pending_after: Vec<PendingAfter>,
    pending_when: Vec<WhenEvent>,
    steps: u32,
    when_fires: u32,
    assert_count: u32,
}

impl<E: ScenarioEngine> ScenarioRunner<E> {
    /// Build a runner from a loaded scenario. Seeds prices + session state
    /// but does not start running — call [`Self::run`].
    pub fn new(scenario: Scenario, engine: E, clock: Arc<dyn Clock>) -> Self {
        // Seed the engine with declared symbols.
        for sym in &scenario.symbols {
            engine.seed_price(&sym.symbol, sym.initial_price);
        }
        Self {
            scenario,
            clock,
            engine,
            named_anchors: BTreeMap::new(),
            fixed: Vec::new(),
            pending_after: Vec::new(),
            pending_when: Vec::new(),
            steps: 0,
            when_fires: 0,
            assert_count: 0,
        }
    }

    /// Run the scenario to completion. Returns a [`ScenarioResult`] on
    /// success or a [`RunnerError`] on the first failing assertion /
    /// expression evaluation.
    pub async fn run(mut self) -> Result<ScenarioResult, RunnerError> {
        self.index_events()?;

        loop {
            // Step 1 — fire any ready `when:` clauses first (pattern fires
            // immediately, ignoring clock advancement).
            let fired_now = self.fire_ready_when_clauses()?;
            if !fired_now.is_empty() {
                for w in fired_now {
                    self.execute_verb(&w.verb, w.name.as_deref()).await?;
                    self.when_fires += 1;
                    self.steps += 1;
                }
                continue;
            }

            // Step 2 — find earliest pending deadline (fixed, after, or
            // auto-fill).
            let next_fixed = self.fixed.first().map(|e| e.deadline);
            let next_after = self.earliest_after();
            // Consider fill deadlines only when there are pending `when:`
            // clauses — otherwise the scenario either waits on a real event
            // or exits.
            let next_fill = if !self.pending_when.is_empty() {
                self.engine.next_fill_deadline()
            } else {
                None
            };
            let next_deadline = [next_fixed, next_after, next_fill]
                .into_iter()
                .flatten()
                .min();
            let Some(deadline) = next_deadline else {
                // Nothing left to fire time-wise — if there are still `when:`
                // clauses that never became true, the scenario completes but
                // warns.
                if !self.pending_when.is_empty() {
                    warn!(
                        scenario = %self.scenario.name,
                        pending = self.pending_when.len(),
                        "scenario ran out of time-events with unfired when clauses",
                    );
                }
                break;
            };

            // Step 3 — advance clock, update mock duration + fills.
            self.clock.sleep_until(deadline).await;
            self.engine.tick_duration_to(deadline);
            self.engine.advance_fills(deadline);

            // Step 4 — dispatch the earliest event at or before `deadline`.
            // If only `next_fill` triggered this iteration, there's no event
            // to dispatch — the `when:` clause will re-evaluate at the top
            // of the loop.
            if matches!(next_fixed, Some(d) if d <= deadline)
                || matches!(next_after, Some(d) if d <= deadline)
            {
                self.dispatch_due(deadline).await?;
            }
        }

        // Flush any pending mock fills so end-of-run asserts see a stable
        // order state (orders placed near the end of the timeline still need
        // their `fill_at` deadline to tick over).
        if let Some(deadline) = self.engine.next_fill_deadline() {
            if deadline.as_duration() > self.clock.now().as_duration() {
                self.clock.sleep_until(deadline).await;
            }
            self.engine.tick_duration_to(deadline);
            self.engine.advance_fills(deadline);
        }

        // End-of-scenario asserts.
        self.run_final_asserts()?;
        self.engine.record(MockCmd::ScenarioCompleted);

        Ok(ScenarioResult {
            steps_executed: self.steps,
            when_clauses_fired: self.when_fires,
            assert_count: self.assert_count,
            scenario_name: self.scenario.name,
        })
    }

    // ------ indexing & scheduling -------------------------------------------

    fn index_events(&mut self) -> Result<(), RunnerError> {
        let events: Vec<ScenarioEvent> = self.scenario.events.clone();
        for (i, ev) in events.into_iter().enumerate() {
            if let Some(at) = &ev.at {
                let deadline = parse_at(at)?;
                if let Some(name) = &ev.named {
                    self.named_anchors.insert(name.clone(), deadline);
                }
                self.fixed.push(TimedEvent {
                    deadline,
                    name: ev.named.clone(),
                    verb: ev.verb.clone(),
                    seq: i as u32,
                });
            } else if let Some(anchor) = &ev.after {
                let delay_s = ev.delay.as_deref().ok_or_else(|| {
                    RunnerError::BadTiming(anchor.clone(), "missing delay".into())
                })?;
                let delay = expr::interpreter::parse_duration(delay_s)
                    .map_err(|e| RunnerError::BadTiming(delay_s.into(), format!("{e}")))?;
                self.pending_after.push(PendingAfter {
                    anchor: anchor.clone(),
                    delay,
                    name: ev.named.clone(),
                    verb: ev.verb.clone(),
                });
            } else if let Some(when_src) = &ev.when {
                let parsed =
                    expr::parse(&when_src.0).map_err(|e| RunnerError::ExpressionParse {
                        src: when_src.0.clone(),
                        err: e,
                    })?;
                self.pending_when.push(WhenEvent {
                    expr: parsed,
                    source: when_src.0.clone(),
                    name: ev.named.clone(),
                    verb: ev.verb.clone(),
                });
            }
        }
        // Fixed events must fire in deadline order; stable tiebreak by
        // declaration order to keep recordings deterministic.
        self.fixed
            .sort_by(|a, b| a.deadline.cmp(&b.deadline).then_with(|| a.seq.cmp(&b.seq)));
        Ok(())
    }

    fn earliest_after(&self) -> Option<VirtualInstant> {
        self.pending_after
            .iter()
            .filter_map(|p| {
                self.named_anchors
                    .get(&p.anchor)
                    .map(|anchor| anchor.saturating_add(p.delay))
            })
            .min()
    }

    fn fire_ready_when_clauses(&mut self) -> Result<Vec<WhenEvent>, RunnerError> {
        let mut fired = Vec::new();
        let mut remaining = Vec::new();
        for w in std::mem::take(&mut self.pending_when) {
            // `when:` predicates run against a partially-populated state —
            // path resolution failures mean "not yet true", not fatal. We
            // still surface type-error / unknown-function bugs.
            let result = expr::eval(&w.expr, &self.engine);
            let is_true = match result {
                Ok(v) => v.as_bool().unwrap_or(false),
                Err(expr::EvalError::PathResolve { .. }) => false,
                Err(other) => {
                    return Err(RunnerError::ExpressionEval {
                        src: w.source.clone(),
                        err: other,
                    });
                }
            };
            if is_true {
                fired.push(w);
            } else {
                remaining.push(w);
            }
        }
        self.pending_when = remaining;
        Ok(fired)
    }

    async fn dispatch_due(&mut self, deadline: VirtualInstant) -> Result<(), RunnerError> {
        // Resolve at-most-one fixed event whose deadline matches.
        if let Some(first) = self.fixed.first() {
            if first.deadline <= deadline {
                let ev = self.fixed.remove(0);
                if let Some(name) = &ev.name {
                    self.named_anchors.insert(name.clone(), ev.deadline);
                }
                self.execute_verb(&ev.verb, ev.name.as_deref()).await?;
                self.steps += 1;
                return Ok(());
            }
        }
        // Otherwise an after-event is due.
        if let Some((i, _)) = self.pending_after.iter().enumerate().find(|(_, p)| {
            self.named_anchors
                .get(&p.anchor)
                .map(|anchor| anchor.saturating_add(p.delay) <= deadline)
                .unwrap_or(false)
        }) {
            let p = self.pending_after.remove(i);
            let fire_at = self
                .named_anchors
                .get(&p.anchor)
                .copied()
                .ok_or_else(|| RunnerError::UnknownAnchor(p.anchor.clone()))?
                .saturating_add(p.delay);
            if let Some(name) = &p.name {
                self.named_anchors.insert(name.clone(), fire_at);
            }
            self.execute_verb(&p.verb, p.name.as_deref()).await?;
            self.steps += 1;
            return Ok(());
        }
        Ok(())
    }

    async fn execute_verb(&mut self, verb: &Verb, name: Option<&str>) -> Result<(), RunnerError> {
        debug!(verb = ?verb, named = ?name, "runner: execute_verb");
        match verb {
            // Scenario-local verbs — handled by the runner directly.
            Verb::Sleep(args) => {
                let d = expr::interpreter::parse_duration(&args.duration)
                    .map_err(|e| RunnerError::BadTiming(args.duration.clone(), format!("{e}")))?;
                let target = self.clock.now().saturating_add(d);
                self.clock.sleep_until(target).await;
                self.engine.tick_duration_to(target);
                self.engine.advance_fills(target);
                self.engine.record(MockCmd::Sleep {
                    duration: args.duration.clone(),
                });
            }
            Verb::SetClockMode(args) => {
                self.engine.record(MockCmd::SetClockMode {
                    mode: format!("{:?}", args.mode).to_lowercase(),
                    multiplier: args.multiplier,
                });
            }
            Verb::Include(_) => {
                // Resolved at load-time by the loader; runner never sees it.
            }
            Verb::Assert(args) => self.run_assert(args)?,
            Verb::AssertClientReceived(args) => self.run_assert_client_received(args),
            Verb::AssertClientEventOrder(args) => self.run_assert_client_event_order(args),

            // `AcceptOrder` needs a bit of runner-side glue because the
            // scenario YAML carries `order_ref` but `EngineCmd::PlaceOrder`
            // doesn't. The runner forwards, then stamps the ref on the mock.
            Verb::AcceptOrder(args) => {
                let order_id = self.engine.next_order_id();
                let cmd = injector::accept_order_to_cmd(args, order_id);
                self.engine.accept(cmd);
                if let Some(order_ref) = args
                    .order_ref
                    .clone()
                    .or_else(|| name.map(|n| n.to_string()))
                {
                    self.engine.attach_order_ref(&order_ref);
                }
                // Bracket: spawn child TP/SL legs.
                if args.order_kind == OrderKindArg::Bracket {
                    let parent_ref = args
                        .order_ref
                        .clone()
                        .unwrap_or_else(|| format!("ord-{}", order_id.0));
                    for leg in injector::bracket_children(args, order_id, &parent_ref) {
                        let child_id = self.engine.next_order_id();
                        let cmd = injector::bracket_leg_to_cmd(&leg, child_id, order_id);
                        self.engine.accept(cmd);
                        self.engine.attach_order_ref(&leg.child_ref);
                    }
                }
            }

            Verb::CancelOrder(args) => {
                // Runner resolves the string order_ref → an i32 id for the
                // engine command, then cancels the order locally (the real
                // engine will resolve via `perm_id`/`order_id` map).
                let _ = self.engine.cancel_by_ref(&args.order_ref);
                self.engine.record(MockCmd::CancelOrder {
                    order_ref: args.order_ref.clone(),
                });
            }

            // Every other verb — pure translation.
            other => {
                if let Some(cmd) = injector::verb_to_cmd(other) {
                    self.engine.accept(cmd);
                } else {
                    // Unreachable today given our verb coverage; flag so it
                    // never gets silently dropped in future additions.
                    warn!(verb = ?other, "verb produced no engine command");
                }
            }
        }
        Ok(())
    }

    fn run_assert(&mut self, args: &AssertArgs) -> Result<(), RunnerError> {
        let src = args.cond.0.clone();
        let parsed = expr::parse(&src).map_err(|e| RunnerError::ExpressionParse {
            src: src.clone(),
            err: e,
        })?;
        let v = expr::eval(&parsed, &self.engine).map_err(|e| RunnerError::ExpressionEval {
            src: src.clone(),
            err: e,
        })?;
        let passed = v.as_bool().unwrap_or(false);
        self.assert_count += 1;
        self.engine.record(MockCmd::Assert {
            cond: src.clone(),
            passed,
            message: args.message.clone(),
        });
        if !passed {
            return Err(RunnerError::AssertFailed {
                cond: src,
                message: args.message.clone().unwrap_or_default(),
            });
        }
        Ok(())
    }

    fn run_assert_client_received(&mut self, args: &AssertClientReceivedArgs) {
        // Mock engine has no wire-level bytes yet — we record the expectation
        // so replay harness can compare against the real engine later.
        self.engine.record(MockCmd::AssertClientReceived {
            session_id: session_selector_string(&args.session_id),
            message: args.message.clone(),
        });
        self.assert_count += 1;
    }

    fn run_assert_client_event_order(&mut self, args: &AssertClientEventOrderArgs) {
        self.engine.record(MockCmd::AssertClientEventOrder {
            session_id: session_selector_string(&args.session_id),
            sequence: args.sequence.clone(),
        });
        self.assert_count += 1;
    }

    fn run_final_asserts(&mut self) -> Result<(), RunnerError> {
        let asserts: Vec<_> = self.scenario.asserts.clone();
        for a in asserts {
            let src = a.cond.0.clone();
            let parsed = expr::parse(&src).map_err(|e| RunnerError::ExpressionParse {
                src: src.clone(),
                err: e,
            })?;
            let v = expr::eval(&parsed, &self.engine).map_err(|e| RunnerError::ExpressionEval {
                src: src.clone(),
                err: e,
            })?;
            let passed = v.as_bool().unwrap_or(false);
            self.assert_count += 1;
            self.engine.record(MockCmd::Assert {
                cond: src.clone(),
                passed,
                message: a.message.clone(),
            });
            if !passed {
                return Err(RunnerError::AssertFailed {
                    cond: src,
                    message: a.message.clone().unwrap_or_default(),
                });
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn parse_at(s: &str) -> Result<VirtualInstant, RunnerError> {
    let d = expr::interpreter::parse_duration(s)
        .map_err(|e| RunnerError::BadTiming(s.into(), format!("{e}")))?;
    Ok(VirtualInstant::from_duration(d))
}

fn session_selector_string(sel: &SessionSelector) -> String {
    match sel {
        SessionSelector::Id(i) => i.to_string(),
        SessionSelector::Named(n) => n.clone(),
    }
}

/// Public helper: translate a [`super::schema::OrderSide`] to an engine
/// [`crate::engine::types::Side`].
pub(crate) fn side_to_engine(s: OrderSide) -> crate::engine::types::Side {
    match s {
        OrderSide::Buy => crate::engine::types::Side::Buy,
        OrderSide::Sell => crate::engine::types::Side::Sell,
    }
}

// Silence `MOCK_FILL_DELAY` unused-warning if the runner never reads it.
// Declaring a public `const` re-export keeps the dep link usable.
pub const _UNUSED_FILL_DELAY: Duration = MOCK_FILL_DELAY;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::clock::VirtualClock;
    use crate::scenario::loader;

    fn build_vclock() -> Arc<VirtualClock> {
        VirtualClock::shared()
    }

    #[tokio::test]
    async fn smoke_scenario_runs_and_records_commands() {
        let yaml = include_str!("../../fixtures/scenarios/smoke.yaml");
        let scenario = loader::load_from_str(yaml).expect("smoke loads");
        let engine = MockEngine::new();
        let clock = build_vclock();

        // Drive the clock from a parallel task so runner `sleep_until`s fire.
        let driver = {
            let clock = Arc::clone(&clock);
            tokio::spawn(async move {
                for ms in [5_000u64, 10_000, 10_500, 15_000, 60_000, 300_000] {
                    // Wait briefly for the runner to register waiters at this step.
                    for _ in 0..50 {
                        if clock.waiter_count() > 0 {
                            break;
                        }
                        tokio::task::yield_now().await;
                    }
                    clock.advance(VirtualInstant::from_millis(ms));
                }
            })
        };

        let runner = ScenarioRunner::new(scenario, engine.clone(), clock.clone());
        let res = runner.run().await.expect("smoke runs");
        driver.await.ok();

        assert_eq!(res.scenario_name, "smoke");
        // Must have sent subscribe + place + scenario_completed at minimum.
        let outgoing = engine.outgoing();
        assert!(
            outgoing
                .iter()
                .any(|c| matches!(c, MockCmd::SubscribeMarketData { .. })),
            "expected SubscribeMarketData in {outgoing:?}"
        );
        assert!(
            outgoing
                .iter()
                .any(|c| matches!(c, MockCmd::PlaceOrder { .. })),
            "expected PlaceOrder in {outgoing:?}"
        );
        assert!(
            outgoing.last() == Some(&MockCmd::ScenarioCompleted),
            "last cmd must be ScenarioCompleted, got {:?}",
            outgoing.last()
        );
    }
}
