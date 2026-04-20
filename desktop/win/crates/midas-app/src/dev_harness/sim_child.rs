//! Compatibility shim: re-exports from the canonical non-feature-gated
//! [`crate::sim_child`] module.
//!
//! The sim-child lifecycle used to live here, feature-gated on
//! `dev_harness`. The production `Sim` broker backend (the default
//! for fresh installs — see `BrokerBackend::Sim`) needs the same
//! spawn / health-check / shutdown code path, so it was promoted to
//! `crate::sim_child`. This module keeps the old import paths valid
//! for the dev-harness command dispatcher and the `app_sim_e2e`
//! integration tests that were written against the old layout.

// Re-export the canonical API; `allow(unused_imports)` because only
// a subset is used from the devloop command dispatcher — but
// external integration tests + fixture scripts import each one by
// this path, so dropping any symbol would break them.
#[allow(unused_imports)]
pub use crate::sim_child::{
    allocate_sim_port, devloop_runtime_dir, resolve_sim_binary, spawn, SimChildError,
    SimChildHandle, SpawnOptions,
};
