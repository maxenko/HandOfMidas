# Feature: Full Chart-Stack Transition

## Overview

Migrate every interactive chart feature from the legacy stack (`desktop/win/crates/midas-chart`, `midas-render::renderer::ChartScene`, `Camera2D`, `midas-core::CandleBuffer`, `Timeframe`, `midas-feed::TestProvider`, `HistoricalDataRegistry`) onto the session-aware new stack (root crates `midas-scene` + `midas-axis` + `midas-bars` + `midas-bars-adapter`, plus the feature-gated `midas-app::session_chart` module), flip the default backend, and delete the legacy. Users keep every feature they have today (bracket placement, decorators, levels, indicators, volume profile, crosshair, keyboard shortcuts, persisted viewport, thumbnails, link groups); developers stop maintaining two chart surfaces.

**Assumption**: the new stack's Phase A/B/C foundations are in (commits `55de9d2`, `86806fc`, `429bcbb`, `dd238f0`, `fad1878`). The session-aware chart is feature-gated today (`session_chart`) and renders crypto M1 + XNYS all periods through a standalone window via `desktop/win/crates/midas-app/src/session_chart_window.rs`. The interactive-layer machinery (tools, hit-testing, decorators, indicators) is not yet implemented.

## Research Summary

### Codebase analysis

- **Two Cargo workspaces**: root (engine + market-data + 7 new-stack crates); `desktop/win` (11 GUI crates). Cross-workspace deps flow one way — desktop depends on root.
- **Legacy chart footprint**: 20 features catalogued (see pass-2 legacy-inventory report). `Camera2D` has **107 occurrences across 28 source files** (raw grep, excluding plan archives). `CandleBuffer` ≈ 79 refs; `Timeframe` ≈ 150 refs. `ChartInput { ... }` construction literal: 4 sites. Legacy decorator subsystem is **2,319 LOC total across 8 files, of which 1,411 LOC is tests (908 LOC impl)**. Combined with `order_bracket/` (611 + 845 impl + 1,870 tests), the decorator + bracket-render port targets ~1,900 LOC impl + ~3,300 LOC tests / 130 legacy tests.
- **New-stack status**: `crates/midas-scene/src/layers/annotations.rs` already ships working `OrderBracketLayer`, `LevelLayer`, and `PriceLineLayer` (line + badge emission, no hit-test / no drag / no tool FSM). `DecoratorLayer` is a documented placeholder (annotations.rs:10–12). `CrosshairLayer` (crates/midas-scene/src/layers/crosshair.rs) renders arms but has no OHLC tooltip. `midas-bars::CandleSeries` exposes `push` + `apply` + `version()`. **`update_last_price` does NOT exist on `CandleSeries` yet** — it was landed on the legacy `CandleBuffer` in `fad1878`. Slice 2 ports it across.
- **Existing strangler seam**: `midas-app`'s `session_chart` feature + standalone `session_chart_window.rs` + toolbar buttons already switch users onto the new stack per-click. No new `ChartHost` widget is needed; the facade already exists.
- **Test baseline**: `grep -rh '#\[test\]\|#\[tokio::test\]' crates desktop/win/crates | wc -l` returns **~2,517** test functions across both workspaces (floor for non-regression gate).
- **Dev-harness**: `desktop/win/crates/midas-app/src/dev_harness/screenshot.rs::capture` uses `image_compare::rgba_hybrid_compare` (dep `image-compare = "0.4"`) which already returns `ssim` + `diff_fraction`. No new dep needed.
- **Bracket model**: `TickerMsg` (`desktop/win/crates/midas-app/src/ticker_state/apply.rs:45`) is **draft-then-save** — `EnsureDraftBracket` → `SetLegPrice` × 3 → `SaveBracket`. There is no single-shot `PlaceBracket` variant, and the tool must drive this existing message sequence (architecture rule 8).
- **`LineInstance`** (`crates/midas-scene/src/primitives.rs:45`) is a flat `{x0, y0, x1, y1, width_px, color}` record. Entry-full-width vs TP-right-only extent is expressed by choosing `x0`/`x1` at emit time, **not** via an invented extent enum.
- **`midas-indicators`**: consumes the `CandleData` trait from `midas-core`, not concrete `CandleBuffer`. Migration reduces to `impl CandleData for CandleSeries` — no indicator rewrite.

### Best practices & idiomatic approach

- **Strangler fig at the widget boundary, not the crate boundary.** The existing `session_chart` feature + per-panel backend selector is the strangler — plan keeps it, does not re-invent.
- **Primitives + z-bands, not god-scene.** TradingView `lightweight-charts` ships three zOrder bands; new stack's `LayerZ` newtype + sub-z-within-layer maps 1:1.
- **Tool state lives in the scene, not iced `advanced::widget::tree::State`.** The widget-tree cache resets on structural rebuild; a 3-click bracket placement would silently drop. `ChartScene.active_tool: Option<Box<dyn InteractiveLayer>>` is the right home (trading-vue-js, lightweight-charts-drawing).
- **Hit-testing cascades top-down; drag captures focus.** `bevy_mod_picking` pattern: scene walks layers top-down, first `Some(Hit)` wins; on `Captured`, events route to that layer until mouseup. Prevents flicker.
- **Text atlas shared.** One `TextContext` threaded through `PaintContext`, not one atlas per layer.
- **`&'static dyn`, `Arc<dyn>`, `Box<dyn>`** — three distinct lifetimes for calendars, hot-swappable backends, and owned tool state. Do not unify.
- **Parity harness with SSIM tolerance** — ≥ 0.995 SSIM + ≤ 0.002 diff-fraction to absorb AA / driver variance. Re-baseline after every slice that changes visual output.
- **Retirement in one PR after soak.** Strangler retrospectives (Shopify) call out teams that left facades forever. Set deletion date at migration start.

## Design Decisions

### D1: Strangler fig mechanism
**Context**: We need to run both stacks in parallel, per-route, during migration.
**Options**:
1. New `ChartHost` widget dispatching internally. Redundant with existing feature flag.
2. **Extend the existing `session_chart` feature + add a per-panel `ChartBackend` selector on `ChartPanel`.** Toolbar toggle + config key.
3. Feature-flag-only — all-or-nothing per build. Kills per-route parity.
**Recommendation**: 2. Matches Shopify strangler + reuses the existing in-tree seam.
**Confidence**: high.

### D2: Tool state placement
**Context**: 3-click bracket FSM must survive re-renders and tab switches.
**Recommendation**: `ChartScene.active_tool: Option<Box<dyn InteractiveLayer + Send + Sync>>`. Tool owns its FSM; scene dispatches input. TickerState stays the single source of truth for bracket mutations (rule 8) — tool commits via the existing draft-then-save `TickerMsg` sequence.
**Confidence**: high.

### D3: Input dispatch
**Recommendation**: `iced::widget::shader::Program::update(event) -> Action::Capture | Publish(Message)`. Do not wrap with `mouse_area` (breaks cursor shape).
**Confidence**: high.

### D4: Hit-testing
**Recommendation**: `Layer::hit_test(pt, price_range) -> Option<Hit>`; scene walks top-down; drag-capture bypasses. Opt-in via a separate `InteractiveLayer` trait that NOT every layer implements (no blanket impl — the blanket-impl shortcut blocks consumer crates from adding their own layer behavior per the orphan rule, flagged by critique).
**Confidence**: high.

### D5: Price axis
**Recommendation**: `PriceAxis` trait in `midas-axis` mirroring `TimeAxis`. One impl for now (`LinearPriceAxis`); log-scale deferred and is the reason for keeping the trait (not over-engineering).
**Confidence**: medium — if log-scale never lands, downgrade to a struct in a future refactor.

### D6: Indicator abstraction
**Context**: Only two indicators ship (ATR, G.ATR). Is a generic `ComputedSeriesLayer<I: Indicator>` right-sized?
**Recommendation**: **Collapse to two concrete layers** (`AtrLayer`, `GerchikAtrLayer`). Both consume `Arc<RwLock<CandleSeries>>` + cached output keyed by `version()`. Add the generic trait when indicator #5 arrives.
**Confidence**: high.

### D7: `CandleData` trait for `CandleSeries`
**Recommendation**: `impl CandleData for CandleSeries` in `midas-bars-adapter` (cross-workspace) OR expose a shim inside `desktop/win/crates/midas-core`. Indicators then consume either buffer uniformly — no rewrite.
**Confidence**: high.

### D8: Snapshot primitive
**Recommendation**: keep `Arc<parking_lot::RwLock<CandleSeries>>` (current). Scrutiny confirmed sub-µs reads; `ArcSwap` is an optimization for post-transition profiling, not this plan.
**Confidence**: high.

### D9: `LineExtent` / extent enum
**Recommendation**: **do not introduce.** Entry full-width and TP-right-only extent are expressed by choosing `x0`/`x1` at emit time on the flat `LineInstance`. Matches existing primitive vocabulary.
**Confidence**: high.

### D10: Retirement cadence
**Recommendation**: three-stage cutover. 9a adds toggle (default legacy) → soak. 9b flips default → 14-day soak. 9c deletes legacy in a single PR.
**Confidence**: high.

## Implementation Plan

15 slices (after splitting 2 → 2a/2b/2c and adding 8.5 for caller migration). Slices 3–8 are serialized when they touch shared files (`session_chart/widget.rs`, `session_chart_window.rs`, `scene_builder.rs`); unit-test work is parallel. **Realistic wall-time is 12–16 weeks with 3 developers**, not the naïve 6–8 weeks — slices 5a, 5b, 8 each carry 2–3 weeks of real port work once legacy LOC is counted honestly. See "Critical path estimate" at the end.

---

### Slice 0 — Parity harness + render-path `.expect()` audit
**Goal**: Dev-harness can render both stacks into PNGs and diff with SSIM tolerance. Render-path panics are enumerated + fixed.
**Depends on**: none.
**Files to create or modify**:
- `desktop/win/crates/midas-devloop-proto/src/lib.rs` — add command `RenderBothStacks { chart_id, fixture: String }` and reply `RenderDiff { ssim: f32, diff_fraction: f32, both_png_paths: (PathBuf, PathBuf) }`.
- `desktop/win/crates/midas-app/src/dev_harness/screenshot.rs` — reuse existing `capture` (which uses `image_compare::rgba_hybrid_compare`). Extend with `capture_both_stacks()` that renders each backend to an offscreen wgpu texture.
- `desktop/win/crates/midas-app/src/dev_harness/parity.rs` (new) — orchestrator + fixture loader.
- `desktop/win/tools/devloop-chart-parity.sh` (new) — CLI for running the fixture sweep.
- `desktop/win/tests/chart_parity_fixture.rs` (new) — five canonical fixtures: `aapl_m1_rth`, `aapl_d1`, `spy_d1_rth`, `btc_m1`, `empty_buffer`. (ES-intraday is out of scope — deferred per Non-Goals.)
- `desktop/win/crates/midas-render/` — audit the seven existing `.expect()` sites in pipelines; convert reachable ones to typed errors + `tracing::error!`. Unreachable ones gain a comment justifying.
**Key implementation details**:
- SSIM threshold ≥ 0.995 AND diff-fraction ≤ 0.002. Both must pass. Text-hinting drift alone stays within these bounds.
- **Harness self-validation**: include five known-good pairs (legacy vs legacy; must score SSIM ≈ 1.0) AND five known-bad pairs (legacy vs solid white; must score SSIM < 0.5). Harness tests fail if thresholds misclassify.
- `.expect()` audit result: a `render-expect-audit.md` artifact under the slice checked in, listing every site + its rationale or replacement.
**Testing**: 5 fixture tests + 10 harness-self-validation pairs + audit artifact. Existing tests stay green.
**Done when**:
- `cargo test --features dev_harness --test chart_parity_fixture` green.
- Self-validation corpus: all 5 good pairs SSIM ≥ 0.999, all 5 bad pairs SSIM < 0.5.
- `render-expect-audit.md` present in the PR, zero unreachable `.expect()` on the render hot path.

---

### Slice 1 — `InteractiveLayer` + tool machinery
**Goal**: `midas-scene` grows the trait, the `active_tool` slot, hit-testing cascade, drag-capture, escape-cancel, error plumbing, and panic recovery so tools can coexist with passive layers.
**Depends on**: 0.
**Files to create or modify**:
- `crates/midas-scene/src/layer.rs` — declare `pub trait InteractiveLayer: SceneLayer + Send + Sync { fn update(&mut self, ev: InputEvent, cx: &mut ToolCtx<'_>) -> EventStatus; fn hit_test(&self, pt: Point, price_range: &PriceRange) -> Option<Hit>; fn cancel(&mut self); }`. **No blanket impl** — non-interactive layers don't implement this; the scene treats absence as `Ignored`. A separate registry maps `LayerId -> Option<&mut dyn InteractiveLayer>` via a per-layer opt-in method `SceneLayer::as_interactive(&mut self) -> Option<&mut dyn InteractiveLayer> { None }` with concrete overrides.
- `crates/midas-scene/src/input.rs` (new) — `InputEvent { MouseDown { button, pt, mods }, MouseUp { button, pt }, MouseMove { pt }, Wheel { dx, dy, pt }, KeyDown { key, mods }, KeyUp { key } }`. `Hit { layer_id: LayerId, sub_z: u16, cursor: iced::mouse::Interaction }`. `EventStatus { Captured, Ignored }` — errors route through `ToolCtx::emit_error(SceneError)` rather than the return type.
- `crates/midas-scene/src/error.rs` (new) — `SceneError` (thiserror enum): `TickerRejected(String)`, `PersistenceFailed(String)`, `AxisRange(String)`, `AnnotationNotFound(AnnotationId)`, `PanicFallback`.
- `crates/midas-scene/src/scene.rs` — extend `ChartScene` with `active_tool: Option<Box<dyn InteractiveLayer>>`, `drag_focus: Option<LayerId>`, `last_error: Option<SceneError>`. Add `ChartScene::handle_input(ev) -> EventStatus`, `ChartScene::on_destroy()` (calls `active_tool.cancel()`, clears `drag_focus`). Extend `ChartSceneBuilder` (in the same file — there is no `scene_builder.rs`) with `.active_tool(...)`.
- `crates/midas-scene/src/interaction.rs` — `InteractionState` gains `drag_focus: Option<LayerId>`, `last_wheel_ts: Option<Instant>`.
- `crates/midas-scene/src/paint.rs` — wrap each `layer.paint(ctx)` call in `std::panic::catch_unwind`; on panic, emit a fallback debug-red quad at the viewport bounds + `tracing::error!(layer = ?, panic = ?, "layer paint panicked; emitting fallback")`. Scene does NOT tear down — other layers continue painting.
- Every new public type gets `tracing::debug!(target: "midas_scene::...", chart_id, …)` on construction, hit-test outcome, and FSM transition.
- Static assertions: `fn _assert_send_sync<T: Send + Sync>(){}; _assert_send_sync::<Box<dyn InteractiveLayer>>();`
- `crates/midas-scene/tests/interactive_layer.rs` (new) — 25 tests: hit-test order (top-down), drag capture round-trip, escape-cancel, on_destroy cleanup, panic in one layer doesn't kill others, `as_interactive` returns None by default, Send+Sync assertions compile.
**Testing**: 25 new unit tests; all existing `midas-scene` tests stay green.
**Done when**: `cargo test -p midas-scene` reports 25+ new tests pass; static assertions compile; `tracing` calls land at the documented sites (verified by a test that captures subscriber output).

---

### Slice 2a — `PriceAxis` + `LabelFormatter`
**Goal**: Axis plumbing + shared label formatting traits land so downstream slices have stable seams. No interaction changes yet.
**Depends on**: 1.
**Files to create or modify**:
- `crates/midas-axis/src/price.rs` (new) — `trait PriceAxis: Send + Sync { fn to_y(&self, price: f64) -> f32; fn from_y(&self, y: f32) -> Option<f64>; fn from_y_snapped(&self, y: f32, dir: SnapDirection) -> (f64, bool); fn height_px(&self) -> f32; fn range(&self) -> PriceRange; }`. `LinearPriceAxis { range: PriceRange, height: f32 }`.
- `crates/midas-axis/src/lib.rs` — re-export.
- `crates/midas-axis/src/format.rs` (new) — **narrow** initial trait: `trait LabelFormatter: Send + Sync { fn price(&self, p: f64, tick_size: f64) -> String; fn time(&self, ts: Timestamp, tz: chrono_tz::Tz, density: TickDensity) -> String; }`. `volume` + `percent` added in slice 6 when indicators need them (avoid speculative methods).
- `crates/midas-scene/src/paint.rs` — `PaintContext` gains `price_axis: &dyn PriceAxis` and `formatter: &dyn LabelFormatter`.
**Testing**: 12 tests — `to_y`/`from_y` round-trip, `from_y_snapped` directions, default formatter locale.
**Done when**: `cargo test -p midas-axis` reports 12+ new tests pass; `PaintContext` constructs with the new fields.

### Slice 2b — Pan / zoom / keyboard / auto-scale
**Goal**: Interaction on the chart: arrow-key pan, +/- zoom, wheel zoom, Home/End, Delete, Escape, auto-scale on first-data. Complete interaction surface for the new chart.
**Depends on**: 2a.
**Files to create or modify**:
- `crates/midas-scene/src/interaction.rs` — `InteractionState` pan/zoom/keyboard handlers. Zoom anchors at cursor (from_x, from_y → target point stays put). Bounds: min 10 candles visible, max 10 years. `auto_scale_price(series: &CandleSeries, visible_range: Range<usize>)` fits high/low + 5% pad.
- `desktop/win/crates/midas-app/src/session_chart/widget.rs` — wire keyboard shortcuts (Arrow = pan, +/- = zoom, Escape = cancel tool, Home = first bar, End = last bar, Delete = remove selected annotation), wheel-driven zoom. On first-ever `CandleSeries::version()` transition from 0 → non-zero AND no saved `ChartViewStore` entry, call `auto_scale_price()`.
- `desktop/win/crates/midas-app/src/session_chart/scene_builder.rs` — app-side builder module (new; not to be confused with `crates/midas-scene/src/scene.rs::ChartSceneBuilder` which lives in the root-workspace scene crate). This app-side module composes layers + axes per panel config.
**Wheel routing policy**: wheel events on an active tool default to `EventStatus::Ignored` — zoom and pan are chart-scene behaviors, tools must opt in explicitly. Documented in Slice 1 contract; enforced by a test.
**Testing**: 20 tests — pan monotonicity, zoom anchor, auto-scale edge cases (high==low, NaN guard), keyboard repeat, wheel routing (BracketTool active + wheel → zoom, tool state unchanged), empty-series no-op.
**Done when**: `cargo test -p midas-scene -p midas-app --features session_chart` reports 20+ new tests pass; opening a BTC-M1 panel pans+zooms smoothly.

### Slice 2c — Tick-cadence live path (`CandleSeries::update_last_price`)
**Goal**: New-stack equivalent of the `fad1878` watchlist↔chart sync fix. Adds the method to `CandleSeries` (it does NOT exist yet — legacy `CandleBuffer` got it, `CandleSeries` didn't), then wires `QuoteBatch` fan-out with coalescing + shared storage.
**Depends on**: 2a (for `PaintContext` dirty plumbing).
**Files to create or modify**:
- `crates/midas-bars/src/series.rs` — **NEW METHOD** `CandleSeries::update_last_price(price: f64)` mirroring `CandleBuffer::update_last_price`: extends current candle's `close = price`, `high = max(high, price)`, `low = min(low, price)`, volume/open/ts unchanged. NaN guard. Emits `tracing::debug!(symbol, price, "tick fold")`.
- `crates/midas-bars/tests/update_last_price.rs` (new) — 7 tests mirroring the `fad1878` `CandleBuffer::update_last_price` suite: non-finite rejection, high-extend, low-extend, in-range-only-close, only-last-candle-touched, empty-is-noop, version bump.
- `desktop/win/crates/midas-app/src/app/handlers.rs` — extend `Message::QuoteBatch` handler. **Key design**: per-symbol, look up the shared `Arc<RwLock<CandleSeries>>` **once** and call `update_last_price` on it **once per batch**. All session-chart panels bound to that symbol share the same `Arc`, so one write-lock per batch covers N panels. No per-panel lock-write.
- `desktop/win/crates/midas-app/src/session_chart/driver.rs` — `SessionChartDriver` registers its `Arc<RwLock<CandleSeries>>` in a per-(router, symbol) dashmap on construction; deregisters on drop. `QuoteBatch` handler reads from this map.
- `desktop/win/crates/midas-app/src/session_chart/registry.rs` (new) — `SymbolSeriesRegistry: Arc<DashMap<(RouterId, SymbolKey), Weak<RwLock<CandleSeries>>>>`. Weak handles so closed panels don't keep series alive.
- **Paint coalescing**: a panel that already has a dirty frame pending must not re-invalidate from `QuoteBatch`. `InteractionState.paint_pending: AtomicBool`; `update_last_price` sets it; the iced render loop clears it.
**Testing**: 7 CandleSeries unit tests + 6 fan-out tests (shared-Arc lookup, weak-ref cleanup on panel close, coalescing under burst, 20-panel × 100Hz stress test targeting zero dropped frames > 33ms).
**Done when**:
- `cargo test -p midas-bars -p midas-app --features session_chart -- update_last_price` reports 13+ new tests pass.
- Opening a session-chart window on BTC-M1, watchlist and chart tick timestamps advance within 50 ms of each other (instrumented via `tracing::debug!` counters).
- Stress test: 20 open panels on the same symbol × 100Hz tick burst, zero frames exceed 33 ms budget.

---

### Slice 3 — Crosshair tooltip + text atlas
**Goal**: Upgrade the existing `CrosshairLayer` with OHLC/time/price/indicator labels. Wire `TextInstance` through cryoglyph.
**Depends on**: 1, 2.
**Files to create or modify**:
- `crates/midas-scene/src/layers/crosshair.rs` — augment: at cursor, resolve candle via `CandleRef` binary search; emit 6 `TextInstance`s (time, OHLC, Δ). Uses `PaintContext.formatter` from slice 2.
- `desktop/win/crates/midas-app/src/session_chart/primitives_bridge.rs` — close the TODO'd G-2 gap: `TextInstance` → `midas-render::TextPipeline::WidgetLabel`. Shared `TextContext` held on the `SessionChartRenderer` (threaded via `&mut` through `prepare`). One atlas per window.
- `desktop/win/crates/midas-app/src/session_chart/gpu_renderer.rs` — construct `TextContext` on init.
- `desktop/win/crates/midas-render/src/pipelines/text.rs` — no signature change expected; audit `cryoglyph` dep chain (is it re-exported by iced 0.14 or a separate workspace dep? answer in the PR).
- `crates/midas-scene/tests/crosshair_labels.rs` (new) — 10 tests: empty series, cursor-over-candle snapping, label positions (right-axis, bottom-axis), multi-line OHLC box, atlas reuse.
**Key implementation details**:
- Text atlas: one `TextContext` per `SessionChartRenderer` (per-window). Layers receive `&mut TextContext` in `PaintContext`. No atlas duplication across layers — stress-test with 5 simultaneous windows.
- Price format: `formatter.price(p, tick_size)` — rounds to tick.
- Time format: follows TickDensity; exchange-local for XNYS, UTC for CryptoSpot.
**Testing**: 10 crosshair tests + 2 parity-harness fixtures (crosshair-over-candle vs over-empty).
**Done when**:
- Parity harness fixtures `crosshair_aapl_m1` and `crosshair_btc_m1` both pass SSIM ≥ 0.995 diff ≤ 0.002 against the legacy chart.
- `cargo test -p midas-scene crosshair` all green.

---

### Slice 4 — Level annotations
**Goal**: Port click-to-place / drag / lock / delete / inline-edit / snap-to-OHLC horizontal price levels end-to-end. `ToolEffect` enum designed in this slice **must** accommodate slice 5's bracket effects (explicit cross-review required before merging).
**Depends on**: 1, 2.
**Files to create or modify**:
- `crates/midas-scene/src/tools/mod.rs` (new) — `ToolEffect` enum: `CreateLevel { price, lock }`, `UpdateLevel { id, price }`, `DeleteLevel { id }`, `CreateBracket { side, entry, tp, sl }` (shape reserved for slice 5; variant unimplemented here but present), `UpdateBracketLeg { id, role, price }`, `OpenContextMenu { pt, items: Vec<ContextMenuItem> }`, `ReportError(SceneError)`. Scene drains per-frame.
- `crates/midas-scene/src/tools/level.rs` (new) — `LevelTool` FSM: `Idle | Placing { snapped_price, preview_px }`. Commits `ToolEffect::CreateLevel` on left-click. Escape cancels.
- `crates/midas-scene/src/tools/snap.rs` (new) — `fn snap_to_ohlc(cursor_price: f64, cursor_x_px: f32, candles: &[CandleRef], axis: &dyn PriceAxis, time_axis: &dyn TimeAxis, candle_width_px: f32) -> f64`. **Algorithm** (verified against `desktop/win/crates/midas-chart/src/level_tool/mod.rs:87-140`): find the nearest candle by x, then examine ±1 candles (at most 3 total); for each, compute y-distance from `cursor_price` to each of O/H/L/C via `axis.to_y`. Snap threshold is **adaptive**: `candle_width_px.clamp(SNAP_THRESHOLD_MIN_PX, SNAP_THRESHOLD_MAX_PX)` (constants `MIN = 3.0, MAX = 12.0` lifted from legacy). Iteration order inside each candle is `[open, high, low, close]` with strict `<` comparison, so ties resolve to the first encountered value (priority Open < High < Low < Close).
- `crates/midas-scene/src/layers/annotations.rs` — augment `LevelLayer`: add `hit_test` (within 4 px vertical of the line), drag-handle at right edge, lock-icon at right margin. `as_interactive()` returns `Some(self)`.
- `desktop/win/crates/midas-app/src/session_chart/level_popup.rs` (new) — iced popup for inline price edit.
- `desktop/win/crates/midas-app/src/session_chart/widget.rs` — drain `ToolEffect`s per frame, translate to app `Message`s (`Message::CreateLevel`, etc.), which call `AnnotationStore::add_level` / `update_level` / `remove_level` (existing paths, no new persistence code).
- `desktop/win/crates/midas-app/src/session_chart_window.rs` — toolbar "Add Level" button activates `LevelTool`.
- `crates/midas-scene/tests/level_tool_flow.rs` (new) — 20 tests: FSM transitions, snap algorithm (exact + tie-break), placement, drag, lock-prevents-drag, escape-cancel, right-click opens context-menu payload.
- `desktop/win/tests/level_end_to_end.rs` (new) — 4 integration tests: place → persist → restart app → level visible.
**Key implementation details**:
- Snap priority: `Open < High < Low < Close`. Rationale: legacy `level_tool::tests` pinned this order.
- Drag hit-test: 4 px vertical tolerance around the line.
- Lock icon hit-test: 16 × 16 px square at x = viewport.width - 24.
**Testing**: 20 tool tests + 4 integration tests.
**Done when**:
- Parity harness `level_seeded_fixture` SSIM ≥ 0.995 (seeded AnnotationStore with 3 levels).
- Integration tests prove place/drag/lock/edit/delete/persist round-trip.

---

### Slice 5a — DecoratorLayer full port
**Goal**: Bring `DecoratorLayer` from placeholder to full: hover-only vs always-visible sub-groups, proximity promotion, drag ghost, four sub-z bands (background < proximity-promoted < hovered < dragged). Legacy subsystem is 2,319 LOC + 1,411 LOC of tests; this slice is sized accordingly.
**Depends on**: 1, 2, 3, 4.
**Files to create or modify**:
- `crates/midas-scene/src/layers/annotations.rs` — replace `DecoratorLayer` placeholder with a real impl: `DecoratorLayer { groups: Vec<DecoratorGroup>, sub_z_bands: [Vec<DecoratorId>; 4] }`. Match legacy z interleave.
- `crates/midas-scene/src/decorator/mod.rs` (new) — `DecoratorGroup { items: Vec<DecoratorItem>, visibility: Visibility { Always | OnHover { parent: Option<AnnotationId> } } }`; `DecoratorItem { Line(LineInstance) | Badge(BadgeInstance) | Button { bounds, action } | Spacer { w, h } }`.
- `crates/midas-scene/src/decorator/layout.rs` (new) — layout algorithm: proximity promotion (within 32 px of cursor → sub_z = 1); hover promotion (group.visibility == OnHover and cursor-over-parent → emit); drag ghost (layer-driven alpha blending at sub_z = 3).
- `crates/midas-scene/src/decorator/tests.rs` (new) — 60 tests: group layout, visibility rules (proximity + hover + drag), z-band ordering, drag ghost alpha, parent-group chain.
**Key implementation details**:
- Proximity threshold: 32 px radius (legacy value from `widget::decorator::proximity`).
- Visibility combinators: hover-group children are promoted only when mouse is within the parent's bounding box AND the parent group's `Always` items are visible.
- Drag ghost: alpha = 0.5 applied layer-wide; original remains at sub_z = 0.
**Testing**: 60 decorator tests. Target matches legacy test count (≈100) scaled to essential behaviors.
**Done when**: existing `LevelLayer` (slice 4) picks up decorator badges (e.g., price label at right edge) through this layer — demonstrated by a parity-harness fixture that exercises hover rules.

---

### Slice 5b — BracketTool FSM + OrderBracketLayer hit-test + drag
**Goal**: Port the 3-click bracket placement FSM + full bracket rendering (entry full-width line, TP/SL right-from-entry draggable, amber wrong-side warning). Commits via the existing draft-then-save `TickerMsg` sequence — NOT via a new `PlaceBracket` variant.
**Depends on**: 1, 2, 3, 4 (ToolEffect shape), 5a (decorator layer).
**Files to create or modify**:
- `crates/midas-scene/src/tools/bracket.rs` (new) — `BracketTool` FSM: `Idle | AwaitingEntry { side } | AwaitingTarget { side, entry } | AwaitingStop { side, entry, target } | Complete`. Directional toggle via `KeyDown { L }` / `KeyDown { S }` before entry click. Preview line at cursor Y for the pending leg. Escape cancels at any stage, emits `ToolEffect::CancelBracket` (maps to `TickerMsg::CancelBracket`).
- Commit sequence (architecture rule 8): on third click, the tool does NOT emit a single "place" effect. It emits `ToolEffect::BeginDraftBracket { side, entry }` → `ToolEffect::SetDraftLeg { role: Tp, price }` → `ToolEffect::SetDraftLeg { role: Sl, price }` → `ToolEffect::CommitDraftBracket`. Each effect maps to an existing `TickerMsg`: `EnsureDraftBracket` / `SetLegPrice` / `SaveBracket`. No new `TickerMsg` variant is introduced.
- `crates/midas-scene/src/layers/annotations.rs` — `OrderBracketLayer`:
  - Entry line: `LineInstance { x0: 0, x1: viewport.width, ... }` — full-width expressed via x-span, no extent enum.
  - TP/SL line: `LineInstance { x0: axis.to_x(entry_ts), x1: viewport.width, ... }` — right-from entry ts.
  - `hit_test`: drag handle within 4 px of `x1` on TP/SL lines; cursor → `mouse::Interaction::Grab`.
  - Amber wrong-side: if `side == Long && tp < entry` OR `sl > entry` (mirrored for Short); tint + "!" badge via DecoratorLayer (slice 5a) at sub_z = 4.
- `crates/midas-scene/src/tools/bracket_status.rs` (new) — `fn is_leg_on_wrong_side(side: Side, entry: f64, leg_price: f64, leg_kind: LegKind) -> bool`. **Algorithm** (verified against `desktop/win/crates/midas-chart/src/widget/order_bracket/mod.rs:246-252` — plan's draft reversed the equality): `match (side, leg_kind) { (Long, Tp) => leg_price <= entry, (Long, Sl) => leg_price >= entry, (Short, Tp) => leg_price >= entry, (Short, Sl) => leg_price <= entry }`. **Equal price IS wrong-side** (inclusive `<=` / `>=`). Preserves legacy muscle-memory.
- `desktop/win/crates/midas-app/src/session_chart/widget.rs` — translate `ToolEffect` bracket variants to `TickerMsg`s via `ticker_state.apply()`. Never mutate bracket state outside `apply` (rule 8).
- `desktop/win/crates/midas-app/src/session_chart_window.rs` — toolbar "Buy Bracket" / "Sell Bracket" buttons activate the tool.
- `crates/midas-scene/tests/bracket_tool_fsm.rs` (new) — 40 FSM tests (every transition × every input class + directional toggle before entry + escape at each state) + 8 wrong-side classifier tests + 12 layer-render tests. Total: 60.
- `desktop/win/tests/bracket_tool_integration.rs` (new) — 6 end-to-end tests using the devloop harness: activate tool, click 3 times, assert TickerState has a committed bracket; assert persistence round-trips; assert drag moves TP/SL only; assert lock prevents TP/SL drag; assert amber fires on wrong-side drag; assert mid-placement window-close cancels the draft.
- `crates/midas-ib-sim/fuzz/fuzz_targets/` (new) — `bracket_tool_fsm.rs` fuzz target using `proptest`-style arbitrary click sequences; must never panic, must always leave tool in a valid state.
**Key implementation details**:
- Drag handle hit-test: 4 px radius in SCREEN space (not DPI-scaled) — legacy convention.
- Window-close mid-drag: `ChartScene::on_destroy()` (slice 1) calls `active_tool.cancel()` → emits `ToolEffect::CancelDraftBracket` → `TickerMsg::CancelBracket`. No orphan drafts.
**Testing**: 60 unit + 6 integration + fuzz harness runs.
**Done when**:
- `cargo test -p midas-scene bracket` all green.
- Fuzz target `cargo +nightly fuzz run bracket_tool_fsm -- -max_total_time=60` completes without panics or invalid states.
- Parity harness `bracket_seeded` SSIM ≥ 0.995 against a seeded AnnotationStore.
- Integration test proves mid-placement window-close leaves zero draft brackets in TickerState.

---

### Slice 6 — Indicators (ATR + G.ATR)
**Goal**: Port ATR band overlay + G.ATR badge + bright-range highlighting via two concrete `*Layer` structs. Implement `CandleData for CandleSeries` so `midas-indicators` compute functions work unchanged.
**Depends on**: 1, 2. Parallel with 3/4/5.
**Files to create or modify**:
- `crates/midas-bars-adapter/src/candle_data_impl.rs` (new) — `impl midas_core::CandleData for midas_bars::CandleSeries`. Cross-workspace bridge. (Or land in a new `midas-bars-core-bridge` crate if dep inversion forbids — verified during implementation.)
- `crates/midas-scene/src/layers/indicator.rs` (new) — two concrete structs:
  - `AtrLayer { series: Arc<RwLock<CandleSeries>>, period: usize, cache: Mutex<AtrCache> }`. Cache keyed on `series.version()`. Emits band as 3 line series (upper/mid/lower).
  - `GerchikAtrLayer { series: Arc<RwLock<CandleSeries>>, cache: Mutex<GatrCache> }`. Emits badge text via `LabelFormatter::percent` + a `Vec<usize>` of bright-highlight indices, delivered to `CandleLayer` via a `HoverIndices` channel (see below).
- `crates/midas-scene/src/layer.rs` — add `pub const INDICATOR: LayerZ = LayerZ(450);` slot (between Candle = 400 and HolidayMarker = 500, per `layer.rs` doc-comment example).
- `crates/midas-scene/src/layers/annotations.rs::CandleLayer` — accept an optional `bright_indices: Arc<RwLock<Vec<usize>>>` shared with `GerchikAtrLayer`. When an index is in the set, apply `CandleStyle.bright_multiplier`.
- `desktop/win/crates/midas-app/src/session_chart/scene_builder.rs` — wire indicator layers when `SceneLayers.atr` / `.gatr` flags are set.
- `desktop/win/crates/midas-app/src/session_chart_window.rs` — "ATR" / "G.ATR" toolbar toggle chips.
- `crates/midas-scene/tests/indicator_layer.rs` (new) — 15 tests: cache invalidates on `version()` bump; empty series is no-op; bright indices flow to `CandleLayer`; ATR band math matches legacy within 1 f32 ULP.
**Key implementation details**:
- Indicator compute is synchronous and <1 ms for 5K bars; no async needed.
- Cache invalidation: `AtomicU64::load == cached_version` skip; else recompute under mutex.
- Precision: both stacks store OHLC as `f32`; indicator f64 arithmetic internal, output f32. Expect bit-exact parity vs legacy; any drift is a bug.
**Testing**: 15 indicator tests + 2 parity-harness fixtures (`atr_aapl_d1`, `gatr_aapl_m1`).
**Done when**: parity fixtures SSIM ≥ 0.999 (indicators must be bit-exact across stacks). Toolbar chips toggle layers.

---

### Slice 7 — Volume profile
**Goal**: Port the horizontal histogram volume-profile overlay.
**Depends on**: 1, 2.
**Files to create or modify**:
- `crates/midas-scene/src/layers/volume_profile.rs` (new) — `VolumeProfileLayer { series, price_bins: u16, visible_range: Range<usize> }`. Emits `QuadInstance` per bin; POC (highest-volume bin) tinted brighter via `QuadInstance.color` delta.
- **Algorithm** (ported from `desktop/win/crates/midas-chart/src/volume_profile/` + `widget/compute/mod.rs:126`): bin count is **viewport-driven and adaptive** — `((viewport_height * 0.8) / 3.0).clamp(20, 200) as usize` (NOT a fixed 40). Bin price range = min-low to max-high of visible candles; for each candle, distribute volume uniformly across the bins intersected by [low, high]; POC = bin with max total.
- `crates/midas-scene/src/layer.rs` — `pub const VOLUME_PROFILE: LayerZ = LayerZ(350);` (between Volume = 300 and Candle = 400).
- `desktop/win/crates/midas-app/src/session_chart/scene_builder.rs` — wire when enabled.
- `desktop/win/crates/midas-app/src/session_chart_window.rs` — "VP" toolbar chip.
- `crates/midas-scene/tests/volume_profile.rs` (new) — 12 tests.
**Testing**: 12 unit + 2 parity fixtures.
**Done when**: parity fixtures pass; bins match legacy ±1 bin (boundary rounding).

---

### Slice 8 — Persistence, dev-harness projection, thumbnails, link groups (after Slice 4)
**Goal**: Every non-rendering integration point works with the new stack. `ChartViewStore` keys migrate. `DumpState` covers new state. Thumbnails render from `CandleSeries`. Link groups propagate symbol/period with the correct reset + auto-scale dance.
**Depends on**: 1, 2, 4. Parallel with 5/6/7.
**Files to create or modify**:
- `desktop/win/crates/midas-app/src/chart_view.rs` — dual schema: write both `(symbol, Timeframe)` AND `(symbol, CalendarId, BarPeriod)` during the transition window; reader prefers v2. Config key `chart_view_store_schema = 2` set on first write. Collapsed-gaps config maps: `collapse_gaps = true` → `SessionedTimeAxis(calendar)`; `false` → `ContinuousTimeAxis`.
- `desktop/win/crates/midas-app/src/dev_harness/dump.rs` — extend `SessionChartProjection` with `axis_range`, `price_range`, `viewport_size`, `active_tool: Option<ToolKind>`, `annotation_ids: Vec<AnnotationId>`, `drag_focus: Option<LayerId>`. Derive via `serde` where possible.
- `desktop/win/crates/midas-app/src/thumbnail_data.rs` — `ThumbnailDataStore` gains an alternate source accepting `Arc<RwLock<CandleSeries>>`. Closes array reads from `CandleSeries` via `CandleRef`.
- `desktop/win/crates/midas-app/src/link.rs` — 4-step link-propagation checklist enforced: (1) drop old `SubscriptionHandle`, (2) acquire new via router, (3) reset `InteractionState`, (4) trigger auto-scale. Test: bright-red regression if any step skipped.
- `desktop/win/crates/midas-app/src/app/fixture.rs` — fixture schema v2 (`schema: 2`). Backward-compat: `schema: 1` fixtures translate forward on load, save as v2. Property test: arbitrary `InteractionState` round-trips byte-identical.
- `desktop/win/tests/app_sim_e2e.rs` — update projection shape; `#[ignore]` guard stays.
- `desktop/win/crates/midas-app/tests/chart_view_migration.rs` (new) — 8 migration round-trip tests including a reverted-binary scenario (reverted code reads the v1 key we kept writing).
**Rollback story**:
- Dual-write of v1 and v2 means reverting code during the migration window reads the still-maintained v1 entries. Dropping v1 writes only happens in slice 9c.
**Testing**: 8 migration tests + 4 fixture tests + 5 link-group tests + 3 DumpState tests + 6 thumbnail tests = 26 new tests.
**Done when**:
- `dev-harness DumpState` returns the full projection; `app_sim_e2e` passes with session_chart enabled.
- Fixture v1 files round-trip to v2.
- Reverted-binary test proves rollback is safe.
- Link-group 4-step checklist enforced by test.

---

### Slice 8.5 — Legacy-caller migration (owner slice for the 9c grep gate)
**Goal**: Every caller of `TestProvider`, `HistoricalDataRegistry`, `Camera2D`, `CandleBuffer`, and `midas_chart::` from `midas-app` is migrated off the legacy type. Without this slice, 9c's pre-deletion grep gate is unsatisfiable.
**Depends on**: 4, 5b, 6, 7, 8 (every feature port complete — callers now have new-stack equivalents to use).
**Files to modify** (list derived from grep of current HEAD):
- `desktop/win/crates/midas-app/src/app.rs` — 11 `HistoricalDataRegistry` + `active_data_provider` call sites: re-route through `midas-bars-adapter::SymbolResolver` + router.
- `desktop/win/crates/midas-app/src/app/persistence.rs` — config load/save references to `active_data_provider`.
- `desktop/win/crates/midas-app/src/thumbnail_data.rs` — switch closes source from `CandleBuffer` to `CandleSeries` via slice 8's adapter.
- `desktop/win/crates/midas-app/src/chart_widget.rs` — remains untouched (it's the legacy widget, deleted in 9c).
- `desktop/win/crates/midas-app/src/app/handlers.rs` + `views.rs` + `ticker_wiring.rs` — any remaining `midas_chart::` or `Camera2D::` imports on paths reachable from `backend: New` panels.
**Key implementation details**:
- Mechanical rewrite work: for each call site, replace the legacy type with its new-stack counterpart. Most sites have a mechanical translation; a few require a small adapter (e.g., `CandleData` shim for `CandleSeries` per D7).
- Cross-workspace `impl CandleData for CandleSeries` lives in `midas-core` (desktop side) since the orphan rule forbids the impl in `midas-bars-adapter`. Creates a `midas-core → midas-bars` Cargo path dep across workspaces — acceptable, flagged in slice.
**Testing**: grep gate matches the 9c pre-deletion checklist — zero references to any of the legacy types from `midas-app`. Full workspace `cargo test --workspace` green.
**Done when**: all grep gates in 9c return zero; `cargo clippy --workspace --all-targets --all-features -- -D warnings` clean.

---

### Slice 9a — Per-panel backend toggle
**Goal**: Users can flip a specific `ChartPanel` between legacy and new backends via a toolbar toggle. Default stays legacy.
**Depends on**: 3, 4, 5a, 5b, 6, 7, 8.
**Files to create or modify**:
- `desktop/win/crates/midas-app/src/app.rs` — `ChartPanel.backend: ChartBackend { Legacy, New }` default `Legacy`. Persisted in config. `Message::ToggleChartBackend(ChartId)`.
- `desktop/win/crates/midas-app/src/app/views.rs` — chart-panel render dispatch reads `panel.backend`; legacy path unchanged, new path routes through existing `session_chart` module. Toolbar chip "New Chart" toggles.
- `desktop/win/crates/midas-core/src/config.rs` — `ChartConfig.backend: Option<ChartBackend>` (default `None` → legacy).
**State handoff on backend switch** — a panel with:
  - **Active bracket DRAFT**: auto-cancel via `TickerMsg::CancelBracket` before swap.
  - **Active LIVE bracket** (entry filled, TP/SL resting at broker): preserved unchanged in `TickerState`; new `OrderBracketLayer` constructor reads `TickerState.live_bracket` on scene-rebuild and seeds its legs from it. Broker-side state is untouched — only rendering changes.
  - **Partially-filled bracket**: `OrderBracketLayer` renders `live_bracket.filled_qty` distinctly from unfilled qty (styled per legacy — typically a brighter entry-line color or a fill-percentage badge). Slice 5b already specifies the layer reads `TickerState.live_bracket`; 9a adds the partial-fill projection into the layer config.
  - **Active tool** (any): `ChartScene::on_destroy()` + scene rebuild cancels; user re-activates on the new backend.
  - **Saved viewport**: `ChartViewStore` entry (slice 8) already keyed by `(symbol, calendar_id, period)` → survives backend switch.
**Feature gate × config mismatch** — `ChartBackend::New` always deserializes (enum parse is feature-independent), but when the binary is built without `--features session_chart` and a config selects `backend: "New"`, the dispatch in `app/views.rs` falls back to Legacy with `tracing::warn!("config selects New backend but build lacks session_chart feature; falling back to Legacy")`. No panic, no silent drop. Four-cell matrix test covers `{feature on/off} × {config New/Legacy}`.
**Testing**: 18 tests: toggle round-trip, persisted selection restores, cancel-active-draft on switch, live-bracket seeds layer legs after switch, partial-fill renders with distinct styling, dual-panel rendering (one legacy + one new simultaneously) doesn't crash under 14 ms frame budget (perf regression gate from R14), feature-gate × config matrix (4 cases), multi-window drag isolation (2 session-chart windows, drag in A does not alter B's `drag_focus`).
**Done when**: toolbar chip works; both backends render cleanly for the same (symbol, timeframe); active-draft canceled on switch; live bracket + partial fill preserved; feature-gate × config matrix all four cases pass.

---

### Slice 9b — Default flip + soak
**Goal**: Flip the default `ChartBackend` to `New`. Legacy is still the fallback. Soak for 14 days of real use; every regression becomes a blocker.
**Depends on**: 9a.
**Files to create or modify**:
- `desktop/win/crates/midas-core/src/config.rs` — `ChartBackend::default() -> ChartBackend::New`. Deprecation `tracing::warn!` on legacy selection.
- `desktop/win/tools/devloop-chart-parity.sh` — `--soak-once` runs full fixture sweep + 20 synthetic-interaction smoke scripts; report any SSIM failures.
- `plan/chart-transition/soak-log.md` (new) — dated entries of any issues reported during soak.
**Soak-log entry template** (enforced by a pre-commit check on the file):
```
## YYYY-MM-DD — <one-line symptom>
- Reproducer: <exact steps or fixture name>
- Expected vs observed: <legacy behavior vs new behavior>
- Fixture candidate: <name to add to parity sweep>
- Severity: BLOCKER | MAJOR | MINOR
- Owner slice: <which implementation slice is responsible>
- Status: open | fixed-in-<commit> | wontfix | rolled-back
```
**Automatic rollback trigger**: **2 or more BLOCKER entries within any 7-day window triggers automatic 9b revert** (flip default back to Legacy). Re-flip requires the two BLOCKERs be closed + 7 clean days. Rollback is a 1-line config change + release.
**Testing**: full-workspace + parity sweep must pass daily for 14 days. Each regression reopens the responsible slice.
**Done when**: 14 days elapsed with zero open BLOCKER entries. Soak log reviewed; each entry resolved or wontfix'd.
**Rollback signal**: any BLOCKER → immediate revert of the default flip. Trivial because no data migration happens in 9b.

---

### Slice 9c — Legacy retirement (single deletion PR)
**Goal**: Delete legacy chart stack. `session_chart` feature gate collapses.
**Depends on**: 9b (soak complete).
**Pre-deletion checklist** (enforced in PR):
- `grep -rn "midas_feed::TestProvider" desktop/win/crates` returns zero.
- `grep -rn "HistoricalDataRegistry" desktop/win/crates` returns zero.
- `grep -rn "midas_chart::" desktop/win/crates/midas-app` returns zero.
- `grep -rn "Camera2D" desktop/win/crates/midas-app` returns zero.
- `grep -rn "CandleBuffer" desktop/win/crates/midas-app` returns zero (all consumers now on `CandleSeries`).
- `cargo clippy --workspace --all-targets --all-features -- -D warnings` clean.
**Files to delete**:
- `desktop/win/crates/midas-chart/` — entire crate.
- `desktop/win/crates/midas-core/src/candle_buffer/` — module.
- `desktop/win/crates/midas-core/src/timeframe/` — module (re-export `BarPeriod` at the old path if any holdout exists, then delete the re-export).
- `desktop/win/crates/midas-feed/src/test_provider.rs` — file.
- `desktop/win/crates/midas-app/src/registry.rs` — entire file.
- `desktop/win/crates/midas-app/src/chart_widget.rs` — entire file.
- All tests for the above.
- `desktop/win/Cargo.toml` — remove `midas-chart` from members.
- `desktop/win/crates/midas-app/Cargo.toml` — remove the `session_chart` feature gate and its optional deps (feature is unconditional now).
**Config compat**: `active_data_provider` is a runtime-registry value, not a persisted `AppConfig` field (verified via `grep` on config schema + `registry.rs:66`). No config migration needed for that specific key. The general safeguard that survives: `AppConfig` deserialization is NOT strict (`deny_unknown_fields` is absent); unknown keys are silently dropped by serde. **If any slice-9c code-path introduces a new persisted chart-related field that later gets removed**, that field must land with `#[serde(default)]` and the removal follows the same pattern — log a `tracing::warn!` on the first encounter in `AppConfig::load`.
**Files to modify**:
- `CLAUDE.md` and `desktop/win/CLAUDE.md` — remove legacy references; update crate list.
- `plan/chart-transition/soak-log.md` → append final retirement entry.
- `plan/session-aware-charts/README.md` — mark Phase D complete.
- `plan/chart-transition/00-index.md` → this file; add post-retirement notes.
**Testing**: full-workspace green + post-deletion clippy clean. Parity harness retired (legacy gone).
**Rollback story**: `git revert` of the deletion PR. Because slices 9a + 9b leave the codebase in a state where legacy is a still-working alternative, revert is mechanical. The only unrecoverable step is removal of the dual-write of `ChartViewStore` v1 keys — done only in 9c, and only after 9b's soak proves new is stable.
**Done when**: all deletion targets removed; clippy + fmt + tests green; `active_data_provider` config migration path exercised by a test; `CLAUDE.md` updated.

---

### Dependency summary

```
              ┌─ 3 (Crosshair) ──┐
              │                   │
0 → 1 → 2a→2b→2c                  │
              ├─ 4 (Levels) ──────┼─ 5a (Decorator) ─── 5b (Bracket)
              │                   │
              ├─ 6 (Indicators)   │
              ├─ 7 (Volume Prof)  │
              │                   │
              └─ 8 (Persistence, after slice 4)
                                  │
                                  └─ 8.5 (Caller migration) → 9a → 9b (14d soak) → 9c (delete)
```

**Critical path**: 0 → 1 → 2a → 2b → 4 → 5a → 5b → 8.5 → 9a → 9b → 9c.
**Shared-file serialization**: slices 3–8 land sequentially (rebase on merge) because `session_chart/widget.rs`, `session_chart/scene_builder.rs`, and `session_chart_window.rs` are touched by multiple slices. Unit-test work is parallel.

### Realistic schedule (engineer-days per slice, mid-of-range)

| Slice | Eng-days | Notes |
|---|---:|---|
| 0 | 6 | Offscreen wgpu readback + self-validation corpus |
| 1 | 6 | Six concerns; `catch_unwind` + `Send + Sync` composition |
| 2a | 4 | Axis + formatter traits only |
| 2b | 7 | Pan/zoom/keyboard/auto-scale (legacy interaction is 1,822 impl + 77 tests) |
| 2c | 4 | `update_last_price` + coalesced fan-out + shared-Arc registry |
| 3 | 6 | Crosshair + text atlas; R2 spike included |
| 4 | 7 | Level tool + snap + popup + ToolEffect shape |
| 5a | 15 | Decorator port (legacy 908 impl + 1,411 test LOC; 60 new tests is a 54% coverage cut — see R18) |
| 5b | 12 | Bracket FSM + layer + fuzz; legacy 1,081 impl + 2,395 test LOC |
| 6 | 5 | Indicators + `impl CandleData for CandleSeries` |
| 7 | 4 | Volume profile |
| 8 | 12 | Dual-schema CVS + fixture v2 + DumpState + thumbnails + link groups (plan admits it bundles 4 sub-projects) |
| 8.5 | 5 | Legacy-caller rewrite sweep |
| 9a | 4 | Toggle + 18 tests |
| 9b | 14 wall | Soak; ~1 eng-day active |
| 9c | 5 | Deletion PR + clippy fallout + doc updates |
| **Total critical path** | **~82 eng-days + 14 day soak = ~16 weeks** with one developer on the critical path. Off-critical-path work (slices 3, 6, 7) consumes ~15 eng-days on a second developer. 3 developers realistic total: **12–16 weeks to 9c merge**. |

## Risks & Unknowns

- **R1 — Camera2D ripple**. 107 call sites across 28 files (not 255). Adapter pattern (new stack on `TimeAxis + PriceAxis`; legacy keeps `Camera2D`) keeps both alive until 9c. Risk: MEDIUM.
- **R2 — Text atlas reuse in wgpu 27**. Cryoglyph atlas is per-pipeline today. If sharing across layers requires pipeline refactor, slice 3 estimate grows. Spike first; if non-trivial, split slice 3.
- **R3 — Bracket FSM edge cases**. Mitigation: port legacy `bracket_tool::tests` verbatim (60+) + proptest fuzz harness in slice 5b.
- **R4 — Soak surprise regressions**. 9a toggle + per-panel selection means users can flip back without blocking anyone. 9b default flip delays deletion; any regression reopens the responsible slice.
- **R5 — Parity harness AA flap**. SSIM ≥ 0.995 + diff-fraction ≤ 0.002. Slice 0 ships with a 10-pair self-validation corpus that locks the thresholds.
- **R6 — ChartViewStore key migration**. Dual-write v1 + v2 during transition; revert is safe until 9c drops v1 writes. Reverted-binary regression test covers.
- **R7 — AnnotationStore format untouched** (D9). New layers read the existing JSON/redb via adapters. Round-trip test in slice 4.
- **R8 — `midas-indicators`**. Consumes `CandleData` trait (not `CandleBuffer`). `impl CandleData for CandleSeries` is the entire port — no indicator rewrite.
- **R9 — Thumbnail drift**. Sparkline pipeline independent of main chart; share `ThemePalette` to prevent color drift across watchlist and chart.
- **R10 — Live-trading guard**. Untouched by this plan (lives in `midas-broker::config`). 9c smoke-tests it explicitly.
- **R11 — Mid-placement bracket on backend switch**. Slice 9a cancels active drafts before switching. Test proves zero orphan drafts in TickerState.
- **R12 — Parity harness own bugs**. Slice 0 includes a 10-pair known-good/known-bad self-validation corpus.
- **R13 — Post-deletion clippy flip**. Removing types flips `dead_code`, `unused_imports`, and `cfg` warnings. 9c PR requires `--all-features` clippy clean.
- **R14 — Dual-render perf cliff during 9a**. Two panels rendering simultaneously (one legacy + one new) must stay under 14 ms. Performance regression test in slice 9a.
- **R15 — `tracing` parity loss at cutover**. Each new layer + tool emits `tracing::debug!` at construction, hit-test outcome, FSM transition, commit, cancel. Enforced by slice 1 as a cross-cutting requirement + a subscriber-capture test.
- **R16 — Render-panic handling**. Slice 1 wraps `layer.paint` in `catch_unwind`; panicking layer emits a debug-red fallback quad + `tracing::error!`; other layers keep painting. Test covers.
- **R17 — Colorblind / accessibility**. Bracket amber + bull/bear red/green are known colorblind concerns. Non-goal for this plan; tracked in a future accessibility-pass plan.
- **R18 — Slice 5a explicit coverage cut**. Legacy decorator + order_bracket have ~130 tests; slice 5a budgets 60 (54% cut). This is a deliberate trade-off to keep the slice to 15 eng-days rather than 25. Mitigation: the 60 tests cover all known legacy bug-fix regressions (port those first) plus proximity/hover/drag axes; the remaining 70 legacy tests are primarily redundant permutations and will be regenerated as parity-harness fixtures when a regression surfaces.
- **R19 — Fuzz CI placement**. Per-PR `cargo-fuzz` for `bracket_tool_fsm` has a cold-install cost (~2 min). Slice 5b's 60-second gate moves to the existing `sim_fuzz_nightly` job (consistent with `decode_incoming`), not per-PR. Per-PR gate is the unit + integration suite; fuzz is a nightly regression net.
- **R20 — Wheel routing policy**. Tools default to `EventStatus::Ignored` on wheel events so chart zoom/pan survives tool activation. Documented on the `InteractiveLayer` trait; enforced by a test (slice 2b).
- **R21 — Two-faucet (tick + RT-bar) not unified**. This plan wires `QuoteBatch` cadence onto `CandleSeries`; it does NOT unify the router's tick-broadcast + RT-bar-broadcast channels (a `midas-market-data` concern). Resulting drift between channels stays as-is; a follow-up plan owns full unification.

## Testing Strategy

- **Unit tests per slice**: floor 25 (slice 1); slice 4 ≥ 24; slice 5a ≥ 60; slice 5b ≥ 60 + fuzz; slice 6 ≥ 15; slice 7 ≥ 12; slice 8 ≥ 26. Total new unit tests: 220+.
- **Parity harness**: fixture sweep green (SSIM ≥ 0.995, diff ≤ 0.002) gates every slice merge. Baselines recaptured after slices 3, 4, 5a, 5b, 6, 7 (when visible output changes).
- **Harness self-validation corpus**: 5 known-good + 5 known-bad pairs; must classify correctly.
- **Integration tests**: `desktop/win/tests/` — level-place-drag-persist, bracket-3-click-commit-persist, backend-toggle-mid-draft, mid-placement-window-close, link-group-propagation.
- **Fuzz harness**: `crates/midas-ib-sim/fuzz/fuzz_targets/bracket_tool_fsm.rs` — proptest-style arbitrary click sequences; ≥60s no-panic run as CI gate for slice 5b.
- **Regression floor**: `cargo test --workspace` on both workspaces must pass. Baseline ~2,517 tests (measured via `grep -rh '#\[test\]\|#\[tokio::test\]' crates desktop/win/crates | wc -l`).
- **Clippy + fmt**: `-D warnings` gate on both workspaces, under every feature combination (`[]`, `[session_chart]`, `[dev_harness]`, `[session_chart, dev_harness]`).
- **Dev-harness scripts**: `devloop-smoke.sh`, `devloop-orders-journey.sh` re-validated after 9a. `devloop-chart-parity.sh` added.
- **`tracing` subscriber capture**: per-slice test that captures debug events and asserts a minimum set of `tracing::debug!` calls fired at the documented sites.

## Non-Goals / Out of Scope

- Log-scale price axis (LinearPriceAxis only).
- Multi-pane indicators (RSI below chart in a separate pane).
- Index-based time axis (trading-vue-js style).
- Futures ETH/RTH calendars (CME Globex).
- Forex regional overlays (Tokyo/London/NY).
- User-configurable session DSL.
- Drawing tools beyond Level and Bracket (trendlines, Fib, measure, polylines).
- `AnnotationStore` persistence-format migration (kept as-is per D9).
- Remote-session replay over network.
- Colorblind / accessibility palette (future plan).
- `ArcSwap` snapshot primitive (post-transition optimization).
- ES-intraday parity fixture (pending XCME calendar — future plan).
- Bracket history rendering (filled/cancelled ghosting).

## Review Notes

**Post-critique corrections (draft → final):**
- `image_compare::rgba_hybrid_compare` (not `ssim_simple`); `ChartSceneBuilder` lives in `scene.rs`; `LineInstance` is flat with no extent enum; bracket commit routes through the existing draft-then-save `TickerMsg` sequence (`EnsureDraftBracket` → `SetLegPrice` × 3 → `SaveBracket`); the new-stack `CandleSeries` ships `push` + `apply` + `version()` but NOT `update_last_price` — that is new work in slice 2c.
- Indicator trait + generic collapsed to two concrete layers (D6).
- Strangler facade is the existing `session_chart` feature + `session_chart_window.rs` — slice 9a adds a per-panel backend field; no new `ChartHost` widget.
- Slice 5 → 5a + 5b; slice 9 → 9a + 9b + 9c.
- Camera2D count 107 across 28 files (draft said 255). Test baseline 2,517.

**Post-plan-eval corrections (this pass):**
- **C1**: `CandleSeries::update_last_price` does not exist yet. Slice 2c lists the method as new work, not verification.
- **C2**: Wrong-side classifier equality flipped to inclusive `<=` / `>=` (equal price IS wrong-side) to match `desktop/win/crates/midas-chart/src/widget/order_bracket/mod.rs:246-252`. The draft had it inverted.
- **C3/C4/C5**: Volume-profile bin count is viewport-adaptive (20..200), not fixed at 40. OHLC snap window is ±1 candle (3 max), not 50. Snap threshold is adaptive to candle width, not fixed 5px.
- **C6**: Decorator LOC is 2,319 TOTAL including 1,411 tests (908 impl) — not 2,319 + 1,411.
- **Slice 2 split** into 2a (axis + formatter traits) / 2b (pan/zoom/keyboard/auto-scale) / 2c (tick-cadence fan-out). Six concerns in one slice was a schedule trap.
- **Slice 2c fan-out design**: per-symbol shared `Arc<RwLock<CandleSeries>>` registry with Weak handles; one write-lock per `QuoteBatch`, not per-panel. Paint coalescing via `AtomicBool`. 20-panel × 100Hz stress test gate. (Scenario 3.)
- **Slice 9a live-bracket handoff**: layer seeds from `TickerState.live_bracket` on scene rebuild; partial-fill `filled_qty` rendered distinctly. Broker-side order state untouched. (Scenarios 1, 5.)
- **Slice 9a feature-gate × config matrix**: `backend: "New"` config under a build without `--features session_chart` falls back to Legacy with a `tracing::warn!`, not panic. 4-cell matrix test. (Scenario 9.)
- **Slice 9c config migration corrected**: `active_data_provider` is runtime-only (not persisted), so no `AppConfig::migrate` is needed for it. General safeguard: `AppConfig` deserialization is already non-strict; `#[serde(default)]` on any new field. (Scenario 11.)
- **Slice 8.5 added** to own the legacy-caller rewrite sweep so 9c's grep gate is satisfiable. Previously this work was implicit and unassigned.
- **R18–R21 added**: slice 5a coverage cut, fuzz CI placement (nightly, not per-PR), wheel routing policy, two-faucet non-unification.
- **Soak triage formalized** in 9b: log-entry template + automatic-rollback trigger (2+ BLOCKERs in 7 days). (Scenario 10.)
- **Schedule corrected** from 6-8 weeks to 12-16 weeks realistic. Per-slice engineer-day estimates added. Off by ~2× was the biggest risk the original plan hid.

**Known small nits accepted**:
- `LabelFormatter` trait initially ships `price` + `time` only; `volume` + `percent` added in slice 6 when callers materialize (avoids speculative methods).
- `scene_builder.rs` name appears in two places: `crates/midas-scene/src/scene.rs::ChartSceneBuilder` (root workspace, inside `scene.rs`) and `desktop/win/crates/midas-app/src/session_chart/scene_builder.rs` (desktop workspace, new file landing in slice 2b). These are distinct artifacts in distinct crates.

Run `/plan-execute plan/chart-transition/00-index.md` to start slice 0.
