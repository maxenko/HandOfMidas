//! Deterministic-RNG helpers.
//!
//! Every random draw in the order simulator is seeded from a tuple of
//! `(base_seed, order_id, draw_kind, step_idx)` hashed into a 64-bit value
//! and fed to `ChaCha8Rng::seed_from_u64`. `draw_kind` is always a named enum
//! variant ([`DrawKind`]); never a string.
//!
//! See `plan/ib-sim/04-order-lifecycle.md` §"Determinism guarantees".

use std::hash::{Hash, Hasher};

use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;

use crate::engine::types::OrderId;

/// Named RNG-stream kinds. Adding a new kind is a deliberate, reviewable
/// change: stringly-typed tags are forbidden.
#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
pub enum DrawKind {
    PatternSelection,
    PatternJitter,
    Slippage,
    Commission,
    PartialChunking,
    BracketActivation,
    OcaCancel,
}

/// Fold `(base_seed, order_id, draw_kind, step_idx)` into a 64-bit seed.
#[inline]
pub fn seed64(base_seed: u64, order_id: OrderId, draw_kind: DrawKind, step_idx: u32) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    base_seed.hash(&mut h);
    order_id.0.hash(&mut h);
    (draw_kind as u32).hash(&mut h);
    step_idx.hash(&mut h);
    h.finish()
}

/// Instantiate a fresh, deterministic RNG for one draw.
#[inline]
pub fn rng_for(
    base_seed: u64,
    order_id: OrderId,
    draw_kind: DrawKind,
    step_idx: u32,
) -> ChaCha8Rng {
    ChaCha8Rng::seed_from_u64(seed64(base_seed, order_id, draw_kind, step_idx))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::Rng;

    #[test]
    fn same_inputs_produce_identical_stream() {
        let mut a = rng_for(42, OrderId(7), DrawKind::PatternJitter, 2);
        let mut b = rng_for(42, OrderId(7), DrawKind::PatternJitter, 2);
        for _ in 0..16 {
            assert_eq!(a.gen::<u64>(), b.gen::<u64>());
        }
    }

    #[test]
    fn different_step_indices_decorrelate() {
        let mut a = rng_for(42, OrderId(7), DrawKind::PatternJitter, 0);
        let mut b = rng_for(42, OrderId(7), DrawKind::PatternJitter, 1);
        assert_ne!(a.gen::<u64>(), b.gen::<u64>());
    }

    #[test]
    fn different_draw_kinds_decorrelate() {
        let mut a = rng_for(1, OrderId(1), DrawKind::Slippage, 0);
        let mut b = rng_for(1, OrderId(1), DrawKind::Commission, 0);
        assert_ne!(a.gen::<u64>(), b.gen::<u64>());
    }
}
