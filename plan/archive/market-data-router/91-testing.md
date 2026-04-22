# Testing Strategy

The refactor is high-risk; the test matrix is correspondingly thorough. Four test tiers:

1. **Type-level / unit** — per-slice, cover the types and helpers.
2. **Provider fidelity** — the sim backend must produce IB-faithful event sequences. Independent, fast.
3. **Router behavior** — refcounting, fan-out, history+live seam. Uses sim provider.
4. **App integration** — end-to-end with iced running (minimal headless iced test harness if possible, else integration over dev_harness).

## Tier 1 — unit tests

Per slice, covered in each slice's own plan doc. Expected coverage:

- S1: type serde roundtrip, `Eq`/`Hash` consistency, `ReqId::next` monotonicity.
- S2: trait object-safety (`impl MarketDataSource for Mock`), `Drop` closures fire on handle drop.
- S3: initial burst shape, tick cadence range, cancel timing, farm-status sequence, reconnect drops subs, historical-stream transition (BR-9).
- S4: live-IB integration tests (feature-gated `ib_live_tests`).
- S5: single upstream per symbol, refcount correctness, publisher task shutdown.
- S6: aggregator tick folding, bar close on window roll, shared aggregator for same `(sym, tf)`.
- S7: per-consumer subscription teardown on chart close, watchlist price updates, chart bar batch delivery.
- S8: FrameCoalescer batching, Lagged→resync flow.
- S9: no dead imports, all doc comments present.

## Tier 2 — provider fidelity (sim ↔ IB isomorphism)

Located in `crates/midas-broker/tests/provider_isomorphism.rs`. Tests run against the sim always; optionally against IB paper when `ib_live_tests` feature is enabled.

**BR-24: Tier 2 live-IB tests are developer-local only.** They are NOT CI-gated. Run checklist: every release branch + every major architecture change. Local command: `cargo test -p midas-broker --features ib_live_tests -- --include-ignored`. Requires a running TWS paper session on 127.0.0.1:4002 (or configured alt port); document the TWS paper setup in the test file's doc-comment.

Every test asserts on the **shape** of the event stream, not exact values or timings:

1. `initial_burst_includes_required_tick_types` — within 3 seconds of `subscribe_ticks`, receive at least Bid, Ask, Last, Volume, and Params tick events.
2. `cancel_terminates_stream_within_500ms` — drop handle, assert stream closes within 500 ms. (BR-20: `start_paused` for sim; wall-clock acceptable in the IB-gated variant.)
3. `multiple_subs_fan_out_independently` — two reqIds for same symbol, each receives its own events, counts are comparable.
4. `historical_stream_end_followed_by_update` (BR-9) — after `End` arrives from `historical_stream`, at least one `Update` follows within 15 seconds.
5. `historical_bars_returns_result_immediately` (BR-9) — `historical_bars` returns `HistoricalBarsResult` without a live tail.
6. `realtime_bars_cadence` — subscribe realtime bars, receive at least 3 bars in 16 seconds (first may take ≤ 10s, subsequent at 5s cadence).
7. `farm_up_reports_within_connect_window` — connect, within 5 s receive MarketDataFarmOk and HistoricalDataFarmOk (M-13).
8. `disconnect_drops_all_streams` — subscribe 3 streams, trigger disconnect, all 3 receive SubscriptionEnded or stream closes.

These tests define the sim's fidelity contract. If the sim diverges from IB, these tests catch it.

## Tier 2b — Ground-truth fidelity fixtures (BR-23)

Sim tests alone are circular (they assert what the sim author specified). Tier 2b backs the sim's behaviour with one-time captures from a real IB paper session, versioned in the repo.

- **Capture tool**: `cargo run -p midas-broker --features ib_live_tests --bin capture_fidelity_fixture -- SPY` records ~60 s of IB events (tick stream, farm status, ordering ready, rt-bars) to `crates/midas-broker/tests/fixtures/<SYMBOL>.jsonl`.
- **Fixture format**: one `MarketEvent` per line, JSON-serialised, timestamped with arrival time.
- **Sim assertion**: `tests/sim_against_fixture.rs` reads the fixture and compares the sim's generated event sequence (over a matched duration) — asserting shape equivalence (which tick types, in roughly what order, roughly what cadence) rather than byte-for-byte.
- **Maintenance**: fixtures are developer-captured, NOT CI-generated. Re-capture at least annually (or whenever rust-ibapi changes major version). Commit checksum + capture date in the header line.
- **Not CI-gated**: sim-against-fixture tests run in CI; the capture tool only runs locally when a fixture needs refresh.

### Tier 2b fixture sanity-schema (NM-7)

Before `sim_against_fixture` runs its event-shape comparison, it asserts the loaded fixture meets a minimal sanity schema. This catches "fixture got corrupted / accidentally truncated / captured under network failure" without forcing the sim author to debug a cryptic shape mismatch.

Assertions (all must pass; on any failure, the test fails early with message `"fixture corrupt — re-capture via cargo run --bin capture_fidelity_fixture -- <SYMBOL>"`):

1. **Minimum event count** — the fixture contains ≥ 50 `MarketEvent` entries. Shorter captures almost always indicate a failed capture session.
2. **Core tick types present** — at least one `MarketEvent::Tick { tick_type: TickType::Bid, .. }`, one `Ask`, one `Last`. Missing any of the three means the capture ran without market data permission or during an outage.
3. **Farm-up observed** — at least one `MarketEvent::FarmStatus { code: FarmCode::MarketDataFarmOk, connected: true, .. }`. A fixture without farm-up indicates capture started before the TWS connection stabilised.
4. **Monotonic timestamps** — iterating events in file order, `ts_{n+1} >= ts_n` for every adjacent pair. Non-monotonic timestamps mean the capture clock jumped (machine sleep, NTP step) during recording.

Implement in a helper `fn validate_fixture_sanity(events: &[MarketEvent]) -> Result<(), &'static str>` at the top of `tests/sim_against_fixture.rs`; invoke before the shape-comparison loop. Helper is pure; no IO.

## Tier 3 — router behavior

Located in `crates/midas-market-data/tests/`. Use `SimMarketData` with a fast tick cadence (50 ms) for speed.

Covered in the slice 5 and slice 6 plan docs. Highlights:

- One upstream sub per symbol regardless of N consumers.
- Last handle drop → upstream cancel.
- `history_then_live` emits no duplicates and no gaps across the seam.
- Aggregator survives consumer churn (subscribe-drop-subscribe).
- Lagged consumer doesn't stall producer.

## Tier 4 — app integration

Located in `desktop/win/crates/midas-app/tests/`. Headless where possible.

1. **Sim-backed startup** — `MidasApp::new` with `BrokerBackend::Sim`, assert router is initialized and connection_state reaches `Ready` within 1 s.
2. **Watchlist price moves** — add AAPL, MSFT to watchlist; wait 3 s; assert both symbols have last_price that's changed at least twice.
3. **Chart updates** — open chart on AAPL (timeframe M1); wait 2 s; assert chart.data last bar's c has moved.
4. **Chart switch symbol** — open chart on AAPL, change to MSFT, assert AAPL aggregator refcount dropped to 0 within 500 ms (instrumentation point).
5. **Fixture load primes subscriptions** — load fixture with 3 charts; assert 3 aggregators are spawned.
6. **Shutdown tears down subscriptions** — drop MidasApp; wait 100 ms; assert no router tasks are running.

Integration tests run via the existing dev-harness TCP harness where UI interaction is needed (`cargo run --features dev_harness`), or as pure tokio tests where only the backend plumbing is exercised.

## Tier 5 — performance / stress

Not CI-gated; manual. Run under `cargo bench` or `cargo run --release`:

1. **Router hot path** — 1M `Arc<Tick>` through router to 10 consumers × 100 symbols, measure ns/publish. Target median ≤ 500 ns.
2. **Aggregator throughput** — 100k ticks/sec on one symbol, one aggregator, one consumer. Target: no `Lagged` over 60 s.
3. **Subscription churn** — 1000 subscribe/drop cycles per second. Target: no memory growth, no backlog in control mpsc.
4. **App end-to-end with 20 charts** — open 20 charts, sim tick cadence 250 ms. Target: < 15% CPU idle frame, < 30% CPU during drag.

## Coverage gate

By end of S9, aim for:
- `midas-broker-core::market_data` — 85%+ line coverage.
- `midas-market-data` — 80%+ line coverage.
- `midas-broker::sim` — 75%+.
- `midas-broker::ib` — 50%+ (hard to cover without real IB).
- `midas-app` subscription code — 60%+.

Measure with `cargo tarpaulin` or similar. Not a CI gate; an audit target.

## Flake policy (BR-20)

No flaky tests. Any test that uses wall-clock timing must either:
- Use `#[tokio::test(start_paused = true)]` + `tokio::time::advance` for deterministic time, OR
- Use explicit `tokio::sync::Notify` / `oneshot` synchronization, NOT `sleep`, OR
- Mark `#[ignore]` with a comment explaining why it can't be deterministic.

**`sleep` is NOT allowed in the main test suite.** Even timing-invariant tests ("tick cadence is in range") must use paused time — the sim's emitter uses `tokio::time::interval`, which advances correctly under `tokio::time::pause`. Document this sim-internals invariant in `crates/midas-broker/src/sim/market_data.rs`.

Explicitly rewritten under BR-20:
- `tick_cadence_is_in_range` (S3)
- `cancel_terminates_stream_within_500ms` (S3 + Tier 2)
- `farm_status_sequence_on_connect` (S3)
- `deterministic_historical_seam` (S3 — also BR-21)
- `history_then_live_*` (S5 — BR-21)
