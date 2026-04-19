//! Fill-pattern scheduling — the non-atomic event ordering that makes real
//! IB's callback stream look the way it does.
//!
//! See `plan/ib-sim/04-order-lifecycle.md` §"Non-atomic event ordering".

use std::time::Duration;

use rand::Rng;
use rand_distr::{Distribution, Exp};
use static_assertions::const_assert;

use crate::engine::types::OrderId;
use crate::orders::determinism::{rng_for, DrawKind};

pub const PATTERN_MAX_JITTER_MS: u64 = 50;

pub const PATTERN_A_OFFSETS_MS: [u64; 4] = [0, 100, 180, 260];
pub const PATTERN_A_MIN_GAP_MS: u64 = 80;

pub const PATTERN_B_OFFSETS_MS: [u64; 4] = [0, 80, 140, 210];
pub const PATTERN_B_MIN_GAP_MS: u64 = 60;

pub const PATTERN_C_OFFSETS_MS: [u64; 7] = [0, 80, 140, 210, 400, 470, 540];
pub const PATTERN_C_MIN_GAP_MS: u64 = 60;

// Compile-time invariant: max jitter < smallest base gap.
const_assert!(PATTERN_A_MIN_GAP_MS > PATTERN_MAX_JITTER_MS);
const_assert!(PATTERN_B_MIN_GAP_MS > PATTERN_MAX_JITTER_MS);
const_assert!(PATTERN_C_MIN_GAP_MS > PATTERN_MAX_JITTER_MS);

// Keep the declared min-gap constants honest vs the offset tables.
const_assert!(PATTERN_A_MIN_GAP_MS == array_min_gap(&PATTERN_A_OFFSETS_MS));
const_assert!(PATTERN_B_MIN_GAP_MS == array_min_gap(&PATTERN_B_OFFSETS_MS));
const_assert!(PATTERN_C_MIN_GAP_MS == array_min_gap(&PATTERN_C_OFFSETS_MS));

const fn array_min_gap<const N: usize>(arr: &[u64; N]) -> u64 {
    let mut min = u64::MAX;
    let mut i = 1;
    while i < N {
        let gap = arr[i] - arr[i - 1];
        if gap < min {
            min = gap;
        }
        i += 1;
    }
    min
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum PatternKind {
    A,
    B,
    C,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum StepKind {
    OpenOrderSubmitted,
    OpenOrderPreSubmitted,
    ExecutionPart { chunk_idx: u8 },
    CommissionPart { chunk_idx: u8 },
    OrderStatusPartiallyFilled { chunk_idx: u8 },
    OrderStatusFilled,
}

pub fn select_pattern(
    base_seed: u64,
    order_id: OrderId,
    kind: crate::engine::types::OrderKind,
    total_qty: f64,
    partial_threshold: f64,
) -> PatternKind {
    let mut rng = rng_for(base_seed, order_id, DrawKind::PatternSelection, 0);
    let draw = rng.gen::<u32>() % 100;
    let large = total_qty > partial_threshold;
    match kind {
        crate::engine::types::OrderKind::Market => {
            if large && draw < 30 {
                PatternKind::C
            } else if draw < 60 {
                PatternKind::B
            } else {
                PatternKind::A
            }
        }
        crate::engine::types::OrderKind::Limit => {
            if large && draw < 10 {
                PatternKind::C
            } else if draw < 20 {
                PatternKind::B
            } else {
                PatternKind::A
            }
        }
        crate::engine::types::OrderKind::Stop | crate::engine::types::OrderKind::StopLimit => {
            if large && draw < 15 {
                PatternKind::C
            } else if draw < 40 {
                PatternKind::B
            } else {
                PatternKind::A
            }
        }
    }
}

pub fn steps_for(pattern: PatternKind) -> Vec<(Duration, StepKind)> {
    match pattern {
        PatternKind::A => vec![
            (ms(PATTERN_A_OFFSETS_MS[0]), StepKind::OpenOrderSubmitted),
            (ms(PATTERN_A_OFFSETS_MS[1]), StepKind::OrderStatusFilled),
            (
                ms(PATTERN_A_OFFSETS_MS[2]),
                StepKind::ExecutionPart { chunk_idx: 0 },
            ),
            (
                ms(PATTERN_A_OFFSETS_MS[3]),
                StepKind::CommissionPart { chunk_idx: 0 },
            ),
        ],
        PatternKind::B => vec![
            (ms(PATTERN_B_OFFSETS_MS[0]), StepKind::OpenOrderPreSubmitted),
            (
                ms(PATTERN_B_OFFSETS_MS[1]),
                StepKind::ExecutionPart { chunk_idx: 0 },
            ),
            (
                ms(PATTERN_B_OFFSETS_MS[2]),
                StepKind::CommissionPart { chunk_idx: 0 },
            ),
            (ms(PATTERN_B_OFFSETS_MS[3]), StepKind::OrderStatusFilled),
        ],
        PatternKind::C => vec![
            (ms(PATTERN_C_OFFSETS_MS[0]), StepKind::OpenOrderPreSubmitted),
            (
                ms(PATTERN_C_OFFSETS_MS[1]),
                StepKind::ExecutionPart { chunk_idx: 0 },
            ),
            (
                ms(PATTERN_C_OFFSETS_MS[2]),
                StepKind::CommissionPart { chunk_idx: 0 },
            ),
            (
                ms(PATTERN_C_OFFSETS_MS[3]),
                StepKind::OrderStatusPartiallyFilled { chunk_idx: 0 },
            ),
            (
                ms(PATTERN_C_OFFSETS_MS[4]),
                StepKind::ExecutionPart { chunk_idx: 1 },
            ),
            (
                ms(PATTERN_C_OFFSETS_MS[5]),
                StepKind::CommissionPart { chunk_idx: 1 },
            ),
            (ms(PATTERN_C_OFFSETS_MS[6]), StepKind::OrderStatusFilled),
        ],
    }
}

#[inline]
fn ms(v: u64) -> Duration {
    Duration::from_millis(v)
}

/// Sample a per-step jitter magnitude (truncated exp, mean 15ms, cap at
/// `PATTERN_MAX_JITTER_MS`). Seeded per `(order_id, step_idx)`.
pub fn jitter_for_step(base_seed: u64, order_id: OrderId, step_idx: u32) -> Duration {
    let mut rng = rng_for(base_seed, order_id, DrawKind::PatternJitter, step_idx);
    let exp = Exp::new(1.0_f64 / 15.0).expect("rate > 0");
    let sample_ms: f64 = exp.sample(&mut rng);
    let clamped = sample_ms.min(PATTERN_MAX_JITTER_MS as f64).max(0.0);
    debug_assert!(
        clamped <= PATTERN_MAX_JITTER_MS as f64,
        "jitter {clamped}ms exceeds PATTERN_MAX_JITTER_MS={PATTERN_MAX_JITTER_MS}"
    );
    Duration::from_millis(clamped.round() as u64)
}

pub fn actual_offset(
    base_offset: Duration,
    base_seed: u64,
    order_id: OrderId,
    step_idx: u32,
) -> Duration {
    base_offset + jitter_for_step(base_seed, order_id, step_idx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::types::OrderKind;
    use proptest::prelude::*;

    fn enumerate_jitter_grid<const N: usize>(base_offsets: &[u64; N]) {
        const JITTER_CHOICES: [u64; 3] = [0, 15, PATTERN_MAX_JITTER_MS];
        let len = base_offsets.len();
        let grid_size = JITTER_CHOICES.len().pow(len as u32);
        for g in 0..grid_size {
            let mut x = g;
            let mut jitters = [0u64; 8];
            for slot in jitters.iter_mut().take(len) {
                *slot = JITTER_CHOICES[x % JITTER_CHOICES.len()];
                x /= JITTER_CHOICES.len();
            }
            let mut last = 0u64;
            for (i, base) in base_offsets.iter().enumerate().take(len) {
                let actual = *base + jitters[i];
                assert!(
                    actual > last || i == 0,
                    "step {i} jitter {}ms reorders ({actual}ms <= prev {last}ms)",
                    jitters[i]
                );
                last = actual;
            }
        }
    }

    #[test]
    fn pattern_a_invariant_grid() {
        enumerate_jitter_grid(&PATTERN_A_OFFSETS_MS);
    }

    #[test]
    fn pattern_b_invariant_grid() {
        enumerate_jitter_grid(&PATTERN_B_OFFSETS_MS);
    }

    #[test]
    fn pattern_c_invariant_grid() {
        enumerate_jitter_grid(&PATTERN_C_OFFSETS_MS);
    }

    proptest! {
        #[test]
        fn jitter_never_exceeds_max(
            seed in any::<u64>(),
            oid in any::<i32>(),
            step in 0u32..32,
        ) {
            let j = jitter_for_step(seed, OrderId(oid), step);
            prop_assert!(j <= Duration::from_millis(PATTERN_MAX_JITTER_MS));
        }
    }

    #[test]
    fn pattern_selection_is_deterministic() {
        let a = select_pattern(1, OrderId(7), OrderKind::Market, 100.0, 1_000.0);
        let b = select_pattern(1, OrderId(7), OrderKind::Market, 100.0, 1_000.0);
        assert_eq!(a, b);
    }

    #[test]
    fn pattern_selection_covers_all_three_kinds() {
        let mut seen = std::collections::BTreeSet::new();
        for i in 0..200 {
            seen.insert(select_pattern(
                42,
                OrderId(i),
                OrderKind::Market,
                5_000.0,
                1_000.0,
            ));
        }
        assert!(seen.contains(&PatternKind::A));
        assert!(seen.contains(&PatternKind::B));
        assert!(seen.contains(&PatternKind::C));
    }

    #[test]
    fn pattern_b_contains_exec_before_status() {
        let steps = steps_for(PatternKind::B);
        let exec_idx = steps
            .iter()
            .position(|(_, k)| matches!(k, StepKind::ExecutionPart { .. }))
            .unwrap();
        let status_idx = steps
            .iter()
            .position(|(_, k)| matches!(k, StepKind::OrderStatusFilled))
            .unwrap();
        assert!(exec_idx < status_idx);
    }
}
