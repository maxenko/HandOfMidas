//! Farm / connection-lifecycle status events.
//!
//! IB emits "system" messages on codes 2103–2158 and 1100–1102. The
//! router fans those out as [`FarmStatus`] on a dedicated broadcast so
//! every consumer sees the same view of data-farm health without
//! tripping through per-symbol plumbing.
//!
//! Per M-14, `NextValidId` is NOT a farm code — it lives on
//! `MarketEvent::OrderingReady`.

use serde::{Deserialize, Serialize};

/// Snapshot of one farm-state transition.
///
/// `detail` is the raw IB text so operators can still see the unparsed
/// message in logs even after we enum-classify the code.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FarmStatus {
    /// Classified code (see [`FarmCode`]).
    pub code: FarmCode,
    /// Whether this transition marks "up" (`true`) or "down" (`false`).
    pub connected: bool,
    /// Raw IB message, preserved for diagnostics.
    pub detail: String,
}

/// IB data-farm and connection codes (M-13 / M-14).
///
/// Numeric comments show the IB error code that triggers each variant.
/// `NextValidId` is deliberately absent — see module docs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FarmCode {
    /// 2104 — market data farm is connected.
    MarketDataFarmOk,
    /// 2106 — historical data farm is connected.
    HistoricalDataFarmOk,
    /// 2108 — market data farm inactive but should be available.
    MarketDataFarmInactive,
    /// 2103 — market data farm connection broken.
    MarketDataFarmBroken,
    /// 2105 — historical data farm connection broken.
    HistoricalDataFarmBroken,
    /// 2158 — security definition farm is connected.
    SecDefFarmOk,
    /// 1100 — connection to TWS lost.
    ConnectionLost,
    /// 1101 — connection restored; subscription data lost.
    ConnectionRestoredDataLost,
    /// 1102 — connection restored; subscription data kept.
    ConnectionRestoredDataKept,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn farm_status_serde_roundtrip() {
        let s = FarmStatus {
            code: FarmCode::MarketDataFarmOk,
            connected: true,
            detail: "Market data farm connection is OK:usfarm".into(),
        };
        let json = serde_json::to_string(&s).unwrap();
        let back: FarmStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(s, back);
    }

    #[test]
    fn farm_code_hash_eq_consistency() {
        use std::collections::HashSet;
        let mut set: HashSet<FarmCode> = HashSet::new();
        set.insert(FarmCode::MarketDataFarmOk);
        set.insert(FarmCode::MarketDataFarmOk);
        set.insert(FarmCode::MarketDataFarmBroken);
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn farm_code_debug_does_not_panic() {
        for c in [
            FarmCode::MarketDataFarmOk,
            FarmCode::HistoricalDataFarmOk,
            FarmCode::MarketDataFarmInactive,
            FarmCode::MarketDataFarmBroken,
            FarmCode::HistoricalDataFarmBroken,
            FarmCode::SecDefFarmOk,
            FarmCode::ConnectionLost,
            FarmCode::ConnectionRestoredDataLost,
            FarmCode::ConnectionRestoredDataKept,
        ] {
            let _ = format!("{c:?}");
        }
    }
}
