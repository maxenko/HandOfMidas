//! Tick-level market-data types.
//!
//! Ticks model the full IB tick taxonomy: `reqMktData` price/size pairs,
//! `reqTickByTickData` flavours, string/params/generic ticks. The
//! [`TickValue`] enum is deliberately heterogeneous so the router can
//! carry every IB tick shape without parallel channels — see M-17 for
//! the atomic `PriceSize` variant that sidesteps the classic
//! "size before price" ordering trap.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::SymbolKey;

use super::req_id::ReqId;

/// A single market-data tick, whatever its flavour.
///
/// `kind` is the IB callback family (price / size / generic / string /
/// params / atomic price-size). `tick_type` is the sub-code (`Bid`,
/// `AskSize`, `Volume`, etc.). `value` carries the typed payload.
/// `attrs` mirrors the IB attribute bitflag block — held on every price
/// tick, defaulted-empty on size ticks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Tick {
    /// Symbol this tick belongs to.
    pub symbol: SymbolKey,
    /// Originating request id (router plumbing).
    pub req_id: ReqId,
    /// IB callback family.
    pub kind: TickKind,
    /// Sub-type (Bid / Ask / Last / Volume / …).
    pub tick_type: TickType,
    /// Typed payload.
    pub value: TickValue,
    /// Per-tick attribute flags.
    pub attrs: TickAttributes,
    /// Event timestamp (UTC).
    pub ts: DateTime<Utc>,
}

/// IB tick callback family.
///
/// `PriceSize` carries an atomic (price, size) pair — used for
/// tick-by-tick callbacks where IB delivers the pair together. Keeping
/// it a first-class variant (rather than two separate ticks) avoids
/// races where the size arrives before the price (M-17).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TickKind {
    /// `tickPrice` family (Bid, Ask, Last, High, …).
    Price,
    /// `tickSize` family (BidSize, AskSize, Volume, …).
    Size,
    /// Atomic price+size pair (tick-by-tick last / bid-ask).
    PriceSize,
    /// Generic numeric tick (e.g. `OptionImpliedVolatility`).
    Generic,
    /// String-valued tick (e.g. `LastTimestamp`).
    String,
    /// `tickReqParams` — market data line params.
    Params,
}

/// IB tick sub-type.
///
/// Non-exhaustive so new IB tick types can be added without breaking
/// consumers that match on known variants. The initial set covers the
/// top-20 most commonly emitted by `reqMktData`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum TickType {
    /// IB tick 1.
    Bid,
    /// IB tick 2.
    Ask,
    /// IB tick 4.
    Last,
    /// IB tick 0.
    BidSize,
    /// IB tick 3.
    AskSize,
    /// IB tick 5.
    LastSize,
    /// IB tick 8.
    Volume,
    /// IB tick 6.
    High,
    /// IB tick 7.
    Low,
    /// IB tick 9.
    Close,
    /// IB tick 14.
    Open,
    /// IB tick 49.
    HaltedState,
}

/// Typed tick payload.
///
/// One variant per IB callback shape. `PriceSize` is the atomic form
/// for tick-by-tick callbacks (M-17).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum TickValue {
    /// `tickPrice` value.
    Price(f64),
    /// `tickSize` value. IB uses i64 on the wire for newer `Decimal`
    /// size callbacks.
    Size(i64),
    /// Atomic (price, size) pair.
    PriceSize {
        /// Trade / quote price.
        price: f64,
        /// Trade / quote size.
        size: i64,
    },
    /// `tickGeneric` value.
    Generic(f64),
    /// `tickString` value.
    Text(String),
    /// Boolean flag (e.g. `HaltedState`).
    Bool(bool),
}

/// Per-tick attribute flags.
///
/// Matches the `rust-ibapi` 2.10 `TickAttribute` set (M-22). All fields
/// default to `false`; size-only ticks carry a defaulted value.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TickAttributes {
    /// IB flag: order can be executed against this quote without a
    /// dedicated market check.
    pub can_auto_execute: bool,
    /// IB flag: trade occurred outside the best bid/ask.
    pub past_limit: bool,
    /// IB flag: quote/trade received before the regular-session open.
    pub pre_open: bool,
    /// IB flag: trade was reported, but did not update the official
    /// last trade price.
    pub unreported: bool,
    /// IB flag: bid moved below the session low.
    pub bid_past_low: bool,
    /// IB flag: ask moved above the session high.
    pub ask_past_high: bool,
}

/// Flavour of a `reqTickByTickData` subscription (BR-11).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TickByTickKind {
    /// Last-trade ticks only.
    Last,
    /// All-last (includes out-of-sequence prints).
    AllLast,
    /// Top-of-book bid/ask ticks.
    BidAsk,
    /// Synthetic midpoint ticks.
    MidPoint,
}

/// IB generic tick list passed to `reqMktData` (BR-10).
///
/// Example entries: `233` (RT Volume), `293` (Trade Count).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GenericTicks(pub Vec<u32>);

impl GenericTicks {
    /// Build a new empty tick-list.
    pub fn new() -> Self {
        Self::default()
    }

    /// Consume a vector of tick codes into a [`GenericTicks`] request.
    pub fn from_codes(codes: Vec<u32>) -> Self {
        Self(codes)
    }

    /// Render the list into IB's comma-separated wire format.
    ///
    /// Empty list → empty string (IB treats both as "no generic ticks").
    pub fn to_ib_string(&self) -> String {
        self.0
            .iter()
            .map(|c| c.to_string())
            .collect::<Vec<_>>()
            .join(",")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn sample_tick() -> Tick {
        Tick {
            symbol: SymbolKey {
                contract_id: 265598,
                symbol: "AAPL".into(),
            },
            req_id: ReqId(7),
            kind: TickKind::PriceSize,
            tick_type: TickType::Last,
            value: TickValue::PriceSize {
                price: 100.25,
                size: 50,
            },
            attrs: TickAttributes {
                can_auto_execute: true,
                ..TickAttributes::default()
            },
            ts: Utc.with_ymd_and_hms(2026, 1, 2, 14, 30, 0).unwrap(),
        }
    }

    #[test]
    fn tick_serde_roundtrip() {
        let t = sample_tick();
        let json = serde_json::to_string(&t).unwrap();
        let back: Tick = serde_json::from_str(&json).unwrap();
        assert_eq!(t, back);
    }

    #[test]
    fn tick_value_text_serde_roundtrip() {
        let v = TickValue::Text("20260102-14:30:00".into());
        let json = serde_json::to_string(&v).unwrap();
        let back: TickValue = serde_json::from_str(&json).unwrap();
        assert_eq!(v, back);
    }

    #[test]
    fn tick_debug_does_not_panic_on_nan() {
        let mut t = sample_tick();
        t.value = TickValue::Price(f64::NAN);
        let _ = format!("{t:?}");
    }

    #[test]
    fn tick_attributes_default_is_all_false() {
        let a = TickAttributes::default();
        assert!(!a.can_auto_execute);
        assert!(!a.past_limit);
        assert!(!a.pre_open);
        assert!(!a.unreported);
        assert!(!a.bid_past_low);
        assert!(!a.ask_past_high);
    }

    #[test]
    fn tick_type_hash_eq_consistency() {
        use std::collections::HashSet;
        let mut set: HashSet<TickType> = HashSet::new();
        set.insert(TickType::Bid);
        set.insert(TickType::Bid);
        set.insert(TickType::Ask);
        assert_eq!(set.len(), 2);
        assert!(set.contains(&TickType::Bid));
    }

    #[test]
    fn generic_ticks_to_ib_string() {
        assert_eq!(GenericTicks::new().to_ib_string(), "");
        assert_eq!(
            GenericTicks::from_codes(vec![233, 293]).to_ib_string(),
            "233,293"
        );
    }

    #[test]
    fn generic_ticks_serde_roundtrip() {
        let g = GenericTicks::from_codes(vec![233, 293, 100]);
        let json = serde_json::to_string(&g).unwrap();
        let back: GenericTicks = serde_json::from_str(&json).unwrap();
        assert_eq!(g, back);
    }
}
