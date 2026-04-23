# Slice 0 — Historical Unification (prerequisite)

**Goal.** Migrate chart historical loads from the legacy `midas_feed::TestProvider` path onto the router's `MarketDataSource::historical_bars`. Resolves the watchlist/chart price-mismatch bug and eliminates the legacy code path before session-aware work touches it.

## Problem

Today:
- Watchlist + router live: `crates/midas-broker/src/testdata/` via `SimMarketData`.
- Chart historical: `desktop/win/crates/midas-feed/src/testdata/` via `midas_feed::TestProvider` → `HistoricalDataRegistry::active_data_provider()`.

Two different synthetic price generators. The numbers drift apart; chart and watchlist show different values for the same symbol.

## Fix

1. `MidasApp` gains an `order_client: Arc<dyn OrderClient>` and `router: Arc<MarketDataRouter>` — already present post-refactor.
2. The chart load path (`app.rs:3224, 3268, 3317, 3367`, `handlers.rs:209-227`) currently calls `self.providers.active_data_provider().get_candles(sym, tf, days)`. Replace with `router.source().historical_bars(&sym_key, con_id, end, duration, tf, WhatToShow::Trades, true)`.
3. `IbDuration::from_lookback(Duration::from_days(days))` produces the `IbDuration` input.
4. `HistoricalBarsResult::bars` → `CandleBuffer` via a new `CandleBuffer::from_bars(&[Bar]) -> Self` helper in `midas-core`.
5. Market snapshot load (`app.rs:3277`) also moves to `router.source().historical_bars(..., Timeframe::D1, 30d)`.
6. `HistoricalDataRegistry` (`registry.rs`) becomes a thin legacy shim with one surviving impl (`TestProvider` — no longer used). Retire after verification.
7. `midas-feed/src/testdata/` — delete the 750+-LOC generator. Keep only the CSV importer if it's still referenced; grep confirms.
8. Update `views.rs:365, 374` which currently read `active_data_provider_name` — surface `router.source().name()` instead or delete the provider-name display entirely.

## What this does NOT fix

- Calendar-aware bar windows: D1 is still rejected by the aggregator. That's slice 3's job. After slice 0, the chart's historical D1 bars come from the sim but live updates for D1 charts still return `UnsupportedTimeframe`. User sees consistent historical, no live extension yet.

## Files touched

- `desktop/win/crates/midas-app/src/app.rs` — 4 call sites, `providers` field eventually deleted in S12.
- `desktop/win/crates/midas-app/src/app/handlers.rs` — 1 call site.
- `desktop/win/crates/midas-app/src/app/persistence.rs` — remove active_data_provider persistence.
- `desktop/win/crates/midas-app/src/app/views.rs` — status-bar display.
- `desktop/win/crates/midas-app/src/registry.rs` — HistoricalDataRegistry → delete or stub.
- `desktop/win/crates/midas-core/src/candle_buffer/mod.rs` — `from_bars` constructor.
- `desktop/win/crates/midas-feed/src/testdata/` — delete.
- `desktop/win/crates/midas-feed/Cargo.toml` — trim deps no longer used.

## Tests

- **Unit**: `CandleBuffer::from_bars` round-trip.
- **Integration**: boot app with Sim backend, add AAPL to watchlist + open a chart on AAPL at D1. Assert:
  - `market_cache[AAPL].last_price` ≈ `chart.data.closes.last()` (within 1%).
  - `SimMarketData::live_subscription_count_for("AAPL")` ≥ 1 (watchlist) within 2s.

## Acceptance

- `cargo test --workspace` green on both workspaces.
- `cargo clippy --workspace --all-targets -- -D warnings` clean on both.
- `cargo fmt --all`.
- Launching `cargo run -p midas-app` with Sim backend, adding AAPL to watchlist + chart: watchlist last price and chart's final historical close agree.

## Commit

Single commit: `fix(chart): route historical loads through router to unify with live`.

## Rollback signal

If chart historical loads break for non-sim paths (real IB), the router's `source().historical_bars` for IB needs pacing + error handling verification. Retry with per-call timeouts (already landed) + the pacing governor (already landed).
