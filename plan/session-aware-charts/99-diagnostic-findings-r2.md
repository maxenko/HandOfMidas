# Diagnostic Findings — Round 2 (ideal design review)

Round-2 plan-eval reviewed `00a-ideal-design.md` + `00b-integration-strategy.md`. All blocker-and major-grade findings are folded back into the ideal design. This doc preserves the findings + resolutions for traceability.

## Blockers (resolved inline in 00a)

- **NB-1 Calendar ownership consistency.** Mixed `&'static dyn` vs `Arc<dyn>`. → Canonical `&'static dyn ExchangeCalendar` everywhere. `Arc<dyn>` is only used for `Clock`.
- **NB-2 `SessionSpan::Eth` on CryptoSpot undefined.** → Validated at `Chart::new` via `calendar.validate_period(period)`. Fatal config error; never mid-stream. Crypto rejects `Eth`; Extended aliases Regular on single-session calendars.
- **NB-3 `PaintContext` undefined.** → Sans-IO primitive emitter: `{ axis, viewport, price_range, palette, out: &mut ScenePrimitives }`. `ScenePrimitives` is an AoS of typed instance vecs (candles, quads, lines, badges, text).
- **NB-4 `BarStream::seek` semantics.** → Split into `BarStream` (minimum) + `SeekableBarStream: BarStream` (opt-in). Live streams don't implement seek.
- **NB-5 `z_order` collisions undefined.** → `LayerZ` enum with enumerated ordinals (`SessionBand=0..Crosshair=10`). Builder sorts by `(LayerZ, insertion_idx)`. Collisions structurally impossible.

## Majors (resolved inline in 00a)

- **NM-1 Clock scope — 163 sites.** → `Clock` trait carries both `now()` (wall) + `now_monotonic()` (Instant). `midas-ib-sim/src/engine/clock.rs` is cited as prior-art.
- **NM-2 MockClock + `tokio::time`.** → `MockClock::advance_by(d)` calls `tokio::time::advance(d)` internally when tokio-test runtime is active.
- **NM-3 Candle redundancy.** → `Candle` = wire/API type (self-contained); `CandleSeries` stores minimum (calendar+period once, SessionKind per-row, synthesize `Session` lazily via `CandleRef`).
- **NM-4 AnnotationLayer escape hatch.** → Rejected generic `AnnotationLayer` replaced by concrete `OrderBracketLayer`, `PriceLineLayer`, `LevelLayer`, `DecoratorLayer` with distinct `LayerZ` slots.
- **NM-5 `CompressedAxis::from_x`.** → `from_x` returns `Option`; `from_x_snapped(x, dir) -> (Timestamp, was_snapped)` always succeeds. Consumer-specific: crosshair uses snapped; hit-test uses raw.
- **NM-6 Zoom-to-continuous auto-switch.** → Deferred to Phase F. MVP: user-explicit toggle between `CompressedAxis` and `ContinuousAxis`.
- **NM-7 Camera demotion.** → `Camera2D` deleted entirely. Fields decompose into `TimeAxis` + `PriceRange` + `Viewport`. `InteractionState` captures pan/zoom/drag/hover.

## Minors (folded)

- **Nm-1 `BarStreamMeta.calendar`.** → Now `&'static dyn ExchangeCalendar`, not `CalendarId`.
- **Nm-2 `MockClock` unit.** → epoch-nanos (i64), explicit in the type.
- **Nm-3 `sessions_between` allocation.** → Changed to caller-owned `&mut SmallVec<[Session; 16]>` out-param.
- **Nm-4 `Session::label`.** → `Option<Cow<'static, str>>` to future-proof user-defined overlays.
- **Nm-5 `covers()` range semantics.** → Documented as half-open `[start, end)`. XNYS coverage `2000-01-01 .. 2032-01-01`.

## Gaps resolved (inline in 00a §Round-2 resolutions appendix)

- **G-1** Volume pane: single-pane bottom strip MVP; multi-pane deferred.
- **G-3** Interaction state: `InteractionState` field on `Chart`.
- **G-4** Persistence: `ChartViewStore` persists `(symbol, calendar_id, period, axis_snapshot, price_range, viewport, eh_policy, layer_config)`. `SessionChart::rehydrate` on app start.
- **G-6** Thumbnail: a `ChartScene` with minimal `LayerConfig`; `ThumbnailDataStore` retired.
- **G-7** `DumpState` projection: auto-derived via `serde_json::to_value`.
- **G-8** Early-close + Clock(H1): truncated bar closed at 13:00 ET with `Completeness::Completed`; next bar opens at 04:00 ET next day.
- **G-9** `SymbolResolver`: specified trait; `ResolvedSymbol { symbol, calendar, provider_id }`. Per-provider impls.

## Gaps deferred to Phase F

- **G-2** Indicators: sibling plan. `ComputedSeriesLayer<I: Indicator>` at a new `LayerZ::Indicator` slot.
- **G-5** Multi-source overlay: requires `CompositeAxis`; out of scope.

## Strengths confirmed

- Clock-first S0 sequencing correct.
- Infallible `classify()` correct.
- Smart-constructor discipline on `Session`/`Candle` enforces invariants.
- History & live unified via `BarStream` is the correct architectural framing.
- `CalendarId(&'static str)` MIC-ish identity interoperable with future exchanges.
- Golden-fixture holiday test with `nyse-holiday-cal` is the right failure mode.
- Phase A = pure new crates bounds risk correctly.
- EhPolicy as per-chart persisted state matches TV/IBKR UX.

## Verdict

Round-2 findings folded. Remaining open items are all Phase F (auto-zoom switch, multi-source overlay, indicator infra) which are orthogonal. Design is ready for implementation. Phase A can start on `midas-clock` (S0) immediately — no blockers remaining in S0's scope. `midas-scene` (S5) can start only after `PaintContext` (NB-3) and `LayerZ` (NB-5) appendix edits are read by the implementing agent.
