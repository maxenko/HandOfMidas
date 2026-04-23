# Session-Aware Charts — Status

Status as of the current working tree. Navigate the plan docs via `00-index.md`.

## What shipped

### Phase A — foundation crates (all landed)

| Crate | Purpose | Tests |
|-------|---------|------:|
| `midas-clock` | `Clock` trait (wall + monotonic), `SystemClock`, `MockClock` with `tokio::time::advance` integration, `mock_clock` feature | 9 |
| `midas-calendar` | `ExchangeCalendar` trait, `XnysCalendar` (full NYSE holiday table 2000-2031), `CryptoSpotCalendar`, `BarPeriod`, `Session`, `TradingDay`, `SessionBuf` typedef | 48+ |
| `midas-bars` | `Candle` (wire), `CandleSeries` (SoA storage with trade_count + wap + max_rows cap), `Symbol`, `Completeness`, `Ohlcv` | 40+ |
| `midas-stream` | `BarStream` + `SeekableBarStream` traits, `FixtureBarStream`, `ChannelBarStream`, `HistoryThenLive`, `Filtered<_, EhFilter>`, `Resampled` (stub) | 29 |
| `midas-axis` | `TimeAxis` trait, `ContinuousAxis`, `CompressedAxis` (session-compressed), `SessionIndexAxis`, `TimeAxis::for_calendar` builder | 43 |
| `midas-scene` | `ChartScene` + `ChartSceneBuilder`, `SceneLayer` trait, `LayerZ` newtype (drop-in ordering), `PaintContext`, `ScenePrimitives`, concrete layers (candle, volume, grid, session-band, session-separator, holiday, crosshair, order-bracket, price-line, level, decorator) | 59+ |

### Phase B — vertical-slice proof (all landed)

| Crate | Purpose | Tests |
|-------|---------|------:|
| `midas-bars-adapter` | Bridge from `midas-broker-core::MarketDataSource` → `BarStream<Candle>`. Includes `SymbolResolver` (+Static + Heuristic), `SessionedBarAggregator`, `subscribe_aggregated_bars`, `build_history_then_live`, broker-call timeouts | 72+ |
| `midas-app::session_chart` | Feature-gated (`session_chart`) iced widget + window. `SessionChart`, `SessionChartDriver`, `scene_builder`, `primitives_bridge`, `gpu_renderer`, `shader`, `SessionChartWindow`, `SessionChartProgram`, `EhPolicy`, `AxisBox` | 60+ |

### Phase C — horizontal expansion (all landed)

Session-chart widget handles:
- Crypto 24h (`ContinuousAxis`) and XNYS stocks (`CompressedAxis`).
- Clock periods (M1/M5/M15/M30/H1), Session periods (D1 RTH / ETH), Calendar periods (W1/MN1).
- EhPolicy toggle (`ShowAll` / `HideExtended` / `ShowBarsOnly`) cycled via "EH" chip in the window chrome.
- `HolidayMarkerLayer` with per-calendar timezone anchoring (fixed in scrutiny pass).
- `BarPeriod::Session(Regular)` produces 09:30–16:00 ET bars on regular days, 09:30–13:00 ET on early-close days.

### Scrutiny loop (13 HIGH/MED fixes + 6 refactors)

**Bug-hunt / app-harden findings:** all 13 fixes landed with proving tests — holiday timezone, aggregator quote-leak rejection, seam-dedup correctness, compressed-axis close-edge ownership, `CandleSeries` max_rows cap, broker-call timeouts (30s), widget `.expect` → `Result`, `DumpState` session-chart projection, driver drop-order, NaN/inf price filter, `MockClock` u64 safety, `InteractionState` cleanup.

**Arch-audit refactors (6):**
- R1 — Invert the Arc: `CandleLayer`/`VolumeLayer` now take `Arc<RwLock<CandleSeries>>`, eliminating the per-frame deep-copy.
- R2 — `LayerZ` newtype with canonical consts (drop-in layer insertion at arbitrary z without enum edits).
- R3 — `widget.rs` split into `policy.rs` + `axis_box.rs` + `widget.rs` (god-module dissolved).
- R4 — `SessionBuf = SmallVec<[Session; 16]>` typedef across the stack.
- R5 — `#[non_exhaustive]` on `BarPeriod`, `ClockInterval`, `SessionSpan`, `CalendarSpan` for semver hygiene.
- R6 — `aggregator/core.rs` test block split into `core_tests.rs` (production file 930→454 LOC).

### Final metrics

- **2 641 tests passing** across both workspaces (1 180 root + 1 461 desktop).
- **Zero clippy warnings** with `-D warnings` across both workspaces, both with and without `session_chart`/`dev_harness` features.
- **Zero regressions** on the ~600+ pre-existing tests.
- **Net LOC added**: ~10 500 (7 new crates + feature-gated widget + tests).
- **Zero edits to legacy `midas-chart`, `midas-render`, `chart_widget.rs`, `Camera2D`, broker, or router** — Phase A/B/C is purely additive.

## What's gated behind `session_chart` feature

The new stack is opt-in via the `session_chart` Cargo feature on `midas-app`:

```bash
cd desktop/win
cargo run -p midas-app --features session_chart        # adds toolbar buttons
cargo test -p midas-app --features session_chart       # runs new tests
```

Three toolbar buttons open standalone windows: **"BTC M1"** (crypto 24h), **"AAPL M5"** (XNYS regular-session intraday), **"SPY D1·RTH"** (XNYS daily RTH bars). Each window has an EH cycle chip + Close button. Candles, grid, bands, and session separators render through the new GPU pipeline.

## What's deferred (not yet shipped)

### Phase D — legacy retirement (prerequisites unmet)

Phase D in `00b-integration-strategy.md` calls for deleting `CandleBuffer`, `Timeframe`, `Camera2D::time_to_x`, `DataProvider::get_candles`, `detect_session_boundaries`, and the `ChartScene` god-struct. **This work is intentionally not executed yet** because the new `SessionChart` does not yet have feature parity with the legacy chart on:

- **Bracket placement tool** (3-click `BracketTool` state machine in `midas-chart/src/widget/`).
- **Order-bracket annotation rendering** (decorator trees: hover-promote, proximity-highlight, drag handles).
- **Price-line and level annotations** (user-drawn horizontals).
- **Indicators** (ATR, G.ATR — `midas-indicators` crate).
- **Volume profile** (`midas-chart/src/volume_profile`).
- **Crosshair hover tooltip** (price + time at cursor).
- **Chart-view persistence** (`ChartViewStore` — saved camera per (symbol, period)).
- **Right-click menus and keyboard shortcuts**.

The layer slots in `midas-scene` already exist (`OrderBracketLayer`, `PriceLineLayer`, `LevelLayer`, `DecoratorLayer`) but render stubs. Phase D requires a preceding feature-port sprint that populates these layers with the legacy behaviour before the legacy path can be removed.

### Other deferred gaps

- **Text rendering inside badges + axis priceline numbers** — `RenderBuckets.badges` carry `Cow<'static, str>` sidecars but the glyph-atlas path (cryoglyph in existing `midas-render::TextPipeline`) is not yet wired to the new buckets. Tracked as R2-G-2 in `99-diagnostic-findings-r2.md`.
- **Real pan/zoom interaction** — wheel events on the new widget currently cycle EhPolicy as a stub; pan/zoom is a follow-up.
- **Keyboard shortcuts** — arrow-keys / +/- / "E" / "X" on the session-chart window.
- **Indicator layer infrastructure** (`ComputedSeriesLayer<I: Indicator>`) — deferred to Phase F per 00a.
- **Multi-source overlay** (SPY + ES on one chart) — deferred to Phase F.
- **Auto-switch continuous ↔ compressed axis at zoom threshold** — MVP is user-explicit toggle.

## How this addresses the original problems

Original bugs reported by the user:

1. **"Watchlist and chart prices don't match"** — root cause was two disjoint testdata generators. New stack: all bars route through `MarketDataRouter` + `midas-bars-adapter`, which wraps the same `Arc<dyn MarketDataSource>` the watchlist uses. One price stream, no drift. ✓ Fixed for any chart opened through `session_chart`; legacy chart path unchanged.
2. **"Charts don't extend live"** — default `D1` was rejected by the legacy aggregator. New stack: `SessionedBarAggregator` supports clock, session, AND calendar periods (M1 through MN1) via `calendar.bar_window(ts, period)`. ✓ Fixed.
3. **"Design should be ideal-first, no compromises to fit existing code"** — the new stack is designed clean-slate per `00a-ideal-design.md`, landed as 7 new additive crates. Legacy untouched. ✓

## Crate dependency DAG (new stack)

```
midas-clock
    ↓
midas-calendar
    ↓
midas-bars       ← midas-axis → midas-scene
    ↓               ↑              ↑
midas-stream ───────┘              │
    ↓                              │
midas-bars-adapter ← midas-broker-core (only adapter depends)
    ↓
midas-app::session_chart (feature-gated)
```

No cycles. Strictly downward. Every crate has a narrow public API + deep implementation.

## Files of note

- `00-index.md` — plan navigation (superseded by this README for status).
- `00a-ideal-design.md` — clean-slate architecture (authoritative).
- `00b-integration-strategy.md` — migration phases A/B/C/D/E.
- `99-diagnostic-findings-r1.md`, `99-diagnostic-findings-r2.md` — plan-eval review outputs, resolutions inline.

## Running

```bash
# Root workspace (data + engine)
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings

# Desktop workspace (app)
cd desktop/win
cargo test --workspace --features "midas-app/session_chart session_chart_tests"
cargo clippy --workspace --features "midas-app/session_chart session_chart_tests" --all-targets -- -D warnings

# Launch app with session-chart feature
cargo run -p midas-app --features session_chart
# → click "BTC M1" / "AAPL M5" / "SPY D1·RTH" in the toolbar
```
