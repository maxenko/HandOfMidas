//! JSON Schema export for the scenario DSL.
//!
//! Editor tooling (VS Code's YAML extension, JetBrains' IDEs) consumes a
//! JSON Schema file to drive autocomplete + validation. We derive the schema
//! from the Rust types via `schemars` so there's a single source of truth —
//! updating `schema.rs` flows through to editor tooling on the next `cargo
//! run --bin midas-ib-sim-server -- export-schema` (Wave 2 hook) or the next
//! build, via the test that round-trips [`export_schema`].

use std::fs;
use std::io;
use std::path::Path;

use schemars::schema_for;
use serde_json::Value;

use crate::scenario::schema::Scenario;

/// Produce a JSON Schema value describing the [`Scenario`] document.
pub fn export_schema() -> Value {
    let schema = schema_for!(Scenario);
    serde_json::to_value(schema).expect("schemars output is always JSON-serialisable")
}

/// Write the JSON Schema to disk, creating parent directories as needed.
///
/// Canonical path is `fixtures/scenarios/schema/v1.json` — invoke with that
/// or any other destination during CI / local dev.
pub fn write_schema_to(path: &Path) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let pretty = serde_json::to_string_pretty(&export_schema()).map_err(io::Error::other)?;
    fs::write(path, pretty)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_describes_an_object() {
        let schema = export_schema();
        // Top-level schema should be an object with a `properties` map that
        // includes at least the fields we rely on from `Scenario`.
        assert_eq!(
            schema.get("type").and_then(Value::as_str),
            Some("object"),
            "scenario schema root must be an object"
        );
        let props = schema
            .get("properties")
            .and_then(Value::as_object)
            .expect("scenario schema should expose `properties`");
        for key in ["version", "name", "events", "asserts"] {
            assert!(props.contains_key(key), "missing property {key}");
        }
    }

    #[test]
    fn schema_roundtrips_via_write() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("sub").join("v1.json");
        write_schema_to(&target).unwrap();
        let text = fs::read_to_string(&target).unwrap();
        let parsed: Value = serde_json::from_str(&text).unwrap();
        assert!(parsed.get("properties").is_some());
    }
}
