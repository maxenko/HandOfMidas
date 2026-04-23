# Slice 4 — Sim session-aware behavior

**Goal.** `SimMarketData` respects the symbol's calendar: ticks classified with the correct `SessionKind`, pre/post volatility scaled down, closed-hour ticks suppressed.

## Scope

### Tick classification

`sim/tick_emitter.rs::tick_once`: when emitting each tick, call the per-symbol calendar to classify `Utc::now()`. Two behaviors change:

1. **Drift scaling.** During `SessionKind::PreMarket` / `PostMarket`, scale `drift_bps` by `0.3×` (less volatile). During `Closed`, suppress emission entirely. During `Regular`, drift unchanged.
2. **Calendar lookup.** The sim holds an `Arc<CalendarRegistry>` and resolves per symbol.

### Historical bar session tagging

`synthesize_historical` in `sim/market_data.rs`: when generating bars, assign each bar's `session_kind` by calling `calendar.classify(bar.ts_open)`. For D1 bars on XNYS, every bar is `Regular` (aligned to 09:30–16:00 ET). For crypto, every bar is `Regular` trivially.

### RT bar session tagging

`subscribe_realtime_bars` task: stamp each emitted `Bar` with `session_kind = calendar.classify(Utc::now())`.

### Closed-hour suppression

On XNYS calendar, the sim's emitter task checks `classify(Utc::now())`:
- `Closed` → skip emission that cycle.
- Otherwise → proceed.

For CryptoSpot, classify always returns `Regular` → emission proceeds.

### Config

Add `sim_session_aware: bool` to `SimMarketDataConfig`. Default `true`. Tests can disable for legacy-style continuous emission.

## Files touched

- `crates/midas-broker/src/sim/tick_emitter.rs` — classify + scale drift + suppress Closed.
- `crates/midas-broker/src/sim/market_data.rs` — stamp session on RT bars + historical.
- `crates/midas-broker/src/sim/config.rs` — new `sim_session_aware` knob.
- `crates/midas-broker/Cargo.toml` — `midas-calendar` dep.

## Tests

- `sim_ticks_classified_correctly`: subscribe AAPL during XNYS RTH-emulated wall-clock, observe `Tick` events whose timestamps fall in the RTH window; assert `calendar.classify(tick.ts) == Regular` via a side-channel or by checking `Bar.session_kind` on derived aggregator output.
- `sim_historical_d1_all_regular`: synthesize 30 days of D1 XNYS historical; every bar has `session_kind = Regular`.
- `sim_closed_hour_suppressed`: configure sim at a wall-clock inside `Closed` (e.g., Saturday 02:00 UTC); assert no ticks emit for ≥10 seconds.
- `sim_crypto_always_regular`: crypto symbol always classifies Regular regardless of wall-clock.

Use `tokio::test(start_paused = true)` + `tokio::time::advance` to drive wall-clock across session boundaries deterministically.

## Acceptance

- All tests pass.
- `cargo clippy`, `cargo fmt` clean.
- `sim_fidelity.rs` existing tests still pass (default config for them is `sim_session_aware = false` or uses crypto symbol to avoid RTH gating).

## Commit

Single commit: `feat(sim): session-aware tick emission + bar session tagging`.
