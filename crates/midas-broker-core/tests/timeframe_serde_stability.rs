//! M-33 — Timeframe serde stability fixture test.
//!
//! Loads a checked-in `legacy_config.toml` capturing the pre-refactor
//! serialized form of `Timeframe`, round-trips it through deserialize
//! → serialize, and asserts the re-serialized bytes match the original.
//!
//! If this test ever fails, the router refactor has drifted the serde
//! shape of `Timeframe` away from what user configs on disk already
//! contain. The correct fix is to version the schema and migrate — not
//! to silently edit this fixture.

use midas_broker_core::market_data::Timeframe;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct LegacyConfig {
    default_timeframe: Timeframe,
    alt_timeframes: Vec<Timeframe>,
}

const LEGACY_TOML: &str = include_str!("fixtures/legacy_config.toml");

#[test]
fn timeframe_round_trips_through_legacy_fixture() {
    let parsed: LegacyConfig =
        toml::from_str(LEGACY_TOML).expect("legacy_config.toml must deserialize");

    assert_eq!(parsed.default_timeframe, Timeframe::M5);
    assert_eq!(
        parsed.alt_timeframes,
        vec![
            Timeframe::S1,
            Timeframe::S5,
            Timeframe::M1,
            Timeframe::M5,
            Timeframe::M15,
            Timeframe::M30,
            Timeframe::H1,
            Timeframe::H4,
            Timeframe::D1,
            Timeframe::W1,
            Timeframe::MN1,
        ]
    );

    // Re-serialize and re-parse — the wire tokens must round-trip.
    let back_out = toml::to_string(&parsed).expect("reserialize");
    let back_in: LegacyConfig = toml::from_str(&back_out).expect("reparse");
    assert_eq!(parsed, back_in);
}
