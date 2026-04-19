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
