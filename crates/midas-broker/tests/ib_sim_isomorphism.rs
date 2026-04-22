//! Isomorphism tests between `IbMarketData` (slice 4) and
//! `SimMarketData` (slice 3).
//!
//! These tests verify that both backends emit the same sequence of
//! [`MarketEvent`](midas_broker_core::market_data::MarketEvent)s when
//! fed identical synthetic inputs. They are `#[ignore]`-d until slice 4
//! lands the IB backend; the S4 implementer is expected to un-ignore
//! them as each pair goes green.

// Placeholder. When S4 lands, import `midas_broker::ib::IbMarketData`
// here and assert event-sequence equivalence with `SimMarketData`.
#[test]
#[ignore = "un-ignored by slice 4 when IbMarketData is available"]
fn initial_burst_sequence_matches_ib() {
    // TODO(S4): construct both backends, subscribe to the same symbol,
    // and diff the first 32 events from each.
}

#[test]
#[ignore = "un-ignored by slice 4 when IbMarketData is available"]
fn farm_up_sequence_matches_ib() {
    // TODO(S4): observe FarmStatus broadcast order on construct.
}
