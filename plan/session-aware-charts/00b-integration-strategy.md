# Integration Strategy — from current codebase to ideal

> Companion to `00a-ideal-design.md`. This doc describes HOW to migrate. The ideal design is fixed; this is logistics.

## Delta summary

What survives unchanged:
- wgpu / iced layering.
- `midas-render` pipeline primitives (candle, line, quad). These are still the right building blocks; `SceneLayer` dispatches to them.
- `midas-broker-core` + router + sim + IB (the S4795651..HEAD refactor). Calendar sits above.
- ticker state / bracket / annotation system.

What is rewritten or retired:
- `CandleBuffer` → `CandleSeries` (new struct, not a field addition).
- `Bar` (current) → `Candle` (new struct with non-optional session).
- `Timeframe` enum → `BarPeriod` enum.
- `Camera2D::time_to_x` → `TimeAxis` trait; camera refactored to own pan/zoom only.
- `ChartScene` god struct → layer stack.
- `DataProvider::get_candles` / `HistoricalDataRegistry` → `BarStream::snapshot`.
- `detect_session_boundaries` heuristic → `SessionSeparatorLayer` driven by calendar.
- `CandleData` trait → `CandleSeries` direct methods; trait object is gone.
- Existing aggregator's `(symbol, tf) -> SubscriptionHandle<Bar>` → same shape, but `tf` is now `BarPeriod` and aggregator construction requires a calendar.

## Principles for migration

1. **Run both shapes in parallel behind feature gates during migration.** Not at the trait level (that's the "parallel implementations" anti-pattern). At the crate boundary: a new `midas-chart-v2` or `midas-session-chart` crate lands alongside the existing `midas-chart`. Consumers migrate per-feature, not per-call-site.
2. **New types land first, in new files.** No edits to existing types until their replacement is tested.
3. **Every ideal type has an adapter from/to the legacy type for the transition period only.** Adapters are marked `#[deprecated]` and deleted in the final slice.
4. **One end-to-end vertical slice is built in the new architecture end-to-end before horizontal migration.** Crypto, M1 period, continuous axis, one symbol — prove the whole stack works before migrating stocks.
5. **Legacy code path stays hot (renders charts correctly) until new path is fully consuming.** No period where a chart fails to render.

## Phased execution

### Phase A — Foundation (parallel crates, no legacy edits)

Goal: the new type system exists and compiles. Nothing consumes it yet.

0. **`midas-clock`** (new, tiny) — `Clock` trait exposing `now()` (wall) + `now_monotonic()` (Instant); `SystemClock`; `MockClock` (feature-gated) with tokio-time integration. Used by every crate below. **This lands first** because otherwise the entire sim+aggregator testing story is non-deterministic. Also includes a clippy `disallowed_methods` config banning raw `chrono::Utc::now()` / `std::time::Instant::now()` / `std::time::SystemTime::now()` in all crates (allow-listed in `midas-clock` itself). ~150 LOC + tests. Per-crate migration of ~160 `now()` sites happens in later slices.
1. **`midas-calendar`** — implementation of `ExchangeCalendar`, `Session`, `CalendarId`, `TradingDay`, `BarWindow`, `CalendarError`. XNYS + CryptoSpot. XNYS holiday table with corrected Black Friday rule (`thanksgiving + 1d`), Juneteenth year-gate, Reagan/Ford ad-hocs, 04:00 ET pre-market with documentation footnote. `classify()` infallible and saturating.
2. **`midas-bars`** (new) — `Candle`, `CandleSeries`, `BarPeriod`, `ClockInterval`, `SessionSpan`, `CalendarSpan`, `Completeness`, `Ohlcv`. Smart constructors, unit tests, serde.
3. **`midas-stream`** (new) — `BarStream` trait, `HistoryThenLive`, `Filtered`, `BarStreamMeta`. Streams capture `&'static dyn ExchangeCalendar` at construction; never look up per tick.
4. **`midas-axis`** (new) — `TimeAxis` trait, `ContinuousAxis`, `CompressedAxis`, `SessionIndexAxis`, `TimeTick`, `PriceRange`, `Viewport`.
5. **`midas-scene`** (new) — `ChartScene`, `ChartSceneBuilder`, `SceneLayer` trait, `LayerConfig`. Concrete layers: `CandleLayer`, `VolumeLayer`, `GridLayer`, `SessionBandLayer`, `SessionSeparatorLayer`, `HolidayMarkerLayer`, `CrosshairLayer`.

Deliverable: six crates compile and pass unit tests in isolation. Legacy chart continues to work unchanged because nothing consumes the new crates yet.

### Phase B — Vertical slice proof (crypto / M1 / continuous)

Goal: one chart, end to end, on the new stack. Prove the architecture is correct before horizontal migration.

1. **New provider adapter**: wrap the existing sim/IB `SubscriptionHandle<Bar>` into a `BarStream` that emits `Candle` (using `CryptoSpotCalendar` classification). Legacy `Bar` → `Candle` conversion at the seam.
2. **New aggregator**: `SessionedAggregator` that takes `(Arc<dyn ExchangeCalendar>, BarPeriod)` and produces `BarStream<Item = Candle>`.
3. **New chart widget**: `SessionChart` in a new feature-gated module inside `midas-app`. Only renders crypto at M1 with ContinuousAxis. All layers enabled.
4. **Feature flag**: `session_chart` Cargo feature on `midas-app`. When on, adds a "New chart" menu entry that opens a `SessionChart` instead of the legacy chart. Legacy charts still work.
5. **Integration test**: open a crypto M1 SessionChart; bars stream; render passes; assertion on ChartScene layers.

Outcome: one working end-to-end vertical slice. Any missing API on the ideal-design crates shakes out here.

### Phase C — Horizontal expansion

Goal: bring other asset classes / periods / axes online.

1. **XNYS + BarPeriod::Clock + CompressedAxis**: stocks at intraday with session-compressed axis. Adds `SessionBandLayer` + `SessionSeparatorLayer` rendering.
2. **XNYS + BarPeriod::Session(Regular)**: daily charts. Calendar-aware bar windows.
3. **XNYS + BarPeriod::Calendar(Week)**: weekly charts.
4. **Holiday handling**: NYSE holiday table + early-close rules land in the XNYS calendar. HolidayMarkerLayer renders on D1+.
5. **EhPolicy toggle**: UI control wired; chart re-renders with filtered aggregator + axis + layers.

Outcome: `SessionChart` is feature-complete for stocks + crypto. Still gated behind `session_chart` feature.

### Phase D — Legacy retirement

Goal: delete the old chart path.

1. **Migrate every legacy chart consumer** to `SessionChart`. Update app's chart-spawn path to use the new widget unconditionally.
2. **Delete** `ChartPanel`'s legacy fields that are replaced (`data: Arc<CandleBuffer>` → `series: CandleSeries`; `timeframe: Timeframe` → `period: BarPeriod`; etc.).
3. **Delete** `midas-chart`'s god-struct `ChartScene` / retire in favour of the `midas-scene` layer stack.
4. **Delete** `CandleBuffer` once no consumers remain.
5. **Delete** `Timeframe` once no consumers remain.
6. **Delete** `Camera2D::time_to_x`; Camera is demoted to pan/zoom state only. `TimeAxis` is authoritative.
7. **Delete** `DataProvider::get_candles` path; `HistoricalDataRegistry` removed.
8. **Delete** `detect_session_boundaries` heuristic; replaced by calendar-driven layer.
9. **Remove** the `session_chart` feature gate.
10. **Update** `CLAUDE.md` / `README.md` / plan archive.

Outcome: clean architecture, no legacy shims, ideal design in production.

### Phase E — Polish

Code-scrutiny + app-harden + arch-audit passes on the new crates. Refactor-pro + rust-uplift. Final CI-green commit gate.

## Migration of the immediate bug (watchlist-chart price mismatch)

The current plan's Slice 0 (historical unification) fits into Phase B:
- The new `BarStream` provider adapter wraps `router.source().historical_bars` + live RT bars. Same source for historical + live.
- Watchlist continues to use `router.last_quote` (unchanged).
- Both are driven by the same sim price curve — prices agree.

The Slice 0 fix lands as part of Phase B's vertical-slice proof, not as a standalone slice.

## Slice-level plan (concrete commits)

Tentative; to be refined by plan-eval:

| # | Name | Phase | Depends |
|---|------|-------|---------|
| S0 | `midas-clock` crate (Clock trait w/ now + now_monotonic, SystemClock, MockClock+tokio-integration, disallowed-methods lint) | A | — |
| S1 | `midas-calendar` crate (infallible classify, correct holiday rules) | A | S0 |
| S2 | `midas-bars` crate (`Candle`, `CandleSeries`, `BarPeriod`) | A | S1 |
| S3 | `midas-stream` crate (`BarStream`, combinators; calendar pinned at construction) | A | S0, S2 |
| S4 | `midas-axis` crate (`TimeAxis`, three impls) | A | S1, S2 |
| S5 | `midas-scene` crate (`ChartScene`, layers) | A | S1..S4 |
| S6 | BarStream adapter from legacy router output → Candle; sim gets `Arc<dyn Clock>` | B | S0..S5 |
| S7 | SessionedAggregator | B | S1..S5 |
| S8 | SessionChart widget (feature-gated) | B | S1..S7 |
| S9 | Crypto M1 end-to-end integration test | B | S8 |
| S10 | XNYS intraday support (CompressedAxis, SessionBandLayer, SessionSeparatorLayer) | C | S9 |
| S11 | XNYS BarPeriod::Session(Regular) (D1 RTH) | C | S10 |
| S12 | XNYS BarPeriod::Calendar(Week/Month) | C | S11 |
| S13 | Holiday / early-close table + HolidayMarkerLayer | C | S11 |
| S14 | EhPolicy UI toggle + filtered aggregator | C | S13 |
| S15 | Migrate app's chart-spawn path to SessionChart | D | S14 |
| S16 | Retire CandleBuffer + Timeframe + legacy chart-scene | D | S15 |
| S17 | Retire DataProvider / HistoricalDataRegistry | D | S16 |
| S18 | Retire Camera2D::time_to_x; demote Camera | D | S16 |
| S19 | Delete detect_session_boundaries heuristic | D | S18 |
| S20 | Remove session_chart feature gate | D | S15..S19 |
| S21 | Docs + plan archive | D | S20 |
| S22 | Polish (refactor-pro / rust-uplift / scrutiny) | E | S21 |

## Risk profile

- **Phase A is low risk**: new crates, no existing-code edits. Can land in parallel, merged anytime.
- **Phase B is medium risk**: adapter layer has to correctly bridge legacy → ideal. Rigorous integration tests at the seam.
- **Phase C is low risk**: each slice is additive (more calendars, more periods, more layers).
- **Phase D is high risk**: deletions ripple. Expect 1-2 slices' worth of whack-a-mole cleanup (tests, fixtures, dev-harness state projections). Follow the big-bang + sub-commit pattern from the router refactor's S7.
- **Phase E is low risk**: polish only.

## LOC estimate

- New code across Phase A+B: 4,000–6,000 LOC.
- Deletions in Phase D: 1,500–2,500 LOC.
- Net +2,500–3,500 LOC after migration.

## What this integration strategy does NOT compromise on

- The ideal type system. `Session` is non-optional on `Candle`. `BarPeriod` is calendar-scoped. `TimeAxis` is pluggable. Layers are composable. None of these are watered down for easier migration.
- The one-end-to-end-vertical-slice-before-horizontal-migration rule. If Phase B uncovers a gap in the ideal design, the ideal design is updated — not bypassed.
- The delete-legacy-once-new-works discipline. No parallel maintenance of two chart paths beyond the minimum necessary.

## What may surface surprise work

- `CandleBuffer`'s mmap binary format: if currently persisted, Phase D must decide (re-generate / migrate / drop support). Flagging as OPEN.
- `DumpState` projection in dev-harness: every field of `ChartPanel` that changes is a projection edit. Flagging as OPEN.
- Annotation system (bracket orders, price lines): these hook into `ChartScene` today. They become an `AnnotationLayer` in the new architecture; migration is a straightforward rewrap but must be done.
- Thumbnail renderer: uses a separate `ThumbnailDataStore` with its own Vec<f32> closes. Likely stays independent, but review.
- `ChartViewStore` persistence: saved cameras and saved view-states need migration or discard. Flagging as OPEN.

These are known risks, not deal-breakers. Each is addressable in the phase it surfaces.
