//! Engine actor — the single-threaded owner of all sim state.
//!
//! Stage 01 declares the actor shell, command channel, and event broadcast
//! surface. Stage 02-09 fill in the per-command handlers. Every Wave 2 edit
//! lands inside a handler function body; adding a new `EngineCmd` variant
//! requires a Stage 01 amendment PR (see `plan/ib-sim/01-architecture.md`
//! §"Extension-enum pattern").

pub mod clock;
pub mod orchestrator;
pub mod scheduler;
pub mod state;
pub mod types;

use std::sync::Arc;

use tokio::sync::{broadcast, mpsc};
use tracing::{debug, instrument, trace, warn};

pub use self::clock::{AcceleratedClock, Clock, ClockMode, RealClock, SessionAnchor, VirtualClock};
pub use self::scheduler::{EngineAction, EventScheduler, ScheduledEvent};
pub use self::state::SessionState;
pub use self::types::{
    EngineCmd, EngineEvent, EngineSnapshot, MarketEmission, MarketSnapshot, OrderEmission,
    QuirkViolation, SessionId,
};

use crate::market_data::MarketDataEngine;
use crate::orders::OrderSimulator;
use crate::quirks::QuirkGuard;

/// Capacity of the command mpsc channel. Sized to absorb a burst of
/// per-session commands without blocking session tasks on `send`.
pub const ENGINE_CMD_CHANNEL_CAP: usize = 4096;

/// Capacity of the event broadcast channel. Consumers (control plane, scenario
/// runner, metrics) may be slow; a bigger lag budget is cheap relative to an
/// actor pause.
pub const ENGINE_EVENT_CHANNEL_CAP: usize = 8192;

/// The engine actor — owns all sim state and processes commands in order.
///
/// Stage 01 carries only the skeleton; per-command handlers (`handle_command`,
/// `handle_scheduled`) are `todo!()` and filled in by Wave 2.
pub struct Engine {
    pub clock: Arc<dyn Clock>,
    pub sessions: std::collections::BTreeMap<SessionId, SessionState>,
    pub market_data: Box<dyn MarketDataEngine>,
    pub orders: Box<dyn OrderSimulator>,
    pub quirks: Box<dyn QuirkGuard>,
    pub scheduler: EventScheduler,
    pub command_rx: mpsc::Receiver<EngineCmd>,
    pub event_tx: broadcast::Sender<EngineEvent>,
}

/// Handle bundling the send halves of the engine's channels + the clock
/// reference. Shared by session tasks, the control plane, and the scheduler.
#[derive(Clone)]
pub struct EngineHandle {
    pub command_tx: mpsc::Sender<EngineCmd>,
    pub event_tx: broadcast::Sender<EngineEvent>,
    pub clock: Arc<dyn Clock>,
}

impl Engine {
    /// Main loop: interleaves session commands and scheduled events under the
    /// cancel-safe pattern from `plan/ib-sim/08-deterministic-clock.md`.
    ///
    /// Both `select!` arms only await cancel-safe primitives
    /// (`mpsc::Receiver::recv`, `Clock::sleep_until`). The scheduler's
    /// `peek_deadline` / `pop_if_due` are synchronous, so scheduler state is
    /// never held across an `await`. If a command handler schedules an event
    /// with an earlier deadline than the one currently being awaited, the
    /// next loop iteration re-peeks and re-sleeps — adding at most one loop
    /// iteration of wake latency relative to the new deadline.
    #[instrument(name = "sim", skip(self))]
    pub async fn run(&mut self) {
        trace!("engine loop starting");
        loop {
            let next_deadline = self.scheduler.peek_deadline();

            tokio::select! {
                maybe_cmd = self.command_rx.recv() => {
                    match maybe_cmd {
                        Some(cmd) => self.handle_command(cmd),
                        None => {
                            debug!("command channel closed — engine draining");
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
        }
    }

    /// Dispatch a single `EngineCmd`. Wave 2 stages implement each arm.
    pub fn handle_command(&mut self, cmd: EngineCmd) {
        match cmd {
            EngineCmd::StartApi { .. } => { /* Stage 02 */ }
            EngineCmd::PlaceOrder { .. } => { /* Stage 04 */ }
            EngineCmd::CancelOrder { .. } => { /* Stage 04 */ }
            EngineCmd::SubscribeMarketData { .. } => { /* Stage 03 + 05 */ }
            EngineCmd::UnsubscribeMarketData { .. } => { /* Stage 03 */ }
            EngineCmd::ReqContractData { .. } => { /* Stage 02 */ }
            EngineCmd::ReqHistoricalData { .. } => { /* Stage 03 + 05 */ }
            EngineCmd::ReqRealTimeBars { .. } => { /* Stage 03 */ }
            EngineCmd::ReqPositions { .. } => { /* Stage 04 */ }
            EngineCmd::ReqAccountSummary { .. } => { /* Stage 04 */ }
            EngineCmd::ReqAccountData { .. } => { /* Stage 04 */ }
            EngineCmd::ReqExecutions { .. } => { /* Stage 04 */ }
            EngineCmd::ReqGlobalCancel { .. } => { /* Stage 04 */ }
            EngineCmd::ReqCurrentTime { .. } => { /* Stage 02 */ }
            EngineCmd::ReqIds { .. } => { /* Stage 02 */ }
            EngineCmd::ReqMarketDataType { .. } => { /* Stage 03 */ }
            EngineCmd::InjectDisconnect { .. } => { /* Stage 06 */ }
            EngineCmd::InjectLag { .. } => { /* Stage 06 */ }
            EngineCmd::InjectPacingViolation { .. } => { /* Stage 06 */ }
            EngineCmd::InjectFarmOutage { .. } => { /* Stage 05 + 06 */ }
            EngineCmd::InjectFarmRestore { .. } => { /* Stage 05 + 06 */ }
            EngineCmd::InjectPriceJump { .. } => { /* Stage 06 */ }
            EngineCmd::InjectGap { .. } => { /* Stage 06 */ }
            EngineCmd::InjectHalt { .. } => { /* Stage 06 */ }
            EngineCmd::InjectBurst { .. } => { /* Stage 06 */ }
            EngineCmd::InjectDailyRestart => { /* Stage 05 */ }
            EngineCmd::LoadScenario(_) => { /* Stage 06 */ }
            EngineCmd::DumpState { reply } => {
                // Stage 01 wiring: send a minimal snapshot so the control plane
                // can round-trip JSON. Wave 2 fills in the real projection.
                let _ = reply.send(self.snapshot());
            }
            EngineCmd::Tick(_) => { /* Stage 08 */ }
        }
    }

    /// Handle an event popped from the scheduler. Wave 2 fills in bodies.
    pub fn handle_scheduled(&mut self, _action: EngineAction) {
        warn!("scheduled-event dispatch not yet implemented");
    }

    /// Read-only projection for `/control/dump`. Wave 2 expands.
    pub fn snapshot(&self) -> EngineSnapshot {
        EngineSnapshot {
            now: Some(self.clock.now()),
            scheduler_queue_depth: self.scheduler.len(),
            ..EngineSnapshot::default()
        }
    }
}
