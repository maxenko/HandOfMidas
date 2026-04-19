//! Scenario script — closed-verb YAML DSL. Stage 06 expands the verb set.

use serde::{Deserialize, Serialize};

/// A parsed scenario loaded from YAML.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Scenario {
    pub name: String,
    pub schema_version: u32,
    pub description: Option<String>,
    pub steps: Vec<ScenarioStep>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ScenarioStep {
    /// Virtual time (milliseconds since scenario start) at which to fire.
    pub at_ms: u64,
    pub verb: ScenarioVerb,
}

/// Closed verb list. Stage 06 expands; new verbs require a schema-version bump.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum ScenarioVerb {
    Disconnect {
        session: u64,
        reason: String,
    },
    Lag {
        session: u64,
        ms: u64,
    },
    FarmOutage {
        code: i32,
        farms: Vec<String>,
    },
    FarmRestore {
        code: i32,
        farms: Vec<String>,
    },
    PriceJump {
        symbol: String,
        magnitude_pct: f64,
    },
    Halt {
        symbol: String,
        duration_ms: u64,
    },
    Burst {
        symbols: Vec<String>,
        multiplier: f64,
        duration_ms: u64,
    },
    DailyRestart,
}
