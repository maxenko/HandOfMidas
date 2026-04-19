//! Wave-3 Part B: canonical scenarios run against the real engine.
//!
//! Mirrors `scenario_canonical.rs` but targets
//! [`midas_ib_sim::scenario::engine_adapter::RealScenarioEngine`] instead of
//! [`midas_ib_sim::scenario::mock_engine::MockEngine`]. Proves that the
//! Wave-3 orchestrator, market-data synthetic engine, order simulator, and
//! quirk guards all wire together cleanly enough to run every shipped
//! fixture to completion under a [`VirtualClock`].
//!
//! The scenario-runner's auto-fill shortcut (mirroring `MOCK_FILL_DELAY`)
//! keeps recordings byte-identical with the mock-engine path — orders
//! deterministically settle in a bounded virtual window regardless of the
//! synthetic engine's Hawkes excitement. Wave-4 scenarios can opt out of
//! the shortcut when they want to exercise the live fill model end-to-end.
//!
//! ### Regenerating recordings
//!
//! Set `REGEN_EXPECTED=1` to overwrite `.expected.jsonl` files.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use midas_broker_core::SymbolKey;
use midas_ib_sim::engine::clock::{Clock, VirtualClock, VirtualInstant};
use midas_ib_sim::market_data::generator::{SymbolPreset, SyntheticEngine};
use midas_ib_sim::market_data::MarketDataEngine;
use midas_ib_sim::quirks::QuirksConfig;
use midas_ib_sim::scenario::engine_adapter::{RealScenarioEngine, ScenarioEngine};
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

/// Drive the virtual clock on a parallel task: advance to the next parked
/// waiter until the runner finishes.
async fn drive_clock(clock: Arc<VirtualClock>, done: Arc<tokio::sync::Notify>) {
    loop {
        for _ in 0..20 {
            if clock.waiter_count() > 0 {
                break;
            }
            tokio::task::yield_now().await;
        }
        if clock.waiter_count() == 0 {
            tokio::select! {
                _ = done.notified() => return,
                _ = tokio::time::sleep(Duration::from_millis(1)) => {}
            }
            if clock.waiter_count() == 0 {
                continue;
            }
        }
        clock.advance_to_next_event();
        tokio::task::yield_now().await;
    }
}

/// Convert the scenario's symbol preset string into a [`SymbolPreset`].
fn preset_from_str(s: &str) -> SymbolPreset {
    match s.to_ascii_lowercase().as_str() {
        "liquid" => SymbolPreset::Liquid,
        "midcap" | "mid_cap" => SymbolPreset::MidCap,
        "illiquid" => SymbolPreset::Illiquid,
        _ => SymbolPreset::Liquid,
    }
}

/// Build a `SyntheticEngine` seeded with every symbol the scenario declares.
/// Uses the same DJB2-derived contract id the scenario injector uses so
/// `SubKey`s line up across subscribe / emit / snapshot paths.
fn build_market_data(
    scenario: &midas_ib_sim::scenario::Scenario,
    seed: u64,
) -> Box<dyn MarketDataEngine> {
    let mut eng = SyntheticEngine::new(seed);
    for sym in &scenario.symbols {
        let key = synth_symbol_key(&sym.symbol);
        let preset = sym
            .preset
            .as_deref()
            .map(preset_from_str)
            .unwrap_or(SymbolPreset::Liquid);
        eng.register(key, preset, sym.initial_price);
    }
    Box::new(eng)
}

fn synth_symbol_key(sym: &str) -> SymbolKey {
    let mut hash = 5381i32;
    for b in sym.bytes() {
        hash = hash.wrapping_mul(33).wrapping_add(b as i32);
    }
    let contract_id = (hash ^ 0x5f5f5f5f).unsigned_abs() as i32;
    SymbolKey {
        contract_id,
        symbol: sym.to_string(),
    }
}

async fn run_one(name: &str) -> RealScenarioEngine {
    let path = fixtures_dir().join(format!("{name}.yaml"));
    let scenario = loader::load(&path).unwrap_or_else(|e| panic!("{name}: loader err: {e}"));

    let clock = VirtualClock::shared();
    let clock_trait: Arc<dyn Clock> = clock.clone();
    let market_data = build_market_data(&scenario, scenario.seed);
    let quirks = QuirksConfig::default();
    let engine = RealScenarioEngine::new(clock_trait.clone(), market_data, &quirks, scenario.seed);

    let done = Arc::new(tokio::sync::Notify::new());
    let driver = {
        let clock = Arc::clone(&clock);
        let done = Arc::clone(&done);
        tokio::spawn(drive_clock(clock, done))
    };

    let runner_handle = engine.handle();
    let runner = ScenarioRunner::new(scenario, runner_handle, clock_trait);
    let result = runner
        .run()
        .await
        .unwrap_or_else(|e| panic!("{name}: runner err: {e}"));
    assert_eq!(result.scenario_name, name);

    done.notify_one();
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
async fn real_smoke() {
    run_canonical("smoke").await;
}

#[tokio::test]
async fn real_bracket_happy() {
    run_canonical("bracket_happy").await;
}

#[tokio::test]
async fn real_pacing_violation() {
    run_canonical("pacing_violation").await;
}

#[tokio::test]
async fn real_farm_outage_mid_order() {
    run_canonical("farm_outage_mid_order").await;
}

#[tokio::test]
async fn real_fast_market() {
    run_canonical("fast_market").await;
}

#[tokio::test]
async fn real_flash_crash() {
    run_canonical("flash_crash").await;
}

#[tokio::test]
async fn real_daily_restart() {
    run_canonical("daily_restart").await;
}

#[tokio::test]
async fn real_line_limit_overflow() {
    run_canonical("line_limit_overflow").await;
}

#[tokio::test]
async fn real_partial_fill_drift() {
    run_canonical("partial_fill_drift").await;
}

/// Determinism: running any canonical scenario three times against the real
/// engine under a fresh `VirtualClock` must produce byte-identical
/// recordings. Guards against RNG / `HashMap` iteration leaks.
#[tokio::test]
async fn real_scenarios_are_deterministic_across_runs() {
    for name in CANONICAL {
        let a = run_one(name).await;
        let b = run_one(name).await;
        let c = run_one(name).await;
        assert_eq!(
            a.outgoing(),
            b.outgoing(),
            "{name}: run 1 vs run 2 diverged"
        );
        assert_eq!(
            b.outgoing(),
            c.outgoing(),
            "{name}: run 2 vs run 3 diverged"
        );
    }
}

/// Safety belt: every canonical fixture must be covered by a test.
#[test]
fn every_canonical_yaml_has_a_real_engine_test() {
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
            "fixture `{name}.yaml` has no real-engine canonical test"
        );
    }
    for name in CANONICAL {
        assert!(
            found.contains(&name.to_string()),
            "canonical `{name}.yaml` fixture is missing"
        );
    }
}
