# Slice 3 — Per-Period VP on the New (`session_chart`) Stack + First-Time Wiring

**Goal:** When `state.volume_profile.anchor != Viewport` AND `show_volume_profile == true`, the **new** `midas-scene` stack renders one VP per period using `Candle.window.open` + the layer's owned `&'static dyn ExchangeCalendar`. **Also includes first-time wiring of `VolumeProfileLayer` into the scene builder** — the layer module exists but is not actually rendered today (`gpu_renderer.rs:230` passes `volume_profile: &[]` hard-coded).

**Depends on:** S1 (schema + duplicate-enum bridge + devloop).

**Out of scope:** UI (S4). Buy/sell split (Slice 6 P5). `collapse_gaps` doesn't apply to the new stack — `CompressedAxis` and `ContinuousAxis` handle gap-collapsing natively via `axis.to_x(ts)`.

## Files to modify

- `crates/midas-scene/src/layers/volume_profile.rs`:
  1. Mark `VolumeProfileStyle` as `#[non_exhaustive]` (future-proofing — no current external constructor breaks).
  2. Add a sibling `VolumeProfileConfig` struct carrying behaviour fields:
     ```rust
     #[derive(Clone, Debug)]
     pub struct VolumeProfileConfig {
         pub anchor: VolumeProfileAnchor,        // root-workspace duplicate (see below)
         pub width_fraction: f32,
         pub max_profiles: usize,                // hard-coded 100 by caller
     }
     impl Default for VolumeProfileConfig { ... }
     ```
  3. Extend `VolumeProfileLayer::new` (and `with_defaults`) to accept `config: VolumeProfileConfig` AND `calendar: &'static dyn ExchangeCalendar`. Store both on `self`. **Layer owns the calendar reference directly** — same pattern as `SessionBandLayer` at `crates/midas-scene/src/layers/session_band.rs:27`. **`PaintContext` is NOT widened.**
  4. Refactor `paint()`:
     - Acquire **one** `RwLockReadGuard` for the entire pass (concurrency safety per `00-index.md` D9 step 6).
     - Call new private helper `partition_by_anchor(&guard, range, config.anchor, self.calendar.tz()) -> Vec<Range<usize>>`. Returns `vec![range]` for `Anchor::Viewport | Anchor::Unknown`. For per-period anchors, walks candles and groups consecutive ones with matching `PeriodKey` derived from `Candle.window.open` in the calendar's timezone.
     - Cap to most recent `config.max_profiles` ranges.
     - For each range, derive `(min_low, max_high)`, compute bins (factor existing logic into `compute_bins_for_range`), find POC.
     - Compute `left_x = ctx.axis.to_x(first_candle.timestamp)` and `right_x = ctx.axis.to_x(next_period_first_candle.timestamp)` (or `ctx.viewport.width_px` for the trailing period). `width_px = (right_x - left_x).clamp(MIN_PROFILE_PX, MAX_PROFILE_PX) * config.width_fraction`.
     - Skip if `right_x - left_x < MIN_PERIOD_PX_TO_RENDER` (= 12 px) — Slice 6 P2 degrades to a 1-px POC tick.
     - **Allocate the bins/quad buffers per-paint**, matching the existing `VolumeProfileLayer::paint` (`vec![Bin::default(); num_bins]` per call). Do NOT add a scratch-buffer field. Reasoning: `SceneLayer: Send + Sync` (`crates/midas-scene/src/layer.rs:106`) forbids `RefCell` (which is `!Sync`), and the trait doc explicitly says "layers mutate state only through dedicated `update_*` methods on the concrete type, called by the scene driver, not by the renderer". Per-paint allocation cost is measured in microseconds for ≤ 100 profiles × 24 bins; if perf later demands scratch reuse, the right fix is in S6 P1 (after P0 baseline) using either the trait-prescribed `update_*` pattern or a `parking_lot::Mutex<Vec<...>>` (which IS `Sync`).
     - Emit per-period `QuadInstance`s with offset `x = left_x` and per-bin `w` derived from `bin_volume / max_bin_volume_in_period * width_px`.
  5. Bin count: `min(24, floor(profile_pixel_height / 2))` for per-period modes; existing `bin_count_for_viewport` retained for `Viewport` mode.
- `crates/midas-scene/src/layers/volume_profile/anchor.rs` — **new file**:
  ```rust
  /// Render-time anchor enum for the new chart stack. Mirrors the
  /// persisted `midas_core::VolumeProfileAnchor` variant set, with no
  /// serde derives. The duplicate exists because Architecture Rule 9
  /// forbids root crates depending on desktop crates; the `From`/`Into`
  /// bridge lives in `midas-app` (the only crate that touches both).
  #[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
  #[non_exhaustive]
  pub enum VolumeProfileAnchor {
      #[default] Viewport,
      Daily, Weekly, Monthly, Yearly, Unknown,
  }

  impl VolumeProfileAnchor {
      /// Minimum bar-period (in days) for which this anchor produces
      /// useful output. Used by `period_blocks_anchor` to gate fallback.
      pub const fn min_period_days(self) -> u32 {
          match self {
              Self::Viewport | Self::Unknown => 0,
              Self::Daily   => 1,
              Self::Weekly  => 7,
              Self::Monthly => 30,
              Self::Yearly  => 365,
          }
      }
  }
  ```
- `desktop/win/crates/midas-app/src/session_chart/scene_builder.rs` — **first-time wiring** + **kill-switch enforcement**:
  1. Add a `volume_profile: bool` flag (or `Option<VolumeProfileConfig>` — simpler to plumb the whole thing) to whatever the equivalent `SceneLayers` config struct is. **Recon resolves the exact struct name**.
  2. **Kill-switch (D15) enforced HERE, not inside the layer.** `midas-scene` is a root-workspace crate and cannot reach `AppConfig` (Architecture Rule 9). The scene builder lives in `midas-app` and CAN read `AppConfig`. Compute the effective anchor before constructing the layer:
     ```rust
     let scene_anchor: midas_scene::VolumeProfileAnchor =
         if app_config.experimental.disable_anchored_vp {
             midas_scene::VolumeProfileAnchor::Viewport   // kill-switch override
         } else {
             state.volume_profile.anchor.into()           // bridge From impl
         };
     ```
     The layer never sees the per-chart `Daily/Weekly/...` value when the kill-switch is on — it gets `Viewport` and behaves like the regression baseline.
  3. Gated on `state.show_volume_profile == true`, construct `VolumeProfileLayer::new(shared_candles, visible_range, VolumeProfileStyle::default(), VolumeProfileConfig { anchor: scene_anchor, width_fraction, max_profiles: 100 }, calendar)`. `calendar` is passed in by the scene builder from whatever holds the `&'static dyn ExchangeCalendar` for the chart (likely the existing `SessionBandLayer` setup — Recon confirms).
  4. The `From<midas_core::VolumeProfileAnchor> for midas_scene::VolumeProfileAnchor` impl lives in `midas-app::session_chart::scene_builder` (or in a small bridge module). The bridge is a 6-arm match; trivial.
  5. Z-order: VP layer renders **BELOW candles**, ABOVE grid + session-band. Use the existing `LayerZ::VOLUME_PROFILE = 350` (vs `LayerZ::CANDLE = 400`); preserve the existing `volume_profile_slots_between_volume_and_candle` test in `layer.rs:230-237` and the doc comment at `layer.rs:71-77`. **Rationale:** anchored profiles extend horizontally from the period's first candle and span hundreds of pixels — the histogram's tail (the rightmost portion sticking past the candle column) is the user-visible part. Layering under the candles preserves the existing slice-7 invariant + tests, avoids a risky renumber that would touch INDICATOR (450) and HOLIDAY_MARKER (500), and matches the visual expectation that candles are the foreground subject.
  6. Pass the layer through to `gpu_renderer` so its emitted `QuadInstance`s end up on the wire (today `gpu_renderer.rs:230` hard-codes `&[]` — this becomes `&buckets.volume_profile_quads` or equivalent).
- `desktop/win/crates/midas-app/src/session_chart/gpu_renderer.rs` (around line 230): replace `volume_profile: &[]` hard-code with the actual buffer from the scene buckets.
- `crates/midas-scene/src/lib.rs` — re-export `VolumeProfileConfig` and `VolumeProfileAnchor` (root copy).
- `desktop/win/crates/midas-app/Cargo.toml` — no new deps (already depends on both `midas-core` and `midas-scene`). The `From` impl bridging the two enum copies lives in `midas-app`.
- **Do NOT add `midas-core` as a dep of `midas-scene`** — that violates Architecture Rule 9. The Recon confirms no root crate currently depends on any desktop crate; adding the first such edge is forbidden.
- `crates/midas-scene/Cargo.toml` — no new deps. `midas-calendar` is already in scope (used by `SessionBandLayer`).

## Files to create

- `crates/midas-scene/src/layers/volume_profile/anchor.rs` (the duplicate enum, see above).
- `crates/midas-scene/src/layers/volume_profile/partition.rs` — `partition_by_anchor` + `PeriodKey` derivation. Created if `mod.rs` would exceed ~1000 LOC; otherwise inline.

## Key implementation details

### Day-key derivation

`Session` does NOT carry a day_start_utc field. Derive from `Candle.window.open` (which is `Timestamp = chrono::DateTime<Utc>`) in the calendar's timezone, fetched from the layer's owned calendar reference (NOT from `PaintContext`):

```rust
fn period_key_for(c: &Candle, anchor: VolumeProfileAnchor, tz: Tz) -> PeriodKey {
    let dt = c.window.open.with_timezone(&tz);
    match anchor {
        VolumeProfileAnchor::Daily   => PeriodKey::Day(dt.year(), dt.month(), dt.day()),
        VolumeProfileAnchor::Weekly  => {
            let iso = dt.iso_week();
            PeriodKey::Week(iso.year(), iso.week())
        }
        VolumeProfileAnchor::Monthly => PeriodKey::Month(dt.year(), dt.month()),
        VolumeProfileAnchor::Yearly  => PeriodKey::Year(dt.year()),
        // Viewport / Unknown handled by caller before reaching here.
        _ => unreachable!(),
    }
}
```

`tz` is `self.calendar.tz()` from inside `paint()`. **Not `ctx.calendar.timezone()` — that field doesn't exist on `PaintContext`.**

### Single read-guard for partition + bin

```rust
fn paint(&self, ctx: &mut PaintContext<'_>) {
    let guard = self.candles.read();   // single guard for entire pass
    if guard.is_empty() { return; }
    let tz = self.calendar.tz();
    // ... partition (uses guard, tz) ...
    // ... bin (uses guard) ...
    // guard dropped at end of fn; no .await possible since paint is sync
}
```

Mandatory per concurrency requirement (00-index.md D9 step 6). Two-guard variant could see a tick land between partition and binning, producing a partial profile.

### `ctx.axis.to_x(ts)` is correct

Existing layers (`session_band.rs:87`, `holiday.rs:114`, `crosshair.rs:254`) use `ctx.axis.to_x(...)`. The trait abstracts the gap-collapsing transparently, so the new stack has no equivalent of the legacy `collapse_gaps` problem.

### Anchor ≥ timeframe fallback (D12)

Mirror Slice 2's `timeframe_blocks_anchor`. The new stack's `BarPeriod` is richer (`Clock(M1) | Session(Regular) | Calendar(D1)`). Recon item R3 (see `00-index.md` addendum) verified that no `unit_days()` method exists on `BarPeriod`, `SessionSpan`, or `CalendarSpan` today, AND that adding such a method to `midas-calendar` is rejected as speculative generality (only this slice consumes it). Implement the unit-days mapping as a private free function inside `crates/midas-scene/src/layers/volume_profile.rs` — symmetric with `min_period_days()` which already lives on the duplicate `VolumeProfileAnchor` in `midas-scene`:

```rust
/// Minimum bar-period (in days) for a given period kind. Private to this
/// layer; do NOT promote to `midas-calendar` (Recon R3 — speculative
/// generality for one consumer).
fn period_unit_days(p: BarPeriod) -> u32 {
    use BarPeriod::*;
    match p {
        Clock(_) | Session(_) => 0,                  // sub-daily, never blocks
        Calendar(CalendarSpan::Week)    => 7,
        Calendar(CalendarSpan::Month)   => 30,
        Calendar(CalendarSpan::Quarter) => 90,
        Calendar(CalendarSpan::Year)    => 365,
        // CalendarSpan is #[non_exhaustive]; future variants default to
        // "treat as never-blocking" so the fallback is opt-in per variant.
        _ => 0,
    }
}

fn period_blocks_anchor(period: BarPeriod, anchor: VolumeProfileAnchor) -> bool {
    period_unit_days(period) >= anchor.min_period_days()
        && anchor.min_period_days() > 0
}
```

`min_period_days()` lives on the duplicate `VolumeProfileAnchor` (see `anchor.rs` above) — guaranteed to exist after this slice.

### Z-order

`LayerZ::VOLUME_PROFILE` already exists. Recon confirms its constant value puts VP **above** candles+grid+session-band but **below** the annotation/bracket slot. If ordering is wrong, bump the constant in this slice.

### Per-paint allocation (NOT scratch buffer)

Allocate the `Vec<Bin>` and `Vec<QuadInstance>` per-paint, matching the existing `VolumeProfileLayer::paint`. **Do NOT add a `scratch` field.** Reasons:
1. `SceneLayer: Send + Sync` (`crates/midas-scene/src/layer.rs:106`); `RefCell<T>` is `!Sync` → would not compile.
2. The trait doc (`layer.rs:101-105`) explicitly: "layers mutate state only through dedicated `update_*` methods on the concrete type, called by the scene driver, not by the renderer". `paint(&self)` is contractually pure-render.
3. Allocation cost for ≤ 100 profiles × ≤ 24 bins = ≤ 2400 quads is microseconds per frame — well within budget.

If perf later demands scratch reuse, the right fix lives in S6 P1 (after S6 P0 baseline measurement). The two valid approaches there are: (a) the trait-prescribed `update_*` pattern (driver pre-computes, paint just emits), or (b) `parking_lot::Mutex<Vec<...>>` (which IS `Sync`). Both are out of scope for S3.

## Testing

### Unit tests in `crates/midas-scene/src/layers/volume_profile/tests.rs`

1. **`anchor_viewport_unchanged`** — regression guard. Existing slice-7 test fixture with `Anchor::Viewport` yields the same `QuadInstance` output as before (after first-time wiring).
2. **`anchor_unknown_treated_as_viewport`** — `Anchor::Unknown` produces same output as `Viewport`.
3. **`anchor_enum_round_trip_through_bridge`** — `midas_core::VolumeProfileAnchor::Daily` → `midas_scene::VolumeProfileAnchor::Daily` via the `From` impl in `midas-app`. Test lives in `midas-app` since the bridge does. All 6 variants (incl. Unknown) round-trip cleanly.
4. **`partition_daily_3_nyse_days_xnys`** — 3 days of M5 fixture under `XnysCalendar`'s ET → 3 partitions. Crosses NYSE close (16:00 ET) but not the calendar day → still one partition per day.
5. **`partition_daily_3_days_crypto_utc`** — same fixture under `CryptoSpotCalendar`'s UTC → boundary at UTC midnight, not ET midnight (different result). Verifies calendar-driven timezone.
6. **`partition_weekly_iso`** — Dec-28 → Jan-4 fixture → 2 partitions on the Mon-start ISO week boundary.
7. **`partition_monthly`** + **`partition_yearly`** — analogous.
8. **`partition_dst_spring_forward`** — fixture spanning the second Sunday of March → no spurious partition; daily count is correct.
9. **`anchor_blocks_when_period_too_coarse`** — `Anchor::Daily` with `BarPeriod::Calendar(D1)` → falls back to single profile (Viewport behaviour).
10. **`max_profiles_cap_drops_oldest`** — 200 daily periods visible with `max_profiles = 100` → 100 profiles emitted, all from the most-recent end.
11. **`width_clamps_min_max`** — synthetic fixture where one period is 5px wide (skipped) and another is 10000px wide (clamped to MAX_PROFILE_PX = 240). Asserts on emitted `QuadInstance.x` and `w`.
12. **`single_read_guard_under_concurrent_writes`** — spawn writer thread that pushes new candles to `SharedCandleSeries` while reader runs `paint()` 100 times. Assert each paint sees a self-consistent series. Use `loom` if it's a workspace dev-dep; else `std::thread`. May `#[ignore]` if flaky on CI.
13. **`first_time_wiring_smoke`** — construct a `SceneBuilder` with `show_volume_profile = true`, run `build_scene`, assert at least one VP `QuadInstance` ends up in the output buckets (proves the wiring is hooked up).
14. **`fallback_emits_tracing_debug`** — assert that toggling Anchor=Daily on a Calendar(D1) period produces exactly one `tracing::debug!(target: "vp", ?reason="anchor_too_coarse_for_period", ...)` per paint where the fallback fires (use `tracing-test` or capture the subscriber output).
15. **`kill_switch_forces_viewport_in_scene_builder`** — lives in `midas-app::session_chart::scene_builder::tests` (NOT in `midas-scene` — the kill-switch is a `midas-app` concern per Architecture Rule 9). Construct a `ChartState` with `volume_profile.anchor = Daily` and an `AppConfig` with `experimental.disable_anchored_vp = true`. Run the scene-builder code path that computes `scene_anchor`. Assert the resulting `VolumeProfileConfig.anchor == midas_scene::VolumeProfileAnchor::Viewport`. Flip the flag off → the same input yields `Daily`. Confirms D15 is enforced upstream of the layer.

### Devloop integration

`desktop/win/tools/devloop-vp-anchored-new.sh`:

```bash
cargo run -p midas-app --features "dev_harness session_chart" -- \
    --fixture vp_daily_aapl_5m_3days &
APP=$!
sleep 2
midas-devloop-cli SetVpSettings --chart-id 0 --anchor Daily --width-fraction 0.7
midas-devloop-cli ToggleChartBackend --chart-id 0   # to "New"
midas-devloop-cli WaitForIdle --timeout-ms 500
midas-devloop-cli Screenshot --out vp-new-daily.png \
    --diff-ref tests/data/refs/vp-new-daily.png --min-ssim 0.98
kill $APP
```

### Reference PNGs owned by S3

S3 generates and commits **its own** reference PNGs (this slice's Done-when does not depend on S5):
- `desktop/win/tests/data/refs/vp-new-viewport.png` — regression baseline captured BEFORE wiring lands (today's `gpu_renderer.rs:230 volume_profile: &[]` produces nothing). After wiring, `Viewport` mode produces the slice-7-equivalent output; this PNG locks it.
- `desktop/win/tests/data/refs/vp-new-daily.png`, `vp-new-weekly.png`, `vp-new-monthly.png`, `vp-new-yearly.png` — captured from this slice's first green build with the corresponding fixture.

S5 (cross-stack parity) reuses these same PNGs.

## Done when

- Tests 1-15 pass.
- Kill-switch (D15) verified end-to-end: `[experimental] disable_anchored_vp = true` in `data/config.toml` forces `Viewport` rendering in the new stack regardless of per-chart `volume_profile.anchor` (test #15).
- All five `vp-new-*.png` reference PNGs generated, committed, and produce SSIM ≥ 0.98 against the running build.
- Toggling backend = New with `Anchor = Daily` shows visibly distinct per-day histograms.
- Toggling backend = New with `Anchor = Viewport` produces the wired-up slice-7 single-profile output (no longer empty).
- `cargo clippy --workspace --features session_chart -- -D warnings` clean.
- Z-order documented: VP layer renders above candles/grid/session-band, below annotation slot.
- `From<midas_core::VolumeProfileAnchor> for midas_scene::VolumeProfileAnchor` exists in `midas-app` and round-trips cleanly (test #3).

## Risks

- **`VolumeProfileLayer` was unwired** (gpu_renderer.rs:230). First-time wiring is real work, not a 1-line tweak.
- **`Session` field shape** — verified does not contain a day_start_utc. Day-key MUST come from `Candle.window.open` + calendar timezone.
- **`SceneLayer::paint(&self)` vs `&mut self`** — Recon resolves; if `&self`, scratch buffer needs interior mutability.
- **Cross-workspace dep `midas-scene → midas-core` is FORBIDDEN** by Architecture Rule 9 (verified: no root crate depends on any desktop crate today). Plan uses duplicate-enum strategy (`anchor.rs` above) + bridge in `midas-app`. Do NOT take a shortcut that violates the rule.
- **`ExchangeCalendar::tz()` (not `timezone()`)** — verified method name. Plan's earlier draft used `timezone()`; corrected here.
- **`PaintContext` has no `calendar` field** — verified. Layer owns its own `&'static dyn ExchangeCalendar` (SessionBandLayer pattern). Do NOT widen `PaintContext` — that cascades to every existing layer.
- **`BarPeriod`/`SessionSpan` `unit_days()`** — Recon R3 (see `00-index.md` addendum) confirmed these do NOT exist today and rejected adding them to `midas-calendar`. Implementation uses a private `period_unit_days(p: BarPeriod) -> u32` free function inside `crates/midas-scene/src/layers/volume_profile.rs` (see "Anchor ≥ timeframe fallback (D12)" key-impl section).
- **Z-order is "VP under candles" by design** — preserves existing `volume_profile_slots_between_volume_and_candle` test and slice-7 doc invariant. The visible histogram is the rightmost tail extending past candle widths. Verified VOLUME_PROFILE = 350, CANDLE = 400, INDICATOR = 450, HOLIDAY_MARKER = 500 in `crates/midas-scene/src/layer.rs`.
- **Concurrent-read-guard test** — may be flaky without `loom`. Mark `#[ignore]` and run nightly only if needed; design still holds.
- **Tracing on silent fallback** — D11/D12 fallbacks emit `tracing::debug!(target: "vp", ?anchor, ?reason, "anchored mode falling back to Viewport")` exactly once per fallback transition (NOT once per paint frame — gate on transition).
- **Kill-switch enforcement site (D15)** — `midas-scene` cannot read `AppConfig` (Architecture Rule 9). The override `disable_anchored_vp == true ⇒ scene_anchor = Viewport` is computed in `midas-app::session_chart::scene_builder` BEFORE constructing `VolumeProfileLayer`. The layer itself has no kill-switch awareness; this localizes the policy to the one crate that already depends on both `midas-core` (for `AppConfig`) and `midas-scene` (for the layer).
