# Sim historical bars return empty for "now" anchor

**Status:** Open. Three tests have been failing on `main` for at least
the duration of the ETH-shading work (S1a–S6 on `feat/eth-shading`,
2026-04-25). Verified by cloning `main` to a clean worktree and
running the suite there — these failures are **not** caused by any
of the ETH-shading slices.

**Severity:** Low priority for shipping (failures are in the test
suite, not the running app), but they're noisy in `cargo test`
output and they block any future "all green" CI gate. Fix soon-ish.

## Failing tests

| Crate | Test file | Test name |
|---|---|---|
| `midas-broker` | `tests/sim_fidelity.rs` | `historical_bars_returns_immediately` |
| `midas-broker` | `tests/sim_fidelity.rs` | `chart_load_historical_and_burst_agree_on_price` |
| `midas-market-data` | `tests/router_behavior.rs` | `history_then_live_no_gap_no_dup` |

All three trip on the same panic shape:

```
assertion failed: !result.bars.is_empty()
                  // or
panicked at ...: 'historical fetch returned no bars'
                  // or
panicked at ...: 'seam produced no bars'
```

## Root cause hypothesis

Each test calls `SimMarketData::historical_bars(...)` with
`end = chrono::Utc::now()` and a short `IbDuration` (1 day or
similar). `synthesize_historical` then asks `TestDataProvider::bars`
for `[end_secs - lookback, end_secs)`.

`TestDataProvider`'s data range is fixed at construction:
- Start: `DATA_START = 1451865600` (2016-01-04 00:00 UTC).
- Length: `TRADING_DAYS = 2700` (~10.4 years of weekdays, ending
  around mid-2026).

When `Utc::now()` falls *outside* this fixed window — or lands on
a weekend / between bars at the chosen lookback — the filtered slice
comes back empty.

Today's date (2026-04-25) is *probably* inside the data range but
near its tail; small lookbacks (e.g. `Days(1)` = 86_400s) can land
entirely between two daily bars or past the last bar's timestamp.

## What ETH-shading already proved works

S4-sim added a `pinned_eth_test_window()` helper in
`crates/midas-broker/tests/sim_fidelity.rs` that anchors the
historical request to a date inside `TestDataProvider::date_range()`
instead of `Utc::now()`:

```rust
fn pinned_eth_test_window() -> (chrono::DateTime<chrono::Utc>, IbDuration) {
    use midas_broker::testdata::TestDataProvider;
    let mut p = TestDataProvider::new();
    let (_, end) = p.date_range("AAPL");
    let day_floor = ((end - 2 * 86400) / 86400) * 86400 + 86400;
    let pinned_end = chrono::DateTime::<chrono::Utc>::from_timestamp(day_floor, 0).unwrap();
    (pinned_end, IbDuration::Days(2))
}
```

The three new ETH tests (`eth_synthesis_emits_pre_and_post_bars`,
`rth_only_strips_eth_bars`, `synthetic_includes_eth_off_overrides_use_rth_false`)
all use this and pass cleanly. The pattern transfers directly to
the three failing tests.

## Suggested fix

**Option A (cheapest):** Promote `pinned_eth_test_window()` (or a
similar helper) to a shared test-utility in
`crates/midas-broker/src/testdata/mod.rs` behind `#[cfg(test)]`,
then have each failing test consume it instead of
`chrono::Utc::now()` + literal `IbDuration::Days(1)`.

**Option B (more involved):** Make `SimMarketData` synthesise bars
*forward* from the real wall-clock `now()` when the request lands
past `TestDataProvider`'s fixed range, so production callers (which
also use `Utc::now()`) see the same "graceful degradation" the
tests need. This is a bigger change with cross-cutting implications
for the seam logic — defer unless we hit it in production too.

**Recommend Option A** for the test failures. Track Option B
separately if/when it actually bites the running app — today the
chart loads 6 months of D1 (`load_chart_with` →
`days_for_timeframe(D1) = 180`) which has no boundary issue.

## Estimated effort

- Option A: ~30 LOC. One helper extraction, three call-site swaps,
  one CI run to confirm green.

## Pointers

- Test files:
  - `crates/midas-broker/tests/sim_fidelity.rs:453` — `historical_bars_returns_immediately`
  - `crates/midas-broker/tests/sim_fidelity.rs:148` — `chart_load_historical_and_burst_agree_on_price`
  - `crates/midas-market-data/tests/router_behavior.rs:401` — `history_then_live_no_gap_no_dup`
- Generator: `crates/midas-broker/src/testdata/generate.rs:13` — `DATA_START`, `TRADING_DAYS`
- Synthesizer: `crates/midas-broker/src/sim/market_data.rs:763` — `synthesize_historical`
- Reference fix shape: `crates/midas-broker/tests/sim_fidelity.rs` →
  `pinned_eth_test_window()` (lands with the eth-shading S4-sim slice)
