# Testing Strategy

## Tier 1 — Unit

Per slice, per crate. Covered in slice plans. Key coverage:

- `midas-calendar`:
  - Classification at every session transition on a known Wednesday (pre-market 04:00–09:30, Regular 09:30–16:00, Post 16:00–20:00, Closed otherwise).
  - Holiday table correctness.
  - DST transitions (2024-03-10 spring forward, 2024-11-03 fall back) — `bar_window` returns correct UTC bounds on both.
  - Out-of-range returns `CalendarError::OutOfRange`.
  - Crypto calendar: all queries return Regular, continuous time axis.
- `midas-broker-core`:
  - `Bar` serde with/without session_kind; legacy records default to None.
- `midas-core`:
  - `CandleBuffer::push_with_session` / `session(idx)` round-trip.
  - `from_bars` constructor round-trip.
- `midas-market-data::aggregator`:
  - Alignment to calendar `bar_window` for D1/W1/H4.
  - Intraday tfs continue to use epoch-modular.
  - `UnsupportedTimeframe` rejection preserved.
- `midas-broker::sim`:
  - Session-aware drift scaling.
  - Closed-hour suppression.
  - Historical bar session tagging.
- `midas-chart::compute`:
  - `build_candle_instances` session tint.
  - `compute_session_bands` band emission.
  - `detect_session_boundaries` BoundaryKind classification.
- `midas-app`:
  - EH toggle Message → ChartInput wiring.
  - ChartViewStore persistence.

## Tier 2 — Cross-calendar cross-check

File: `crates/midas-calendar/tests/nyse_cross_check.rs` (dev-dep `nyse-holiday-cal`).

- Iterate every day 2020-01-01..2031-12-31.
- Assert XnysCalendar::is_trading_day == !nyse_holiday_cal::is_holiday.
- On mismatch, log both and fail with date + reason.

Not run in normal CI if `nyse-holiday-cal` is heavy (it's tiny, so ship it).

## Tier 3 — Integration (end-to-end)

File: `desktop/win/crates/midas-app/tests/session_aware_e2e.rs`. See Slice 11 for the matrix (10 tests).

## Tier 4 — Visual / manual smoke

Not CI-gated:

1. Boot app (Sim backend) → open AAPL M1 chart → observe pre-market tinted candles + faint reddish band.
2. Toggle EH chip off → pre/post candles disappear; timeline compresses.
3. Switch to BTC/USD symbol → no tint, no band, continuous timeline.
4. Open D1 chart on AAPL → watch it extend live once a trading day advances past 16:00 ET.
5. Live-verify the 2024-11-29 early-close fixture: sim clock jumped to that day, D1 bar closes at 13:00 ET.

## Tier 5 — Perf / load

Not CI-gated. Manual:

- 100 watchlist symbols + 20 D1 charts on NYSE calendar → frame budget < 16ms.
- `calendar.bar_window` calls on the aggregator hot path benchmark: ≤ 500ns median.
- Calendar registry lookup: `&'static` refs → O(1), zero alloc.

## Flake policy

Same as router refactor:
- Wall-clock tests use `tokio::test(start_paused=true)` + `tokio::time::advance`.
- `sleep` in non-test code prohibited except behind a paused-timer-friendly helper.
- Tests using real system time: marked `#[ignore]`.

## Coverage targets

- `midas-calendar`: 90%+ line coverage (cross-check test gives much of this for free).
- `midas-chart::compute` session paths: 85%+.
- `midas-market-data::aggregator` calendar paths: 85%+.
- Integration `session_aware_e2e`: 10/10 tests passing.
