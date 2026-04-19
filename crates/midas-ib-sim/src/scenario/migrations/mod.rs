//! Scenario-schema migration chain.
//!
//! Each migration is a pure function `fn(Value) -> Result<Value, ScenarioError>`
//! that transforms a YAML document from version N to N+1. [`migrate_to_current`]
//! walks the chain in order, so a v1 document is promoted to `CURRENT_VERSION`
//! with no caller awareness of the intermediate steps.
//!
//! ## Adding a new migration
//!
//! 1. Bump [`CURRENT_VERSION`](crate::scenario::schema::CURRENT_VERSION).
//! 2. Add a new `vN_to_vM.rs` module here.
//! 3. Call the new `migrate_vN_to_vM` from [`migrate_to_current`], gated by
//!    `if from < M`.
//! 4. Copy the old fixtures to `fixtures/scenarios/legacy/vN/` — the cross-
//!    version fixture test in `tests/` asserts they still load.

pub mod v1_to_current;

use serde_yaml::Value;

use crate::scenario::loader::ScenarioError;

/// Walk the migration chain, promoting `raw` from `from` to `CURRENT_VERSION`.
pub fn migrate_to_current(from: u32, raw: Value) -> Result<Value, ScenarioError> {
    let mut current = raw;
    // v1 is the current version — this is a no-op today, but the call is
    // kept on the path so future bumps follow the same shape.
    if from <= 1 {
        current = v1_to_current::migrate(current)?;
    }
    Ok(current)
}
