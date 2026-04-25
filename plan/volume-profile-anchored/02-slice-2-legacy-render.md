# Slice 2 — Per-Period VP on the Legacy Stack

**Goal:** When `state.volume_profile.anchor != Viewport` AND `show_volume_profile == true`, the **legacy** `midas-chart` stack renders one VP per calendar period (Day / Week / Month / Year), each left-anchored at its first candle. Buy/sell split preserved.

**Depends on:** S1 (schema + devloop).

**Out of scope:** UI to flip the mode (S4) — exercised via S1's `SetVpSettings` devloop command. Cross-stack parity tests (S5).

## Files to modify

- `desktop/win/crates/midas-chart/Cargo.toml` — add deps:
  ```toml
  chrono = { workspace = true }
  chrono-tz = { workspace = true }    # added in S1 to desktop workspace
  ```
- `desktop/win/crates/midas-chart/src/volume_profile/mod.rs` — three additions:
  1. New pub function `candle_period_boundaries(timestamps: &[i64], anchor: VolumeProfileAnchor, tz: Tz) -> Vec<usize>`. Returns indices of the **first candle** of each period over the visible range.
  2. New pub function `compute_anchored_volume_profiles(data, vis_start, vis_end, boundaries, num_bins_per_profile, max_profiles) -> Vec<VolumeProfile>`. Loops the existing per-bin allocation logic, one profile per `(boundaries[i]..boundaries[i+1])` slice, taking the slice's own `(min_low, max_high)`. Caps to `max_profiles` MOST RECENT profiles (drops oldest from view).
  3. New pub function `anchored_profiles_to_instances(profiles, lefts_px, widths_px, camera) -> Vec<GridLineInstance>` — extends `profile_to_instances` to take per-profile horizontal offset + width caps. Same buy/sell + POC dot logic, applied per profile.
- `desktop/win/crates/midas-chart/src/compute/mod.rs` — branch in `build_volume_profile()` (line 118). **Reads `effective_anchor` from `ChartInput` (see S1) — NOT `state.volume_profile.anchor` directly.** The kill-switch is applied upstream at the assembly site in `midas-app` (which can read `AppConfig`); `midas-chart` is a sans-IO leaf crate and cannot reach `AppConfig` (Architecture Rule 9). Branches:
  - `effective_anchor == Viewport | Unknown` → existing single-profile call path (regression-zero for default users; also the kill-switch path because the assembly site forces `effective_anchor = Viewport` when `app_config.experimental.disable_anchored_vp == true`).
  - Else, AND `!state.collapse_gaps`, AND timeframe-anchor compat (D12) → call `candle_period_boundaries` + `compute_anchored_volume_profiles` + `anchored_profiles_to_instances`. Compute per-profile `left_x` via `Camera2D::time_to_x(period.first_candle.timestamp)` and `right_x` via the next period's first-candle (or the right edge of the viewport for the trailing period). `width_px = (right_x - left_x).clamp(MIN, MAX) * settings.width_fraction`.
  - Else (collapse_gaps OR anchor ≥ timeframe) → fall back silently to the existing Viewport call path. **Emit `tracing::debug!(target: "vp", ?anchor, ?reason, "anchored mode falling back to Viewport")` once per fallback transition** (gate on transition, not per frame). The popup note (Slice 4) is the user-facing signal.

## Files to create

None.

## Key implementation details

### `candle_period_boundaries` signature + algorithm

> **Critical:** `CandleData::timestamp(i) -> i64` is **epoch milliseconds** per `desktop/win/crates/midas-core/src/candle_data/mod.rs:33-34`. The boundary helper MUST use `from_timestamp_millis`, NOT `from_timestamp` (which expects seconds). Mixing units silently breaks boundary detection — every candle ends up in its own "period" and the whole anchored-VP feature renders as one-bar-per-profile garbage.

```rust
use chrono::{DateTime, Datelike, NaiveDate};
use chrono_tz::Tz;

pub fn candle_period_boundaries(
    timestamps_ms: &[i64],       // EPOCH MILLISECONDS, one per visible candle
    anchor: VolumeProfileAnchor, // Daily / Weekly / Monthly / Yearly
    tz: Tz,                      // chrono_tz::US::Eastern by default for v1
) -> Vec<usize> {
    if timestamps_ms.is_empty() || matches!(
        anchor,
        VolumeProfileAnchor::Viewport | VolumeProfileAnchor::Unknown
    ) {
        return Vec::new();
    }

    // Sane-range clamp in MILLISECONDS. ~1970 (epoch start) to ~2100.
    // 4_102_444_800_000 ms = 2100-01-01T00:00:00Z. Anything outside is
    // logged and skipped — chrono-tz lookups can be unstable far from now.
    const MIN_MS: i64 = 0;
    const MAX_MS: i64 = 4_102_444_800_000;

    let mut out = Vec::with_capacity(timestamps_ms.len() / 4);
    let mut prev_key: Option<PeriodKey> = None;
    for (i, &ts) in timestamps_ms.iter().enumerate() {
        if !(MIN_MS..=MAX_MS).contains(&ts) {
            tracing::warn!(target: "vp", ts, "candle ts out of safe range");
            continue;
        }
        let dt: DateTime<Tz> = DateTime::from_timestamp_millis(ts)
            .expect("ts within MIN_MS..=MAX_MS")
            .with_timezone(&tz);
        let key = PeriodKey::from(&dt, anchor);
        if Some(key) != prev_key {
            out.push(i);
            prev_key = Some(key);
        }
    }
    out
}

#[derive(Copy, Clone, PartialEq, Eq)]
enum PeriodKey {
    Day(i32, u32, u32),       // year, month, day
    Week(i32, u32),           // ISO year, ISO week
    Month(i32, u32),          // year, month
    Year(i32),
}
```

**`Anchor::Viewport` and `Anchor::Unknown` return empty `Vec` — caller treats both as "no boundaries, fall back to single profile".**

### Branching in `build_volume_profile()`

```rust
// `input.effective_vp_anchor` is set by the app.rs assembly site:
//   effective = if app_config.experimental.disable_anchored_vp { Viewport }
//               else { state.volume_profile.anchor };
let anchor = input.effective_vp_anchor;
let use_anchored = !matches!(anchor,
        VolumeProfileAnchor::Viewport | VolumeProfileAnchor::Unknown)
    && !state.collapse_gaps                         // D11
    && !timeframe_blocks_anchor(timeframe, anchor); // D12

if use_anchored {
    // ... per-period path
} else {
    // ... existing single-profile path (unchanged)
}
```

### `timeframe_blocks_anchor`

Recon item R2 (see `00-index.md` addendum) verified that `Timeframe` exposes `as_secs() -> u32` (already const, all variants covered) but NO `unit_minutes()`. Use `as_secs()` directly — adding a parallel `unit_minutes()` accessor is speculative generality:

```rust
fn timeframe_blocks_anchor(tf: &Timeframe, anchor: VolumeProfileAnchor) -> bool {
    use VolumeProfileAnchor::*;
    let tf_secs = tf.as_secs() as u64;
    let anchor_min_secs: u64 = match anchor {
        Daily   => 86_400,
        Weekly  => 7 * 86_400,
        Monthly => 30 * 86_400,
        Yearly  => 365 * 86_400,
        Viewport | Unknown => return false,
    };
    tf_secs >= anchor_min_secs
}
```

### `compute_anchored_volume_profiles`

Loops `boundaries.windows(2).chain(once((boundaries.last(), vis_end)))`. For each slice, computes `(min_low, max_high)` over its candles, then runs the existing per-bin allocation logic against that range. Caps with `profiles.into_iter().rev().take(max_profiles).collect::<Vec<_>>().into_iter().rev().collect()` (drop oldest). `max_profiles` is hard-coded `100` for v1 (D10).

### `anchored_profiles_to_instances`

Extends `profile_to_instances` to accept `lefts_px: &[f32]` and `widths_px: &[f32]` of the same length as `profiles`. For each `(profile, left, width)`, runs the existing buy/sell + POC instance emission with `rect[0] = left`, `rect[2] = left + bar_width` (instead of `0..bar_width`).

### Coordinate transform — collapse_gaps fallback

`Camera2D::time_to_x` is linear in time and is wrong in `collapse_gaps == true` mode. The branch above explicitly skips the anchored path when `collapse_gaps` is on. **Full fix (use the index→x function in `compute_collapsed_scene`) is Slice 6 polish.**

### Reset-Chart preservation

Verify `Message::ResetChart` (`handlers.rs` — grep for `ResetChart`) does NOT touch `chart_state.volume_profile`. If it does today, that's a bug; if it doesn't, no change. Document either way in the PR.

## Testing

### Unit tests

In `midas-chart/src/volume_profile/tests.rs`:

1. **`boundaries_empty_input`** — empty `timestamps_ms` yields empty `Vec`.
2. **`boundaries_viewport_yields_empty`** — `anchor = Viewport` returns empty regardless of input.
3. **`boundaries_unknown_yields_empty`** — `anchor = Unknown` returns empty.
4. **`boundaries_daily_three_nyse_days_ms_scale`** — **3 days of M5 timestamps in ET, expressed in MILLISECONDS** (e.g., `1717200000000` for 2024-06-01 00:00:00Z) → exactly 3 indices. **This is the critical-fix regression test.** Asserts that ms-scale inputs produce per-day grouping (NOT per-millisecond which would produce N indices for N candles).
5. **`boundaries_daily_dst_spring_forward`** — timestamps spanning the second Sunday of March → still produces correct daily count (DST shift doesn't create a spurious boundary).
6. **`boundaries_daily_dst_fall_back`** — timestamps spanning the first Sunday of November → same.
7. **`boundaries_iso_week_53_to_1`** — timestamps spanning Dec 28 2026 → Jan 4 2027 (week 53 of 2026 → week 1 of 2027) → boundary at the week 1 transition.
8. **`boundaries_monthly_year_end`** — last bar of Dec + first bar of Jan → 2 boundaries.
9. **`boundaries_yearly`** — year transitions → boundary at each Jan 1 (in ET).
10. **`boundaries_zero_volume_day`** — a day in the input with all `volume == 0` candles still gets a boundary (boundary helper is volume-agnostic; profile compute handles zero-volume gracefully).
11. **`boundaries_nyse_half_day`** — July 3 (typical NYSE early close) → boundary as expected; downstream profile is shorter but valid.
12. **`boundaries_clamps_out_of_range_timestamps`** — `i64::MIN`, `i64::MAX`, ms timestamp for year 1700 (`-8520336000000`), ms timestamp for year 9999 (`253370764800000`) → logged + skipped, no panic. Also asserts that an in-range millisecond ts (e.g., `1717200000000`) is NOT skipped (clamp guards against unit confusion regressions).
13. **`anchored_compute_3_profiles`** — boundaries `[0, 50, 100]` over a 100-candle fixture → exactly 2 profiles (windows of `boundaries`).
14. **`anchored_compute_per_period_poc`** — each period has independent POC argmax over its own slice.
15. **`anchored_compute_max_profiles_cap`** — 5 boundaries with `max_profiles = 1` → 1 profile (most recent).
16. **`anchored_compute_empty_period`** — boundary range with zero candles (shouldn't happen if boundaries from helper, but guard) → emits no profile, no panic.

### Reference PNGs owned by S2

S2 generates and commits **its own** reference PNGs (this slice's Done-when does not depend on S5):
- `desktop/win/tests/data/refs/vp-legacy-viewport.png` — regression baseline captured from current `main` BEFORE this slice's render changes land. Confirms `Anchor = Viewport` is byte-identical (SSIM ≥ 0.99) post-slice.
- `desktop/win/tests/data/refs/vp-legacy-daily.png` — captured from this slice's first green build with the daily fixture.
- `desktop/win/tests/data/refs/vp-legacy-weekly.png` — same with weekly fixture.
- `desktop/win/tests/data/refs/vp-legacy-monthly.png` — same with monthly fixture.
- `desktop/win/tests/data/refs/vp-legacy-yearly.png` — same with yearly fixture.

S5 (cross-stack parity) reuses these same PNGs in its parity-comparison scripts; S5 does NOT regenerate them.

### Integration test via devloop

In `desktop/win/tools/devloop-vp-anchored-legacy.sh`:

```bash
cargo run -p midas-app --features dev_harness -- \
    --fixture vp_daily_aapl_5m_3days &
APP=$!
sleep 2
midas-devloop-cli SetVpSettings --chart-id 0 \
    --anchor Daily --width-fraction 0.7
midas-devloop-cli WaitForIdle --timeout-ms 500
midas-devloop-cli Screenshot --out vp-legacy-daily.png \
    --diff-ref tests/data/refs/vp-legacy-daily.png \
    --min-ssim 0.98
kill $APP
```

(Exact `midas-devloop-cli` invocation matches the existing `tools/devloop-smoke.sh` pattern; verify the binary name / argv shape during slice kickoff.)

### Snapshot test (unit-level, doesn't need GPU)

17. **`anchored_profiles_snapshot_legacy`** — for the 3-day AAPL fixture, snapshot `Vec<VolumeProfile>` JSON (POC indices, total volumes, bin counts). Use `insta` if it's already a workspace dev-dep; else assert on a small set of invariants. Catches regressions in S2 before S5's pixel diffs.

## Done when

- Tests 1-17 pass.
- All five `vp-legacy-*.png` reference PNGs generated, committed, and produce SSIM ≥ 0.98 against the running build.
- `Anchor = Viewport` reproduces `vp-legacy-viewport.png` (captured from current `main` BEFORE this slice's changes) at SSIM ≥ 0.99 — regression guard.
- Pre-feature config (`show_volume_profile = true`, no `[volume_profile]` table on disk) loads + renders identically to current `main` for the daily fixture.
- `cargo clippy --workspace -- -D warnings` clean.
- `collapse_gaps == true` + `anchor = Daily` produces the Viewport output (unit test asserting `use_anchored == false`; cross-stack screenshot in S5).
- `Daily` anchor + `1D` timeframe produces Viewport output (unit test).
- `R` (reset chart) does not affect `volume_profile` settings (handler unit test).

## Risks

- **`Timeframe` API for `unit_minutes`** — Recon R2 (`00-index.md` addendum) confirmed `unit_minutes()` does NOT exist. `timeframe_blocks_anchor` uses the existing `Timeframe::as_secs() -> u32` directly (no new method on `Timeframe`).
- **Hard-coded ET timezone** for crypto on legacy stack will look wrong (anchored to NY midnight, not UTC). Acceptable v1 limitation per Open Question 2.
- **`chrono-tz` binary size** ≈ 200KB. Confirmed worth it; root workspace already pays this cost.
- **`Camera2D::time_to_x` semantics** — pure linear; collapse_gaps fallback handles the only divergence. If a future optimization changes its semantics, the screenshot regression test in S5 catches it.
- **Buy/sell split preservation** — existing `compute_volume_profile` per-bin loop reads `data.close(i) >= data.open(i)`. The new per-period loop does the same; no behaviour change per period.
- **Kill-switch enforcement site** — `app_config.experimental.disable_anchored_vp` cannot be read from `midas-chart` (leaf crate, no `AppConfig` access). Enforcement happens at the assembly site in `midas-app` (S1 widens `ChartInput` with `effective_vp_anchor: VolumeProfileAnchor`). `build_volume_profile()` only sees the already-overridden `effective_anchor`. Kill-switch unit test lives in `midas-app` (assert `disable_anchored_vp = true` + per-chart `anchor = Daily` ⇒ `ChartInput.effective_vp_anchor == Viewport`).
