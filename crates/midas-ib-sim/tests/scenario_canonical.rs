//! Integration tests for the 9 canonical scenarios.
//!
//! Each scenario loads, runs against a [`MockEngine`] under a
//! [`VirtualClock`], and compares the resulting outgoing-command log to a
//! committed `.expected.jsonl` recording.
//!
//! ### Regenerating recordings
//!
//! Set `REGEN_EXPECTED=1` in the environment to overwrite the `.expected.jsonl`
//! files from this run. Recommended only when a scenario YAML change was
//! intentional.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use midas_ib_sim::engine::clock::{Clock, VirtualClock, VirtualInstant};
use midas_ib_sim::scenario::mock_engine::MockEngine;
use midas_ib_sim::scenario::{loader, recording};
use midas_ib_sim::ScenarioRunner;

const CANONICAL: &[&str] = &[
    "smoke",
    "bracket_happy",
    "pacing_violation",
    "farm_outage_mid_order",
    "fast_market",
    "flash_crash",
    "daily_restart",
    "line_limit_overflow",
    "partial_fill_drift",
];

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("scenarios")
}

/// Drive the virtual clock: for every waiter the runner parks, advance to
/// its deadline. Loops until the runner finishes (signalled by
/// `done.await`).
async fn drive_clock(clock: Arc<VirtualClock>, done: Arc<tokio::sync::Notify>) {
    loop {
        // Bounded wait: runner parks waiters as it encounters `at:` times.
        for _ in 0..20 {
            if clock.waiter_count() > 0 {
                break;
            }
            tokio::task::yield_now().await;
        }
        if clock.waiter_count() == 0 {
            // Either runner finished or produced no more waiters.
            tokio::select! {
                _ = done.notified() => return,
                _ = tokio::time::sleep(Duration::from_millis(1)) => {}
            }
            if clock.waiter_count() == 0 {
                continue;
            }
        }
        // Advance to the largest known waiter deadline — simplest way to fire
        // them all in order (the VirtualClock fires in deadline order).
        clock.advance_to_next_event();
        tokio::task::yield_now().await;
    }
}

async fn run_one(name: &str) -> MockEngine {
    let path = fixtures_dir().join(format!("{name}.yaml"));
    let scenario = loader::load(&path).unwrap_or_else(|e| panic!("{name}: loader err: {e}"));
    let engine = MockEngine::new();
    let clock = VirtualClock::shared();
    let done = Arc::new(tokio::sync::Notify::new());

    let driver = {
        let clock = Arc::clone(&clock);
        let done = Arc::clone(&done);
        tokio::spawn(drive_clock(clock, done))
    };

    // Use the clock trait handle for the runner; keep the concrete clock
    // for the driver.
    let clock_trait: Arc<dyn Clock> = clock.clone();
    let runner = ScenarioRunner::new(scenario, engine.clone(), clock_trait);
    let result = runner
        .run()
        .await
        .unwrap_or_else(|e| panic!("{name}: runner err: {e}"));
    assert_eq!(result.scenario_name, name);

    done.notify_one();
    // Drop the clock wait by advancing past any remaining schedule.
    clock.advance(VirtualInstant::from_secs(24 * 3600));
    driver.abort();
    let _ = driver.await;
    engine
}

fn expected_path(name: &str) -> PathBuf {
    fixtures_dir().join(format!("{name}.expected.jsonl"))
}

async fn run_canonical(name: &str) {
    let engine = run_one(name).await;
    let recording_path = expected_path(name);

    if std::env::var("REGEN_EXPECTED").is_ok() || !recording_path.exists() {
        recording::save(&engine, &recording_path)
            .unwrap_or_else(|e| panic!("{name}: save recording: {e}"));
        // Re-read immediately — catches serialisation quirks.
        let rec = recording::load(&recording_path)
            .unwrap_or_else(|e| panic!("{name}: load recording: {e}"));
        recording::assert_matches(&engine, &rec)
            .unwrap_or_else(|e| panic!("{name}: new recording mismatch: {e}"));
        return;
    }

    let expected =
        recording::load(&recording_path).unwrap_or_else(|e| panic!("{name}: load recording: {e}"));
    recording::assert_matches(&engine, &expected)
        .unwrap_or_else(|e| panic!("{name}: recording mismatch: {e}"));
}

#[tokio::test]
async fn canonical_smoke() {
    run_canonical("smoke").await;
}

#[tokio::test]
async fn canonical_bracket_happy() {
    run_canonical("bracket_happy").await;
}

#[tokio::test]
async fn canonical_pacing_violation() {
    run_canonical("pacing_violation").await;
}

#[tokio::test]
async fn canonical_farm_outage_mid_order() {
    run_canonical("farm_outage_mid_order").await;
}

#[tokio::test]
async fn canonical_fast_market() {
    run_canonical("fast_market").await;
}

#[tokio::test]
async fn canonical_flash_crash() {
    run_canonical("flash_crash").await;
}

#[tokio::test]
async fn canonical_daily_restart() {
    run_canonical("daily_restart").await;
}

#[tokio::test]
async fn canonical_line_limit_overflow() {
    run_canonical("line_limit_overflow").await;
}

#[tokio::test]
async fn canonical_partial_fill_drift() {
    run_canonical("partial_fill_drift").await;
}

#[test]
fn every_canonical_yaml_is_covered_by_a_test() {
    // Safety belt: if someone adds a fixture file, this fails until the
    // CANONICAL list (and corresponding test) grows.
    let found: Vec<String> = std::fs::read_dir(fixtures_dir())
        .unwrap()
        .filter_map(|r| r.ok())
        .filter(|e| e.path().extension().map(|x| x == "yaml").unwrap_or(false))
        .filter_map(|e| {
            e.path()
                .file_stem()
                .and_then(|s| s.to_str())
                .map(|s| s.to_string())
        })
        .collect();
    for name in &found {
        assert!(
            CANONICAL.contains(&name.as_str()),
            "new fixture `{name}.yaml` has no canonical test — add to CANONICAL + expected recording"
        );
    }
    for name in CANONICAL {
        assert!(
            found.contains(&name.to_string()),
            "canonical fixture `{name}.yaml` is missing"
        );
    }
}
