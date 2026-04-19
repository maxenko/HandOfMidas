//! v1 → current migration.
//!
//! v1 **is** the current schema version today, so this is an identity
//! transform. It exists so the migration chain has a well-defined entry
//! point and adding a v2 is a purely additive code change:
//!
//! ```ignore
//! // When bumping schema to v2, this file stays, and the new file
//! // `v2_to_current.rs` defines another identity transform; the old
//! // `v1_to_current` is renamed to `v1_to_v2` and grows a real body.
//! ```
//!
//! Keeping the skeleton in place catches "forgot to wire up the chain"
//! bugs the first time we bump — the compiler will tell us to add the
//! new call.

use serde_yaml::Value;

use crate::scenario::loader::ScenarioError;

pub fn migrate(raw: Value) -> Result<Value, ScenarioError> {
    // Identity. Kept explicit so the diff reviewer in the first real
    // v1 → v2 migration sees exactly where to add transforms.
    Ok(raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_preserves_document() {
        let v: Value = serde_yaml::from_str("version: 1\nname: x\n").unwrap();
        let out = migrate(v.clone()).unwrap();
        assert_eq!(v, out);
    }
}
