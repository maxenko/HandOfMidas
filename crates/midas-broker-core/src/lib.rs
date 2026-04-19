use ordered_float::OrderedFloat;
use serde::{Deserialize, Serialize};
use std::fmt;

// ---------------------------------------------------------------------------
// ContractSpec — simplified, serializable instrument identifier
// ---------------------------------------------------------------------------

/// Both midas-broker and midas-feed convert to/from ibapi::Contract internally.
/// The UI crate only ever sees ContractSpec.
#[derive(Debug, Clone, Hash, Eq, PartialEq, Serialize, Deserialize)]
pub enum ContractSpec {
    Stock {
        symbol: String,
        exchange: String,
        currency: String,
    },
    Option {
        symbol: String,
        expiry: String,
        /// OrderedFloat<f64> implements Hash + Eq, which bare f64 does not.
        strike: OrderedFloat<f64>,
        right: OptionRight,
        exchange: String,
    },
    Future {
        symbol: String,
        expiry: String,
        exchange: String,
    },
    Forex {
        pair: String,
    },
}

impl ContractSpec {
    /// Returns the primary symbol for any contract type.
    pub fn symbol(&self) -> &str {
        match self {
            Self::Stock { symbol, .. }
            | Self::Option { symbol, .. }
            | Self::Future { symbol, .. } => symbol,
            Self::Forex { pair } => pair,
        }
    }
}

// ---------------------------------------------------------------------------
// SecurityType — IB security type for contracts
// ---------------------------------------------------------------------------

/// IB security type identifier. Replaces raw strings like "STK", "OPT", etc.
/// with a type-safe enum that serializes to the same IB API strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SecurityType {
    Stock,
    Option,
    Future,
    Forex,
}

impl fmt::Display for SecurityType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Stock => f.write_str("STK"),
            Self::Option => f.write_str("OPT"),
            Self::Future => f.write_str("FUT"),
            Self::Forex => f.write_str("CASH"),
        }
    }
}

impl std::str::FromStr for SecurityType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "STK" => Ok(Self::Stock),
            "OPT" => Ok(Self::Option),
            "FUT" => Ok(Self::Future),
            "CASH" => Ok(Self::Forex),
            other => Err(format!("unknown SecurityType: {other}")),
        }
    }
}

// ---------------------------------------------------------------------------
// OptionRight
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum OptionRight {
    Call,
    Put,
}

impl fmt::Display for OptionRight {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Call => write!(f, "C"),
            Self::Put => write!(f, "P"),
        }
    }
}

// ---------------------------------------------------------------------------
// SymbolKey — compact symbol identifier for lookups
// ---------------------------------------------------------------------------

/// Wraps the IB contract ID for efficient comparison on the hot path.
/// The string symbol is carried for display purposes.
///
/// `Ord` / `PartialOrd` compare by `(contract_id, symbol)` so `BTreeMap`
/// keyed on `SymbolKey` iterates in a stable, deterministic order — the
/// IB sim (and anyone else doing cross-symbol aggregation) relies on this
/// for reproducible output.
#[derive(Clone, Debug, Hash, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
pub struct SymbolKey {
    pub contract_id: i32,
    pub symbol: String,
}

impl fmt::Display for SymbolKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}({})", self.symbol, self.contract_id)
    }
}

// ---------------------------------------------------------------------------
// Timeframe — standard bar durations
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Timeframe {
    S1,
    S5,
    S15,
    S30,
    M1,
    M5,
    M15,
    M30,
    H1,
    H4,
    D1,
    W1,
    MN1,
}

impl Timeframe {
    /// Duration of one bar in seconds.
    pub fn as_secs(&self) -> u64 {
        match self {
            Self::S1 => 1,
            Self::S5 => 5,
            Self::S15 => 15,
            Self::S30 => 30,
            Self::M1 => 60,
            Self::M5 => 300,
            Self::M15 => 900,
            Self::M30 => 1800,
            Self::H1 => 3600,
            Self::H4 => 14400,
            Self::D1 => 86400,
            Self::W1 => 604800,
            Self::MN1 => 2592000, // ~30 days
        }
    }
}

impl fmt::Display for Timeframe {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::S1 => "1s",
            Self::S5 => "5s",
            Self::S15 => "15s",
            Self::S30 => "30s",
            Self::M1 => "1m",
            Self::M5 => "5m",
            Self::M15 => "15m",
            Self::M30 => "30m",
            Self::H1 => "1h",
            Self::H4 => "4h",
            Self::D1 => "1d",
            Self::W1 => "1w",
            Self::MN1 => "1M",
        };
        write!(f, "{s}")
    }
}

// ---------------------------------------------------------------------------
// OhlcvBar — single candlestick bar
// ---------------------------------------------------------------------------

/// A single OHLCV candlestick bar. Used by both the broker (historical data)
/// and the UI (chart rendering).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OhlcvBar {
    /// Bar open time (UTC epoch seconds).
    pub timestamp: i64,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: i64,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contract_spec_stock_hashable() {
        use std::collections::HashSet;
        let mut set = HashSet::new();
        let spec = ContractSpec::Stock {
            symbol: "AAPL".into(),
            exchange: "SMART".into(),
            currency: "USD".into(),
        };
        set.insert(spec.clone());
        assert!(set.contains(&spec));
    }

    #[test]
    fn contract_spec_option_hashable() {
        use std::collections::HashSet;
        let mut set = HashSet::new();
        let spec = ContractSpec::Option {
            symbol: "AAPL".into(),
            expiry: "20260320".into(),
            strike: OrderedFloat(150.0),
            right: OptionRight::Call,
            exchange: "SMART".into(),
        };
        set.insert(spec.clone());
        assert!(set.contains(&spec));
    }

    #[test]
    fn contract_spec_symbol() {
        let stock = ContractSpec::Stock {
            symbol: "MSFT".into(),
            exchange: "SMART".into(),
            currency: "USD".into(),
        };
        assert_eq!(stock.symbol(), "MSFT");

        let forex = ContractSpec::Forex {
            pair: "EUR.USD".into(),
        };
        assert_eq!(forex.symbol(), "EUR.USD");
    }

    #[test]
    fn timeframe_seconds() {
        assert_eq!(Timeframe::M5.as_secs(), 300);
        assert_eq!(Timeframe::D1.as_secs(), 86400);
    }

    #[test]
    fn security_type_display_fromstr() {
        let cases = [
            (SecurityType::Stock, "STK"),
            (SecurityType::Option, "OPT"),
            (SecurityType::Future, "FUT"),
            (SecurityType::Forex, "CASH"),
        ];
        for (st, s) in cases {
            assert_eq!(st.to_string(), s);
            assert_eq!(s.parse::<SecurityType>().unwrap(), st);
        }
        assert!("BOND".parse::<SecurityType>().is_err());
    }

    #[test]
    fn timeframe_display() {
        assert_eq!(Timeframe::S1.to_string(), "1s");
        assert_eq!(Timeframe::H4.to_string(), "4h");
    }
}
