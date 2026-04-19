//! Deterministic-RNG helpers.
//!
//! Every random draw in the order simulator is seeded from a tuple of
//! `(base_seed, order_id, draw_kind, step_idx)` folded into a 64-bit value
//! and fed to `ChaCha8Rng::seed_from_u64`. `draw_kind` is always a named enum
//! variant ([`DrawKind`]); never a string.
//!
//! See `plan/ib-sim/04-order-lifecycle.md` §"Determinism guarantees".
//!
//! # Why SplitMix64, not `DefaultHasher`
//!
//! `std::collections::hash_map::DefaultHasher` is explicitly documented as
//! "not specified, should not be relied upon over releases." Every recorded
//! fixture would silently break on a Rust toolchain bump. We use a hand-rolled
//! [SplitMix64] finalizer instead — stable by definition (it's just arithmetic
//! on `u64`), bit-mixing quality is known-good, and there are no new deps.
//!
//! [SplitMix64]: https://xorshift.di.unimi.it/splitmix64.c

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

/// Stable 64-bit bit-mixer. Identical output across Rust versions, platforms,
/// and optimisation levels.
#[inline]
pub(crate) const fn splitmix64(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = x;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Fold `(base_seed, order_id, draw_kind, step_idx)` into a 64-bit seed.
///
/// Stable across Rust toolchain versions — every byte of the output is
/// determined by arithmetic on `u64`, not by `std`'s hasher.
#[inline]
pub fn seed64(base_seed: u64, order_id: OrderId, draw_kind: DrawKind, step_idx: u32) -> u64 {
    // Mix the four inputs through SplitMix64 one at a time; XOR-chaining means
    // permuting the inputs is not a symmetry. Using wrapping_add before the
    // mixer keeps zero-valued inputs from collapsing the state.
    let mut acc = splitmix64(base_seed);
    acc = splitmix64(acc ^ (order_id.0 as u64));
    acc = splitmix64(acc ^ (draw_kind as u64));
    splitmix64(acc ^ (step_idx as u64))
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

    /// Pin a single seed to a byte-exact value so future edits to the mixer
    /// (or accidental toolchain-dependent changes) fail loudly. Changing this
    /// value is a fixture-breaking change and must be deliberate: bump every
    /// recorded scenario output alongside.
    #[test]
    fn seed64_is_stable_across_toolchain() {
        // SplitMix64 fold of (42, OrderId(7), DrawKind::PatternJitter=1, 2).
        // Hand-verified against the reference implementation.
        let pinned: u64 = 0x259f_c1d0_00a9_b1a0;
        let actual = seed64(42, OrderId(7), DrawKind::PatternJitter, 2);
        assert_eq!(
            actual, pinned,
            "seed64 drift: fixture-breaking change. actual=0x{actual:016x}"
        );
    }

    #[test]
    fn splitmix64_known_vectors() {
        // Reference: https://xorshift.di.unimi.it/splitmix64.c.
        // Pinning two points directly. These are hand-computed from the
        // reference implementation; a drift is a fixture-breaking change.
        assert_eq!(splitmix64(0), 0xE220_A839_7B1D_CDAF);
        assert_eq!(splitmix64(0xE220_A839_7B1D_CDAF), 0xA706_DD2F_4D19_7E6F);
    }
}
