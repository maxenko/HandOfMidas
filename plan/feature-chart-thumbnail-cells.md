# Feature: Chart Thumbnail Grid Cells

## Overview

Add a compact chart thumbnail that can be embedded in `midas-grid` cells (watchlist, order blotter, future grids). Each thumbnail shows the last N closes for a ticker as a **mountain (area fill)** — never candlesticks, because the cell is too small (~100×24 px). Clicking the thumbnail cycles its interval `1m → 5m → 1d → 1m`. A compact label ("1m" / "5m" / "1d") renders inside the thumbnail so the user always knows which interval is shown.

Interval preference is **per-ticker, session-scoped** (alignment with the existing `ChartViewStore` pattern). Persistence across restarts is out of scope for v1.

## Research Summary

### Codebase Analysis

- **Grid integration is clean.** `midas-grid::GridColumn::cell()` (`desktop/win/crates/midas-grid/src/column.rs:92`) returns `Element<'a, M>` — any iced widget, including `iced::widget::shader::Shader`. `grid_body_cell()` applies `clip(true)` so overflow is contained. The hand-built order blotter (per commit `f4ffb9b`) uses per-cell helpers that accept pre-built elements, so the same thumbnail widget drops into both grids.
- **No line/mountain pipeline exists today.** `desktop/win/crates/midas-render/src/pipelines/mod.rs` declares `badge`, `candle`, `grid`, `text`, `volume` — nothing for polylines. `grid.wgsl` is chart-background grid lines, not arbitrary closes. A new minimal pipeline is required.
- **Shader widget pattern is established.** `desktop/win/crates/midas-app/src/chart_widget.rs:1-150` shows how iced 0.14's `shader::Program` + `shader::Primitive` bridges to a wgpu renderer. `ChartRenderer::new(device, queue, format)` (`midas-render/src/renderer.rs:93`) owns all pipelines; it is constructed app-side with the iced-supplied device. Thumbnails follow the same pattern with a far smaller snapshot.
- **Data layer is ready.** `CandleBuffer` (SoA, `desktop/win/crates/midas-core/src/candle_buffer/mod.rs:94-138`) exposes zero-copy `slice(Range)` and SIMD `price_range(Range)` for Y auto-scale. `Timeframe` (`midas-core/src/timeframe/mod.rs:13-40`) has the variants needed (`M1`, `M5`, `D1`, …).
- **`market_cache` is daily-snapshot-only.** `desktop/win/crates/midas-app/src/market_cache.rs` holds `MarketDataCache → MarketSnapshot` (price, change%, GATR computed from daily) keyed by symbol. It is **not** a per-timeframe candle cache; thumbnails need parallel storage.
- **Per-view state precedent.** `desktop/win/crates/midas-app/src/chart_view.rs` keys camera state by `(symbol, Timeframe)`. The same shape fits a `ThumbnailStore` (interval choice) and a sibling `ThumbnailDataStore` (close slices).
- **iced widgets available:** `mouse_area().on_release(msg)` is already used in `app/views.rs:1538,2588`. The `stack!` macro is used in `views.rs` for overlay composition. Both are Slice 4 prerequisites.
- **No existing thumbnail/sparkline/mini-chart code.** Net-new feature.

### Best Practices & Idiomatic Approach

1. **One pipeline, many instances.** Create the sparkline pipeline **once** and reuse across every thumbnail widget. Store it on a device-owning struct (not a module-level `OnceLock`) so it can be dropped/recreated if the wgpu device is lost or recreated on resize.
2. **iced `shader::Shader` over `Canvas`** for per-cell GPU rendering. `Shader` gets scissor clipping via widget bounds for free; `Canvas` tessellates CPU-side (lyon) and wastes work for many small redraws.
3. **No per-cell textures.** Do not render-to-texture per thumbnail. Let iced's scissor + direct-to-surface compositing handle it.
4. **Dirty-flag gating on `queue.write_buffer`.** Only re-upload the per-thumbnail close slice when the underlying `CandleBuffer` generation changes; drawing the same buffer 60 times per second is free, uploading is not.
5. **Mountain (area fill) is the industry default** at <100 px width (TradingView Minicharts, Bloomberg Launchpad). User confirmed: no candles.
6. **Cycle-on-click** for timeframe is less intrusive than a popup in a dense grid (TradingView/Bloomberg precedent). Right-click popup for explicit selection is future work.
7. **Performance budget**: 50 rows × 100 closes ≈ <1 ms GPU time. If observed >2 ms, investigate per-cell pipeline creation or CPU tessellation.

## Design Decisions

### Decision 1: Dedicated thumbnail pipeline vs. reuse `ChartRenderer`
**Context**: The main chart pipeline renders candles/volume/grid/text/decorators. A thumbnail needs closes-only, normalize-to-bounds, area fill.
**Options**:
1. Reuse `ChartRenderer` with a fixed-camera `ChartInput` — hauls decorators/volume into every cell.
2. New `sparkline` pipeline (~80 LOC WGSL) — one storage buffer of f32 closes, uniform for bounds + color, triangle strip for fill.
**Recommendation**: Option 2.
**Confidence**: high.

### Decision 2: Mountain vs. Polyline
**Context**: User said "line or mountain, no candles".
**Recommendation**: Ship **Mountain only** for v1 — single style keeps config surface minimal. The WGSL shader can trivially degrade to a line later; no enum yet.
**Confidence**: high (user confirmed either works; simpler wins).

### Decision 3: Interval storage — per-ticker
**Context**: Where does "AAPL thumbnail is showing 5m" live?
**Options**:
1. Per-ticker `HashMap<Symbol, Timeframe>` — matches `ChartViewStore`.
2. Per-grid single interval — simpler, but every click changes *all* thumbnails in a grid (poor UX since clicking AAPL to check its intraday would also switch SPY).
3. Global single interval — even worse.
**Recommendation**: Option 1 (per-ticker). Natural semantics for "click *this* thumbnail to cycle its interval".
**Confidence**: high.

### Decision 4: Data source for thumbnail intervals
**Context**: `market_cache` holds daily `MarketSnapshot` structs, not candles. Thumbnails need last-N closes at (symbol, 1m/5m/1d).
**Recommendation**: Reuse the existing `Arc<dyn DataProvider>` already wired into `MidasApp` via `TestProvider` (`desktop/win/crates/midas-feed/src/test_provider.rs:46`). `TestProvider::get_candles(symbol, timeframe, days)` is async, ~1-5 ms per call, supports every `Timeframe` variant, and requires no files on disk. New `ThumbnailDataStore` keyed by `(Symbol, Timeframe)` holds `Arc<Vec<f32>>` of the last-N closes. On a `fetch()` miss, spawn a `tokio::spawn(provider.get_candles(...))` that fills the cache and dispatches a refresh message when done; return `Arc::new(Vec::new())` until then (renders as the empty-state placeholder from Decision 8).
**Confidence**: high. When real IB data replaces the test provider, the same `DataProvider` trait contract holds — only the invalidation counter (Decision 9) starts ticking.

### Decision 5: Label rendering
**Context**: Need "1m" / "5m" / "1d" inside the thumbnail.
**Recommendation**: Overlay via iced `stack![shader, container(text("5m")).padding(2).align_right().align_bottom()]`. Zero GPU cost; reuses existing iced text rendering.
**Confidence**: high.

### Decision 6: Click handling
**Context**: `iced::widget::shader::Shader` does not natively emit `on_press`.
**Recommendation**: Wrap the `Shader` in `mouse_area(..).on_release(Message::ThumbnailIntervalCycle(symbol))` — matches `grid_body_row` (`body_row.rs:23`) and `app/views.rs:1538,2588` precedent.
**Confidence**: high.

### Decision 7: Pipeline ownership (device-loss safe)
**Context**: If the sparkline pipeline lives in a module-level `OnceLock`, a lost/recreated wgpu device leaves a dangling pipeline and the next upload panics.
**Recommendation**: Own the `SparklinePipeline` on `MidasApp` (same way `ChartRenderer` is owned today), constructed lazily on first `prepare()` when the device becomes available. Drop-and-recreate when iced signals a device change. No static/`OnceLock`.
**Confidence**: high.

### Decision 8: Empty / loading state
**Context**: Per-(symbol, interval) data may not be loaded at render time.
**Recommendation**: When `ThumbnailDataStore::fetch()` returns an empty slice, render a dim horizontal midline at 50% of cell height in muted theme color and show `…` as the label. When the slice later becomes non-empty, the generation bump triggers a redraw automatically.
**Confidence**: high.

### Decision 9: Cache invalidation via generation counter
**Context**: `ThumbnailDataStore` caches `Arc<Vec<f32>>` of closes per `(symbol, tf)`. When the underlying `CandleBuffer` gets live ticks appended (real IB data in a later phase), the cache must reslice.
**Options**:
1. **`AtomicU64` version on `CandleBuffer`**, bumped on push/replace; readers `load(Relaxed)` and compare to a stored `source_version: u64`. Mirrors the project's existing `midas-chart::dirty::DirtyFlags` pattern (generation counters per domain, `DirtyTracker::last_seen` for comparison — see `midas-chart/src/dirty/mod.rs:14-96`).
2. Full seqlock around `CandleBuffer` — rejected: seqlocks require `Copy` values and the read-side RMW destroys cache-line sharing across readers (pitdicker, 2020).
3. Hash of the last timestamp — cheap but fragile (timestamp could match across bar replacements); not the canonical pattern.
**Recommendation**: Option 1. Add `version: AtomicU64` to `midas-core::CandleBuffer`, `fn version(&self) -> u64 { self.version.load(Relaxed) }`, and bump via `fetch_add(1, Relaxed)` in every mutation method (`push`, `extend`, `replace_all`, whatever the project uses). `ThumbnailDataStore::Entry { closes, source_version }` stores the version at slice time; on `fetch()`, reload the current version and reslice if advanced. `Relaxed` is sufficient because the counter is not ordering any other memory — it only signals "something changed".
**Why this is safe**: broker writes to `CandleBuffer` and reader (view-build thread) coordinate via whatever `market_cache.rs` already uses. The `AtomicU64` adds no new synchronization — it only exposes "did a write happen" to the cache layer.
**Confidence**: high. Matches an existing project idiom and the wider Rust canon (matklad's "Caches in Rust", 2022).

## Preflight (resolved — no spike needed)

- **Data source**: `Arc<dyn DataProvider>` already lives on `MidasApp`, backed by `TestProvider` (`midas-feed/src/test_provider.rs`). `get_candles(symbol, tf, days)` is ~1-5 ms, deterministic, supports every `Timeframe` variant — confirmed by `test_provider_multiple_timeframes` in the same file. No disk dependency.
- **Invalidation**: use the project's own generation-counter idiom (`midas-chart/src/dirty/mod.rs:14-96`). Add `version: AtomicU64` to `CandleBuffer`; readers `load(Relaxed)` and compare (Decision 9).
- **Concurrency**: `TestProvider` is `Send + Sync` (wrapped in `parking_lot::Mutex`), safe to call from any thread. Loader tasks run via `tokio::spawn`, dispatch a `Message::ThumbnailDataReady(symbol, tf)` on completion.

## Implementation Plan

### Slice 1: Sparkline GPU pipeline (isolated)
**Goal**: A new `midas-render` pipeline that renders a mountain (area fill) + optional line overlay from a `&[f32]` of closes.
**Depends on**: None.
**Files to create or modify**:
- `desktop/win/crates/midas-render/src/pipelines/sparkline.rs` (new) — `SparklinePipeline { pipeline, bind_group, closes_buf, uniforms_buf }` with `new(device, format)`, `update_buffer(device, queue, &[f32])`, `render(&mut RenderPass, bounds, color)`. Mirror structure from `pipelines/candle.rs`.
- `desktop/win/crates/midas-render/shaders/sparkline.wgsl` (new) — storage buffer `closes: array<f32>`, uniform `Viewport { min: vec2<f32>, max: vec2<f32>, count: u32, pad: u32 }`, triangle-strip vertex shader alternating baseline + value per index. Fragment returns uniform color.
- `desktop/win/crates/midas-render/src/pipelines/mod.rs` — add `pub mod sparkline;`.
- `desktop/win/crates/midas-render/src/lib.rs` — re-export `SparklinePipeline`.
**Key implementation details**:
- Storage buffer layout: `[f32; tail_len]`, one close per index. X is `index / (count-1)`; Y is normalized against uniform `[y_min, y_max]` computed CPU-side via `CandleBuffer::price_range()`.
- Mountain: `draw(2 * N, 1)` emitting a triangle strip alternating `(x, y_min)` baseline and `(x, close)` per index.
- Color chosen CPU-side based on `closes.first()` vs `closes.last()` (up vs down tint from theme).
- Non-indexed draw to sidestep the indexed-instancing vendor quirks flagged in research.
**Testing**:
- `desktop/win/crates/midas-render/tests/sparkline_smoke.rs` — spin up a headless `wgpu::Instance`, build pipeline, render a 4-close sample into a 100×30 texture, read back, assert non-background pixels exist in the expected column range. No validation errors.
- `desktop/win/crates/midas-render/examples/sparkline_demo.rs` — render 3 PNGs (up/down/flat) to disk for eyeball check.
**Done when**:
- `cargo test -p midas-render` passes.
- `cargo clippy -p midas-render -- -D warnings` clean.
- `cargo run -p midas-render --example sparkline_demo` produces 3 readable thumbnails.

### Slice 2: Thumbnail iced Shader widget + device-safe pipeline ownership
**Goal**: Embeddable iced widget wrapping the sparkline pipeline, with correct lifecycle across device recreation.
**Depends on**: Slice 1.
**Files to create or modify**:
- `desktop/win/crates/midas-app/src/thumbnail_widget.rs` (new) — `ThumbnailProgram { snapshot: ThumbnailSnapshot }`, `ThumbnailPrimitive`, and a `ThumbnailSnapshot { closes: Arc<Vec<f32>>, y_min: f32, y_max: f32, color: Color, generation: u64 }`. Public helper `fn thumbnail_cell<M: 'static + Clone>(snapshot, on_click: M) -> Element<'_, M>` returns `mouse_area(stack![shader(...), label]).on_release(on_click)`.
- `desktop/win/crates/midas-app/src/app.rs` — add `sparkline_pipeline: Option<SparklinePipeline>` on `MidasApp`. Initialize on first `prepare()` via a shared handle (same plumbing as the existing `ChartRenderer`). Drop on device change (if iced 0.14 exposes a device-change callback; otherwise check equality of the `wgpu::Device` handle per frame and recreate on mismatch).
**Key implementation details**:
- Follow `chart_widget.rs:1-150` exactly for `prepare()` / `render()` plumbing.
- `prepare()` compares `snapshot.generation` to per-widget state (`ThumbnailWidgetState { last_generation: u64 }`); only call `update_buffer` on change.
- `render()` scopes the draw to the widget's `Rectangle` bounds; wgpu `set_scissor_rect` is clamped to surface size.
- Empty-close case: render a flat mid-line in muted color; label becomes `…`.
- Label text is a sibling `Text` widget in the `stack!`, not a shader primitive.
**Testing**:
- Unit tests for `ThumbnailSnapshot` equality / generation semantics.
- Manual example: `desktop/win/crates/midas-app/examples/thumbnail_demo.rs` displays 3 thumbnails (up/down/flat) in a standalone iced window.
**Done when**:
- Demo window renders three distinct thumbnails with labels.
- Window resize does not panic (validates Decision 7 device-lifecycle handling).
- Clippy and fmt clean.

### Slice 3: CandleBuffer version counter + ThumbnailStore + ThumbnailDataStore
**Goal**: Per-ticker interval preference + lazy per-(symbol, interval) close-slice cache driven by the existing `DataProvider` trait.
**Depends on**: None (parallel to Slices 1-2).
**Files to create or modify**:
- `desktop/win/crates/midas-core/src/candle_buffer/mod.rs` — add `version: AtomicU64` field, `pub fn version(&self) -> u64`, and `fetch_add(1, Relaxed)` in every mutation method (`push`, `extend`, `replace_all` — enumerate them by reading the file and updating each). Document increment points in the type's `///` doc comment. `AtomicU64` is *not* `Clone`; if `CandleBuffer: Clone` today, implement `Clone` manually to copy the current version (or reset to zero — pick whichever matches existing test expectations).
- `desktop/win/crates/midas-app/src/thumbnail_store.rs` (new) — `ThumbnailStore { intervals: HashMap<Symbol, Timeframe>, default: Timeframe }` with `get(&self, symbol: &str) -> Timeframe`, `cycle(&mut self, symbol: &str) -> Timeframe`. Cycle order: `M1 → M5 → D1 → M1`.
- `desktop/win/crates/midas-app/src/thumbnail_data.rs` (new) — `ThumbnailDataStore { cache: HashMap<(String, Timeframe), Entry>, tail_len: usize, pending: HashSet<(String, Timeframe)> }`; `Entry { closes: Arc<Vec<f32>>, y_min: f32, y_max: f32, source_version: u64 }`. Methods:
  - `fetch(&mut self, symbol: &str, tf: Timeframe, source: Option<&CandleBuffer>) -> Arc<Vec<f32>>` — if `source` is `Some`, reslice when `source.version() != entry.source_version`; if `None` and the entry is absent, return `Arc::new(Vec::new())` (caller should trigger a load).
  - `request_load(&mut self, symbol, tf) -> Option<LoadTask>` — returns `Some(task)` if not already pending; `MidasApp` drives it via `tokio::spawn(provider.get_candles(...))` and feeds the result back through a `Message::ThumbnailDataReady { symbol, tf, buffer }` handler.
- `desktop/win/crates/midas-app/src/app.rs` — hold `thumbnail_store`, `thumbnail_data` on `MidasApp`; add `Message::ThumbnailIntervalCycle(String)`, `Message::ThumbnailDataReady { symbol: String, tf: Timeframe, buffer: Arc<CandleBuffer> }`. Cycle handler: call `store.cycle()`, then `data.fetch()` for the new tf (dispatches `request_load` if needed), then mark the watchlist view dirty.
**Key implementation details**:
- `tail_len = 100` closes (f32 → 400 bytes per entry; 50 tickers × 3 intervals ≈ 60 KB).
- `days` parameter for `provider.get_candles`: choose a value that yields at least 100 candles at the requested tf. `M1 → 1 day`, `M5 → 1 day`, `D1 → 180 days` is a safe starting heuristic; verify against `TestDataProvider`'s generator to make sure the returned buffer has ≥ 100 rows.
- Loader deduplication: `pending` set prevents spawning a second task for the same `(symbol, tf)` while one is in flight.
- Concurrency: `ThumbnailDataStore` is mutated only on the main (view) thread; async loads return via `Message::ThumbnailDataReady` which re-enters the main thread. No locks needed on the store itself. The underlying `CandleBuffer`'s `AtomicU64` counter is the only cross-thread surface.
**Testing**:
- `ThumbnailStore::cycle`: M1→M5→D1→M1 round trip.
- `CandleBuffer::version`: starts at 0, advances by 1 per mutation method (one test per mutator).
- `ThumbnailDataStore::fetch`: `Arc` identity stable when `source.version()` unchanged; fresh `Arc` when the version advances; empty-source path returns a non-null empty `Arc` without panic.
- `request_load` dedup: two consecutive calls for the same `(symbol, tf)` return `Some` then `None`.
**Done when**:
- All tests above pass.
- `cargo clippy -p midas-core -p midas-app -- -D warnings` clean.

### Slice 4: Grid integration — watchlist + order blotter
**Goal**: Both grids grow a "Chart" column showing `thumbnail_cell()` with click-to-cycle, empty-state fallback, and minimum column width enforced.
**Depends on**: Slices 1, 2, 3.
**Files to create or modify**:
- `desktop/win/crates/midas-app/src/watchlist_columns/mod.rs` — add `WatchlistColumn::Thumbnail` variant. Impl `cell()` builds `ThumbnailSnapshot` from `app.thumbnail_data.fetch(row.symbol, app.thumbnail_store.get(row.symbol), source_buffer)` and returns `thumbnail_cell(snapshot, Message::ThumbnailIntervalCycle(row.symbol.clone()))`. Impl `min_width() -> 80.0`; `width() -> ColumnWidth::Fixed(120.0)`; `sortable() -> false`.
- `desktop/win/crates/midas-app/src/watchlist/mod.rs` — include `Thumbnail` in default column set; thread `thumbnail_store` + `thumbnail_data` through when constructing columns.
- `desktop/win/crates/midas-app/src/order_blotter/columns.rs` — add equivalent thumbnail column descriptor.
- `desktop/win/crates/midas-app/src/order_blotter/panel.rs` — call the same `thumbnail_cell()` helper per row. Order blotter rows are denser (~20 px); pass a smaller viewport hint.
- Expose `fn default_row_height() -> f32` in `midas-grid` if not already present so both grids share the value. Read actual row height from existing watchlist code before coding (grep `WATCHLIST_ROW_HEIGHT` / body_row.rs) and use the constant.
**Key implementation details**:
- **Critical ordering:** the thumbnail's `mouse_area` is *inside* the row's `mouse_area` in the iced widget tree. iced's `mouse_area::on_release` consumes the event before it bubbles to the outer row handler — verify with a 5-minute spike before committing Slice 4 (Risk #4). If it doesn't consume, set the row-click handler to ignore when the thumbnail sub-region was pressed.
- Handle the empty-closes path via the widget (Decision 8); no special-case in the column.
- Use the existing `grid_body_cell()` wrapper for clip + padding.
**Testing**:
- Unit test: `WatchlistColumn::Thumbnail.cell()` returns a non-empty element for a row with a loaded buffer and for a row with an empty buffer (no panic).
- Manual smoke: `cargo run -p midas-app` — verify thumbnails render in both grids, cycling works, label updates, other columns unaffected, row-click still selects row when clicking outside the thumbnail.
- `cargo run -p midas-app --features dev_harness` — verify the event log captures `ThumbnailIntervalCycle` messages and screenshot capture still works with shader widgets present.
**Done when**:
- Watchlist and order blotter both render thumbnails.
- Click on a thumbnail cycles its label (1m → 5m → 1d → 1m) within one frame.
- Click outside the thumbnail still triggers row selection.
- Tickers with no data show the muted placeholder + `…` label without panicking.
- Dragging the column narrower than 80 px is clamped by `min_width()`.
- `cargo test --workspace`, `cargo clippy --workspace -- -D warnings`, `cargo fmt --check` all green.

### Slice 5 (post-v1): Persistence, line style, polish
**Goal**: Survive restart. Optional line-only style. Theme integration.
**Depends on**: Slices 1-4.
**Files to create or modify**:
- `desktop/win/crates/midas-core/src/config/` — add `ThumbnailConfig { default_interval: Timeframe, overrides: HashMap<String, Timeframe>, style: ThumbnailStyle }`.
- `desktop/win/crates/midas-app/src/app.rs` — load/save on app startup/shutdown.
- `desktop/win/crates/midas-app/src/theme.rs` — `thumbnail_up_color`, `thumbnail_down_color`, `thumbnail_muted_color`.
- `desktop/win/crates/midas-render/shaders/sparkline.wgsl` — add a `style: u32` uniform branch (0 = mountain, 1 = line).
**Key implementation details**:
- Override removal: if a per-symbol override equals the default, drop it from the map on save (keeps TOML small).
- No LRU eviction yet — flag as future work if cache memory is ever measured as problematic.
**Done when**:
- Restart round-trips per-ticker intervals.
- Theme toggle changes thumbnail colors (if hot-swap exists).
- `cargo test --workspace` green.

### Dependency Summary

```
Slice 1 (pipeline) ──┐
                     ├──► Slice 2 (widget) ──┐
Slice 3 (stores) ────┘                       ├──► Slice 4 ──► Slice 5 (post-v1)
                                             ┘
```

- Slices 1 and 3 are independent and can be coded in parallel.
- Slice 2 depends on Slice 1.
- Slice 4 depends on 1+2+3 and is the minimum viable v1.
- Slice 5 is explicitly out of v1 scope.

## Risks & Unknowns

1. **iced 0.14 shader widget submission overhead** for ~50 widgets per frame. Research suggests <1 ms, but this is the target-hardware unknown. Measure during Slice 2 demo; threshold: >2 ms total ⇒ refactor to a single parent shader widget with per-instance layout in the shader (expensive; defer unless measured).
2. **wgpu device recreation on resize/suspend.** Mitigated by Decision 7 (app-owned pipeline, dropped on device change). Validate during Slice 2 by resizing the demo window.
3. **Click routing when thumbnail lives inside an already-clickable row.** Mitigated by 5-minute spike before Slice 4.
4. **`CandleBuffer: Clone` semantics** after adding `AtomicU64`. `AtomicU64` is not `Clone`; pick a `Clone` impl that matches existing tests (copy current version or reset to zero). Verify with `cargo test -p midas-core` after Slice 3 change.
5. **`days` heuristic for `DataProvider::get_candles`.** If the chosen value returns <100 candles at some tf, the thumbnail will still render (shorter slice, auto-scaled). Not a crash, just a scope/quality issue — tune after eyeball test.
6. **GPU vendor shader quirks.** Non-indexed triangle strip avoids the indexed-instancing hazard flagged in research. Still, test on at least one Intel iGPU + one discrete GPU before shipping.
7. **Rollback.** Each slice is additive. Reverting Slice 4 hides the column everywhere; Slices 1-3 leave dead code but no user-visible surface. The `AtomicU64` on `CandleBuffer` is harmless dead weight if Slice 3 is reverted.

## Testing Strategy

- **Unit tests** for `ThumbnailStore::cycle`, `ThumbnailDataStore::fetch` identity semantics — match `chart_view.rs` test style.
- **Render smoke test** for the sparkline pipeline — headless wgpu, verify no validation errors and non-trivial output.
- **Manual visual smoke** via `cargo run -p midas-app` with a known fixture.
- **dev_harness acceptance check** in Slice 4: event log captures `ThumbnailIntervalCycle`; screenshot capture still works with shader widgets present.
- **CI gate**: `cargo test --workspace` + `cargo clippy --workspace -- -D warnings` + `cargo fmt --check` in both workspaces (already required by `.github/workflows/rust.yml`).
- Visual regression / pixel-diffing is out of scope for v1.

## Non-Goals / Out of Scope

- Candlesticks in thumbnails — explicitly excluded.
- Right-click popup for explicit interval selection — future work.
- Persisting interval preferences across restarts — Slice 5 (post-v1).
- Polyline style (no fill) — Slice 5 (post-v1); v1 is mountain-only.
- Per-thumbnail color customization — inherits from theme.
- Indicators (RSI, MACD) / volume bars — closes only.
- Historical range scrubbing inside the thumbnail — read-only.
- Multi-ticker overlay per cell — one ticker per thumbnail.
- Crosshair / hover tooltip inside the thumbnail — too busy at this size.
- Sync with the main chart's interval — intentionally independent.
- Accessibility tooltip (alt text for the thumbnail's trend direction) — future work; note in v1 that thumbnails are visual-only.
- LRU eviction for `ThumbnailDataStore` — not needed at v1 cache sizes; revisit if memory is ever measured.

## Review Notes

- **`ThumbnailDataStore` vs. extending `market_cache`**: a reviewer suggested extending `market_cache` instead. Rejected because `market_cache` holds daily `MarketSnapshot` structs (price, change%, GATR) — a different shape from close-slice arrays. Merging would conflate two cache concerns. Keeping them separate preserves single responsibility.
- **Per-ticker interval confirmed by the user.** Clicking a single thumbnail cycles its own interval; other thumbnails are unaffected.
- **Generation counter pattern (Decision 9) mirrors the project's own `DirtyFlags`.** No new idiom introduced. Web research (matklad 2022, pitdicker 2020) confirmed this over seqlocks — seqlocks need `Copy` types and suffer cache-line contention; we only need "has it changed", for which `AtomicU64 + load(Relaxed)` is canonical.
- **Data source is `TestProvider` today, real IB tomorrow.** Both implement the same `DataProvider` trait — no code change in thumbnail layers when IB replaces test.
- **`plotters-iced` as an alternative**: research flagged it as production-used. Not chosen because (a) it is a `Canvas`-based (CPU-tessellated) renderer — wrong for many tiny widgets; (b) integrating it would bypass the established `midas-render` shader pipeline pattern and fragment the rendering stack. The custom sparkline pipeline is ~80 LOC of WGSL and aligns with how every other chart primitive is shipped today.
- **Canvas fast-path**: could ship a CPU-rendered thumbnail in one slice instead of two. Rejected because the project is firmly on wgpu for all chart drawing, and introducing a CPU path creates a second rendering system to maintain.
- **Feature flag for rollback**: not added. The column is gated by user config (presence in the column set), so users who hate it can hide it by editing their layout without recompiling.
