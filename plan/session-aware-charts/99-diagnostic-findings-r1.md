# Diagnostic Findings — Round 1 (superseded bolt-on plan)

Four plan-eval agents reviewed the first-draft bolt-on plan (S0–S12). Their output is preserved here as diagnostic input. The ideal design in `00a-ideal-design.md` and the integration strategy in `00b-integration-strategy.md` fold these findings in.

## Correctness lens

### Blockers from review
- **B1: Two `Timeframe` enums (root vs desktop).** Resolved in ideal — `BarPeriod` is defined once in `midas-bars` and shared.
- **B2: `SessionKind` duplication.** Resolved — single definition in `midas-bars`, re-exported by `midas-calendar`.
- **B3: `static XNYS = XnysCalendar::new()` requires `const fn` with tables.** Use `std::sync::LazyLock<XnysCalendar>` pattern.
- **B4: `tokio::time::pause()` does not affect `Utc::now()`.** Adopted — `midas-clock` crate (S0) added as prerequisite. Every `Utc::now()` routes through `Clock`.
- **B5: bands implementation inconsistency.** Resolved — `SessionBandLayer` consumes `calendar.sessions_between(viewport)` at render time; `Candle.session` is redundant metadata carried separately.

## Trading-domain lens

### Blockers from review
- **B2-fidelity: Black Friday rule "4th Friday of November" is wrong.** Must be `thanksgiving_date + 1 day`. Corrected in `00a-ideal-design.md` §Holiday rules.
- **Missing Reagan (2004-06-11) and Ford (2007-01-02) ad-hoc closures.** Added.
- **Juneteenth year-gating:** must only apply `year >= 2022`. Corrected.
- **Day before Good Friday is NOT an NYSE early close.** Removed from the rule set; that's a SIFMA bond-market convention.
- **04:00 ET pre-market footnote:** reflects ECN/ARCA convention, not NYSE floor (06:30 ET). Documented.

### Confirmed accurate
- 09:30–16:00 ET regular session, 04:00–20:00 ET full extended hours, 13:00 ET early-close RTH, 13:00–17:00 ET early-close post-market.
- NASDAQ = NYSE calendar simplification is defensible for MVP.
- UTC midnight D1 for spot crypto (Binance, Coinbase, Kraken, TV all align).
- `use_rth=true` default matches IB TWS + Bloomberg convention.
- One contiguous intraday stream with per-bar session metadata is industry standard.
- Per-chart EH toggle chip matches TV + IBKR pattern.

## Performance + testability lens

### Test-blockers
- **`Utc::now()` + `tokio::time::pause()` incompatibility (T1):** six of ten integration tests and all four sim session tests would be non-deterministic. Resolved via `Clock` abstraction in S0.
- **`sim_fidelity.rs` existing tests:** `sim_session_aware` must default `false` for legacy tests so they pass on any wall-clock.

### Performance findings
- No blockers on memory, GPU, or rendering.
- Optimization: calendar cached at subscribe time inside `BarStream::meta()` — no per-tick registry lookup.
- Optimization: `classify()` DST lookup cheap (~100ns), fine for 400tps sim emitter.
- `sessions_between` allocation per frame: noted; `SmallVec<[Session; 16]>` or reused buffer.

### Confirmed fast
- `calendar.bar_window` per RT bar: <1µs.
- `CandleSeries::session_at(idx)` at render: O(1), <10ns.
- Memory cost of session metadata: trivial (<1% of candle data).
- `chrono-tz` DST handles all edge cases for UTC-on-the-wire inputs.

## Migration risk lens

### Critical findings that apply to the ideal design's Phase B (integration adapter)

Even though the ideal design replaces `CandleBuffer` with `CandleSeries`, the legacy codebase has real coupling the adapter must survive:

- **Binary .midas file format**: `desktop/win/crates/midas-data/src/binary/mod.rs:268` reads into `CandleBuffer::push(ts, o, h, l, c, v)`. The ideal `CandleSeries::push` requires a full `Candle` with session metadata. Adapter strategy: during Phase B (`midas-data`/`midas-bars` bridge), on-disk v1 records get session synthesized via `calendar.classify(ts)` at load time. Binary format v1 with `CandleRecord._padding: u32` can carry a session byte without version bump; v2 if we want explicit encoding. Spec to add to Phase B.

- **`CandleSlice` zero-copy view**: same module, line 326, exposes `&[u32] volumes` + siblings. `CandleSeries` needs the equivalent zero-copy slice type (or `CandleRef<'a>` per ideal design) that includes sessions slice. 9 `impl CandleData for ...` sites in desktop total.

- **`SymbolKey::contract_id` plumbing** (critical, unresolved by ideal design alone): the ideal `BarStream::meta().symbol` must carry whatever `con_id` IB needs. Two paths:
  - Symbol becomes a richer structured type carrying `(ticker, con_id, calendar_mic)`.
  - `SymbolResolver` trait lazily resolves `ticker → (con_id, calendar)` on first subscribe.
  - Sim generates stable-hash `con_id` for any ticker; IB does a `reqContractDetails` round-trip.
  - Phase B adapter must pick one approach — flag as open decision for Phase B kickoff.

- **Provider picker UI + ProviderConfig TOML persistence**: 11 call sites across 5 files reference `active_data_provider`. Phase D (legacy retirement) must:
  - Remove or repurpose the toolbar provider picker.
  - Keep `ProviderConfig.active_data: Option<String>` with `#[serde(default)]` to avoid breaking existing user configs; ignore at load if the `HistoricalDataRegistry` is gone.
  - Decide whether a future multi-source chart reinstates the picker with different semantics.

### Workspace-crossing dep edge

- Desktop `midas-core` may gain a dep on root-workspace `midas-broker-core` (or on the new `midas-bars`) if it wants `SessionKind`. Current state: desktop `midas-core` is a pure leaf (per `desktop/win/CLAUDE.md`).
- **Ideal design resolution**: `Candle` lives in `midas-bars` (root workspace). Desktop `midas-core`'s legacy `CandleBuffer` is retired in Phase D. Desktop `midas-chart` / `midas-render` depend on `midas-bars` directly. `midas-core` stops being a leaf only to the extent it re-exports or bridges types — but the ideal design prefers pushing the dep *above* `midas-core`, making `midas-chart` the consumer of `midas-bars` rather than `midas-core`.

### `ChartInput` + `ChartPanel` field ripple

- Current code has 4 `ChartInput { ... }` literal construction sites. Ideal design replaces `ChartInput` with a `ChartSceneBuilder`; adapter between old `ChartPanel` and the new scene must translate once during Phase C when each chart migrates.
- `ChartPanel` gets retired in Phase D. Adapter layer pairs legacy `ChartPanel` with new `SessionChart` during Phase B/C; tests construct the new type directly.

### DumpState projection

- `dev_harness/dump.rs::ChartProjection` today projects `(chart_id, symbol, timeframe, camera)`. Ideal design's `SessionChart` has `(symbol, calendar, period, eh_policy, axis, layer_config)`. Phase D updates the projection schema; `app_sim_e2e.rs` baselines re-captured once; fixture-replay tests pick up new shape.

### Commit strategy for legacy retirement

Phase D commits should be split narrowly:
- `chore: route chart historical loads through BarStream` (the router/source swap, preserves tests).
- `chore: retire ProviderRegistry picker` (UI + TOML migration, one PR).
- `chore: delete midas-feed testdata generator` (data deletion, one PR).
- `chore: retire CandleBuffer + Timeframe` (final deletion after all consumers migrated).

Total Phase D: ~4–6 narrow commits rather than one big-bang.

## Verdict

## Verdict

All round-1 findings are either (a) corrected in the ideal design (00a) or (b) preserved as explicit requirements in the integration strategy (00b). No finding required revisiting the ideal architectural shape; all were bolt-on-plan-specific tactical issues resolved by the clean-slate design.

Round-2 plan-eval should be run against `00a-ideal-design.md` + `00b-integration-strategy.md` specifically, not the superseded S0–S12 slice docs.
