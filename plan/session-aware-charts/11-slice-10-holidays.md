# Slice 10 — Holidays + early-close handling

**Goal.** Complete NYSE holiday + early-close data; verify full calendar correctness against `nyse-holiday-cal` cross-check fixture.

## Scope

Most of the data is in Slice 1 (`xnys/holidays.rs`). This slice:

1. **Ad-hoc close dates** (historical): 9/11/2001–9/14/2001, Hurricane Sandy 10/29–10/30/2012, GHW Bush funeral 12/5/2018, Jimmy Carter day-of-mourning 2025-01-09.
2. **Early-close rules**:
   - Day after Thanksgiving (4th Friday of November — 13:00 ET).
   - July 3 when weekday (13:00 ET).
   - Christmas Eve when weekday (13:00 ET).
   - Day before Good Friday when weekday + markets open (variable — look up each year).
3. **Future-dated placeholders**: none — the calendar covers 2000–2031; beyond that returns `OutOfRange`.
4. **Chart render at early close**: regular session shortens; post-market shortens to 17:00 ET on those days. Verified in integration.

### Test fixture: full cross-check

`xnys/tests.rs` iterates every date in 2020-01-01..2031-12-31 (the overlap of our coverage and `nyse-holiday-cal`'s). For each date:
- Assert `XnysCalendar::is_trading_day(d) == !nyse_holiday_cal::is_holiday(d)`.
- On early-close days, assert the regular session closes at 13:00 ET (via `trading_day(d).sessions[1].close`).

## Files touched

- `crates/midas-calendar/src/xnys/holidays.rs` — complete the table.
- `crates/midas-calendar/src/xnys/tests.rs` — cross-check harness.

## Tests

- `nyse_holiday_cross_check_2020_to_2031`: as described above.
- `early_close_thanksgiving_2024`: `trading_day(2024-11-29).is_early_close == true`, regular session closes 13:00 ET (18:00 UTC EST).
- `early_close_christmas_eve_2024`: 2024-12-24 is_early_close == true.
- `ad_hoc_sandy_2012`: 2012-10-29 is_trading_day == false.
- `no_future_data_beyond_2031`: `trading_day(2032-01-01)` returns OutOfRange.

## Acceptance

- Cross-check test passes on every day in the 12-year range.
- All early-close ad-hoc dates covered.

## Commit

Single commit: `feat(calendar): complete NYSE holiday + early-close data`.
