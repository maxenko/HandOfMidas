# Slice 11 — Integration tests

**Goal.** End-to-end verification of session-aware behavior across the full stack: sim → router → aggregator → chart data → rendering.

## Scope

Tests live in `desktop/win/crates/midas-app/tests/session_aware_e2e.rs`.

### Test matrix

1. **Watchlist + chart agree on price (Slice 0 verification)**
   - Boot sim-backed app.
   - Add AAPL to watchlist.
   - Open chart on AAPL at D1.
   - Assert: `market_cache[AAPL].last_price` ≈ `chart.data.closes.last()` (within 0.5%).

2. **Pre-market candle renders with tint**
   - Boot app, set sim wall-clock to Tuesday 06:00 ET (pre-market).
   - Open AAPL M1 chart.
   - Advance 10 minutes.
   - Assert: latest M1 bar has `session_kind == PreMarket`.
   - Assert: rendered `CandleInstance.color` RGB equals base × pre_market_tint_mult.

3. **Session boundary separator at RTH close**
   - Boot app, set sim wall-clock to 15:55 ET (5 min before RTH close).
   - Advance 10 minutes (crosses 16:00 ET).
   - Query `compute_chart_scene` session_boundaries.
   - Assert: one boundary with `kind = RthClose`.

4. **EH toggle hides pre/post candles + bands**
   - Same setup as test 2 (pre-market candles present).
   - Send `Message::ToggleEh(chart_id)`.
   - Re-compute scene.
   - Assert: `show_extended_hours = false`; no session bands in `grid_instances`; pre/post candles rendered with full RTH color (no tint applied).

5. **Crypto chart has no session chrome**
   - Boot app with BTC/USD (CryptoSpotCalendar).
   - Open BTC/USD M5 chart.
   - Assert: `show_extended_hours` is N/A / chip hidden; no bands emitted; time axis is continuous (no compression).

6. **Early-close day renders correctly**
   - Set sim wall-clock to 2024-11-29 (Black Friday).
   - Open AAPL D1 chart.
   - Assert: that day's D1 bar `session_kind = Regular`; window close is 13:00 ET; RTH-close separator appears at 13:00 ET.

7. **Holiday skipped**
   - Sim clock 2024-07-04 (Independence Day).
   - Open AAPL M1 chart.
   - Advance 2 hours.
   - Assert: no ticks emitted (calendar classifies Closed, sim suppresses).

8. **D1 on XNYS aggregates from RTH only**
   - Sim emits ticks across pre-market + RTH + post-market on a Tuesday.
   - D1 aggregator produces one completed bar: open at 09:30 ET, close at 16:00 ET, OHLC from RTH ticks only.

9. **Crypto D1 aggregates UTC-midnight-aligned**
   - Sim emits ticks across midnight UTC for BTC/USD.
   - D1 aggregator closes the bar at 00:00 UTC.

10. **History + live seam respects session**
    - Open AAPL D1 chart.
    - Historical loads 30 days of D1 bars (each 09:30–16:00 ET).
    - Live extension starts; new bar arrives tagged `Regular`, opening at the next session's 09:30 ET.
    - Assert: no duplicate bar at seam; timeline continuous across the fold.

## Dependencies

All other slices landed. This is the last technical slice before docs.

## Acceptance

- All 10 integration tests pass deterministically (using `tokio::test(start_paused=true)` where wall-clock drives behavior).
- `cargo test -p midas-app --test session_aware_e2e` passes.

## Commit

Single commit: `test(chart): end-to-end session-aware integration tests`.
