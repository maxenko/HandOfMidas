//! Control-plane endpoint payloads. Stage 06 fills in the inject API.

use serde::{Deserialize, Serialize};

/// Body of `POST /control/inject/disconnect`. Stage 06 implements.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DisconnectInject {
    pub session_id: u64,
    pub reason: String,
}

/// Body of `POST /control/inject/lag`. Stage 06 implements.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LagInject {
    pub session_id: u64,
    pub duration_ms: u64,
}

/// Body of `POST /control/inject/farm`. Stage 06 implements.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FarmInject {
    pub code: i32,
    pub farms: Vec<String>,
    pub up: bool,
}

/// Body of `POST /control/inject` — the Stage 09 unified fault-injection
/// surface exposed to the `midas-app` devloop. Variants mirror the
/// `midas-devloop-proto::SimFault` shape so callers can deserialize on
/// one side and re-serialise on the other with no translation.
///
/// Routing:
/// - `Disconnect` → broadcast `InjectDisconnect` to every active session.
/// - `PacingViolation` → `InjectPacingViolation` on every active session.
/// - `FarmOutage { farms }` → `InjectFarmOutage { code: 2103, farms }`.
/// - `FarmRestore { farms, data_lost }` →
///   `InjectFarmRestore { code: 2104 | 2105, farms }` (1101 ≙ data lost,
///   1102 ≙ data retained — both are emitted via the 2104/2105 farm
///   events in the engine's internal numbering).
/// - `PriceJump` / `Gap` / `Halt` / `Burst` — routed as-is. These
///   require `HybridEngine` wiring that lands in Wave 4; the endpoint
///   accepts them today so devloop journeys keep a stable surface.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FaultInject {
    Disconnect,
    PacingViolation,
    FarmOutage {
        farms: Vec<String>,
    },
    FarmRestore {
        farms: Vec<String>,
        #[serde(default)]
        data_lost: bool,
    },
    PriceJump {
        symbol: String,
        magnitude_pct: f64,
    },
    Gap {
        symbol: String,
        to: f64,
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fault_inject_matches_sim_fault_wire_format() {
        // These exact JSON shapes are what the midas-app devloop
        // produces via `midas_devloop_proto::SimFault` serialisation.
        // Breaking wire compat here breaks the devloop integration.
        let cases = [
            (r#"{"type":"disconnect"}"#, FaultInject::Disconnect),
            (
                r#"{"type":"pacing_violation"}"#,
                FaultInject::PacingViolation,
            ),
            (
                r#"{"type":"farm_outage","farms":["usfarm"]}"#,
                FaultInject::FarmOutage {
                    farms: vec!["usfarm".into()],
                },
            ),
            (
                r#"{"type":"price_jump","symbol":"AAPL","magnitude_pct":-5.0}"#,
                FaultInject::PriceJump {
                    symbol: "AAPL".into(),
                    magnitude_pct: -5.0,
                },
            ),
        ];
        for (json, expected) in cases {
            let parsed: FaultInject = serde_json::from_str(json).unwrap();
            assert_eq!(parsed, expected, "failed on {json}");
        }
    }

    #[test]
    fn farm_restore_accepts_absent_data_lost() {
        let parsed: FaultInject =
            serde_json::from_str(r#"{"type":"farm_restore","farms":["usfarm"]}"#).unwrap();
        match parsed {
            FaultInject::FarmRestore { data_lost, .. } => {
                assert!(!data_lost, "data_lost defaults to false");
            }
            other => panic!("unexpected variant: {other:?}"),
        }
    }
}
