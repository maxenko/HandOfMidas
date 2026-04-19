//! `.expected.jsonl` recording + replay.
//!
//! One JSON object per line. Each line is a projected [`MockCmd`]. The
//! format is deliberately boring — `jq` reads it, `diff` compares it,
//! CI blocks PRs that drift without updating the recording.
//!
//! ### Record a run
//!
//! ```no_run
//! # use midas_ib_sim::scenario::{mock_engine::MockEngine, recording};
//! # let engine = MockEngine::new();
//! // After running a scenario against `engine`:
//! recording::save(&engine, "fixtures/scenarios/smoke.expected.jsonl").unwrap();
//! ```
//!
//! ### Compare a replay
//!
//! ```no_run
//! # use midas_ib_sim::scenario::{mock_engine::MockEngine, recording};
//! # let engine = MockEngine::new();
//! let expected = recording::load("fixtures/scenarios/smoke.expected.jsonl").unwrap();
//! recording::assert_matches(&engine, &expected).unwrap();
//! ```

use std::fs;
use std::io::{self, Write};
use std::path::Path;

use super::engine_adapter::ScenarioEngine;
use super::mock_engine::MockCmd;
#[cfg(test)]
use super::mock_engine::MockEngine;

/// I/O + diff diagnostics.
#[derive(Debug, thiserror::Error)]
pub enum RecordingError {
    #[error("i/o: {0}")]
    Io(#[from] io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("recording mismatch at line {line}: expected\n  {expected:?}\ngot\n  {got:?}")]
    Mismatch {
        line: usize,
        expected: Box<MockCmd>,
        got: Box<MockCmd>,
    },
    #[error("recording has {expected_len} lines but run produced {got_len}")]
    LengthMismatch { expected_len: usize, got_len: usize },
}

/// Persist the engine's current outgoing-command log to `path`. Overwrites
/// any existing file. Works for any [`ScenarioEngine`] — Wave 3 uses this
/// against both [`MockEngine`] and the real-engine adapter.
pub fn save(engine: &dyn ScenarioEngine, path: impl AsRef<Path>) -> Result<(), RecordingError> {
    let path = path.as_ref();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = fs::File::create(path)?;
    for cmd in engine.outgoing() {
        let line = serde_json::to_string(&cmd)?;
        writeln!(file, "{line}")?;
    }
    Ok(())
}

/// Read a `.expected.jsonl` file into a vector of projected commands.
pub fn load(path: impl AsRef<Path>) -> Result<Vec<MockCmd>, RecordingError> {
    let text = fs::read_to_string(path)?;
    let mut out = Vec::new();
    for raw in text.lines() {
        let raw = raw.trim();
        if raw.is_empty() {
            continue;
        }
        let cmd: MockCmd = serde_json::from_str(raw)?;
        out.push(cmd);
    }
    Ok(out)
}

/// Assert the engine's captured command sequence matches `expected`, command-
/// by-command. Emits a [`RecordingError::Mismatch`] on the first divergence.
pub fn assert_matches(
    engine: &dyn ScenarioEngine,
    expected: &[MockCmd],
) -> Result<(), RecordingError> {
    let actual = engine.outgoing();
    if actual.len() != expected.len() {
        return Err(RecordingError::LengthMismatch {
            expected_len: expected.len(),
            got_len: actual.len(),
        });
    }
    for (i, (a, b)) in actual.iter().zip(expected.iter()).enumerate() {
        if a != b {
            return Err(RecordingError::Mismatch {
                line: i + 1,
                expected: Box::new(b.clone()),
                got: Box::new(a.clone()),
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_and_load_round_trip() {
        let engine = MockEngine::new();
        engine.record(MockCmd::Sleep {
            duration: "1s".into(),
        });
        engine.record(MockCmd::InjectDailyRestart);
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("rec.jsonl");
        save(&engine, &path).unwrap();
        let loaded = load(&path).unwrap();
        assert_eq!(loaded.len(), 2);
        assert_matches(&engine, &loaded).unwrap();
    }

    #[test]
    fn mismatch_reports_line_number() {
        let engine = MockEngine::new();
        engine.record(MockCmd::Sleep {
            duration: "1s".into(),
        });
        let expected = vec![MockCmd::InjectDailyRestart];
        match assert_matches(&engine, &expected) {
            Err(RecordingError::Mismatch { line, .. }) => assert_eq!(line, 1),
            other => panic!("expected mismatch, got {other:?}"),
        }
    }

    #[test]
    fn length_mismatch_detected() {
        let engine = MockEngine::new();
        engine.record(MockCmd::InjectDailyRestart);
        let expected = vec![];
        assert!(matches!(
            assert_matches(&engine, &expected),
            Err(RecordingError::LengthMismatch { .. })
        ));
    }
}
