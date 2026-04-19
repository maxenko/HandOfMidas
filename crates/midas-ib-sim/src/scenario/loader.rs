//! Scenario YAML loader + validator.
//!
//! Entry point is [`load`]: read a file, detect the declared `version:`,
//! dispatch to the migration chain if < `CURRENT_VERSION`, then validate the
//! final document (timeline shape, anchor references, include cycles).
//!
//! The loader is deliberately synchronous + allocation-oblivious. Scenarios
//! are tens of KB; we don't stream them.

use std::collections::{BTreeSet, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde_yaml::Value;
use tracing::warn;

use crate::scenario::migrations;
use crate::scenario::schema::{Scenario, Verb, CURRENT_VERSION};

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// All errors the loader can report. Each variant carries enough context for
/// the `.expected.jsonl` regression harness to render a helpful message.
#[derive(Debug, thiserror::Error)]
pub enum ScenarioError {
    #[error("i/o error loading scenario: {0}")]
    Io(#[from] io::Error),

    #[error("yaml parse error: {0}")]
    YamlParse(#[from] serde_yaml::Error),

    #[error("scenario declares version {got}, but the current schema only supports up to {max}")]
    VersionTooNew { got: u32, max: u32 },

    #[error("scenario declares version {0}, which is older than anything the loader supports")]
    UnsupportedVersion(u32),

    #[error("unknown verb `{0}` — valid verbs are listed in scenario/schema.rs::Verb")]
    UnknownVerb(String),

    #[error("malformed event timing: {0}")]
    BadTiming(String),

    #[error("event references unknown anchor `{0}` via `after:`")]
    DanglingReference(String),

    #[error("`include:` chain contains a cycle: {0}")]
    CircularInclude(String),

    #[error("scenario validation failed: {0}")]
    Invalid(String),
}

// ---------------------------------------------------------------------------
// Public entry points
// ---------------------------------------------------------------------------

/// Read and fully validate a scenario YAML document.
///
/// Steps:
///   1. Parse to untyped `serde_yaml::Value`.
///   2. Detect `version:` — missing → default to 1 (warn).
///   3. Reject versions > `CURRENT_VERSION`.
///   4. Run the migration chain to the current version.
///   5. Deserialise to the strongly-typed `Scenario`.
///   6. Validate: timing shape, anchor references, include cycles.
pub fn load(path: &Path) -> Result<Scenario, ScenarioError> {
    let bytes = fs::read(path)?;
    let mut visited = BTreeSet::new();
    load_with_visited(path, &bytes, &mut visited)
}

/// Load a scenario from already-in-memory bytes (handy for tests). No include
/// cycle tracking — callers embedding this must not use `include:`.
pub fn load_from_str(yaml: &str) -> Result<Scenario, ScenarioError> {
    parse_and_validate(yaml, None, &mut BTreeSet::new())
}

// ---------------------------------------------------------------------------
// Internals
// ---------------------------------------------------------------------------

fn load_with_visited(
    path: &Path,
    bytes: &[u8],
    visited: &mut BTreeSet<PathBuf>,
) -> Result<Scenario, ScenarioError> {
    let canonical = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    if !visited.insert(canonical.clone()) {
        return Err(ScenarioError::CircularInclude(format!(
            "{} already on the include stack",
            canonical.display()
        )));
    }
    let yaml = std::str::from_utf8(bytes)
        .map_err(|e| ScenarioError::Invalid(format!("scenario is not valid UTF-8: {e}")))?;
    parse_and_validate(yaml, Some(&canonical), visited)
}

fn parse_and_validate(
    yaml: &str,
    source_path: Option<&Path>,
    visited: &mut BTreeSet<PathBuf>,
) -> Result<Scenario, ScenarioError> {
    let mut raw: Value = serde_yaml::from_str(yaml)?;

    // Untyped verb-name check first — gives a much better error than serde's
    // generic "unknown variant" for the common typo case.
    check_known_verbs(&raw)?;

    let version = detect_version(&raw);

    if version > CURRENT_VERSION {
        return Err(ScenarioError::VersionTooNew {
            got: version,
            max: CURRENT_VERSION,
        });
    }
    if version == 0 {
        return Err(ScenarioError::UnsupportedVersion(version));
    }

    // Inject the defaulted version back into the raw doc so the typed
    // deserialiser sees a `version:` field.
    if let Value::Mapping(ref mut map) = raw {
        map.entry(Value::String("version".into()))
            .or_insert(Value::Number(u64::from(version).into()));
    }

    let migrated = migrations::migrate_to_current(version, raw)?;
    let mut scenario: Scenario = serde_yaml::from_value(migrated)?;

    // Round-trip include: scenarios may reference other files via the
    // `Include` verb. Each include resolves relative to the including file.
    resolve_includes(&mut scenario, source_path, visited)?;

    validate(&scenario)?;
    Ok(scenario)
}

fn detect_version(raw: &Value) -> u32 {
    match raw.get("version").and_then(Value::as_u64) {
        Some(v) => v as u32,
        None => {
            warn!("scenario has no `version:` field — defaulting to v1");
            1
        }
    }
}

/// Walk the raw YAML looking for events with unknown `do:` verbs and return
/// a crisp error. Also lets us accept scenarios whose `events:` may be
/// missing entirely (no verbs to check).
fn check_known_verbs(raw: &Value) -> Result<(), ScenarioError> {
    let Some(events) = raw.get("events").and_then(Value::as_sequence) else {
        return Ok(());
    };
    for event in events {
        let Some(verb_name) = event.get("do").and_then(Value::as_str) else {
            // Events without `do:` get caught later by typed deserialisation.
            continue;
        };
        if !KNOWN_VERBS.contains(&verb_name) {
            return Err(ScenarioError::UnknownVerb(verb_name.to_owned()));
        }
    }
    Ok(())
}

/// Mirror of the `Verb` variants as their YAML-serialised (`snake_case`) tags.
/// Keep in sync with `schema::Verb`. The scenario-roundtrip test below asserts
/// this stays in sync (count matches the variant count).
const KNOWN_VERBS: &[&str] = &[
    "subscribe_market_data",
    "unsubscribe_market_data",
    "accept_order",
    "cancel_order",
    "inject_disconnect",
    "inject_farm_outage",
    "inject_farm_restore",
    "inject_pacing_violation",
    "inject_lag",
    "inject_bad_frame",
    "inject_price_jump",
    "inject_gap",
    "inject_halt",
    "inject_burst",
    "inject_duplicate_order_status",
    "inject_slow_commission_report",
    "inject_out_of_order_events",
    "inject_daily_restart",
    "sleep",
    "set_clock_mode",
    "include",
    "assert",
    "assert_client_received",
    "assert_client_event_order",
];

/// Resolve `Include` verbs by loading referenced files and splicing their
/// events + asserts in place. Performs cycle detection via `visited`.
fn resolve_includes(
    scenario: &mut Scenario,
    source_path: Option<&Path>,
    visited: &mut BTreeSet<PathBuf>,
) -> Result<(), ScenarioError> {
    let mut resolved = Vec::with_capacity(scenario.events.len());
    // Moving out is easier than indexed mutation since includes change count.
    let original = std::mem::take(&mut scenario.events);
    for event in original {
        match &event.verb {
            Verb::Include(args) => {
                let include_path = resolve_include_path(source_path, &args.path)?;
                let bytes = fs::read(&include_path)?;
                let sub = load_with_visited(&include_path, &bytes, visited)?;
                resolved.extend(sub.events);
                // Accumulate sub-scenario asserts onto the parent.
                scenario.asserts.extend(sub.asserts);
            }
            _ => resolved.push(event),
        }
    }
    scenario.events = resolved;
    Ok(())
}

fn resolve_include_path(source: Option<&Path>, rel: &str) -> Result<PathBuf, ScenarioError> {
    let rel_path = Path::new(rel);
    if rel_path.is_absolute() {
        return Ok(rel_path.to_path_buf());
    }
    match source.and_then(Path::parent) {
        Some(base) => Ok(base.join(rel_path)),
        None => Err(ScenarioError::Invalid(format!(
            "cannot resolve relative include `{rel}` — scenario was loaded from a string, not a file"
        ))),
    }
}

/// Final structural validation: each event has exactly one timing mode,
/// `after:` references resolve, etc.
fn validate(scenario: &Scenario) -> Result<(), ScenarioError> {
    let mut anchors: HashSet<&str> = HashSet::new();
    for event in &scenario.events {
        let timing_count = [
            event.at.is_some(),
            event.after.is_some(),
            event.when.is_some(),
        ]
        .iter()
        .filter(|b| **b)
        .count();
        if timing_count != 1 {
            return Err(ScenarioError::BadTiming(format!(
                "event must declare exactly one of `at:`, `after:`, `when:` (got {timing_count})"
            )));
        }
        if event.after.is_some() && event.delay.is_none() {
            return Err(ScenarioError::BadTiming(
                "`after:` event missing `delay:`".into(),
            ));
        }
        if let Some(name) = event.named.as_deref() {
            anchors.insert(name);
        }
    }
    // Second pass so forward references work (named can appear after use).
    for event in &scenario.events {
        if let Some(anchor) = event.after.as_deref() {
            if !anchors.contains(anchor) {
                return Err(ScenarioError::DanglingReference(anchor.to_owned()));
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scenario::schema::Verb;

    fn variant_count() -> usize {
        // Keep KNOWN_VERBS in sync with schema::Verb. If this fails, the
        // loader will miss new verbs in its fast-path check.
        //
        // We can't enumerate variants at runtime without a derive — use the
        // verb list emitted by the schema JSON below as a stable surrogate.
        let schema = schemars::schema_for!(Verb);
        let root = serde_json::to_value(schema).unwrap();
        // The generated schema for an externally-tagged enum is a `oneOf`.
        root.pointer("/oneOf")
            .and_then(|v| v.as_array())
            .map(|v| v.len())
            .unwrap_or(0)
    }

    #[test]
    fn known_verbs_matches_schema_variants() {
        let n = variant_count();
        assert_eq!(
            KNOWN_VERBS.len(),
            n,
            "KNOWN_VERBS ({}) drifted from Verb variants ({}) — update loader.rs",
            KNOWN_VERBS.len(),
            n,
        );
    }

    #[test]
    fn loads_minimal_scenario() {
        let yaml = r#"
version: 1
name: "minimal"
"#;
        let s = load_from_str(yaml).expect("minimal scenario should load");
        assert_eq!(s.name, "minimal");
    }

    #[test]
    fn rejects_unknown_verb() {
        let yaml = r#"
version: 1
name: "bad"
events:
  - at: "00:00:01"
    do: jiggle_market
    args: {}
"#;
        match load_from_str(yaml) {
            Err(ScenarioError::UnknownVerb(v)) => assert_eq!(v, "jiggle_market"),
            other => panic!("expected UnknownVerb, got {other:?}"),
        }
    }

    #[test]
    fn rejects_future_version() {
        let yaml = "version: 99\nname: future\n";
        match load_from_str(yaml) {
            Err(ScenarioError::VersionTooNew { got, max }) => {
                assert_eq!(got, 99);
                assert_eq!(max, CURRENT_VERSION);
            }
            other => panic!("expected VersionTooNew, got {other:?}"),
        }
    }

    #[test]
    fn rejects_dangling_anchor() {
        let yaml = r#"
version: 1
name: dangling
events:
  - after: ghost
    delay: 1s
    do: inject_daily_restart
"#;
        match load_from_str(yaml) {
            Err(ScenarioError::DanglingReference(name)) => assert_eq!(name, "ghost"),
            other => panic!("expected DanglingReference, got {other:?}"),
        }
    }

    #[test]
    fn rejects_bad_timing() {
        let yaml = r#"
version: 1
name: bad_timing
events:
  - at: "00:00:01"
    when: "orders[0].status == Filled"
    do: inject_daily_restart
"#;
        match load_from_str(yaml) {
            Err(ScenarioError::BadTiming(_)) => {}
            other => panic!("expected BadTiming, got {other:?}"),
        }
    }

    #[test]
    fn missing_version_defaults_to_v1_with_warning() {
        let yaml = "name: no_version\n";
        let s = load_from_str(yaml).unwrap();
        assert_eq!(s.version, 1);
    }

    #[test]
    fn circular_include_detected() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.yaml");
        let b = dir.path().join("b.yaml");
        std::fs::write(
            &a,
            format!(
                "version: 1\nname: a\nevents:\n  - at: \"00:00:01\"\n    do: include\n    args: {{ path: \"{}\" }}\n",
                b.file_name().unwrap().to_str().unwrap()
            ),
        )
        .unwrap();
        std::fs::write(
            &b,
            format!(
                "version: 1\nname: b\nevents:\n  - at: \"00:00:01\"\n    do: include\n    args: {{ path: \"{}\" }}\n",
                a.file_name().unwrap().to_str().unwrap()
            ),
        )
        .unwrap();
        match load(&a) {
            Err(ScenarioError::CircularInclude(_)) => {}
            other => panic!("expected CircularInclude, got {other:?}"),
        }
    }

    #[test]
    fn after_with_forward_declared_anchor_ok() {
        let yaml = r#"
version: 1
name: forward_ref
events:
  - after: start
    delay: 1s
    do: inject_daily_restart
  - at: "00:00:01"
    named: start
    do: inject_daily_restart
"#;
        load_from_str(yaml).expect("forward-declared anchors are valid");
    }
}
