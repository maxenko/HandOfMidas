# Architecture Review: Session-Aware Chart Stack

Reviewer lens: `arch-audit` 4-lens framework (coupling / cohesion / abstraction / evolution). Scope: the 7 new root-workspace crates (`midas-clock`, `midas-calendar`, `midas-bars`, `midas-stream`, `midas-axis`, `midas-scene`, `midas-bars-adapter`) plus the desktop-side `session_chart` module, `chart_parity.rs`, and `session_chart_window.rs`. This is a review, not a refactor — no code was modified.

## Summary

The session-aware stack is a strong piece of design. It is organized as a clean Phase-A → Phase-B pipeline with a strict downward dependency flow (`clock/calendar` → `bars` → `stream`/`axis` → `scene`/`bars-adapter` → `session_chart` in the desktop crate), enforced by the workspace split: root-workspace crates physically cannot depend on anything in `desktop/win/`. Traits are well-chosen (`BarStream`/`SeekableBarStream`, `TimeAxis`, `ExchangeCalendar`, `Clock`, `SceneLayer`/`InteractiveLayer`, `SymbolResolver`) and mostly do one thing. The main soft spots are (1) `midas-scene` has grown large (12.1k LOC and 3 of 4 sub-areas over 1 kLOC) and is absorbing more than the "sans-IO layers" charter; (2) the `midas-bars-adapter` seam into `midas-broker-core` is necessarily tight (29 imports across 10 files) and worth formalising as a named port; (3) the desktop-side `session_chart::gpu_renderer` crosses back into legacy `midas_chart::*` types and is the most fragile coupling in the stack. None of these are fatal — they are follow-ups already acknowledged in plan comments (slice 9c, plan D6).

---

## Lens 1 — Coupling

### Dependency DAG (computed from `[dependencies]` blocks)

```
midas-clock          (chrono, tokio)                                       — leaf
midas-calendar       (chrono, chrono-tz, smallvec, thiserror)              — leaf
midas-bars           (midas-calendar)                                      — 1 internal
midas-stream         (midas-bars, midas-calendar, midas-clock)             — 3 internal
midas-axis           (midas-bars, midas-calendar)                          — 2 internal
midas-scene          (midas-axis, midas-bars, midas-calendar)              — 3 internal
midas-bars-adapter   (midas-bars, midas-calendar, midas-clock, midas-stream, midas-broker-core)
                                                                           — 5 internal (incl. broker-core)
midas-app (session_chart feature)  (midas-scene, midas-bars, midas-bars-adapter,
                                    midas-calendar, midas-clock, midas-stream, midas-axis,
                                    midas-broker-core, midas-chart, midas-render, ...)
```

**No cycles.** The DAG is strictly acyclic; each crate's Cargo.toml points only downward. `midas-scene` intentionally does not depend on `midas-stream` or `midas-clock` (the scene is sans-IO and stateless-in-time; streams live upstream), which is the correct choice. Compared to the alternative ("one big chart crate"), this DAG has the right shape: the leaves (`clock`, `calendar`) are the most stable and the roots (`scene`, `bars-adapter`) the most volatile.

**Fan-in / fan-out.** `midas-calendar` is the stable hub — 6 direct internal dependents (bars, stream, axis, scene, bars-adapter, session_chart) — all pointing the right way (SDP-clean: stable depended on by less stable). `midas-bars` has 5 dependents, `midas-axis` 2, `midas-scene` 1 (the app). `midas-clock` fan-in is minimal (stream + bars-adapter); this is fine — `Clock` is only needed by time-sensitive code (timeouts, pump pacing).

### The `midas-bars-adapter` → `midas-broker-core` seam

`midas-bars-adapter/Cargo.toml:11` adds `midas-broker-core`. This is the **only** upward peek in the new stack and is genuinely necessary — the adapter consumes `MarketDataSource` to emit `BarStream<Candle>`. Coupling breadth: 29 `midas_broker_core::` imports across 10 files, reaching `market_data::{Tick, TickKind, TickType, TickValue, Bar, BarCompleteness, SymbolKey, Timeframe, WhatToShow, IbDuration, ReqId, GenericTicks, MarketDataError}` and `provider::{MarketDataSource, HistoricalBarsResult, RealtimeBarStream}` — 13 concepts. Correct shape for an anti-corruption layer (smell-catalog § "Missing ACL"), but the name "bars-adapter" understates scope: this is a full ACL between two bounded contexts. Rename to `midas-broker-session-bridge` would clarify; not a blocker.

### Desktop ↔ root coupling

`session_chart` is feature-gated behind seven `optional = true` root deps (`desktop/win/crates/midas-app/Cargo.toml:36-44`). The gate is honoured — `session_chart/mod.rs:115` begins with `#![cfg(feature = "session_chart")]`, and every submodule carries the same guard. When the feature is off, nothing in these 9 files compiles.

The one creep: `session_chart/gpu_renderer.rs:64-67` and `primitives_bridge.rs:54-55` import `midas_chart::compute`, `midas_chart::instances`, `midas_chart::widget::compute::WidgetLabel`, `midas_chart::BadgeInstance`, `midas_chart::DirtyFlags`. This is by design (slice 9c pre-deletion; explicit block-comment labels it "grep-gate exception, documented GPU-pipeline bridge") — the only place where Phase-B code still reads legacy types. Until slice 9c lands, retiring `midas-chart` breaks `session_chart`.

### Connascence across the `ToolEffect` seam

`midas-scene::tools::ToolEffect` (`tools/mod.rs:104-149`) uses `AnnotationId = u64` as an opaque id; the desktop workspace translates to `TickerMsg` (`bracket_effects::project_effect_to_ticker_msg`). Correct shape — static CoN across the boundary, not CoP. `widget.rs:80-83` renames (`SceneBracketSide`, `SceneLegRole`) so the scene vocabulary does not leak into app-level matches.

---

## Lens 2 — Cohesion / Complexity

### LOC by file (hot spots in scope)

| File | LOC | Notes |
|---|---|---|
| `midas-scene/src/tools/bracket.rs` | 1 578 | Largest single file in scope |
| `midas-scene/src/layers/annotations.rs` | 1 342 | 4 layers + 2 InteractiveLayer impls |
| `midas-scene/src/layers/indicator.rs` | 1 202 | Includes a duplicated copy of ATR/G-ATR math |
| `midas-scene/src/layers/crosshair.rs` | 1 014 | |
| `midas-scene/src/decorator/tests.rs` | 1 840 | Test file; fine |
| `midas-bars/src/series.rs` | 887 | SoA + version + rolling cap |
| `midas-scene/src/scene.rs` | 844 | Scene + Builder + dispatch |
| `midas-scene/src/layers/volume_profile.rs` | 779 | |
| `midas-axis/src/compressed.rs` | 720 | Includes session gap math |
| `session_chart/widget.rs` (desktop) | 1 684 | Widget + two tool hosts |

`bracket.rs` (1.58 kLOC) and `annotations.rs` (1.34 kLOC) approach "god module" (Rust threshold ~500-800 LOC for a module with >20 `pub` items). `bracket.rs` is a pure FSM with most mass in test arms; pub surface is small — acceptable, watch the trend. `annotations.rs` bundles four conceptually independent layers (`OrderBracketLayer`, `PriceLineLayer`, `LevelLayer`, `DecoratorLayer`) with distinct zs (600/700/800/900). They share only drag-state plumbing. P2 split.

### `midas-scene::tools` vs `midas-scene::layers::annotations` — right seam?

Yes. `tools::BracketTool` / `tools::LevelTool` are **transient placement FSMs** (awaiting_entry → awaiting_tp → awaiting_sl → complete). `layers::OrderBracketLayer` / `layers::LevelLayer` are **persistent visualisations + drag handlers** for committed annotations. They collaborate via `ToolEffect` emissions (`ToolContext::emit_effect`, `layer.rs:202`). Correct split: tool creates, layer displays+edits. Both implement `InteractiveLayer` (as_interactive overrides at annotations.rs:424/809/1099, tools/bracket.rs:263, tools/level.rs:172) without sharing a base — they share a method (update/hit_test/cancel), not a responsibility.

### `midas-stream` vs `midas-bars-adapter` — distinct concerns?

Yes. `midas-stream` is the abstract combinator surface (`BarStream`, `FixtureBarStream`, `HistoryThenLive<H, L>`, `Filtered<S, P>`, `ChannelBarStream`) with **no broker knowledge**. `midas-bars-adapter` is the Phase-B concrete impl (`SessionedBarAggregator`, `RealtimeBarAdapter`, `SymbolResolver`, `build_history_then_live`) that produces `BarStream<Candle>` from `MarketDataSource`. Stream: 1 416 LOC; adapter: 2 338 LOC. Genuinely distinct — "what is a bar stream" vs "here's how broker-core builds one". Not shallow: `midas-stream` hides `HistoryThenLive` seam-dedup + `EhFilter` + `FilterPolicy` behind a three-method trait.

### Cohesion of `midas-scene`

12 146 LOC and growing. Internally cohesive, but the crate carries five responsibilities: (a) `SceneLayer` trait + dispatch, (b) 11 concrete visual layers, (c) decorator subsystem (legacy UI port), (d) interactive tool FSMs, (e) primitives + palette. A future `midas-scene-core` / `-layers` / `-tools` split matches how the z-ordinal design already separates them. P3 only — not actively hurting; each sub-module tests in isolation.

---

## Lens 3 — Abstraction Quality

### `BarStream` / `SeekableBarStream`

Minimal surface: `meta() + next() + snapshot()`, with seek split into an opt-in sub-trait (`midas-stream/src/lib.rs:216-242`). Textbook correct. `async_trait` is used; native `async fn in trait` migration would be mechanical if ever needed. `TimeRange` smart-constructed (rejects `from > to`); `StreamError` has a closed variant set with `CoverageExceeded` validated at construction — good parse-don't-validate hygiene. `Send` but not `Sync` by design (`next/snapshot` take `&mut self`) — eliminates accidental multi-consumer.

### `TimeAxis`

Six methods (`to_x`, `from_x`, `from_x_snapped`, `ticks`, `width_px`, `policy`; `midas-axis/src/lib.rs:226-258`). `from_x` returning `Option<Timestamp>` paired with infallible `from_x_snapped` matches R2-NM-5 — clean split of "don't know" vs "snap to nearest". `clippy::wrong_self_convention` allow is justified.

### `ExchangeCalendar`

Twelve methods (`id, tz, covers, time_axis_policy, trading_day, is_trading_day, classify, bar_window, validate_period, sessions_between, next_open, prev_close`). Large surface, but each answers a distinct question and the doc explicitly forbids hand-rolled session arithmetic. `SessionBuf` allocation-free idiom + process-global `&'static dyn` (vs `Arc<dyn>`) are well-chosen.

Design-balance: `classify` is infallible-saturating, `trading_day` returns `Result`. Rationale sound (hot-path vs cold-path) but undocumented on the trait — P1 doc comment.

### `Clock`

Two methods (`now`, `now_monotonic`), `Send + Sync + 'static`. `SystemClock` for prod, `MockClock` behind `feature = "mock_clock"`. Carrying BOTH wall-clock and monotonic on one trait (`lib.rs:7-13`) avoids the paused-tokio footgun where tests mock wall-time but `Instant::now()` in a pacer keeps ticking. `MockClock::advance_by` drives `tokio::time::advance` in lockstep (lines 171-174). Deep abstraction: tests see ONE handle, not three (wall / monotonic / tokio-timer).

### `SceneLayer` + `InteractiveLayer` + `as_interactive` opt-in

Default `None` opt-in (`layer.rs:126-128`) instead of a blanket impl. Per D4 doc: a blanket `impl<T: SceneLayer> InteractiveLayer for T` would block downstream crates from opting specific layers in under the orphan rule. Cost today: 5 `fn as_interactive` overrides (`OrderBracketLayer`, `LevelLayer`, `DecoratorLayer`, `BracketTool`, `LevelTool`). Correct usage.

### `ToolEffect`

10 variants / 3 groups: Level (3), Bracket (5), Generic (2). Shape frozen deliberately. Variants carrying `AnnotationId` scale fine. For measure / trendline / fib, the per-tool-per-variant pattern does not scale past 2-3 tools without flattening to a dozen constructors. P2: before the third tool lands, refactor to a `CreateAnnotation(AnnotationSpec)` + `UpdateAnnotation(AnnotationId, AnnotationPatch)` shape.

### `SymbolResolver` / `LabelFormatter`

`SymbolResolver` (`bars-adapter/src/resolver.rs:56-58`) — single-method trait returning `ResolvedSymbol { symbol, calendar: &'static dyn ExchangeCalendar, contract_id: i32 }`. Two impls (`Static`, `Heuristic`); heuristic uses seeded DJB2 for cross-run reproducibility. Deterministic, testable, small.

`LabelFormatter` (`midas-axis::format`) — single-method trait, default `DefaultFormatter`. Formatter isolated from axis projection math — good split.

---

## Lens 4 — Evolution Signals

### Adding a new calendar (XCME) — **2 files**

New `crates/midas-calendar/src/xcme.rs` implementing `ExchangeCalendar` (pattern from `xnys/` or `crypto_spot.rs`) + `lib.rs` mod declaration + `pub fn xcme()` helper. Optional: `resolver.rs` for futures tickers. Nothing else in the stack touches — calendars are addressed through `&'static dyn ExchangeCalendar` and `TimeAxisPolicy`. Textbook "adding a variant is cheap".

### Adding a new indicator (MACD) — **1-2 files**

Per `midas-scene/src/layers/indicator.rs:1-20` doc (and plan D6): two concrete structs, no `trait Indicator`. Adding MACD means a `MacdLayer` struct + `impl SceneLayer` in `indicator.rs` (or `indicator/macd.rs`) + a re-export. At the third indicator, the YAGNI calculus flips — three structs sharing the `Arc<RwLock<CandleSeries>>` + version-keyed cache pattern signal the extraction. **Reassess at the third.**

Separately, `indicator.rs:32-43` documents a duplicated-math concern: ATR math is a bit-exact copy of `midas-indicators::WildersAtr` + `midas-core::gerchik_gatr_detail` because `midas-scene` sits in the root workspace and those live in `desktop/win/`. Pragmatic today; cost doubles at the third indicator. P2: consider moving `midas-indicators` to the root workspace.

### Adding a new annotation tool (trendline) — **4-5 files**

New `tools/trendline.rs` (`TrendlineTool` + FSM + `InteractiveLayer`) + `layers/trendline.rs` (or append to `annotations.rs`) + `LayerZ::TRENDLINE` (use the 100-wide gaps) + re-exports + `ToolEffect` variants. Scene-internal. The 100-wide `LayerZ` design (`layer.rs:59-99`) was built for exactly this — good evolution signal. But the `ToolEffect` per-tool-per-variant pattern strains here (see abstraction notes).

### Adding a new backend (OANDA for FX)

Lives in `midas-broker-core` + a new concrete backend crate implementing `MarketDataSource` + `OrderClient` — **unchanged by this stack**. `midas-bars-adapter` consumes any `MarketDataSource` impl generically; an `FxCalendar` ships alongside `XcmeCalendar`. Zero changes to `session_chart` beyond a new `SessionChartRequest` constructor. Right seam.

### Adding a new chart period (range bars, volume bars) — **the blocker**

`midas-calendar::BarPeriod` is `Clock(ClockInterval) | Session(SessionSpan) | Calendar(CalendarSpan)`. Range bars (per-price-move) and volume bars (per-size) do not fit — their window is not a function of time. Adding them requires new `BarPeriod` variants AND redefining `ExchangeCalendar::bar_window` semantics AND new rollover predicates in `SessionedBarAggregator` AND rethinking `CompressedAxis`. The **only place in the audit where abstractions break down**. `BarPeriod` is conceptually "time-based bar window"; extending it to non-time windows complects concerns. Plan option: introduce a `BarSpec` supertype with `BarPeriod` as one case and `Range(f64)` / `Volume(u64)` / `Tick(u32)` alongside. P3 today (no demand), but the evolution story must not be "add a variant to BarPeriod".

---

## Specific concerns

- **`Arc<RwLock<CandleSeries>>` vs `ArcSwap`** — invariant (`driver.rs:29-34`): single writer (pump), many readers; never hold a guard across `.await`. Enforced by `parking_lot::RwLock` (no async poisoning). `SymbolSeriesRegistry` (`registry.rs:47-48`) stores `Weak<RwLock<_>>` so series frees on panel close. `ArcSwap` would pay off if writers were mutation-rare (they aren't — ~25 Hz for M1 tick-driven aggregation) AND reads highly contended (they aren't — ≤20 panels × 60 Hz = 1.2 kHz). `RwLock` is fine at this scale. Plan D6 got it right. **P3 — revisit past 100 panels.**

- **`session_chart::gpu_renderer.rs` → `midas_chart::widget::compute::WidgetLabel`** — documented GPU-bridge (`gpu_renderer.rs:55-63`). Tracked technical debt for slice 9c; blast radius is exactly the two files (`gpu_renderer.rs`, `primitives_bridge.rs`). Until slice 9c lands, retiring `midas-chart` breaks `session_chart`. **P2 — prioritise 9c before any larger `midas-chart` refactor.**

- **`SceneError` / `SceneBuildError` split** — runtime faults (`error.rs:19-47`: panic fallback, ticker rejection, persistence, annotation-not-found, axis-range) vs build-time faults (`scene.rs:556-563`: missing axis/price_range/viewport). Zero overlap. Merging would force every build call site to match impossible variants. Keep split. **P3 — rename `SceneBuildError` → `BuilderError` for clarity.**

- **`SessionChartDriver` + pump task** — lifecycle well-managed. `_pump: JoinHandle<()>` declared first (`driver.rs:64`) so Rust field-order drop aborts the pump BEFORE the shared `Arc<RwLock<CandleSeries>>` drops. Load-bearing; correctly commented ("App-harden L1"). Back-pressure: pump reads `stream.next().await` directly and writes synchronously under `parking_lot::RwLock` — no bounded channel needed; `.await` yields naturally on slow writes. `watch::Sender<u64>` uses `send_replace` to coalesce between reader wakeups — correct for paint-coalescing. No lifecycle concerns at this scale.

- **`passes_parity_gate` compound predicate** — `chart_parity.rs:140-142` requires BOTH `ssim >= 0.995` AND `diff_fraction <= 0.002`. Mathematically sound: SSIM alone misses color-swap-with-same-structure (bull/bear flip — called out in the docstring); pixel-diff alone misses sub-threshold drift. AND catches both. Self-validation corpus has `SELF_VALIDATION_GOOD_MIN_SSIM = 0.999` to guard harness drift. YIQ-proxy luma (`dr*30 + dg*59 + db*11`) is the standard approximation — acceptable for parity.

---

## Strengths

- **Acyclic DAG, strict downward dependency flow** — the workspace split makes violations physically impossible.
- **Smart constructors everywhere** — `TimeRange::new`, `Ohlcv::new`, `Candle::new` (reconciling R2-NM-3 three-path `CalendarId`), `PriceRange::new`. Parse-don't-validate applied at every boundary.
- **`ExchangeCalendar::classify` infallibility** — hot-path trusts the calendar; construction-time `validate_period` catches misuse.
- **`SessionBuf` allocation-free** — caller-owned buffers in `sessions_between`.
- **`LayerZ` as wide-numbered ordinal** — 100-unit gaps; new layer is `LayerZ(450)` with no renumber.
- **`as_interactive` opt-in** — preserves the orphan rule for downstream `InteractiveLayer` impls.
- **Deterministic test clock** — `MockClock::advance_by` drives `tokio::time::advance` in lockstep.
- **Grep-gate exception comments** — every `midas_chart::*` import in the desktop bridge carries a named rationale + tracked follow-up.

---

## Actionable recommendations (ranked)

### P1 — ship this release

- **P1a. Document the `ExchangeCalendar` dual error policy** (classify infallible; trading_day falliable). One doc-comment explaining hot-path vs cold-path rationale. File: `crates/midas-calendar/src/exchange.rs:43-65`.
- **P1b. Prioritise slice 9c** before any larger `midas-chart` refactor — today `session_chart/{gpu_renderer,primitives_bridge}.rs` are load-bearing bridges back into legacy types. Slice 9c either migrates types into `midas-render` or adds a neutral GPU-primitive crate.

### P2 — before the next tool or indicator lands

- **P2a. Split `midas-scene/src/layers/annotations.rs`** (1 342 LOC, 4 `InteractiveLayer` impls) into `order_bracket.rs`, `price_line.rs`, `level.rs`, `decorator_layer.rs`.
- **P2b. Refactor `ToolEffect` before the third tool** — today's per-tool-per-variant pattern does not scale past 2-3 tools. Plan a `CreateAnnotation(AnnotationSpec)` + `UpdateAnnotation(AnnotationId, AnnotationPatch)` shape. File: `crates/midas-scene/src/tools/mod.rs:104-149`.
- **P2c. Move `midas-indicators` to the root workspace** so `midas-scene::layers::indicator` can depend on it rather than carry duplicated ATR/G-ATR math. Root cause: workspace split doesn't match dependency split for indicator code. File: `crates/midas-scene/src/layers/indicator.rs:32-43`.
- **P2d. Rename `midas-bars-adapter` → `midas-broker-session-bridge`** — current name understates scope; this is a full ACL between two bounded contexts. File: `crates/midas-bars-adapter/Cargo.toml:2` + workspace members.

### P3 — watch list; reassess at the next milestone

- **P3a. `midas-scene` split** into `-core` + `-layers` + `-tools` when LOC crosses ~15k (currently 12k).
- **P3b. Rename `SceneBuildError` → `BuilderError`** inside `midas-scene`; eliminates the SceneError/SceneBuildError naming collision.
- **P3c. Design the non-time-based `BarSpec`** (range/volume/tick bars) — do NOT extend `BarPeriod` with non-time variants; plan the evolution before the feature request arrives.
- **P3d. Reassess `Arc<RwLock<CandleSeries>>` vs `ArcSwap`** if fan-out crosses ~100 panels.
- **P3e. Reassess `trait Indicator`** when the third indicator lands — three concrete structs sharing the version-keyed cache pattern is the extraction signal.
