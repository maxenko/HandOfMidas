//! Event scheduler — priority queue on virtual time.
//!
//! Stage 01 declares the types and `EventScheduler::new / schedule /
//! peek_deadline / pop_if_due / len` signatures. Stage 08 fills in the
//! cancel-safety integration with the engine `select!` loop.

use std::cmp::{Ordering, Reverse};
use std::collections::BinaryHeap;

use crate::engine::clock::VirtualInstant;
use crate::engine::types::{OrderId, SessionId, SubKey};

/// All the things the scheduler can fire back into the engine. Stage 01
/// freezes the shape here so Wave 2 can schedule actions without central-
/// file merge conflicts; extension via `EngineActionExt` if needed.
#[derive(Clone, Debug)]
pub enum EngineAction {
    /// Emit a synthetic tick update for the symbol identified by `key`.
    EmitTick { key: SubKey },
    /// Emit the next step of a fill pattern (Stage 04).
    EmitFillPatternStep { order_id: OrderId, step_idx: u32 },
    /// Emit a farm-status bulletin (Stage 05).
    EmitFarmStatus { code: i32, farm: String, up: bool },
    /// Emit a daily-restart disconnect to every session (Stage 05).
    EmitDailyRestart,
    /// Deliver a historical-data batch (Stage 03).
    DeliverHistoricalBatch { session: SessionId, key: SubKey },
    /// Generic deferred command re-injection — used by `InjectLag` (Stage 06).
    Deferred(Box<EngineActionPayload>),
}

/// Wrapper so `Deferred` can carry future opaque payloads without forcing a
/// cascade of `Clone`/`Debug` bounds through the scheduler. Stage 06 may
/// replace the `description` field with a real payload enum.
#[derive(Clone, Debug)]
pub struct EngineActionPayload {
    pub description: String,
}

#[derive(Debug)]
pub struct ScheduledEvent {
    pub deadline: VirtualInstant,
    pub seq: u64,
    pub action: EngineAction,
}

impl PartialEq for ScheduledEvent {
    fn eq(&self, other: &Self) -> bool {
        self.deadline == other.deadline && self.seq == other.seq
    }
}

impl Eq for ScheduledEvent {}

impl PartialOrd for ScheduledEvent {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for ScheduledEvent {
    fn cmp(&self, other: &Self) -> Ordering {
        // Earlier deadline wins; `seq` tie-breaks in insertion order.
        self.deadline
            .cmp(&other.deadline)
            .then_with(|| self.seq.cmp(&other.seq))
    }
}

/// Deterministic priority queue over scheduled actions.
#[derive(Default)]
pub struct EventScheduler {
    queue: BinaryHeap<Reverse<ScheduledEvent>>,
    next_seq: u64,
}

impl EventScheduler {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.queue.len()
    }

    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    pub fn schedule(&mut self, deadline: VirtualInstant, action: EngineAction) {
        let seq = self.next_seq;
        self.next_seq += 1;
        self.queue.push(Reverse(ScheduledEvent {
            deadline,
            seq,
            action,
        }));
    }

    /// Peek the deadline of the next event, if any.
    pub fn peek_deadline(&self) -> Option<VirtualInstant> {
        self.queue.peek().map(|Reverse(e)| e.deadline)
    }

    /// Pop the next event if its deadline is ≤ `now`.
    pub fn pop_if_due(&mut self, now: VirtualInstant) -> Option<EngineAction> {
        match self.queue.peek() {
            Some(Reverse(e)) if e.deadline <= now => {
                let Reverse(event) = self.queue.pop().expect("peeked, must pop");
                Some(event.action)
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fifo_tie_break_by_seq() {
        let mut sched = EventScheduler::new();
        let t = VirtualInstant::from_millis(100);
        sched.schedule(
            t,
            EngineAction::EmitFarmStatus {
                code: 2104,
                farm: "a".into(),
                up: true,
            },
        );
        sched.schedule(
            t,
            EngineAction::EmitFarmStatus {
                code: 2104,
                farm: "b".into(),
                up: true,
            },
        );
        // First-scheduled fires first.
        match sched.pop_if_due(t) {
            Some(EngineAction::EmitFarmStatus { farm, .. }) => assert_eq!(farm, "a"),
            other => panic!("expected EmitFarmStatus(a), got {other:?}"),
        }
        match sched.pop_if_due(t) {
            Some(EngineAction::EmitFarmStatus { farm, .. }) => assert_eq!(farm, "b"),
            other => panic!("expected EmitFarmStatus(b), got {other:?}"),
        }
        assert!(sched.pop_if_due(t).is_none());
    }

    #[test]
    fn pop_if_due_respects_deadline() {
        let mut sched = EventScheduler::new();
        let t10 = VirtualInstant::from_millis(10);
        let t20 = VirtualInstant::from_millis(20);
        sched.schedule(t20, EngineAction::EmitDailyRestart);
        assert!(sched.pop_if_due(t10).is_none());
        assert!(sched.pop_if_due(t20).is_some());
    }
}
