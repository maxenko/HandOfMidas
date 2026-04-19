//! Integration tests for the Stage 06-skeleton scenario loader.
//!
//! Unit tests live alongside the modules; this file covers the
//! file-system-facing paths (real fixtures, JSON Schema export to disk).

use std::fs;
use std::path::{Path, PathBuf};

use midas_ib_sim::scenario::json_schema;
use midas_ib_sim::scenario::loader;
use midas_ib_sim::{Scenario, ScenarioError};

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("scenarios")
}

#[test]
fn every_fixture_loads_cleanly() {
    let dir = fixtures_dir();
    let entries: Vec<_> = fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("reading {}: {e}", dir.display()))
        .filter_map(|r| r.ok())
        .filter(|e| {
            e.path()
                .extension()
                .map(|ext| ext == "yaml" || ext == "yml")
                .unwrap_or(false)
        })
        .collect();

    assert!(
        !entries.is_empty(),
        "no .yaml fixtures under {}",
        dir.display()
    );

    for entry in entries {
        let path = entry.path();
        let scenario: Scenario = loader::load(&path)
            .unwrap_or_else(|e| panic!("fixture {} failed to load: {e}", path.display()));
        assert!(
            scenario.version >= 1,
            "{} deserialised with version 0",
            path.display()
        );
    }
}

#[test]
fn smoke_fixture_has_expected_shape() {
    let path = fixtures_dir().join("smoke.yaml");
    let s = loader::load(&path).expect("smoke.yaml loads");
    assert_eq!(s.name, "smoke");
    assert_eq!(s.version, 1);
    assert!(!s.events.is_empty(), "smoke scenario should have events");
    assert!(
        !s.asserts.is_empty(),
        "smoke scenario should have end-of-run asserts"
    );
}

#[test]
fn json_schema_export_validates_smoke_fixture_structure() {
    // We don't pull in a full JSON Schema validator in the crate (would
    // add a heavy dep just for this test). Instead, we check the two
    // invariants we care about:
    //   1. The exported schema is structurally a JSON Schema (`type`,
    //      `properties`, `$schema` or `definitions` block).
    //   2. Every YAML key present in smoke.yaml maps to a known property in
    //      the schema's `properties` map.

    let schema = json_schema::export_schema();
    let props = schema
        .get("properties")
        .and_then(|v| v.as_object())
        .expect("schema must have `properties`");

    let smoke_yaml = fs::read_to_string(fixtures_dir().join("smoke.yaml")).unwrap();
    let doc: serde_yaml::Value = serde_yaml::from_str(&smoke_yaml).unwrap();
    let mapping = doc.as_mapping().unwrap();

    for (k, _) in mapping {
        let key = k.as_str().unwrap();
        assert!(
            props.contains_key(key),
            "smoke.yaml uses top-level key `{key}` absent from exported JSON schema"
        );
    }
}

#[test]
fn schema_writes_to_disk() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("schema").join("v1.json");
    json_schema::write_schema_to(&target).expect("write schema");
    let text = fs::read_to_string(&target).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(
        parsed.get("type").and_then(|v| v.as_str()),
        Some("object"),
        "written schema root must describe an object"
    );
}

#[test]
fn unknown_verb_is_caught_on_fs_load() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bad.yaml");
    fs::write(
        &path,
        "version: 1\nname: bad\nevents:\n  - at: \"00:00:01\"\n    do: do_the_wiggle\n    args: {}\n",
    )
    .unwrap();
    match loader::load(&path) {
        Err(ScenarioError::UnknownVerb(v)) => assert_eq!(v, "do_the_wiggle"),
        other => panic!("expected UnknownVerb from fs load, got {other:?}"),
    }
}
