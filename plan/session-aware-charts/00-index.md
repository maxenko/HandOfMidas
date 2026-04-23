# Session-Aware Charts — Plan Index

> **STATUS — Phase A, B, C + scrutiny + refactor loops landed.** See [`README.md`](README.md) for current status, landed crates, test counts, and deferred work. Phase D (legacy retirement) is intentionally not executed yet — it requires a preceding feature-port sprint to bring brackets / annotations / indicators onto the new stack.
>
> **NOTE — PLAN SUPERSEDED.** Per user directive after initial drafting: the session-aware chart system is designed ideal-first (see [`00a-ideal-design.md`](00a-ideal-design.md)) with no compromises to fit the existing codebase. Integration is a separate concern in [`00b-integration-strategy.md`](00b-integration-strategy.md). The slice plan below (S0–S12) reflects an earlier bolt-on approach and is retained here only as a contrast reference — the authoritative plan is S1–S22 in the integration-strategy doc. The older S0–S12 slice docs are NOT the implementation plan.
>
> If you are reading this to execute work, read `README.md` for current status, then `00a-ideal-design.md`, then `00b-integration-strategy.md`. The rest of this index is historical.

## Why

The chart system today treats every timestamp identically — no concept of pre-market vs regular vs after-hours, no trading calendar, no exchange-local time awareness. Post router refactor, two concrete failures surface:

1. **Watchlist and chart prices don't match.** Chart historical still loads from `midas_feed::TestProvider` while the router sources watchlist + live from `crates/midas-broker/src/testdata`. Two disjoint price generators; the numbers drift apart.
2. **Charts don't extend live.** Default timeframe is `D1`, which the aggregator rejects (`UnsupportedTimeframe`). Even M1 would append live bars at `Utc::now()` — off the right edge of historical data because the two sources aren't on the same timeline.

Both symptoms collapse into one root cause (disjoint data sources) plus one missing capability (calendar-aware bar aggregation for D1/W1/H4/MN1). Fix the source + add a trading calendar and the chart starts behaving.

Beyond the immediate fix, the broader goal is session-aware rendering: visually distinguish pre-market / regular / after-hours bands, support 24h instruments (crypto) as a first-class case, and leave the door open for futures (CME Globex ETH/RTH) and FX (regional session overlays) later.

## Target architecture

```
┌──────────────────────────────────────────────────────────────────┐
│   Trading Calendar layer (crates/midas-calendar)                │
│                                                                  │
│   trait ExchangeCalendar                                        │
│      .session_for(ts) -> Session { kind, label, window }        │
│      .bar_window(ts, tf) -> (open, close)                       │
│      .classify(ts) -> SessionKind                               │
│      .sessions_between(from, to) -> Iterator                    │
│      .time_axis() -> { Continuous | CompressedClosed }          │
│                                                                  │
│   Implementations:                                              │
│     XnysCalendar (US equities: pre/RTH/post + NYSE holidays)    │
│     CryptoSpotCalendar (24/7, UTC-aligned, no sessions)         │
│     (deferred: XCME, FxOtc)                                     │
└──────────────────────────────────────────────────────────────────┘
                      ↓ consumed by
┌──────────────────────────────────────────────────────────────────┐
│   Aggregator (midas-market-data)                                │
│   BarAggregator now uses calendar.bar_window(ts, tf) to align   │
│   D1/H4/W1/MN1 to session boundaries instead of UTC-epoch mod.  │
│   Supported set grows to: S5,S15,S30,M1,M5,M15,M30,H1,H4,D1,W1  │
└──────────────────────────────────────────────────────────────────┘
                      ↓
┌──────────────────────────────────────────────────────────────────┐
│   Candle layer (midas-core + midas-broker-core)                 │
│   Bar.session_kind: Option<SessionKind>                         │
│   CandleData::session(idx) -> SessionKind (default Regular)     │
│   Both populated by the producer using the symbol's calendar.   │
└──────────────────────────────────────────────────────────────────┘
                      ↓
┌──────────────────────────────────────────────────────────────────┐
│   Rendering (midas-chart + midas-render)                        │
│   build_candle_instances applies per-candle tint based on       │
│   session_kind (reuses existing .color attribute, no shader     │
│   change).                                                       │
│   build_grid_instances emits background band overlays for       │
│   pre/post as GridLineInstance rectangles.                      │
│   Existing session-boundary detection (collapsed-mode gap       │
│   rendering) is reused for the RTH-close vertical separator.    │
└──────────────────────────────────────────────────────────────────┘
                      ↓
┌──────────────────────────────────────────────────────────────────┐
│   UI (midas-app)                                                │
│   Per-chart EH toggle (bottom-right chip + settings checkbox).  │
│   ChartViewStore gains `show_extended_hours: bool`.             │
│   Default: ON for intraday, N/A for D1+ (feed-driven).          │
└──────────────────────────────────────────────────────────────────┘
```

No `ibapi` types leak. The `midas-calendar` crate is pure domain (`chrono` + `chrono-tz` only). Rendering stays sans-IO. All existing architecture rules from `CLAUDE.md` remain satisfied.

## Invariants this plan enforces

1. **One contiguous intraday bar stream per symbol** — never per-session sub-streams. A 5-minute bar at 09:25 ET carries `SessionKind::PreMarket`; at 09:30 ET the next bar carries `SessionKind::Regular`. Sessions are rendering metadata, not identity.
2. **Daily bars respect the exchange's declared session.** `use_rth: bool` is honoured on historical fetches; D1 bars for stocks are 09:30–16:00 ET by default.
3. **Session windows are labeled time-ranges, not a fixed enum.** The `TradingCalendar` structure matches TradingView / NT8 / MultiCharts; forex regional overlays (Tokyo/London/NY) and futures ETH/RTH fit the same shape without schema migration.
4. **UTC on the wire, local time at the edges.** Every stored `ts` is `DateTime<Utc>`; exchange tz is applied only in the calendar for window alignment and in the UI for display.
5. **No vendor types past `midas-broker`.** `chrono` + `chrono-tz` are the only external time deps.
6. **Determinism on backtest data.** Calendars carry a `covers: Range<NaiveDate>`; out-of-range queries return `CalendarError::OutOfRange`, never guess.
7. **Half-day early closes honour the exchange calendar verbatim.** The day-after-Thanksgiving 13:00 ET close is one shortened session, not a merged bar.

## Slice dependency graph

```
S0  (prereq: historical unification)
   └─▶ S1 + S2 (parallel foundation: calendar crate + type hooks)
         ├─▶ S3  (aggregator D1/H4/W1/MN1)
         │     └─▶ S11 (integration tests across calendars)
         ├─▶ S4  (sim session-aware price behavior)
         │
         ├─▶ S6  (session-aware candle coloring)
         │     └─▶ S9 (EH toggle UI)
         ├─▶ S7  (session background bands)
         │     └─▶ S9
         ├─▶ S8  (RTH-close separator)
         │     └─▶ S9
         │
         └─▶ S10 (holiday early-close)
               └─▶ S11
                    └─▶ S12 (docs)
```

Parallelism: after S0, {S1, S2} in parallel. After S1+S2 land, {S3, S4, S6, S7, S8, S10} can all run in parallel. S9 + S11 + S12 sequence the finish.

## Slices

| # | File | Status | Depends on | Parallelizable with |
|---|------|--------|------------|---------------------|
| S0 | [02-slice-0-historical-unification.md](02-slice-0-historical-unification.md) — chart historical loads via router | pending | — | — |
| S1 | [03-slice-1-midas-calendar.md](03-slice-1-midas-calendar.md) — `midas-calendar` crate + XNYS + CryptoSpot | pending | S0 | S2 |
| S2 | [04-slice-2-candle-session-hooks.md](04-slice-2-candle-session-hooks.md) — `Bar.session_kind`, `CandleData::session` | pending | S0 | S1 |
| S3 | [05-slice-3-aggregator-calendar.md](05-slice-3-aggregator-calendar.md) — calendar-aware bar aggregation | pending | S1, S2 | S4, S6, S7, S8, S10 |
| S4 | [06-slice-4-sim-session-aware.md](06-slice-4-sim-session-aware.md) — sim respects sessions | pending | S1, S2 | S3, S6, S7, S8, S10 |
| S6 | [07-slice-6-candle-tint.md](07-slice-6-candle-tint.md) — session-aware candle coloring | pending | S2 | S3, S4, S7, S8, S10 |
| S7 | [08-slice-7-session-bands.md](08-slice-7-session-bands.md) — background band overlays | pending | S1, S2 | S3, S4, S6, S8, S10 |
| S8 | [09-slice-8-session-separator.md](09-slice-8-session-separator.md) — RTH-close vertical separator | pending | S1, S2 | S3, S4, S6, S7, S10 |
| S9 | [10-slice-9-eh-toggle-ui.md](10-slice-9-eh-toggle-ui.md) — per-chart EH toggle | pending | S6, S7, S8 | — |
| S10 | [11-slice-10-holidays.md](11-slice-10-holidays.md) — holiday + early-close data & tests | pending | S1 | S3, S4, S6, S7, S8 |
| S11 | [12-slice-11-integration-tests.md](12-slice-11-integration-tests.md) — cross-slice end-to-end | pending | S3, S9, S10 | — |
| S12 | [13-slice-12-docs.md](13-slice-12-docs.md) — CLAUDE.md + plan archive | pending | S11 | — |

S5 is intentionally skipped — was reserved for "sim connection-state integration with calendar" but folded into S4. Numbering gap preserved for revision traceability.

## Cross-cutting docs

- [01-architecture.md](01-architecture.md) — detailed design, type definitions, visual convention decisions
- [90-product-decisions.md](90-product-decisions.md) — the baked-in product choices (display tints, toggle location, daily bar aggregation rule)
- [91-testing.md](91-testing.md) — test matrix, including NYSE calendar cross-check against `nyse-holiday-cal`
- [92-deferred.md](92-deferred.md) — explicitly out of scope (futures ETH/RTH, forex overlays, index-based time axis rewrite)

## Non-goals

- **Index-based time axis** (trading-vue-js style, used for extreme zooms with no gaps). Our collapsed-mode rendering already handles gap compression via `detect_session_boundaries`. A full index-based rewrite is deferred.
- **Futures ETH/RTH templates**. Calendar surface is designed to admit `XcmeCalendar` later, but the implementation is not in this plan.
- **Forex regional session overlays** (Tokyo/London/NY). Same — future slice.
- **Rollover markers on continuous futures contracts**. Out of scope.
- **User-configurable session strings** (`0930-1600:23456` DSL like TradingView). Default to hard-coded calendars for now; DSL is a future extension.
- **Session-indicators** (Opening Range, Session VWAP). These are indicators, not chart chrome.

## Blast-radius estimate

- **New code**: ~2500-3500 LOC (crate + extensions across 5 existing crates).
- **Modified**: ~600-800 LOC (compute/mod.rs, aggregator, app chart state, sim).
- **Deleted**: ~300-500 LOC (HistoricalDataRegistry, midas-feed::TestProvider if fully retired).
- **Test count**: +40-60 tests (calendar, session classification, aggregator alignment, rendering smoke tests).

## Revision log

- 2026-04-21 — round 1 plan drafted from research output.
