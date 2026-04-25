# Volume Profile Refinements — Per-Chart Anchored Profiles

> **Status:** Plan v3.1 (post `/plan-eval` round 3). All Critical/High findings addressed across three rounds; Round 3's surgical Mediums (kill-switch enforcement site, ChartInput widening, S4 heading) incorporated. Ready for execution.
> **Convention:** Index + per-slice sub-files (matches `plan/session-aware-charts/` layout). Read this file first; it's the navigation root.

## Files in this plan

| File | Contents |
|------|----------|
| `00-index.md` | Overview, motivation, design decisions, slice index, dependencies, risks (this file) |
| `01-slice-1-schema-and-devloop.md` | Settings schema + persistence + devloop `SetVpSettings` command + pre-S1 reconnaissance |
| `02-slice-2-legacy-render.md` | Per-session VP rendering on the **legacy** chart stack |
| `03-slice-3-new-stack-render.md` | Per-session VP rendering on the **new** (`session_chart`) stack — includes first-time `VolumeProfileLayer` wiring |
| `04-slice-4-gear-popup-ui.md` | Toolbar `⋮` glyph + settings popup panel |
| `05-slice-5-cross-stack-parity.md` | Devloop screenshot tests for cross-stack visual parity (per-stack screenshots owned by S2/S3) |
| `06-slice-6-polish.md` | Polish micro-PRs (P0 baseline measurement, P1 cache, P2 narrow-period tick, P3 hover, P4 value-area, P5 up/down split, P6 collapse-gaps full fix) |

## Motivation

**Problem:** Today's Volume Profile draws a single histogram smeared across the entire visible viewport. Discretionary intraday equity traders compare auction structure across days — prior-day POC, gap-up/gap-down behaviour, and overnight value-area drift are core trading signals. With one viewport-wide histogram, none of this is visible: the per-day POCs collapse into a single weighted average and prior-day support/resistance lines vanish.

**Who benefits:** Active traders watching multi-day intraday charts (the primary user persona for Hand of Midas). The reference TradingView screenshot the user provided shows ~6 daily-anchored profiles side-by-side — the standard view in the trading workflow we're targeting.

**What changes:** Each chart instance picks how VP is anchored — Viewport (current behaviour, default) or per **Day / Week / Month / Year**. Per-anchor profiles are left-anchored at each period's first candle, drawn alongside the candles, with their own POC.

## Cross-plan alignment

Two parallel feature plans share surface area with this one:

- `plan/session-aware-charts/eth-shading.md` — also widens `ChartConfig` (disjoint nested keys: `show_extended_hours`, `show_extended_hours_bands`) and `ChartInput` (4 disjoint fields). Promotes `midas-calendar` / `midas-bars` / `midas-bars-adapter` to baseline `midas-app` deps — no-op for VP.
- `plan/multi-window-support/README.md` — bumps `AppConfig` to v3 but the migration does NOT touch nested `ChartConfig` fields or top-level `[experimental]`. VP's `chart.volume_profile` and `experimental.disable_anchored_vp` flow through the migration unchanged.

See `plan/cross-plan-alignment.md` for the full touchpoint matrix.

**Devloop note for S1**: if `plan/multi-window-support/` slice G lands first, `SetVpSettings` (S1) MUST take `#[serde(default)] window: Option<String>` from the start — a chart can live in any named window. If VP S1 ships first, that field is added in a follow-up commit alongside multi-window's slice G; document the dependency in S1's "Files to modify" or wire-version note.

**Open Question 6** (VP for floating session-chart preset windows) becomes moot once multi-window's slice F1/F2 retires those popouts — they become regular named windows that inherit VP through the standard per-chart config path.

## Overview

A small `⋮` glyph appears immediately right of the existing `VP` toolbar button. Clicking it opens a popup panel with the anchor radio + a width slider + two italic notes (gap-collapse fallback / anchor-too-fine). Settings persist per chart instance in `data/config.toml`.

The work touches **both** chart backends (legacy `midas-chart` and new `midas-scene` / `session_chart`) so users don't lose VP when toggling backends. The new stack uses `midas-calendar` natively; the legacy stack uses a small chrono-based shim acknowledged as throwaway-when-Phase-D-ships.

### Architecture diagram

```
┌─────────────────────────────────┐
│  data/config.toml               │
│   [charts.N.volume_profile]     │
│   [experimental]                │
│     disable_anchored_vp         │ ← global kill-switch (D15)
└──────────────┬──────────────────┘
               │ load / save (debounced)
               ▼
┌─────────────────────────────────┐         ┌──────────────────┐
│  ChartConfig::volume_profile    │ ◄──────►│  Gear popup      │
│  (midas-core, persisted form)   │         │  (midas-app)     │
└──────────────┬──────────────────┘         └────────┬─────────┘
               │ restore_panel / build_config        │ UpdateVpAnchor /
               ▼                                     │ UpdateVpWidthFraction
┌─────────────────────────────────┐ ◄────────────────┘
│  ChartState::volume_profile     │
│  (midas-chart, in-memory)       │
│  anchor: midas_core::Anchor     │
└──┬─────────────────────────┬────┘
   │ build_volume_profile     │ scene_builder.rs
   │ (uses midas_core::Anchor │ (CONVERTS via From in midas-app:
   │  directly)               │  midas_core::Anchor → midas_scene::Anchor)
   ▼                          ▼
┌─────────────────────┐    ┌──────────────────────────┐
│ LEGACY              │    │ NEW (session_chart)      │
│ compute_anchored_   │    │ VolumeProfileLayer       │
│ volume_profiles     │    │ (first-time wired in S3) │
│ → VolumePipeline    │    │ uses midas_scene::Anchor │
│                     │    │ → ScenePrimitives quads  │
└─────────────────────┘    └──────────────────────────┘
```

The `Anchor` enum is **duplicated** in two places (D5/D9): `midas_core::VolumeProfileAnchor` (persisted form, used everywhere downstream of the desktop workspace) and `midas_scene::VolumeProfileAnchor` (root workspace, used by `VolumeProfileLayer` only). `midas-app` provides `From<midas_core::VolumeProfileAnchor> for midas_scene::VolumeProfileAnchor` (and reverse) — this is the **only** crate that depends on both copies. The duplication respects Architecture Rule 9 (no root → desktop dep edge).

## Research Summary

### Codebase analysis

**VP today (the seam being extended):**
- `desktop/win/crates/midas-chart/src/volume_profile/mod.rs` — `compute_volume_profile()` returns `VolumeProfile { bins, poc_index, total_volume }` over `vis_start..vis_end`. Renders via `profile_to_instances()` → `VolumePipeline` (`midas-render/src/pipelines/volume.rs`). Single profile, visible-range, with buy/sell split.
- `crates/midas-scene/src/layers/volume_profile.rs` — `VolumeProfileLayer` is implemented (slice-7 simplified port, no buy/sell split) **but is not wired into the scene builder yet**. `gpu_renderer.rs:230` passes `volume_profile: &[]` hard-coded. First-time wiring is part of Slice 3.
- `desktop/win/crates/midas-chart/src/state/mod.rs:174` — `ChartState::show_volume_profile: bool`.
- `desktop/win/crates/midas-core/src/config/mod.rs:320` — `ChartConfig::show_volume_profile: bool` persisted in `data/config.toml`.
- `desktop/win/crates/midas-app/src/app/views.rs:717-728` — toolbar `VP` button. Title-bar row at `views.rs:764-776`.
- `desktop/win/crates/midas-app/src/app/handlers.rs:1001-1009` — `Message::ToggleVolumeProfile` flips the bool, calls `dirty.mark_data()` and `mark_config_dirty()`.

**Critical timestamp unit (verified):** `CandleData::timestamp(i) -> i64` is **epoch milliseconds** (`desktop/win/crates/midas-core/src/candle_data/mod.rs:33-34`). The boundary helper in Slice 2 uses `chrono::DateTime::from_timestamp_millis(ts)` accordingly.

**Popup pattern (in-repo precedent — link-picker / level editor / column selector):**
- State: `MidasApp::link_picker_open: Option<(PickerTarget, LinkDimension)>` (`app.rs:418`).
- Render (`views.rs:247-271`): `stack(chart_layers)` with backdrop `mouse_area(Space::new()).on_press(Dismiss)` + panel `container(...).align_x(Right).align_y(Top).padding(...)`.
- Builder: `views.rs:1076-1192` (`build_link_picker`) — column of `iced::widget::button(...)` rows in a styled `container`.

**Critical iced-0.14 gotcha** (per `plan/feature-popup-clickable.md`): clickable rows MUST be `iced::widget::button`, never `mouse_area().on_release(...)`. With `mouse_area`, the press passes through to the backdrop and dismisses the popup before the row's release fires.

**Glyph choice** (per `plan/feature-header-settings-button.md`): `⚙` (U+2699) renders inconsistently at small sizes on Windows 11 in this codebase. Use `⋮` (U+22EE) at `text(...).size(12)`. **Italic font is also unsupported** — codebase has zero italic precedent (verified). Use plain `text` + `ⓘ` glyph for the popup notes.

**`PaintContext` shape (verified):** `paint.rs` exposes `axis: &dyn TimeAxis`, `viewport`, `price_range`, `palette`, `price_axis`, `formatter`, `out`. **No `calendar` field.** Layers that need calendar info own a `&'static dyn ExchangeCalendar` directly (precedent: `SessionBandLayer` at `crates/midas-scene/src/layers/session_band.rs:27`). `ExchangeCalendar`'s timezone accessor is `fn tz(&self) -> Tz` (not `timezone()`).

**Cross-workspace dependency rule (verified):** No root crate currently depends on any desktop crate. CLAUDE.md Architecture Rule 9: "Dependency flow is strictly downward". Adding `midas-scene → midas-core` would be the first inverse cross-workspace edge — explicitly disallowed. The plan duplicates `VolumeProfileAnchor` in `midas-scene` (root) and `midas-core` (desktop) instead, with a `From`/`Into` bridge in `midas-app`. See D5 & D9.

**`detect_session_boundaries` is wrong for this** (`compute/mod.rs:796-832`): finds time gaps (overnight/weekend), not calendar period changes. Cannot be reused. We need a chrono-based helper in `midas-chart` (D7).

**Persistence pipeline:**
- Handler calls `self.mark_config_dirty()`.
- `Message::Tick` → `maybe_save_config()` → after 2-second debounce (`CONFIG_SAVE_DEBOUNCE_SECS = 2.0` at `app.rs:598`) → `flush_config()` → `config.save(path)` writes TOML async.
- Restoration on startup at `app.rs:2874` copies `chart_cfg.*` into `ChartPanel.chart_state`.

**Render coordinate gotcha (legacy):** `Camera2D::time_to_x` (`midas-chart/src/camera/mod.rs:43`) is a pure linear `(ts - time_start) / (time_end - time_start) * width`. **It has no `collapse_gaps` awareness.** When `collapse_gaps == true`, `compute_collapsed_scene` uses an internal closure `index_to_x` (`compute/mod.rs:424`) that maps candle index to x — but it's a closure, not a callable function. P6 (Slice 6) lifts it to a free helper.

**Render coordinate (new stack):** Existing layers use `ctx.axis.to_x(ts)`. The trait abstracts gap-collapsing transparently — the new stack has no equivalent of the legacy `collapse_gaps` problem.

**Tracing convention:** All VP code uses `tracing` target `"vp"`. New tracing call sites in S2/S3/S4 share this target.

### Best-practices summary (TradingView / Sierra Chart / NinjaTrader)

- **Per-period price domain**, not shared. Each profile uses its own min-low/max-high.
- **Left edge** pinned to the period's first candle; **right edge** stops at the next period boundary. Width capped at a percentage of the period's pixel span (50–80% typical).
- **POC per period** — independent argmax. Value-area (VAH/VAL) at 70% volume threshold is conventional; we **reserve** the schema fields in v1 and ship rendering in S6.
- **Buy/sell** from `close >= open` is widespread but acknowledged low-resolution. Legacy stack already does this; we keep it. New stack v1 stays aggregate-only.
- **Bin count:** TradingView default = 24 rows. Cap to `min(rows, floor(profile_pixel_height / 2))`.
- **Caching:** completed periods are immutable. Deferred to Slice 6 P1 with a measurement gate (P0).
- **Timezone for daily anchors:** anchor to the **exchange's local calendar day** — `XnysCalendar` for stocks; `chrono_tz::US::Eastern` for the legacy shim.
- **Performance:** ~250 visible profiles is a safe ceiling; ~50 daily profiles is trivially cheap (~1.2k GPU quads).

## Design Decisions

### D1. Stack strategy — implement in BOTH legacy and new stacks

**Context:** Per-chart `Message::ToggleChartBackend` lets users flip backends. Implementing only one stack means users on the other lose the feature.

**Alternatives considered:**
- *New stack only* — push users to migrate. Rejected because legacy is the default backend; many users never toggle. Visible regression for them.
- *Legacy stack only* — easy (no calendar dep) but new-stack VP stays unwired (matches today). Rejected: leaves the feature half-built and worsens Phase D drag.
- **Both stacks (chosen)** — strangler-fig discipline. Legacy gets a chrono shim acknowledged as throwaway when Phase D ships. New stack gets calendar-native treatment.

**Confidence:** High (Fowler strangler-fig is the textbook pattern).

### D2. Per-session VP layout — left-anchor at period's first candle

For each period `i`:
```
left_x = axis.to_x(period[i].first_candle.timestamp)
right_boundary_x = axis.to_x(period[i+1].first_candle.timestamp)   // viewport-right for trailing period
period_pixel_span = right_boundary_x - left_x
profile_width_max = period_pixel_span * settings.width_fraction
profile_width_max = profile_width_max.clamp(MIN_PROFILE_PX, MAX_PROFILE_PX)
                                                  // MIN = 8, MAX = 240 px
if period_pixel_span < MIN_PERIOD_PX_TO_RENDER { skip }     // = 12 px (S6 P2 degrades to 1-px POC tick)
```

Each bin's width = `(bin_volume / max_bin_volume) * profile_width_max`. Profiles do NOT overflow into the next period. **Confidence:** High.

### D3. Per-period price domain
Each period uses its own `(min_low, max_high)` over its own candle slice; POC is per-period. Matches TradingView. **Confidence:** High.

### D4. Settings popup — link-picker stack/backdrop/container pattern with `button` rows
`MidasApp::vp_settings_open: Option<ChartId>`. Render in the chart-area stack used by the link-picker. Backdrop = full-area `mouse_area(Space).on_press(Message::DismissVpSettingsPanel)`. **Every clickable row inside the panel is `iced::widget::button`.** Popup stays open until backdrop click or pane close (column-selector behaviour, not link-picker auto-close). **Confidence:** High.

### D5. Settings schema — single nested struct on `ChartConfig`; enum duplicated across workspaces

**Context:** Adding a `volume_profile` settings group with anchor + width + reserved value-area fields. Architecture Rule 9 forbids root crate `midas-scene` depending on desktop crate `midas-core`.

**Alternatives considered for the struct shape:**
- *Five flat `vp_*` fields on `ChartConfig`* — matches existing convention, but the prefix smell intensifies as v2 adds value-area fields. Rejected.
- **Nested `volume_profile: VolumeProfileSettings` (chosen)** — first feature group complex enough to warrant nesting; serialises as TOML sub-table.

**Alternatives considered for the enum location:**
- *Single enum in `midas-core`, consumed by `midas-scene`* — violates Architecture Rule 9. Rejected.
- *Single enum in a new shared root-workspace crate consumed by both* — adds a new tiny crate. Rejected as overkill for one enum (6 variants).
- **Duplicate enum (chosen)** — `midas_core::VolumeProfileAnchor` (persisted, used by legacy + UI), `midas_scene::VolumeProfileAnchor` (root, used by new-stack layer). `midas-app` provides `From`/`Into` bridge. Cost = ~12 lines of duplication; benefit = no architecture-rule violation, no new crate.

```rust
// midas-core/src/config/mod.rs — appended to ChartConfig
#[serde(default)]
pub volume_profile: VolumeProfileSettings,

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VolumeProfileSettings {
    #[serde(default)]
    pub anchor: VolumeProfileAnchor,
    #[serde(default = "default_vp_width_fraction")]
    pub width_fraction: f32,
    #[serde(default)]                                       // RESERVED for v2
    pub show_value_area: bool,
    #[serde(default = "default_vp_value_area_pct")]
    pub value_area_pct: f32,
}

impl VolumeProfileSettings {
    /// Clamp out-of-range values. Called on read AND on write.
    pub fn sanitized(&self) -> Self { /* clamps width_fraction to [0.05, 1.0],
                                          value_area_pct to [0.10, 0.95] */ }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum VolumeProfileAnchor {
    #[default] Viewport,
    Daily, Weekly, Monthly, Yearly,
    /// Forward-compat sink: a downgraded binary loading a config with a
    /// future-version anchor falls back here, then default behaviour.
    #[serde(other)]
    Unknown,
}

// crates/midas-scene/src/layers/volume_profile/anchor.rs — DUPLICATE of the
// six variants (no serde — used purely as a render-time enum). midas-app
// converts via From<midas_core::VolumeProfileAnchor> for midas_scene::....
```

`show_volume_profile: bool` remains the on/off master switch. We deliberately drop `max_profiles` and `show_poc` knobs from v1 — hard-cap at 100 internally, POC always shown. Re-add as fields if user requests. **Confidence:** High.

### D6. Per-instance, NOT shared by symbol/timeframe
User explicitly asked "settings just for that one specific chart instance". Settings live on `ChartConfig` (per-pane row in `data/config.toml`). **Confidence:** High.

### D7. Boundary detection — chrono helper in `midas-chart` (legacy only)

**Context:** New stack derives day key from `Candle.window.open` + calendar timezone. Legacy stack has only millisecond timestamps.

**Alternatives considered:**
- *Helper in `midas-core`* — leaf crate; avoids cross-crate move. Rejected as speculative generality (only legacy uses it; pulls chrono-tz into a foundational crate).
- *Helper in `midas-calendar`* — calendar code already there. Rejected: cross-workspace dep direction (same Architecture Rule 9 issue as D5).
- **Helper in `midas-chart::volume_profile::candle_period_boundaries` (chosen)** — only consumer is also there; helper dies with the crate when Phase D ships.

Default `tz = chrono_tz::US::Eastern` for v1 (correct for stocks; wrong for crypto on legacy stack — see Open Question 2). **Confidence:** High.

### D8. Legacy stack — fork render path, keep buy/sell split

**Context:** Existing `compute_volume_profile` does buy/sell + POC. Per-period mode needs a `Vec<VolumeProfile>` instead of one.

**Alternatives considered:**
- *Extend `compute_volume_profile` with optional per-period boundaries* — bloats the existing signature; harder to test the `Anchor::Viewport` regression path stays bit-identical.
- **New sibling `compute_anchored_volume_profiles` (chosen)** — leaves existing code untouched (regression-zero for the default path), reuses the per-bin allocator in a loop.

**In `collapse_gaps` mode, per-period anchoring is disabled** (D11). **Confidence:** High.

### D9. New stack — extend `VolumeProfileLayer`, **wire it for the first time**, own the calendar

**Context:** The layer exists but is unwired — `scene_builder.rs` never constructs it. Adding per-period anchoring requires the layer to know which calendar to use for day-key derivation.

**Alternatives considered for calendar plumbing:**
- *Widen `PaintContext` with a `calendar: &dyn ExchangeCalendar` field* — rejected: cascades to every existing layer, forces all layers to handle a None case for layers that don't need it (e.g., `CrosshairLayer`), and changes a fundamental sans-IO abstraction for one consumer.
- **Layer owns `&'static dyn ExchangeCalendar` directly (chosen)** — `SessionBandLayer` (`crates/midas-scene/src/layers/session_band.rs:27`) already does exactly this. Localised dependency, no abstraction churn, matches existing precedent. Cost: layer constructor takes one extra argument.

**Concrete steps Slice 3 must perform:**
1. Add a `volume_profile: bool` flag to `SceneLayers` config.
2. Plumb `state.show_volume_profile` from `ChartState` → `SceneLayers`.
3. Conditionally construct `VolumeProfileLayer` in `scene_builder.rs::build_scene` when the flag is true.
4. **Layer owns `&'static dyn ExchangeCalendar` directly** (precedent: `SessionBandLayer` at `crates/midas-scene/src/layers/session_band.rs:27`). `PaintContext` is NOT widened. `paint()` calls `self.calendar.tz()` — NOT `ctx.calendar.timezone()` (which doesn't exist).
5. Add a sibling `VolumeProfileConfig` carrying behaviour (anchor, width_fraction, max_profiles); mark `VolumeProfileStyle` as `#[non_exhaustive]`.
6. `paint()` partitions on derived day-key from `Candle.window.open` under a single `RwLockReadGuard`, emits per-period quads from a reused `Vec<QuadInstance>` scratch buffer.

**Confidence:** Medium-High (wiring is real first-time integration; calendar-on-layer pattern is proven).

### D10. Performance budget — internal cap of 100 profiles
Computation is O(visible_candles) regardless of N. Render emits ≤ `100 × 24 = 2400` quads. Hard-cap the most recent 100. Cache deferred to Slice 6 P1 with a measurement gate (P0). **Confidence:** High.

### D11. `collapse_gaps` interaction (legacy only)
`Camera2D::time_to_x` is linear-time. Per-period x-positions are wrong while collapse_gaps is on. **For v1, when `collapse_gaps == true` AND `volume_profile.anchor != Viewport`, render Viewport mode silently** AND **revert the toolbar mode indicator label to `VP` (drop the `·D/W/M/Y` suffix)** so the on-screen state stays internally consistent (D13). The popup carries an explanatory note. Full fix in S6 P6. **Confidence:** Medium.

### D12. Anchor ≥ timeframe fallback
`Daily` anchor on `1D` timeframe = visual garbage. **When `timeframe.unit() >= anchor.unit()`, render Viewport mode silently** AND revert toolbar label to `VP`. Popup shows note. Mapping: `Daily` blocks at TF ≥ 1D, `Weekly` at TF ≥ 1W, etc. **Confidence:** High.

### D13. Toolbar mode indicator with fallback awareness
When `volume_profile.anchor != Viewport` AND no fallback active (D11/D12), the `VP` button label becomes `VP·D / VP·W / VP·M / VP·Y`. **When a fallback IS active, the suffix is omitted (label reverts to plain `VP`)** so users don't see a label that contradicts what's drawn. The popup is the source of truth for what was selected vs what's effective. **Confidence:** High.

### D14. `Reset Chart` (R button) does NOT clear VP settings
Existing `R` resets camera + zoom only. VP settings persist across pane resets, mirroring `show_volume_profile` today. Documented in Slice 4 with a regression test. **Confidence:** High.

### D15. Global kill-switch for anchored VP

**Context:** Plan ships behind per-chart settings on both backends. If a serious render bug ships, hotfix-only rollback would require either a release or every user manually reverting per-chart settings. Risk: real but low-probability.

**Recommendation:** Add a top-level `[experimental] disable_anchored_vp: bool = false` field on `AppConfig` (NOT on `ChartConfig` — global, not per-chart). When `true`, both render branches receive `Anchor = Viewport` regardless of per-chart settings.

**Enforcement sites (Architecture Rule 9 compliant):** `midas-chart` and `midas-scene` are leaf crates that cannot reach `AppConfig`. The override is therefore computed in `midas-app` at the two assembly sites that already see both `AppConfig` and per-chart state:
- **Legacy stack:** the per-frame `ChartInput` builder in `midas-app` sets `chart_input.effective_vp_anchor = if app_config.experimental.disable_anchored_vp { Viewport } else { state.volume_profile.anchor }`. `build_volume_profile()` reads only `effective_vp_anchor`, never `state.volume_profile.anchor` directly. (S1 widens `ChartInput`; S2 wires the read site.)
- **New stack:** `midas-app::session_chart::scene_builder` computes `scene_anchor` with the same conditional and passes it into `VolumeProfileLayer::new(...)`. (S3.)

Per-chart settings on disk are preserved either way (toggling the kill-switch off restores the user's choice). Cost: ~10 LOC across two assembly sites; payoff: a single config edit reverts the feature workspace-wide without recompile.

**Confidence:** High.

### D16 (revised from D9 step 4). Z-order — VP renders BELOW candles by design

**Context:** `LayerZ::VOLUME_PROFILE = 350` and `LayerZ::CANDLE = 400` (verified `crates/midas-scene/src/layer.rs:77,79`), with explicit test (`layer.rs:230-237`) and doc comment (`layer.rs:71-77`) asserting "VP histogram paints UNDER candles". The slice-7 wiring established this invariant.

**Alternatives considered:**
- *Renumber to 475 (between INDICATOR=450 and HOLIDAY_MARKER=500)* — would put VP above candles. Rejected: requires updating the existing test, doc comment, and (potentially) other layers that assume the current ordering. High blast radius, marginal visual benefit.
- **Keep VP under candles (chosen)** — Anchored profiles extend horizontally from the period's first candle and span hundreds of pixels (D2's width formula). The histogram's tail (the rightmost portion sticking past the candle column) is the user-visible part — it's what the TradingView reference screenshot shows. Layering under the candles preserves the existing slice-7 invariant + tests, avoids a risky renumber, and matches the visual expectation that candles are the foreground subject.

**Confidence:** High. (Updated from earlier "verify in slice kickoff" stance.)

## Pre-S1 Reconnaissance (≤30 min, single dev)

Several slices contain "verify at kickoff" notes. Several were grep-resolvable and have been **inline-resolved here** during round-2 plan revision. The remaining items genuinely need verification at execution time.

### Already resolved (do NOT redo)

- **R1. Devloop dispatch dir** — `desktop/win/crates/midas-app/src/devloop/handler.rs` (the `InjectTickerMsg` match arm lives here; new `SetVpSettings` arm goes alongside).
- **R5. `SceneLayers` config struct** — `desktop/win/crates/midas-app/src/session_chart/scene_builder.rs:65 pub struct SceneLayers`, re-exported at `mod.rs:135`.
- **R6. `SceneLayer::paint(&self)` vs `&mut self`** — confirmed `&self` (`crates/midas-scene/src/layer.rs:115`). Combined with `SceneLayer: Send + Sync` (line 106), `RefCell` interior mutability is **not** an option. Plan dropped scratch-buffer optimization from S3 (see D9 / S3 "Per-paint allocation").
- **Z-order constants** — `LayerZ::VOLUME_PROFILE = 350`, `CANDLE = 400`, `INDICATOR = 450`, `HOLIDAY_MARKER = 500` (`crates/midas-scene/src/layer.rs:77,79`). VP under candles by design (D16).
- **Devloop proto strict deserialise** — confirmed: `Command` enum is internally-tagged with no `#[serde(other)]`. Old apps + new scripts will return parse error. S1 documents wire-version requirement; `SetVpSettings` uses the `serde_json::Value` payload pattern (per `InjectTickerMsg`) to avoid pulling `midas-core` into devloop-proto.
- **`PaintContext` shape + `ExchangeCalendar::tz()`** — confirmed: no `calendar` field on `PaintContext`; method is `tz()` not `timezone()`. Layer owns `&'static dyn ExchangeCalendar` directly (SessionBandLayer pattern).
- **`restore_panel` location** — `desktop/win/crates/midas-app/src/app.rs` (around line 2874), NOT `persistence.rs`. `build_config` lives in `persistence.rs:23+`. S1 + S2 file lists corrected.

### Items still needing verification (resolved 2026-04-25)

**All R2–R11 resolved** in the "Pre-S1 Recon Addendum" section at the
bottom of this file (jump there for full findings). Quick summary:

| # | Item | Resolution (pointer to addendum) |
|---|------|-----------|
| R2 | `Timeframe::unit_minutes()` | Does NOT exist; S2 adds inline at `desktop/win/crates/midas-core/src/timeframe/mod.rs:42`. Or use existing `as_secs()` directly — S2's call. |
| R3 | `BarPeriod`/`SessionSpan` `unit_days()` | Do NOT exist; S3 adds a private free function in `crates/midas-scene/src/layers/volume_profile.rs` (NOT in `midas-calendar`). |
| R7 | `Message::PaneClose` handler | `desktop/win/crates/midas-app/src/app/handlers.rs:408`. |
| R8 | `midas-ui::Tooltip` | `desktop/win/crates/midas-ui/src/tooltip.rs:23`. S6 P3 prefers `IconButton::tooltip(&str)` builder for toolbar buttons. |
| R9 | `chrono-tz` pin | `0.10` per-crate pin in 3 root crates; desktop workspace has NO pin. S1 adds `chrono-tz = "0.10"` to `desktop/win/Cargo.toml [workspace.dependencies]`. |
| R10 | CI runs `--features session_chart`? | YES — `desktop_session_chart_lint` + `desktop_session_chart_tests` jobs (both `continue-on-error: true`). NO workflow change needed in S1. |
| R11 | `Message` size budget | `<= 256` (const-assert at `app.rs:1141-1144`). Per-knob S4a split is well under budget; no boxing required. |
| R12 | Devloop reference PNGs (carved from OQ5) | **Committed to git** under `desktop/win/tests/data/screenshots/`. S2/S3 commit + hand-verify in same PR. |

### Recon Done-when (✅ complete 2026-04-25)

- ✅ Addendum committed at the bottom of this `00-index.md` recording findings for R2–R11 (+ R12 from OQ5).
- ✅ R2/R3/R7/R8: missing-API entries in the addendum name the file+line where the helper will be added.
- ✅ R10: feature gate already runs in CI (non-blocking jobs); S1's "Files to modify" does NOT include the workflow.
- ✅ R11: `Message` size budget healthy with the per-knob split; no boxing required in S4a.
- ✅ R12 (OQ5): reference PNGs committed to git under `desktop/win/tests/data/screenshots/`.

## Implementation Plan

Six slices, ordered render-before-UI so user-visible behaviour ships in S2/S3 with the popup on top in S4.

### Slice index

| # | File | Goal | Depends on |
|---|------|------|------------|
| Recon | (this file, addendum) | Resolve R2–R11 in one batch | None (do first) |
| S1 | [01-slice-1-schema-and-devloop.md](01-slice-1-schema-and-devloop.md) | Schema (nested struct + duplicate-enum bridge + kill-switch) + persistence + devloop `SetVpSettings` JSON command | Recon |
| S2 | [02-slice-2-legacy-render.md](02-slice-2-legacy-render.md) | Per-period VP on the legacy stack; **owns `vp-legacy-*.png` reference PNGs** | S1 |
| S3 | [03-slice-3-new-stack-render.md](03-slice-3-new-stack-render.md) | Per-period VP on the new stack; first-time wiring; **owns `vp-new-*.png` reference PNGs** | S1 |
| S4a | [04-slice-4-gear-popup-ui.md § S4a](04-slice-4-gear-popup-ui.md) | `MidasApp::vp_settings_open` + Toggle/Dismiss/UpdateVpAnchor/UpdateVpWidthFraction handlers + pane-close cleanup + handler unit tests. **No popup render.** | S1 |
| S4b | [04-slice-4-gear-popup-ui.md § S4b](04-slice-4-gear-popup-ui.md) | Toolbar `⋮` icon + popup panel + mode-indicator label + screenshot Done-when. **Requires a working render path.** | S4a + (S2 OR S3) |
| S5 | [05-slice-5-cross-stack-parity.md](05-slice-5-cross-stack-parity.md) | Cross-stack parity tests + 2 fallback PNGs (per-stack screenshots already in S2/S3) | S2 + S3 |
| S6 | [06-slice-6-polish.md](06-slice-6-polish.md) | P0 baseline → P1 cache, P2 narrow-period tick, P3 hover, P4 value-area, P5 up/down split, P6 collapse-gaps full fix | S2 + S3 |

### Dependency graph

```
Recon (≤30m) ─→ S1 (schema + devloop) ──┬─→ S2 (legacy render, owns vp-legacy-*.png) ──┬─→ S5 (cross-stack parity)
                                        ├─→ S3 (new render + wiring,                  ─┘
                                        │       owns vp-new-*.png)                       └─→ S6 (polish)
                                        └─→ S4a (state + handlers, mergeable after S1)
                                              └─→ S4b (popup render + screenshot test, requires S2 OR S3)
```

**Critical path:** `Recon → S1 → max(S2, S3) → S5`. **S3 is the higher-risk leg** (first-time wiring, calendar-on-layer pattern) — staff with the more experienced dev or schedule first.
**Parallelism by team size:**
- **3 devs:** dev A → S3 (highest risk, on critical path); dev B → S2; dev C → S4a (mergeable after S1, then waits for S2/S3 to start S4b).
- **2 devs:** senior → S3 → S4b; second → S2 → S4a → S5.
- **1 dev:** linear `S1 → S3 → S2 → S4a → S4b → S5 → S6` (S3 first because it's higher-risk and on the critical path either way; S6 last because it's optional polish).

## Risks & Unknowns

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| Timestamp unit confusion (sec vs ms) | Confirmed | Critical if missed | S2 uses `from_timestamp_millis` + ms-scale clamp + ms-scale regression test. |
| `VolumeProfileLayer` wiring is greenfield work, not "extend existing" | Confirmed | Med | S3 explicitly scopes the wiring (D9 step 1-3). |
| `PaintContext` has no `calendar` field; method is `tz()` not `timezone()` | Confirmed | Med | D9 step 4 — layer owns `&'static dyn ExchangeCalendar` directly (SessionBandLayer pattern). |
| Cross-workspace dep `midas-scene → midas-core` violates Architecture Rule 9 | Confirmed | High if attempted | Duplicate-enum strategy in D5/D9; bridge in `midas-app`. |
| `collapse_gaps + anchor != Viewport` produces wrong x positions | Confirmed | Med | D11 — silent fallback to Viewport in collapsed mode for v1; toolbar label reverts to plain `VP` (D13); popup note. |
| Anchor ≥ timeframe produces garbage | Confirmed | Low | D12 — silent fallback + label revert + popup note. |
| iced 0.14 popup row click bug (`feature-popup-clickable.md`) | Low | High if missed | Plan + S4 require `button` rows. |
| `⚙` glyph rendering on Win11 | Confirmed | Med | Use `⋮` (U+22EE) at `size(12)`. |
| Italic font unsupported in this codebase (verified zero precedent) | Confirmed | Low | Use plain `text` + leading `ⓘ` glyph. |
| Per-instance timezone (D7) for crypto on legacy stack | Med | Med | Hard-code ET in v1; document in popup; recommend crypto users switch to New backend. |
| Concurrency: write-during-paint on `Arc<RwLock<CandleSeries>>` | Low | High | S3 mandates a single `RwLockReadGuard` covers partition + bin. |
| Screenshot determinism (DPI scaling, animation in flight) | Med | Med | S5 enforces fixed window size, DPI=100%, `WaitForIdle` to drain animations before `Screenshot`. SSIM ≥ 0.98 per stack, ≥ 0.85 cross-stack. |
| `chrono-tz` not in desktop workspace deps | Confirmed | Low | Recon R9 confirmed: pin is `0.10` (per-crate in 3 root crates). S1 adds `chrono-tz = "0.10"` to `desktop/win/Cargo.toml [workspace.dependencies]`. |
| Phase D will delete legacy code | Confirmed | Low | Acknowledged throwaway; chrono helper lives in `midas-chart` and dies with it. |
| Multi-pane popup state cleanup on pane close | Med | Med | Mirror link-picker cleanup in `app.rs:418`; pane-close handler resets `vp_settings_open` if it pointed there. |
| Devloop forward-compat: old app + new script breaks | Confirmed | Low | S1 documents wire-version requirement; optional `#[serde(other)]` on `Command` enum. |
| TOML enum serialization style | Low | Low | Round-trip test in S1; `#[serde(rename_all = "PascalCase")]` to get `anchor = "Daily"`. |
| Future-version downgrade with unknown anchor variant | Low | Low | `#[serde(other)] Unknown` variant in `VolumeProfileAnchor`. |
| CI doesn't run `--features session_chart` | Confirmed-resolved | Low | Recon R10: `desktop_session_chart_lint` + `desktop_session_chart_tests` jobs already cover it (both `continue-on-error: true` until the project-wide flip-to-required follow-up). |
| Z-order conflict with brackets/annotations | Low | Med | VP layer renders **above candles + grid + session-band, below annotations + brackets**. Spelled out in D9 / S3. |
| Schema additivity / rollback | Low | Low | All new fields have serde defaults; rollback to pre-feature binary loses values but does not crash. |
| Operational rollback (kill-switch) | Low | High if needed, no fix | D15 — `[experimental] disable_anchored_vp = true` in `data/config.toml` forces all charts to Viewport regardless of per-chart settings. Single config edit reverts the feature without a release. |
| Persistence-failure surfacing | Low | Low | Recon verifies `flush_config()` already logs via `tracing::error!`; if not, add it (one-line in S1). |
| `index_to_x` is a closure not a function (P6) | Confirmed | Low | S6 P6's first task is to lift it to a free helper. |

## Testing Strategy

- **Schema (S1):** unit tests in `midas-core/src/config/tests.rs` following `save_load_roundtrip_preserves_all_fields` (line 98+). Round-trip every new field; load a legacy TOML without `[volume_profile]` table and assert defaults; load a TOML with `Unknown` anchor; verify clamp on read+write.
- **Pre-feature config compatibility (S2):** explicit regression test — pre-feature config (`show_volume_profile = true`, no `[volume_profile]` table) loads + renders identically to current `main` (verified by `vp-legacy-viewport.png` regression baseline captured before this feature).
- **Boundary helper (S2):** unit tests in `midas-chart/src/volume_profile/` — DST spring-forward & fall-back, month-end, year-end, ISO-week 52→1, weekend gap, NYSE half-day, Christmas zero-candle day, `i64::MIN/MAX` clamp. **Ms-scale fixtures** (e.g., `1717200000000` for 2024-06-01) — explicitly NOT seconds.
- **Per-period compute (S2 + S3):** synthetic-fixture tests; max-profiles cap; anchor-≥-timeframe fallback; collapse_gaps fallback (S2 only — new stack uses calendar-aware axis instead). **Concurrent-write stress test (S3 only)** — `Arc<RwLock<CandleSeries>>` is a new-stack concern; legacy `compute_volume_profile` reads via `&dyn CandleData` with no equivalent.
- **UI (S4):** handler unit tests for state mutations + popup-orphan-after-VP-off + pane-close-clears-state + reset-chart-preserves-vp.
- **End-to-end (S5):** devloop screenshot tests — cross-stack parity + backend-flip sequence. Per-stack screenshots are owned by S2/S3 themselves.
- **Regression:** `Anchor = Viewport` with default settings produces SSIM ≥ 0.99 vs current behaviour.
- **CI:** both feature-gate combos build + test + clippy clean (verified by Recon).

## Non-Goals / Out of Scope

- **Value Area (VAH/VAL) rendering** — schema fields reserved in S1; rendering is S6 P4.
- **Volume Profile HD / auto-rebin on zoom** — TradingView's adaptive-row feature.
- **Custom session windows** (user-defined Pre-market only / London session).
- **Per-trade buy/sell split** — Hand of Midas doesn't have per-trade data; legacy stack uses `close >= open` heuristic; surface as "(estimated)" if shown in tooltips.
- **VP for floating session-chart preset windows** (`BTC M1` / `AAPL M5` / `SPY D1·RTH` toolbar buttons that open standalone windows). Different toolbar shape; tackle in a follow-up.
- **Cross-pane sync** of VP settings — user explicitly asked for per-instance.
- **Anchored VP drawing tool** (TradingView's user-drawn anchor) — different feature.
- **Migration of existing `show_volume_profile = true` charts** — old charts keep `Viewport` (the default), preserving current look.
- **`max_profiles` and `show_poc` user-tunable knobs** — hard-coded for v1.

## Open Questions for Human

1. **`[blocks: S1]`** Default anchor when user enables VP for the first time — `Viewport` (current behaviour, no surprise) vs `Daily` (matches reference screenshot). ✅ **RESOLVED 2026-04-25**: default is `Viewport` — single histogram across the entire visible viewport, not sectioned. Matches today's behaviour. Plan already defaults to this in `VolumeProfileSettings::default()` (S1 schema); no change required.
2. **`[non-blocking, can defer to v2]`** Crypto on legacy stack — ET-anchored daily profiles will look wrong. Acceptable v1 limitation (recommend crypto users switch to New backend)? Or block per-period modes when on legacy + non-equity?
3. **`[blocks: S4 visual review]`** Width-fraction default per mode — `Viewport: 0.25` matches today; per-period default proposed at `0.70`.
4. **`[blocks: S4 visual review]`** Slider vs +/- chips for width % — iced 0.14 slider styling may clash with dark theme. Plan ships slider; falls back to chips if dev-loop screenshot looks bad.
5. **`[blocks: S2 + S3 + S5]`** Devloop reference-PNG generator — committed to git, or generated on first run and gitignored? Plan assumes committed. (S2 and S3's Done-when explicitly require committed PNGs, so this question gates the entire render-slice phase, not just S5.) Resolve during Recon as item R12.
6. **`[non-blocking, can defer]`** Floating session-chart preset windows — exclude from v1 (their toolbar differs)? Plan excludes.
7. **`[non-blocking, can defer]`** First-use discoverability hint — auto-open the popup the first time `show_volume_profile` flips on after upgrade? Plan currently leaves it to users.

## Review Notes

- **Plan v3.1 changes vs v3** (post `/plan-eval` round 3 — surgical revisions, no design changes):
  - High fix: stale `Message::UpdateVpSettings` references in S4 code samples (anchor row, width slider) replaced with the per-knob form (`Message::UpdateVpAnchor`, `Message::UpdateVpWidthFraction`) introduced in v3.
  - High fix: D15 kill-switch enforcement clarified — the override is computed in `midas-app` (legacy: `ChartInput.effective_vp_anchor`; new: `scene_builder.rs::scene_anchor`), NOT inside the leaf crates. Avoids an Architecture Rule 9 violation that v3's "one-line guard in `build_volume_profile()`" wording implied.
  - Medium fix: S1 `Files to modify` widened with `desktop/win/crates/midas-chart/src/input.rs` (or wherever `ChartInput` lives) to add the `effective_vp_anchor` field, and the assembly site in `midas-app` that computes the override per frame.
  - Medium fix: S3 gained kill-switch enforcement at `scene_builder.rs` (test #15) + a Risks entry naming the enforcement site.
  - Medium fix: field path `app_config.experimental.disable_anchored_vp` used consistently across S1, S2, S3 (was inconsistent in v3).
  - Low fix: S4 `handlers.rs` bullet moved from S4b's heading to S4a's heading, matching the slice-split intent.
  - New tests: 5a/5b in S1 (kill-switch ChartInput override + AppConfig default), #15 in S3 (kill-switch scene_builder override).

- **Plan v3 changes vs v2** (post `/plan-eval` round 2):
  - High fix: Z-order intent committed (D16) — VP renders BELOW candles by design; rationale documented (visible portion is the rightmost histogram tail extending past candle widths). No layer renumber.
  - High fix: scratch-buffer optimisation dropped from S3 (`SceneLayer: Send + Sync` forbids `RefCell` interior mutability). Per-paint allocation matches existing slice-7 behaviour; defer optimisation to S6 P1.
  - Med fix: devloop `SetVpSettings` uses `serde_json::Value` payload (matches `InjectTickerMsg` precedent); preserves `midas-devloop-proto`'s "no domain dependencies" rule.
  - Med fix: `Message` payload split into per-knob variants (`UpdateVpAnchor`, `UpdateVpWidthFraction`) — keeps Message size budget healthy and avoids slider-tick storm.
  - Med fix: D15 global kill-switch `[experimental] disable_anchored_vp` for operational rollback without binary release.
  - Med fix: architecture diagram annotated with the duplicate-enum bridge.
  - Med fix: S5 / P6 cross-slice PNG coordination noted; P6 explicitly handles `vp-collapse-gaps-fallback.png` removal.
  - Med fix: S4 Files-to-modify partitioned into S4a / S4b sub-sections.
  - Med fix: Recon "verify at kickoff" items split — grep-resolvable items (R1, R5, R6, Z-order, devloop strict deserialise, restore_panel location, PaintContext shape, ExchangeCalendar method name) inline-resolved during this revision; remaining items (R2, R3, R7-R11) explicitly time-boxed with Done-when criteria.
  - Low fix: critical-path / parallelism guidance added per team size; D11/D12 fallbacks emit transition-gated tracing; slice index split into S4a/S4b rows; testing-strategy concurrency attribution; Q5 tag promoted; D9 Alternatives block added; D16 added; restore_panel location corrected in S1/S2 file lists.

- **Plan v2 changes vs v1** (post `/plan-eval` round 1):
  - Critical fix: timestamp unit (sec → ms) in S2 boundary helper.
  - High fix: layer owns `&'static dyn ExchangeCalendar` (not via `PaintContext`).
  - High fix: duplicate `VolumeProfileAnchor` enum across workspaces (Architecture Rule 9 compliance) instead of cross-workspace dep.
  - High fix: S4 dependency split (S4a state + S4b render); S5 PNGs moved into S2/S3 ownership.
  - Med fix: explicit motivation paragraph; alternatives blocks for D1, D5, D7, D8.
  - Med fix: D11/D13 toolbar label reverts to plain `VP` during fallback (consistency).
  - Med fix: italic font dropped (zero precedent in codebase); `ⓘ` glyph + `text::tertiary`.
  - Med fix: pre-S1 Recon task batches all "verify at kickoff" unknowns.
  - Med fix: S6 P0 baseline measurement before P1 cache; P6 closure-lift framing.
  - Low fix: critical path corrected to `S1 → max(S2, S3) → S5`; tracing convention; architecture diagram; sanitized() in D5 schema; Open Questions tagged with [blocks].

- Two prior in-repo plans materially shaped this plan: `feature-popup-clickable.md` (mandatory `button` rows) and `feature-header-settings-button.md` (use `⋮` not `⚙`).
- **Slice ordering** is render-then-UI; user-visible behaviour ships in S2/S3 verifiable via devloop, with the popup polish on top in S4.
- **Settings as a nested `volume_profile: VolumeProfileSettings` struct**, against the existing flat `ChartConfig` convention — chosen because (a) v2 will add value-area fields, (b) the prefix-noise of 5+ flat fields is a smell, (c) TOML sub-tables read better in `data/config.toml`.
- **`max_profiles` and `show_poc` knobs were dropped** from v1. Hard-coded internally (cap = 100, POC always on). Reduces UI surface and config noise. Re-add as fields if requested.
- **`candle_period_boundaries` in `midas-chart`**, not `midas-core` — only legacy uses it; pulling chrono-tz into a leaf crate to serve one consumer is speculative generality. Helper dies with `midas-chart` when Phase D ships.
- **`#[serde(other)] Unknown`** on `VolumeProfileAnchor` for forward-compat. Downgraded binary won't crash on a future-variant config.
- **Caching** (S6 P1) deferred until P0 baseline measurement says it's needed.

---

## Pre-S1 Recon Addendum (resolved 2026-04-25)

Recon items R2–R11 from the "Pre-S1 Reconnaissance" section above, plus
R12 carved out of Open Question 5. Each entry records the verified
finding so a slice can act on it without re-grepping. Findings are
authoritative as of commit `e5fc1f1` (HEAD on `main`).

### R2 — `Timeframe::unit_minutes()` does **not** exist (S2 adds it)

- `Timeframe` enum: `desktop/win/crates/midas-core/src/timeframe/mod.rs:13`
- Existing methods on `impl Timeframe` (line 42): `as_secs()`,
  `file_suffix()`, `display_name()`, `from_suffix()`, `is_calendar()`,
  `floor_timestamp()`, `next_boundary()`. **No `unit_minutes()`**.
- **S2 action**: add `pub const fn unit_minutes(&self) -> u32` as a
  one-line-per-variant `match`, alongside `as_secs()` (line 45). Sub-minute
  variants (`S1`, `S5`, `S15`, `S30`) return `0` — the plan's D12
  "anchor ≥ timeframe" guard never fires for sub-minute timeframes
  because daily/weekly/monthly/yearly anchors are all ≥ 1 minute.
  `M1`→1, `M5`→5, `M15`→15, `M30`→30, `H1`→60, `H4`→240, `D1`→1440,
  `W1`→10_080, `MN1`→43_200. (Note: `as_secs()` already returns enough
  precision for the D12 comparison; introduce `unit_minutes()` only if
  S2 finds the call sites read better in minutes. Otherwise S2 may
  drop this method entirely and use `as_secs()` directly — the recon
  result is the same: no existing helper to reuse.)

### R3 — `BarPeriod::*::unit_days()` and `SessionSpan::unit_days()` do **not** exist (S3 adds inline)

- `BarPeriod` enum at `crates/midas-calendar/src/period.rs:22`,
  `SessionSpan` at line 52, `CalendarSpan` at line 68.
- `impl BarPeriod` block (line 75) holds only smart constructors:
  `m1`, `m5`, `h1`, `d1_rth`, `d1_eth`, `w1`, `mn1`. **No `unit_days`
  / `unit_minutes` / unit accessor of any kind.**
- **S3 action**: add a private inline helper inside
  `crates/midas-scene/src/layers/volume_profile.rs` (NOT in
  `midas-calendar` — speculative generality across all consumers for
  one slice's needs) that maps a `BarPeriod` to a numeric "anchor unit"
  for the D12 comparison. Pattern: a free function
  `fn period_unit_days(p: BarPeriod) -> u32` with arms `Clock(_)` →
  small fixed mapping by minutes-to-days, `Session(_)` → 1,
  `Calendar(Week)` → 7, `Calendar(Month)` → 30, `Calendar(Quarter)` →
  90, `Calendar(Year)` → 365. The function is private to the layer.

### R7 — `Message::PaneClose` handler location

- Variant defined: `desktop/win/crates/midas-app/src/app.rs:650`
  (`PaneClose(pane_grid::Pane)`).
- Routed via `dispatch_pane_msg` at `app.rs:3925`
  (`Message::PaneClose(..) => self.handle_pane_msg(message)`).
- **Handler body**:
  `desktop/win/crates/midas-app/src/app/handlers.rs:408`
  (`Message::PaneClose(pane) => { ... }`).
- Other `.on_press(Message::PaneClose(pane))` call sites in
  `views.rs`: lines 843, 1262, 1594, 2237 (chart, watchlist, order,
  account headers respectively).
- **S4a action**: in the `handlers.rs:408` handler, after the
  existing pane-close logic resolves which `ChartId` (if any) was
  hosted in the closing pane, clear `self.vp_settings_open` if it
  matches that `ChartId`. Same pattern as the existing
  `link_picker_open` cleanup.

### R8 — `midas-ui` tooltip widget paths

- Module: `desktop/win/crates/midas-ui/src/tooltip.rs` (entire file is
  the widget; 103 lines).
- Type: `pub struct Tooltip<'a, Message>` at `tooltip.rs:23`.
- Re-exported: `desktop/win/crates/midas-ui/src/lib.rs:36` —
  `pub use tooltip::Tooltip;`.
- Public API:
  ```rust
  Tooltip::new(content: Element<'a, Message>, tip_text: &'a str)
      .position(iced::widget::tooltip::Position)   // default Bottom
      .gap(f32)                                    // default 4.0
      .view(&UiTheme) -> Element<'a, Message>
  ```
- Convenience: `IconButton::tooltip(text)` builder
  (`midas-ui/src/icon_button.rs:108`) wraps an icon button with a
  Tooltip when rendered. **S6 P3 hover should prefer
  `IconButton::tooltip(text)` for the toolbar buttons** (matches the
  existing icon-button hover pattern); use the bare `Tooltip` wrapper
  only for non-button hover surfaces.

### R9 — `chrono-tz` pin must be added to desktop workspace

- **Root workspace**: `chrono-tz = "0.10"` is pinned per-crate (NOT
  workspace-level) in three crates:
  - `crates/midas-axis/Cargo.toml:9`
  - `crates/midas-calendar/Cargo.toml:9`
  - `crates/midas-scene/Cargo.toml:9`
  - The root `Cargo.toml` `[workspace.dependencies]` table does NOT
    pin `chrono-tz` workspace-wide.
- **Desktop workspace**: zero references to `chrono-tz` in any
  desktop `Cargo.toml`. The `[workspace.dependencies]` table at
  `desktop/win/Cargo.toml:79-187` does not include it.
- **S1 action**: add `chrono-tz = "0.10"` to the desktop workspace's
  `[workspace.dependencies]` table at `desktop/win/Cargo.toml`
  (insert near the existing `chrono = { version = "0.4", features = ["serde"] }` at line 141 to keep time-related deps grouped). Then in
  `desktop/win/crates/midas-chart/Cargo.toml` add
  `chrono-tz = { workspace = true }` to `[dependencies]`.
- **Pin parity**: matches the root workspace's `0.10` per-crate pins —
  no version conflict.

### R10 — `--features session_chart` already runs in CI (with caveats)

- `.github/workflows/rust.yml` has THREE relevant jobs covering VP:
  - **`broker`** (line 18, BLOCKING) — runs `cargo test --workspace`
    at the root. Covers `midas-scene::VolumeProfileLayer` unit tests
    (`crates/midas-scene/src/layers/volume_profile/tests.rs`) since
    those live in the **root** workspace and don't need the
    `session_chart` feature.
  - **`desktop`** (line 66, BLOCKING) — runs
    `cargo test --workspace` in `desktop/win/`. Covers
    `midas-chart::compute::build_volume_profile` legacy-stack tests
    and the schema/persistence tests in `midas-core`.
  - **`desktop_session_chart_lint`** (line 107, **`continue-on-error: true`**)
    — clippy with `--features session_chart` on `midas-app`. Covers
    the `midas-app::session_chart::scene_builder` wiring (where the
    `From<midas_core::VolumeProfileAnchor>` bridge and the kill-switch
    enforcement live).
  - **`desktop_session_chart_tests`** (line 149, **`continue-on-error: true`**)
    — full-workspace test with `--features session_chart_tests`
    (transitively enables `session_chart`). Covers the
    `midas-app::session_chart` wiring tests.
- **Test-failure attribution at a glance**:
  - Layer-internal regression (partition, bin-compute, POC) →
    `broker` job (root `cargo test`). RED-blocks PR.
  - Legacy-stack regression (`compute_anchored_volume_profiles`,
    `candle_period_boundaries`, schema round-trip) → `desktop` job.
    RED-blocks PR.
  - Wiring / kill-switch / scene_builder regression →
    `desktop_session_chart_*` jobs. **YELLOW-warns** until
    flip-to-required.
- **Trade-off (explicit)**: a regression in the new-stack wiring
  (scene_builder.rs, the From bridge, the kill-switch override
  computation) surfaces as a yellow warning rather than a red block
  until the project-wide flip-to-required follow-up lands. This is
  acceptable because:
  1. Layer-internal correctness is gated by the blocking `broker`
     job — the most likely regression site.
  2. The S3/S5 screenshot tests are the operational gate for visual
     regressions; both run before merge.
  3. The kill-switch (D15) is a single-config-edit operational
     rollback if a wiring regression slips.
- A "flip-to-required" follow-up (described in the workflow comment
  at line 96) drops `continue-on-error: true` from the four
  non-blocking desktop jobs in one go; tracked outside this plan.
- **S1 action**: NO workflow change required. The feature gate is
  already covered. S1's "Files to modify" list does NOT add
  `.github/workflows/rust.yml`.

### R11 — `Message` size budget healthy with per-knob split

- Const-assert: `desktop/win/crates/midas-app/src/app.rs:1141-1144` —
  `assert!(std::mem::size_of::<Message>() <= 256, "Message enum
  exceeds size budget — box a large variant payload");`. Comment
  states "headroom for ~1 small new variant".
- **S4a's four new variants** (per the v3.1 per-knob split):
  - `ToggleVpSettingsPanel(ChartId)` — ChartId = `u32` newtype, 4 B + tag = ≤8 B.
  - `DismissVpSettingsPanel` — discriminant only, ≤8 B.
  - `UpdateVpAnchor(ChartId, VolumeProfileAnchor)` — `u32` + 1-byte
    enum + padding ≤ 12 B.
  - `UpdateVpWidthFraction(ChartId, f32)` — `u32` + `f32` ≤ 12 B.
- All four are **smaller than the current largest variant** (which is
  `RouterReadyPayload`-bearing variants and other handle-carriers, all
  on the order of 16–32 B). Adding them does NOT grow
  `size_of::<Message>()`.
- **Verdict**: const-assert holds post-S4a. The per-knob split (vs.
  a single `UpdateVpSettings(ChartId, VolumeProfileSettings)` payload
  which would be ~32–48 B and risk slider-tick storm) is correct and
  is already the v3.1 plan choice.
- **S4a action**: no boxing required. Land variants as plain.

### R12 — Devloop reference PNGs (Open Question 5)

- This was carved out of Open Question 5 ("committed to git, or
  generated on first run and gitignored?") because S2 and S3's
  Done-when criteria gate on PNG location.
- **Resolution (default — overridable by user)**: PNGs are
  **committed to git** under `desktop/win/tests/data/screenshots/`
  (mirroring the eth-shading plan S5's location for
  `aapl-eth-day` reference image). Generated by the dev-loop the
  first time a slice's screenshot Done-when runs; reviewed by hand
  and committed in the same PR.
- **Rationale**: regression detection requires a stable baseline. A
  gitignored / generated-per-run baseline silently passes a
  regressed render (the new run becomes the new "truth").
- **Storage cost**: each PNG is ~30–80 KB at 1280×720. The full set
  per S2/S3 is ~6 PNGs (Viewport / Daily / Weekly / collapse-gaps
  fallback / anchor-ge-timeframe fallback) × 2 stacks = ~12 PNGs
  ≈ 0.5–1 MB total. Acceptable for a git repo of this size.
- **S2/S3 action**: commit the PNGs in the same PR as the slice;
  reviewer must hand-verify they look right before approval. Open
  Question 5 is now closed at the recon addendum level; the user can
  override during execution if they prefer gitignored.

### Recon items NOT carved out (already inline-resolved)

R1 (devloop dispatch dir), R5 (`SceneLayers` location), R6
(`SceneLayer::paint` immutability), Z-order constants, devloop strict
deserialise, `restore_panel` location, `PaintContext` shape,
`ExchangeCalendar::tz()` — all resolved inline in the plan body
during round-2 revision. Do not redo.

### Open Questions still open after recon

Open Questions 1, 2, 3, 4, 6, 7 from the "Open Questions for Human"
section above remain unresolved. They are **non-blocking for S1**
(only Q5 was a blocker; the recon resolves it as R12). S2 and S3 may
hit Q1 (default anchor) and Q3 (default width-fraction per mode);
those decisions can be made during slice review.
