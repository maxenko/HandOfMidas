# ETH session shading on the legacy chart

Status: **draft v5** — folded findings from plan-eval iteration 4
(Agent A soundness/risk + Agent B best-practices/executability). See
"Iteration history" at the bottom.

## Problem

The chart users actually see (legacy `midas-chart` widget on the main
app window) has no concept of session kind. Pre-market and post-market
candles render indistinguishable from RTH candles, and depending on
broker config they may not reach the chart at all. We need
TradingView-style session shading: a faint coloured overlay behind the
candles for the pre-market and post-market windows so a trader can
see at a glance which bars happened during ETH.

Reference: TradingView 1-min AAPL chart — pre-market and post-market
are tinted (brown/yellow in the user-supplied screenshot), RTH uses
the default chart background. The tint runs **from the first trade to
the last trade** of the session (not the full 04:00 ET → 09:30 ET
calendar window), so empty pre-markets show no band.

User-facing terms:
- **RTH** — Regular trading hours, 09:30–16:00 ET (13:00 ET on
  early-close days).
- **ETH** — Extended trading hours, 04:00–09:30 ET (pre-market) and
  16:00–20:00 ET (post-market). 17:00 ET on early-close.

## Cross-plan alignment

Two parallel feature plans share surface area with this one:

- `plan/volume-profile-anchored/00-index.md` — also widens
  `ChartConfig` (disjoint nested keys: `volume_profile`) and
  `ChartInput` (disjoint field: `effective_vp_anchor`). Additive merge.
- `plan/multi-window-support/README.md` — bumps `AppConfig` to v3, but
  the migration only rewrites `LayoutNode::Chart` leaves and inserts
  the `windows` map; it does NOT touch nested `ChartConfig` fields, so
  ETH's `chart.show_extended_hours*` flow through unchanged regardless
  of order.

See `plan/cross-plan-alignment.md` for the full touchpoint matrix
(Cargo dep promotion, devloop window-targeting convention,
session_chart compose). All three plans are order-independent.

## Current state (verified against the codebase)

| Surface | Session awareness | Notes |
|---|---|---|
| `midas-chart` (legacy, main app window) | **none** | No `session_kind()` on `CandleData`. No band-render code. |
| `midas-render` | none | Already supports rectangle primitives via `GridLineInstance`. |
| `midas-core::CandleData` | none | Trait abstracts data source — no session method. |
| `midas-core::CandleBuffer` | **none, no session column** | Legacy SoA. (The struct lives in `midas-core/src/candle_buffer/`, not `midas-data` — `midas-data/src/candle.rs` is a 4-line re-export.) |
| `midas-bars::CandleSeries` | full | New SoA. One byte per row. Used only by feature-gated `session_chart`. |
| `midas-broker-core::Bar` | none | Wire bar broker backends emit. No session field. |
| `midas-calendar::XnysCalendar` | full | `classify(ts) -> Session` infallible + saturating. |
| `midas-scene::ThemePalette` | full | Has `band_pre`, `band_post`, `band_regular`, `band_closed`. |
| `midas-scene::SessionBandLayer` | full | Walks `sessions_between`, paints **full-calendar-window** bands. |
| `MarketDataRouter::historical_bars` (`midas-market-data/src/router/mod.rs:281`) | **hardcodes `use_rth=true`** | Public router method takes no `use_rth` parameter. |
| `MarketDataRouter::history_then_live` | hardcoded `true` | **Has no callers in `desktop/win/`.** Confirmed via grep. |
| `IbMarketData::historical_bars` | plumbed | Forwards `use_rth` → `ibapi::TradingHours::Regular | Extended`. |
| `SimMarketData::historical_bars` | ignores `_use_rth` | ETH bars depend on `TestDataProvider::bars()`. |
| `ChartInput` (legacy) | flat colour fields | `bull_color`, `bear_color`, `grid_color` — no `&ThemePalette`. Has `collapse_gaps: bool`. |
| `HeuristicSymbolResolver` (`midas-bars-adapter::resolver`) | full | `resolve(ticker) -> ResolvedSymbol { calendar, … }`. Heuristic: `-USD`/`USDT`/`BTC`/`ETH` → `crypto_spot()`; everything else → `xnys()`. |
| `midas-app/Cargo.toml` | `midas-calendar`, `midas-scene`, `midas-bars`, `midas-bars-adapter` are `optional = true` | Pulled in only via the `session_chart` feature. |
| `midas-core/Cargo.toml` | `midas-calendar` is dev-dep only | Production code in `midas-core` cannot reference `SessionKind` directly — must route through `midas-bars::SessionKindByte`. |
| `.github/workflows/rust.yml` | `session_chart_tests` job runs `cargo test --workspace --features session_chart_tests`; `session_chart` jobs are currently `continue-on-error: true` | A test gated on `session_chart_tests` IS run by CI; whether a failure blocks merge depends on the flip-to-required schedule documented in CI. |

## What "from first to last trade" means

Default mode is **trim-to-data**: each contiguous run of non-Regular
candles inside the visible viewport tints
`[run.first.ts_open, run.last.ts_close + bar_duration]`.

- Pre-market trading 07:32–09:29 ET tints that range, not 04:00–09:30.
- Post-market trading 16:00–17:48 ET tints that range, not 16:00–20:00.
- Empty pre-market emits no band.
- Closed/holiday/weekend candles never tint (defensive).

`FullCalendarWindow` mode is **out of scope**.

## Architectural changes

### A. `CandleBuffer` (in `midas-core`) gains a `sessions: Vec<u8>` column

`SessionKind` is `#[repr(u8)]`, so `as u8` and the reverse cast (via
checked match) are well-defined.

```rust
// desktop/win/crates/midas-core/src/candle_buffer/mod.rs
pub struct CandleBuffer {
    // ... existing parallel vecs ...
    pub sessions: Vec<u8>,   // SessionKind as u8 — one byte per row
    version: AtomicU64,
}
```

**Every mutation entry point** must keep the column in lockstep with
the existing OHLCV columns. All listed APIs are on `CandleBuffer`
and live in `midas-core/src/candle_buffer/mod.rs`:

| Entry point | Action |
|---|---|
| `CandleBuffer::push` | Existing 6-arg `push` keeps signature; defaults session = `Regular`. New 7-arg `push_with_session` takes the explicit kind. |
| `CandleBuffer::apply_bar` | Adds an explicit `session: SessionKind` argument. The "overwrite-in-place on matching ts_open" branch updates `sessions[last]`. The "push new" branch routes through `push_with_session`. |
| `CandleBuffer::merge_bar` | Same treatment as `apply_bar`. |
| `CandleBuffer::update_last` / `update_last_price` | OHLC of last bar; session unchanged (price-tick of an existing bar). |
| `lod::downsample_minmax` | **Does NOT track sessions** and does not need to in this scope. Verified via grep that the legacy chart pipeline does not exercise the LOD downsampler in production — `select_lod` and `downsample_minmax` exist in `midas-data::lod` but no chart-side caller invokes them today. If/when LOD lands on the legacy chart, `compute_session_bands` will need a guard ("skip when each visible bar represents > N source bars"); deferred. |
| `Bar` → `CandleBuffer` conversion (`bars_to_candle_buffer` in `app.rs`) | Classifies via the symbol's calendar. See §B. |
| Live-append in `chart_subscription.rs` (`ChartBarBatch`, `ChartSubBarBatch`) | Same calendar lookup as §B. |

### Binary loaders — out of scope for this slice

The on-disk binary candle format does **not** change. Loaders
(`read_midas_file`, `to_candle_buffer`, `slice_by_time` in
`midas-data/src/binary/`) populate the `sessions` column with
all-`Regular` on load. Implications:

- A `.midas` file produced before this change loads with no bands —
  correct, since RTH-only stored data has no ETH to display anyway.
- A `.midas` file produced AFTER this change with classified
  in-memory data, written back to disk, will lose the column on
  next load — accepted limitation. Persisted ETH-aware history is
  a follow-up that bumps the binary schema version.
- `midas-data` does NOT gain a `midas-calendar` dep. The earlier
  v3 plan to classify in the binary loader is wrong: `midas-data`
  has no symbol context (`CandleBuffer` carries no `Symbol`), so
  it cannot pick the right calendar. Moving classification into
  the host (where the symbol is known) is the only correct
  approach.

This means the legacy "load yesterday's stored AAPL data, see no
bands" UX exists. If the user wants ETH bands on stored historical
data, the path is: (a) re-pull from the live broker (which now
classifies at conversion), or (b) wait for the schema bump
follow-up. The user has explicitly OK'd that scope boundary —
historical ETH replay is an open question, not a hard requirement.

### B. Calendar selection at conversion uses `HeuristicSymbolResolver`

Hardcoding `xnys()` fails for non-XNYS symbols (the existing
`CryptoSpotCalendar`-backed crypto symbols). The project already has
the abstraction: `midas-bars-adapter::resolver::HeuristicSymbolResolver`.

The resolver's existing doc-comment classifies it as "sim-only," but
that scope refers to the synthesised `con_id` it stamps (which IB's
real `reqContractDetails` already provides). The `.calendar` field
is a pure function of the ticker string and is universally valid
for both sim and IB symbols. **This plan uses `resolver.resolve(t)
.calendar` only**; the resolver's `.contract_id` is ignored on this
path. S1b includes a one-line doc update widening the
"sim-only" framing to clarify that calendar resolution is universal.

Heuristic gaps (e.g., the rare equity ticker `ETH` — Ethan Allen
NYSE — would be misclassified as crypto under the `-USD`/`USDT`/`BTC`/`ETH`
suffix rule) are an accepted miss-rate. Today's user trades US
equities + a handful of crypto pairs; the heuristic is correct for
all of them. If a misclassification surfaces, the fix is registering
the ticker explicitly in `StaticSymbolResolver` and using a
`StaticSymbolResolver → HeuristicSymbolResolver` chain — out of
scope for this plan.

```rust
// desktop/win/crates/midas-app/src/app.rs (bars_to_candle_buffer)
fn bars_to_candle_buffer(
    bars: &[Bar],
    calendar: &'static dyn ExchangeCalendar,
) -> CandleBuffer {
    let mut buf = CandleBuffer::with_capacity(bars.len());
    for bar in bars {
        buf.push_with_session(
            bar.ts_open.timestamp_millis(),
            bar.o as f32, bar.h as f32, bar.l as f32, bar.c as f32,
            bar.volume.try_into().unwrap_or(u32::MAX),
            calendar.classify(bar.ts_open).kind(),
        );
    }
    buf
}

// load_chart_via_router + live-append paths
let calendar = HeuristicSymbolResolver::new()
    .resolve(symbol)
    .map_err(/* ... */)?
    .calendar;
let buf = bars_to_candle_buffer(&bars, calendar);
```

The resolver is the single source of symbol→calendar truth. The new
stack already uses it; the legacy stack now joins it. No duplicated
heuristic.

### C. `CandleData` trait gains `session_kind`

```rust
// desktop/win/crates/midas-core/src/candle_data/mod.rs
pub trait CandleData {
    // ... existing ...
    fn session_kind(&self, idx: usize) -> midas_bars::SessionKindByte {
        midas_bars::SessionKindByte::Regular
    }
}
```

`SessionKindByte = SessionKind` is already a public re-export from
`midas-bars`. Routing the trait method's return through `midas-bars`
(already a regular dep of `midas-core`) keeps `midas-calendar` out of
`midas-core`'s production deps.

`CandleBuffer::session_kind` reads the new column.
`CandleSeries::session_kind` reads its existing byte.
`CandleSlice::session_kind` reads through to the underlying buffer.

Out-of-tree `impl CandleData for X` would silently inherit `Regular`.
None today; in-tree impls (`CandleBuffer`, `CandleSlice`,
`CandleSeries`, ~5 test mocks) are all updated in the same commit.

### D. `MarketDataRouter::historical_bars` accepts `use_rth`

Today the router method hardcodes `true`. Add an explicit parameter.
`history_then_live` is **not** modified — verified via grep that it
has zero callers in `desktop/win/`, so adding a `_with_rth` variant
would be dead code. If a future migration revives it, that's the
slice that should add the parameter.

`load_chart_via_router` (`app.rs:3539`) is the single helper used by
both chart loads and watchlist snapshots. It grows a `use_rth: bool`
parameter; each caller decides:

| Caller | Site | `use_rth` | Reason |
|---|---|---|---|
| Chart load (initial + restore) | `load_chart_with`, `load_chart_async_restore` | `!cfg.chart.show_extended_hours` | Drives ETH visibility on the chart. |
| Watchlist market snapshot | `load_market_snapshot` (`app.rs:3578`) | **`true`** (preserved) | Watchlist's "last close" is RTH-only by user convention. Changing this would silently shift the displayed last-close to 20:00 ET prints. Out of scope. |

The plan does NOT introduce `load_chart_for_floating_chart` (no such
function exists; the floating-chart path already routes through
`load_chart_via_router`).

### E. `ChartConfig` gains a `show_extended_hours` knob

```toml
[chart]
show_extended_hours       = true   # default — drives use_rth on data fetch
show_extended_hours_bands = true   # default — drives the band-render compute pass
```

`midas-core::config` default-fills missing fields via serde defaults
(verified pattern from prior config additions). Existing
`data/config.toml` files load unchanged.

### F. Compute pass: `compute_session_bands`

In `desktop/win/crates/midas-chart/src/compute/mod.rs`. Sans-IO,
consumes `&dyn CandleData`. Output is one or more `GridLineInstance`
filled rectangles, appended to the existing grid-line bucket so no
new GPU pipeline is needed.

Inputs (passed through `SessionBandParams`, filled from `ChartInput`):
- `bar_duration_ms: i64` — sourced from `ChartPanel.timeframe`.
- `pre_color: [f32; 4]`, `post_color: [f32; 4]` — see §G.
- `collapse_gaps: bool` — branches X computation between
  `camera.time_to_x(ts)` (false) and the index helper the candle
  pass already uses (true).

No LOD guard. The legacy chart does not exercise the LOD downsampler
in production today (verified: zero callers of `select_lod` /
`downsample_minmax` in `midas-chart` or `midas-app`). If LOD lands
later, the guard is added then.

Pseudocode:

```
1. If !show_extended_hours_bands → return empty.
2. Walk vis_start..vis_end:
     kind = data.session_kind(i)
     if kind == PreMarket or PostMarket:
         start a run if no run is active or kind changed
         extend the active run
     else:
         flush the active run (if any)
3. After loop, flush trailing run.
4. For each flushed run:
     x0 = x_for_index(run.first)                         // index OR time
     x1 = x_for_index_at_close(run.last, bar_duration_ms)
     emit GridLineInstance { rect, color: pick(kind) }
```

Caching: cost is O(`vis_end - vis_start`). Performance budget:
**≤0.3 ms at 5 000 visible bars**, measured via the existing chart
bench harness. If breached, key a per-frame cache on `(vis_start,
vis_end, params_hash)`.

The `downsample_minmax` path is untouched.

### G. Theme palette source-of-truth — accept duplication, enforce parity

The legacy `ChartInput` carries flat colour fields, not a
`&ThemePalette`. Promoting `midas-scene` (currently `optional = true`)
to non-optional would pull scene/layer/decorator types into the
legacy chart's dep graph — too much blast radius for this scope.

**Decision**: accept palette duplication for this round.

- Legacy: `LEGACY_BAND_PRE` / `LEGACY_BAND_POST` constants in
  `midas-chart` next to existing colour constants.
- New stack: `midas_scene::ThemePalette::dark_default().band_pre /
  band_post` — updated to the same TradingView-matching values.
- A workspace-level integration test in `desktop/win/tests/`
  (NOT a `#[cfg(feature = "session_chart")]` unit test inside
  `midas-app`) asserts byte-for-byte equality. Workspace tests run
  under `--features session_chart_tests` (which transitively
  enables `session_chart`), so the parity test executes in the
  `desktop_session_chart_tests` CI job.

CI gate honesty (accepted risk): the `desktop_session_chart_tests`
job is currently `continue-on-error: true` — drift surfaces a yellow
warning on the Actions tab but does NOT block merge until the
project's flip-to-required schedule lands (tracked separately,
outside this plan's scope). Mitigation responsibility falls on
human review until then: any PR touching either palette constant
must touch both. S6 calls this out as a known weakness with a
pointer to the flip-to-required follow-up.

Alternative considered + rejected: extract `ThemePalette` (or just
the band colour constants) to a tiny shared `midas-theme` crate
that both `midas-chart` and `midas-scene` depend on. Cleaner long
term, but ~40 LOC + new crate + workspace edit, for a problem
that flip-to-required eliminates entirely. Defer until either (a)
flip-to-required slips substantially, or (b) Phase D begins.

Trade-off: legacy and new stacks paint different band extents
(trim-to-data vs. full-calendar-window). The CI test enforces
colour parity, not visual parity. Visual unification is a follow-up
post-Phase-D.

### H. Dependency graph delta (Cargo.toml changes)

| Crate | File | Change | Reason |
|---|---|---|---|
| `midas-calendar` in `midas-app` | `desktop/win/crates/midas-app/Cargo.toml` | promote from `optional = true` to regular | `bars_to_candle_buffer` calls `calendar.classify()` always. |
| `midas-bars-adapter` in `midas-app` | same | promote from `optional = true` to regular | `HeuristicSymbolResolver::resolve` is the symbol→calendar source of truth (§B). |
| `midas-bars` in `midas-app` | same | promote from `optional = true` to regular | Transitive of the above; also direct use for `SessionKindByte`. |
| `session_chart` feature | same | drop `dep:midas-calendar`, `dep:midas-bars-adapter`, `dep:midas-bars` from feature's `dep:` array | Mechanical; they're now mandatory. |
| `midas-core/Cargo.toml` | unchanged | `CandleData::session_kind` returns `midas_bars::SessionKindByte` (already a regular dep). |
| `midas-data/Cargo.toml` | unchanged | Binary loaders default `Regular` — no calendar lookup at load. |
| `midas-scene` in `midas-app` | unchanged (`optional = true`) | §G accepts colour-constant duplication; parity test enforces drift-free. |

Net effect: three small dep-light crates (`midas-calendar`,
`midas-bars`, `midas-bars-adapter`) become baseline deps of
`midas-app`. None pull in iced/wgpu/heavy transitive deps. Cold-build
impact: ~1 s on a warm toolchain.

## Implementation slices

Each slice ends green: `cargo test --workspace` + `cargo clippy
--workspace --all-targets -- -D warnings` on both workspaces, plus
`cargo fmt --all`.

### Dependency DAG

```
S1a (data type changes: CandleBuffer column in midas-core, push
       variants, trait method, impls — leaves apply_bar/merge_bar
       signatures unchanged so callers still compile)
  │
  ├─→ S1b (mutation API session-aware: apply_bar/merge_bar gain
  │         session param; ALL handler callers updated in same
  │         commit; bars_to_candle_buffer uses HeuristicSymbolResolver;
  │         live-append paths classify per bar; Cargo.toml dep
  │         promotions land here)
  │     │
  │     └─→ S2 (compute pass + ChartInput wiring +
  │           collapse_gaps branch + LOD-level guard)
  │           │
  │           └─→ S3 (theme palette + ChartConfig knobs +
  │                 workspace-level parity test)
  │                 │
  │                 ├─→ S4-router (use_rth parameter on
  │                 │              MarketDataRouter::historical_bars
  │                 │              + per-call-site values)
  │                 │
  │                 └─→ S4-sim (TestDataProvider ETH range)  ──┐
  │                                                             │
  │                                                             ▼
  │                                                S5 (devloop fixture +
  │                                                     CI sans-IO test +
  │                                                     dev-only screenshot)
  │                                                             │
  └─────────────────────────────────────────────────────────────┴──→ S6 (docs)
```

Key: **S5 depends on S4-sim** (fixture replays via the broker, so
sim must emit ETH bars before screenshots show bands). S4-router
and S4-sim parallelise within S4. S4-IB is verification-only and
can land any time after S4-router.

### S1a — Data type changes

- `CandleBuffer.sessions: Vec<u8>` column added in
  `midas-core/src/candle_buffer/mod.rs`.
- `push` keeps existing 6-arg signature (defaults `Regular`); add
  `push_with_session` for the explicit kind.
- `CandleData::session_kind` trait method with `Regular` default.
- `CandleBuffer::session_kind` / `CandleSlice::session_kind` /
  `CandleSeries::session_kind` impls.
- Test mocks updated.
- `apply_bar`, `merge_bar`, `update_last`, `update_last_price`
  signatures **UNCHANGED in this slice** so callers still compile.
  `apply_bar`/`merge_bar`'s "push new" branch defaults `Regular`;
  S1b corrects this with the param + caller update in one commit.
- Binary loaders unchanged — `sessions` column populated with
  all-`Regular` on load.
- LOD downsampler unchanged — output's `sessions` column populated
  with all-`Regular`.
- Tests:
  - `push_with_session` round-trips kind.
  - `CandleBuffer` loaded from a fixture file reports `Regular`
    for every index (binary loader baseline).
  - `CandleData::session_kind` returns the column value via
    trait dispatch.
- ~200 LOC + 150 LOC of tests. Slice ends green: zero callers
  break.

### S1b — Mutation API session-aware + classification at host

- `apply_bar` and `merge_bar` gain `session: SessionKind` param.
- All handler callers in `midas-app/handlers.rs` updated in the
  same commit (~5 sites: `apply_bar_to_buffer`, `merge_bar_to_buffer`,
  3 `update_last_price` callers preserve session via API choice).
- `bars_to_candle_buffer` in `app.rs` takes a calendar parameter,
  uses `push_with_session` (§B).
- `load_chart_via_router` and live-append paths use
  `HeuristicSymbolResolver::resolve(ticker).calendar` to pick the
  calendar.
- `chart_subscription.rs` (`ChartBarBatch`, `ChartSubBarBatch`)
  classify each bar via the same resolver.
- Cargo.toml: `midas-calendar`, `midas-bars`, `midas-bars-adapter`
  promoted from `optional = true` to regular deps in
  `midas-app/Cargo.toml`. `session_chart` feature's `dep:` array
  trimmed accordingly.
- Tests:
  - Live-append a PreMarket-timestamped bar via
    `apply_bar_to_buffer`; assert `buf.session_kind(last) ==
    PreMarket`.
  - Same for `merge_bar`.
  - `bars_to_candle_buffer` round-trip with a 04:00–20:00 ET range
    of XNYS bars: each bar's session matches the calendar.
  - Same with a CRYPTO ticker (`BTC-USD`): every bar reports
    `Regular` (CryptoSpotCalendar has no extended sessions).
  - Doc-comment update on `HeuristicSymbolResolver` to clarify
    `.calendar` is universal (only `.con_id` is sim-only).
  - Cargo build matrix: `cargo build -p midas-app` (no features),
    `--features session_chart`, `--features dev_harness` — all
    green.

The private `bars_to_candle_buffer` helper in `midas-feed/src/
testdata/mod.rs:696` (which converts the legacy `OhlcvBar` type for
CSV-imported data) is **unchanged** in this slice. CSV-imported
equity history is RTH-only by user convention; defaulting `Regular`
matches existing semantics.
- ~250 LOC + 200 LOC of tests.

### S2 — Compute pass

- `SessionBandParams` + `compute_session_bands` in `compute/mod.rs`.
- `ChartInput` gains `show_extended_hours_bands: bool`,
  `bar_duration_ms: i64`,
  `pre_market_band_color: [f32; 4]`, `post_market_band_color: [f32;
  4]`.
- Branches X computation on `collapse_gaps`.
- Tests:
  - 10 pre + 10 RTH + 10 post → 1 pre band + 1 post band.
  - Empty pre-market run → 0 pre bands.
  - Discontinuous run (pre, RTH, pre) → 2 separate pre bands.
  - `collapse_gaps = true` → bands' X positions match the candle
    pass's index-mode positions (within float tolerance).
  - Last-bar edge: rect right edge = `data.timestamp(last) +
    bar_duration_ms`.
  - Bench: ≤0.3 ms at 5 000 visible bars.
- ~200 LOC + 250 LOC of tests.

### S3 — Theme palette + ChartConfig knobs + parity test

- Update `midas_scene::ThemePalette::dark_default` and `light_default`
  band-pre/band-post constants.
- Add legacy-side `LEGACY_BAND_PRE` / `LEGACY_BAND_POST` constants
  in `midas-chart`.
- **Workspace-level integration test in `desktop/win/tests/`**
  asserts byte-for-byte equality. Runs under
  `--features session_chart_tests` in the
  `desktop_session_chart_tests` CI job. Currently
  `continue-on-error`; documented in S6.
- Add `chart.show_extended_hours: bool` and
  `chart.show_extended_hours_bands: bool` to
  `midas-core::config::ChartConfig` with serde-default fallbacks.
- TOML round-trip + default-value tests.
- ~120 LOC + 120 LOC of tests.

### S4-router — Router `use_rth` parameter

(Independent of S4-sim within S4.)

- `MarketDataRouter::historical_bars` gains `use_rth: bool`.
- `load_chart_via_router` gains `use_rth: bool`. Caller table
  per §D.
- `history_then_live` UNCHANGED (no callers in `desktop/win/`).
- Tests:
  - Chart load with `chart.show_extended_hours = true` issues
    `use_rth = false` to the router.
  - `load_market_snapshot` issues `use_rth = true` regardless of
    config (preserved semantics).
- ~80 LOC + 80 LOC of tests.

### S4-sim — Sim ETH bar emission

(Independent of S4-router within S4.)

- Audit `TestDataProvider::bars()`. If RTH-only, extend to cover
  04:00–20:00 ET on weekdays. Gate behind a
  `synthetic_includes_eth: bool` config (default `true`).
- Tests: sim emits ≥1 PreMarket and ≥1 PostMarket bar for an
  AAPL fixture spanning a full ET day.
- ~70 LOC + 100 LOC of tests.

### S4-IB — IB plumbing verification

- IB's `historical_bars` already plumbs `use_rth` →
  `ibapi::TradingHours::Regular | Extended`.
- Test: a chart load with `chart.show_extended_hours = true`
  issues `TradingHours::Extended`.
- ~30 LOC of test only.

### S5 — Devloop verification

(Depends on S4-sim — fixture replays via the broker.)

- New fixture `desktop/win/.devloop/fixtures/aapl-eth-day.json` —
  AAPL M1 covering one full ET trading day around an early-close
  date (so the 17:00 post-close edge is exercised).
- New devloop smoke `desktop/win/tools/devloop-eth-bands.sh` —
  start app → LoadFixture → WaitForIdle → Screenshot → save
  reference. Reference image checked into
  `desktop/win/tests/data/screenshots/`. Hand-verified with the
  user before check-in.
- **CI gating**: a sans-IO `#[test]` in `desktop/win/tests/`
  loads the fixture's bar sequence directly into a `CandleBuffer`,
  runs `compute_session_bands`, and asserts band count + bounds.
  No harness command needed (the harness's `DumpState` projects
  state, not compute output; building a `DumpChartCompute`
  command for one test is over-engineered).
- The pixel-level SSIM diff is **dev-only** — local + dev-loop.
  Reason: GPU driver / font / DPI variance across CI runners.
- ~130 LOC of tooling + fixture (~30KB JSON) + ~100 LOC of test.

### S6 — Docs + memory

- Update `plan/session-aware-charts/README.md` "Phase D deferred"
  list — bands ship on the legacy path; Phase D no longer blocked
  on band rendering.
- Cross-link from `plan/session-aware-charts/00-index.md`.
- Note in root `CLAUDE.md` that ETH shading is the legacy-chart
  default, that the new `session_chart` window paints
  full-calendar-window bands until a future unification, and that
  the parity test currently runs in a non-blocking CI job (until
  the flip-to-required lands per the CI rollout note).

## What this does NOT do

- Does not migrate `CandleBuffer` to `CandleSeries`. Two storage
  types continue to coexist until Phase D.
- Does not change the on-disk binary candle format. Loaders
  populate the new `sessions` column with all-`Regular`. Stored
  ETH-aware history is a follow-up that bumps the schema version.
- Does not classify in the binary loaders. `midas-data` has no
  symbol context, so it can't pick the right calendar; the host
  layer does it instead at conversion time (§B).
- Does not propagate session info through the LOD downsampler.
  The legacy chart pipeline does not exercise the downsampler in
  production today (verified). When LOD lands later, the band
  pass needs a guard added then.
- Does not modify `MarketDataRouter::history_then_live` (no
  callers in `desktop/win/`; adding a parameter would be dead code).
- Does not introduce `FullCalendarWindow` shade mode.
- Does not change the new `session_chart` window's
  `SessionBandLayer` (continues to paint full-calendar-window
  bands; visual divergence documented).
- Does not change `load_market_snapshot`'s `use_rth` (preserved at
  `true` per watchlist semantics).
- Does not extract `ThemePalette` to a shared crate (palette
  duplication accepted; parity test enforces drift-free).
- Does not add a runtime ETH on/off toggle (read at startup from
  config).
- Does not promote `midas-scene` to non-optional (avoids pulling
  scene/layer/decorator types into legacy chart's graph).
- Does not touch holiday markers (legacy chart has none).

## Open questions for the user

1. **Trim-to-data vs. full-calendar-window default.** Plan defaults
   to trim-to-data (matches user's "first to last trade" wording).
   TradingView reference shows full-window. Confirm before S2.
2. **Single tint or distinct.** ✅ **RESOLVED 2026-04-25**: two
   distinct tints, TradingView-style — warm brown for pre-market,
   cool blue for post-market. Plan already specifies this in §G; no
   change required.
3. **Suppress bands when zoomed out far.** Plan does NOT suppress —
   the legacy chart doesn't downsample in production today, so
   nothing to guard against. If LOD ever lands on the legacy chart,
   a guard like "skip bands when each visible bar represents > N
   source bars" is the right shape; deferred until then.
4. **Land on legacy or fast-track Phase D.** S4-router is mandatory
   either way. Legacy-only slices: S1a, S1b, S2, parts of S3, S5.
   Fast-tracking Phase D would skip those but require completing
   bracket/indicator/level/tool feature parity.

## Risk + mitigation

| Risk | Mitigation |
|---|---|
| (v1 critical, fixed in v2): bands never render because data path drops session | `CandleBuffer.sessions` column + classify-at-conversion. |
| (v2 high, fixed in v3, refined in v4): xnys() hardcoded; ignores crypto | §B uses `HeuristicSymbolResolver::resolve(ticker).calendar`. |
| (v3 critical, fixed in v4): plan referenced wrong CandleBuffer location | All §A references corrected to `midas-core/src/candle_buffer/`. |
| (v3 critical, fixed in v4): binary loaders introduced silent crypto-corruption + undeclared `midas-data → midas-calendar` edge | Binary loaders dropped from scope; populate `Regular` on load. Stored ETH replay = future schema bump. |
| (v3 high, fixed in v4): `_with_rth` variant was dead code | Dropped. `historical_bars` parameter only. |
| (v3 high, fixed in v4): §D table referenced non-existent `load_chart_for_floating_chart` | Removed; `load_chart_via_router` is the single helper that grows the parameter. |
| (v3 high, fixed in v4): parity test gated on `session_chart` unit-test feature CI doesn't run | Test moved to workspace-level integration tests under `session_chart_tests`. |
| (v3 high, fixed in v4): missing `midas-bars-adapter` Cargo.toml promotion | Added to §H. |
| (v3 medium, fixed in v4 then refined in v5): LOD "dominant kind" tie-break overengineered | Dropped in v4 (early-return); v5 also drops the early-return because the legacy chart doesn't downsample in production. |
| (v4 high, fixed in v5): `lod_level: u8` was a phantom dependency — no LOD level concept exists in `midas-chart` or `midas-app` today | Removed from §F and S2; documented as a future addition when LOD wires up. |
| (v3 low, fixed in v4): binary writer `debug_assert_eq!` was self-cancelling | Dropped along with binary-loader classification. |
| Sim doesn't emit ETH bars | S4-sim explicitly extends `TestDataProvider::bars()`. |
| Band colours look wrong on dark background | S5 dev-loop iteration with screenshot review. |
| Bar-duration on last bar of a run | S2 uses `bar_duration_ms` from `ChartPanel.timeframe`. |
| `collapse_gaps = true` mode drift | S2 branches X computation. Test covers it. |
| Hot-path perf budget blown | S2 sets 0.3 ms budget. Cache hatches available. |
| Watchlist semantics broken by `use_rth=false` | §D table preserves `true` for `load_market_snapshot`. |
| CI screenshot SSIM flapping | S5 splits CI gate (sans-IO test) from dev-only (pixel diff). |
| Parity test in non-blocking CI job | Documented in S6; flip-to-required tracked separately. |

## Iteration history

### Iteration 1 → v2
- Critical (Agent A): v1 deferred adding the session column to
  `CandleBuffer`. v2 made it the central change.
- High (Agent A): v1 described S4 as "audit"; `MarketDataRouter`
  hardcodes `use_rth=true`. v2 spelled out the API change.
- High (Agent A): v1 proposed legacy-only band colour constants.
  v2 sourced from `ThemePalette`.
- Several mediums + lows folded.

### Iteration 2 → v3
- High (both agents): v2 claimed S5 parallelizes with S4 — but the
  `.devloop` fixture replays via the broker, so S5 needs S4-sim
  first. v3 fixed the DAG.
- High (Agent A): v3 promoted `midas-calendar` to non-optional in
  `midas-app`.
- High (Agent B): v3 enumerated all `CandleBuffer` mutation entry
  points; split S1.
- Medium: v3 used `match symbol_kind` for crypto correctness; v3
  binary loaders classified on load.
- Low + medium fixes folded.

### Iteration 3 → v4 (this revision)
- Critical (Agent A): v3 referenced `CandleBuffer` in `midas-data`
  but it lives in `midas-core`. All §A paths corrected.
- Critical (Agent A): v3's binary-loader classification introduced
  a `midas-data → midas-calendar` edge §H didn't list AND silently
  re-corrupted crypto bars (no symbol context in `midas-data`).
  v4 drops binary-loader classification entirely; loaders default
  to `Regular`. Persisted ETH-aware history = future schema bump.
- High (Agent A): v3's §B "match symbol_kind" had no codebase
  referent. v4 uses `HeuristicSymbolResolver::resolve(ticker).
  calendar` and promotes `midas-bars-adapter` to non-optional.
- High (Agent A): v3 introduced `_with_rth` variant; verified zero
  callers of `history_then_live` in `desktop/win/`. v4 drops the
  variant.
- High (Agent A): v3 §D listed non-existent
  `load_chart_for_floating_chart`. v4 removes the row;
  `load_chart_via_router` is the single helper that gets the
  parameter.
- High (Agent B): v3's parity test gated on `session_chart` unit
  feature CI doesn't run. v4 moves it to workspace integration
  tests under `session_chart_tests`.
- High (Agent B): v3 §H missed `midas-bars-adapter` Cargo.toml
  promotion. v4 §H lists it.
- Medium (Agent B): v3 LOD "dominant kind" tie-break rule
  overengineered. v4 drops the rule; `compute_session_bands`
  early-returns at `lod_level >= 2`.
- Medium (Agent A): v3 binary writer `debug_assert_eq!` was
  largely cosmetic and didn't catch crypto corruption. v4 drops
  the writer assert along with binary-loader classification.
- Low (Agent B): v3 `_with_rth` style smell. v4 obviated by
  dropping the variant.
- Low (Agent B): v3 `downsample_minmax` extra work had no budget.
  v4 obviated by not doing the work (LOD guard).
- Several CI honesty notes added (parity test runs in
  non-blocking job; flip-to-required follows separate schedule).

### Iteration 4 → v5 (this revision)
- High (both agents): v4's `lod_level: u8` field on `ChartInput`
  and `compute_session_bands` early-return at `lod_level >= 2`
  reference a concept (`lod_level`) that does not exist in
  `midas-chart` or `midas-app` today. Verified: zero matches for
  `lod_level`, `current_lod`, `LodSelection` in chart code; the
  `select_lod` API in `midas-data::lod` returns a `target_count`,
  not a level index. v5 drops the LOD field and guard entirely.
  When LOD lands, the guard is added then.
- Medium (both agents): `HeuristicSymbolResolver` is doc'd as
  "sim-only," but its `.calendar` field is a pure function of
  ticker string and universally valid. v5 §B clarifies and S1b
  adds a one-line doc-comment update.
- Medium (Agent B): the parity-test "fails CI" framing was
  inaccurate — drift fails a `continue-on-error: true` job. v5
  reframes as accepted risk pending flip-to-required, surfaces
  the alternative (extract palette to shared crate) and the
  rationale for deferring it.
- Low (Agent A): second `bars_to_candle_buffer` helper in
  `midas-feed::testdata`. v5 explicitly notes it stays unchanged.
- Open Question 3 updated to reflect "no LOD threshold" stance.

### Iteration 5 — pending

To be evaluated via plan-eval again. Iteration appended only after
material changes.
