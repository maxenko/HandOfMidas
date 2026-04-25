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

/// B1 — `IbMarketData::connect` must NOT transition to
/// `ConnectionState::Ready` until the first
/// `FarmStatus { code: MarketDataFarmOk, connected: true, .. }` is
/// observed after `nextValidId`.
///
/// End-to-end coverage against the in-process `midas-ib-sim` server is
/// non-trivial: it requires spawning the sim as a child process, wiring
/// its stdout, and waiting for its port to settle. The isolated gate
/// behaviour is covered by
/// `midas_broker::ib::market_data::tests::ready_waits_for_farm_up_mkt`
/// (happy path), `wait_for_mkt_farm_up_skips_unrelated_events`
/// (noise rejection), and `farm_rx_subscribed_before_send_buffers_event`
/// (subscription-before-send ordering — the invariant `connect()`
/// relies on). When a sim-based IB end-to-end harness lands, un-ignore
/// this test and observe the `Connecting → Connected{..} → Ready`
/// watch-channel transitions against a sim configured to delay its MKT
/// bulletin via `FarmStatusEmitter::with_initial_delay`.
#[test]
#[ignore = "pending end-to-end sim harness; gate logic is covered by in-module tests"]
fn ready_waits_for_farm_up_mkt() {
    // TODO(B1 end-to-end): spawn midas-ib-sim with
    // FarmStatusEmitter::with_initial_delay(Duration::from_millis(500)),
    // then call IbMarketData::connect and assert the state transitions.
}
