//! IB `WhatToShow` enum — what kind of series to build for historical
//! and realtime-bar requests.
//!
//! Values mirror the IB API strings one-to-one; the [`WhatToShow`]
//! enum exists so callers never pass a raw `&str` across the
//! broker/router boundary.

use serde::{Deserialize, Serialize};
use std::fmt;

/// `WhatToShow` parameter for IB bar requests.
///
/// Each variant maps to exactly one IB wire token. Use [`as_ib_str`]
/// or [`Display`] to render; the strings are case-sensitive on the IB
/// side, so we centralise them here and never build them by hand.
///
/// [`as_ib_str`]: WhatToShow::as_ib_str
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum WhatToShow {
    /// Regular trades (wire token `TRADES`).
    Trades,
    /// Midpoint prices (`MIDPOINT`).
    Midpoint,
    /// Bid prices (`BID`).
    Bid,
    /// Ask prices (`ASK`).
    Ask,
    /// Combined bid/ask bars (`BID_ASK`).
    BidAsk,
    /// Historical volatility (`HISTORICAL_VOLATILITY`).
    HistoricalVolatility,
    /// Option implied volatility (`OPTION_IMPLIED_VOLATILITY`).
    OptionImpliedVolatility,
    /// Yield-ask (`YIELD_ASK`).
    YieldAsk,
    /// Yield-bid (`YIELD_BID`).
    YieldBid,
    /// Yield bid/ask combined (`YIELD_BID_ASK`).
    YieldBidAsk,
    /// Yield-last (`YIELD_LAST`).
    YieldLast,
    /// Trading-session schedule (`SCHEDULE`).
    Schedule,
    /// Split/dividend-adjusted last price (`ADJUSTED_LAST`).
    AdjustedLast,
}

impl WhatToShow {
    /// IB wire representation.
    pub const fn as_ib_str(&self) -> &'static str {
        match self {
            Self::Trades => "TRADES",
            Self::Midpoint => "MIDPOINT",
            Self::Bid => "BID",
            Self::Ask => "ASK",
            Self::BidAsk => "BID_ASK",
            Self::HistoricalVolatility => "HISTORICAL_VOLATILITY",
            Self::OptionImpliedVolatility => "OPTION_IMPLIED_VOLATILITY",
            Self::YieldAsk => "YIELD_ASK",
            Self::YieldBid => "YIELD_BID",
            Self::YieldBidAsk => "YIELD_BID_ASK",
            Self::YieldLast => "YIELD_LAST",
            Self::Schedule => "SCHEDULE",
            Self::AdjustedLast => "ADJUSTED_LAST",
        }
    }
}

impl fmt::Display for WhatToShow {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_ib_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ib_strings_match_spec() {
        let cases = [
            (WhatToShow::Trades, "TRADES"),
            (WhatToShow::Midpoint, "MIDPOINT"),
            (WhatToShow::Bid, "BID"),
            (WhatToShow::Ask, "ASK"),
            (WhatToShow::BidAsk, "BID_ASK"),
            (WhatToShow::HistoricalVolatility, "HISTORICAL_VOLATILITY"),
            (
                WhatToShow::OptionImpliedVolatility,
                "OPTION_IMPLIED_VOLATILITY",
            ),
            (WhatToShow::YieldAsk, "YIELD_ASK"),
            (WhatToShow::YieldBid, "YIELD_BID"),
            (WhatToShow::YieldBidAsk, "YIELD_BID_ASK"),
            (WhatToShow::YieldLast, "YIELD_LAST"),
            (WhatToShow::Schedule, "SCHEDULE"),
            (WhatToShow::AdjustedLast, "ADJUSTED_LAST"),
        ];
        for (w, s) in cases {
            assert_eq!(w.as_ib_str(), s);
            assert_eq!(w.to_string(), s);
        }
    }

    #[test]
    fn serde_roundtrip() {
        for w in [
            WhatToShow::Trades,
            WhatToShow::BidAsk,
            WhatToShow::AdjustedLast,
        ] {
            let json = serde_json::to_string(&w).unwrap();
            let back: WhatToShow = serde_json::from_str(&json).unwrap();
            assert_eq!(w, back);
        }
    }
}
