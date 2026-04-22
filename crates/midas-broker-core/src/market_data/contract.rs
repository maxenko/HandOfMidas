//! Contract-resolution types (M-34).
//!
//! [`ContractDetails`] is what `MarketDataSource::resolve_contract`
//! returns — a fully-qualified instrument descriptor the router can
//! memoise. `SecurityType` is re-exported from the crate root; it
//! already exists there and carries IB wire strings.

use serde::{Deserialize, Serialize};

// `SecurityType` lives at the crate root (`crate::SecurityType`) —
// re-export for callers who import the whole `market_data` module.
pub use crate::SecurityType;

/// Fully-qualified contract descriptor.
///
/// Enough information for the router to pick a SMART / primary
/// exchange route and for downstream persistence to round-trip an
/// instrument across sessions.
///
/// `TODO(S1-followup)`: extend with trading-hours strings once the
/// router's pre-flight cache needs them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContractDetails {
    /// IB contract id (unique primary key).
    pub contract_id: i32,
    /// Short symbol (`"AAPL"`, `"ES"`, `"EUR.USD"`).
    pub symbol: String,
    /// Security type.
    pub sec_type: SecurityType,
    /// Routing exchange (`"SMART"`, `"CME"`, `"IDEALPRO"`, …).
    pub exchange: String,
    /// Primary listing exchange (`"NASDAQ"`, `"NYSE"`, …).
    pub primary_exchange: Option<String>,
    /// Quote currency.
    pub currency: String,
    /// Long legal description from IB, when available.
    pub long_name: Option<String>,
    /// Minimum tradeable tick size.
    pub min_tick: f64,
    /// IB `multiplier` field (options / futures).
    pub multiplier: Option<String>,
    /// IB `tradingClass` field.
    pub trading_class: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_details() -> ContractDetails {
        ContractDetails {
            contract_id: 265598,
            symbol: "AAPL".into(),
            sec_type: SecurityType::Stock,
            exchange: "SMART".into(),
            primary_exchange: Some("NASDAQ".into()),
            currency: "USD".into(),
            long_name: Some("Apple Inc".into()),
            min_tick: 0.01,
            multiplier: None,
            trading_class: Some("NMS".into()),
        }
    }

    #[test]
    fn contract_details_serde_roundtrip() {
        let d = sample_details();
        let json = serde_json::to_string(&d).unwrap();
        let back: ContractDetails = serde_json::from_str(&json).unwrap();
        assert_eq!(d, back);
    }

    #[test]
    fn contract_details_partial_eq_is_reflexive() {
        // `min_tick: f64` rules out `Eq`/`Hash`; `PartialEq` is still
        // useful for assertions and diff output.
        let d = sample_details();
        assert_eq!(d, d.clone());
    }

    #[test]
    fn contract_details_debug_does_not_panic_on_nan_tick() {
        let mut d = sample_details();
        d.min_tick = f64::NAN;
        let _ = format!("{d:?}");
    }
}
