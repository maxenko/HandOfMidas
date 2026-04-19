//! Bracket parent-child (OCA) semantics.
//!
//! See `plan/ib-sim/04-order-lifecycle.md` §"Bracket semantics".

use std::time::Duration;

use rand::Rng;

use crate::engine::types::OrderId;
use crate::orders::determinism::{rng_for, DrawKind};

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum BracketLifecycle {
    ParentWorking,
    ParentFilled,
    OneChildFilled,
    Complete,
}

#[derive(Clone, Debug)]
pub struct BracketGroup {
    pub parent_id: OrderId,
    pub take_profit: Option<OrderId>,
    pub stop_loss: Option<OrderId>,
    pub oca_group: Option<String>,
    pub state: BracketLifecycle,
}

impl BracketGroup {
    pub fn new(parent_id: OrderId, oca_group: Option<String>) -> Self {
        Self {
            parent_id,
            take_profit: None,
            stop_loss: None,
            oca_group,
            state: BracketLifecycle::ParentWorking,
        }
    }
}

pub const BRACKET_ACTIVATION_MIN_MS: u64 = 5;
pub const BRACKET_ACTIVATION_MAX_MS: u64 = 50;
pub const OCA_CANCEL_MIN_MS: u64 = 10;
pub const OCA_CANCEL_MAX_MS: u64 = 100;

pub fn sample_activation_jitter(
    base_seed: u64,
    parent_id: OrderId,
    child_id: OrderId,
) -> Duration {
    let mut rng = rng_for(
        base_seed,
        parent_id,
        DrawKind::BracketActivation,
        child_id.0 as u32,
    );
    let ms = BRACKET_ACTIVATION_MIN_MS
        + (rng.gen::<u64>() % (BRACKET_ACTIVATION_MAX_MS - BRACKET_ACTIVATION_MIN_MS + 1));
    Duration::from_millis(ms)
}

pub fn sample_oca_cancel_jitter(
    base_seed: u64,
    filled_child_id: OrderId,
    sibling_id: OrderId,
) -> Duration {
    let mut rng = rng_for(
        base_seed,
        filled_child_id,
        DrawKind::OcaCancel,
        sibling_id.0 as u32,
    );
    let ms = OCA_CANCEL_MIN_MS + (rng.gen::<u64>() % (OCA_CANCEL_MAX_MS - OCA_CANCEL_MIN_MS + 1));
    Duration::from_millis(ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn activation_jitter_in_range() {
        for i in 0..200 {
            let d = sample_activation_jitter(1, OrderId(i), OrderId(i + 1));
            assert!(d >= Duration::from_millis(BRACKET_ACTIVATION_MIN_MS));
            assert!(d <= Duration::from_millis(BRACKET_ACTIVATION_MAX_MS));
        }
    }

    #[test]
    fn oca_jitter_in_range() {
        for i in 0..200 {
            let d = sample_oca_cancel_jitter(1, OrderId(i), OrderId(i + 1));
            assert!(d >= Duration::from_millis(OCA_CANCEL_MIN_MS));
            assert!(d <= Duration::from_millis(OCA_CANCEL_MAX_MS));
        }
    }

    #[test]
    fn activation_jitter_deterministic() {
        let a = sample_activation_jitter(42, OrderId(1), OrderId(2));
        let b = sample_activation_jitter(42, OrderId(1), OrderId(2));
        assert_eq!(a, b);
    }

    #[test]
    fn oca_jitter_deterministic() {
        let a = sample_oca_cancel_jitter(42, OrderId(1), OrderId(2));
        let b = sample_oca_cancel_jitter(42, OrderId(1), OrderId(2));
        assert_eq!(a, b);
    }
}
