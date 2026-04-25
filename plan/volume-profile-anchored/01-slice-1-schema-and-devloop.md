# Slice 1 — Settings Schema, Persistence, Devloop Command

**Goal:** Add `VolumeProfileSettings` to `ChartConfig` + `ChartState`, define the duplicate `VolumeProfileAnchor` enum in `midas-scene` plus the `From`/`Into` bridge in `midas-app`, and prove the schema round-trips through `save()`/`load()`. Add a devloop command `SetVpSettings` so subsequent slices can drive the schema without UI. **No render behaviour change yet.**

**Depends on:** Recon (the 1-2 hour pre-S1 reconnaissance task in `00-index.md`).

## Files to modify

- `desktop/win/crates/midas-core/src/config/mod.rs` — append `VolumeProfileSettings` struct + `VolumeProfileAnchor` enum (per `00-index.md` D5). Add `pub volume_profile: VolumeProfileSettings` field to `ChartConfig` with `#[serde(default)]`. Re-export both new types.
- `desktop/win/crates/midas-core/src/config/tests.rs` — append three tests (see Testing).
- `desktop/win/crates/midas-chart/src/state/mod.rs` — add `pub volume_profile: VolumeProfileSettings` to `ChartState`. **Update `ChartState::new(camera: Camera2D)` only — there is no `Default` impl on `ChartState`** (verified: line 126 has `derive(Clone, Debug)` only).
- `desktop/win/crates/midas-chart/src/input.rs` (or wherever `ChartInput` is defined — Recon resolves the path; grep `pub struct ChartInput`) — add a new field `pub effective_vp_anchor: VolumeProfileAnchor`. **This widening is the kill-switch transport for the legacy stack**: `midas-chart` is a sans-IO leaf crate and cannot reach `AppConfig` directly (Architecture Rule 9). The `midas-app` assembly site (which CAN read `AppConfig`) sets this field per-frame to either `state.volume_profile.anchor` (kill-switch off) or `Viewport` (kill-switch on); `build_volume_profile()` in S2 reads this field instead of going through `state.volume_profile.anchor`. Slice 1 only adds the field with a `Viewport` default; S2 wires the read site, and the assembly site that computes the override lives in `midas-app` (see below).
- `desktop/win/crates/midas-app/src/...` — wherever `ChartInput` is constructed for the legacy chart per frame (Recon: grep `ChartInput {` in `desktop/win/crates/midas-app/`). Compute and set `effective_vp_anchor`:
  ```rust
  let effective_vp_anchor = if app_config.experimental.disable_anchored_vp {
      midas_core::VolumeProfileAnchor::Viewport       // D15 kill-switch
  } else {
      panel.chart_state.volume_profile.anchor
  };
  ```
  This is the **single enforcement point for the legacy stack**. The new stack's parallel enforcement lives in `session_chart/scene_builder.rs` (S3). Both sites read the same `AppConfig` field; never check the kill-switch in `midas-chart` or `midas-scene` (leaf crates by Architecture Rule 9).
- `desktop/win/crates/midas-app/src/app/persistence.rs` — extend `build_config` (write side, around line 23+) to read `panel.chart_state.volume_profile.sanitized()` into the new `ChartConfig.volume_profile` field. Mirror the existing `show_volume_profile` plumbing.
- `desktop/win/crates/midas-app/src/app.rs` (around line 2874, the `restore_panel` site — `restore_panel` lives in `app.rs`, NOT `persistence.rs`) — restore `panel.chart_state.volume_profile = chart_cfg.volume_profile.sanitized();` (sanitize on read also).
- `desktop/win/Cargo.toml` `[workspace.dependencies]` — add `chrono-tz = "0.10"` matching the root workspace pin (Recon confirms exact version). `chrono` is already there at `Cargo.toml:141`.
- `crates/midas-scene/src/layers/volume_profile/anchor.rs` (**new file**) — duplicate `VolumeProfileAnchor` enum without serde, with `min_period_days()` const method. See Slice 3 for full definition.
- `desktop/win/crates/midas-app/src/session_chart/scene_builder.rs` (or a small new bridge module in `midas-app`) — `From<midas_core::VolumeProfileAnchor> for midas_scene::VolumeProfileAnchor` + reverse. 6-arm match, trivial. Test in slice 3.
- `desktop/win/crates/midas-core/Cargo.toml` — no new deps (settings struct is plain serde). `chrono`/`chrono-tz` get pulled in by Slice 2 in `midas-chart` only.
- `desktop/win/crates/midas-devloop-proto/src/lib.rs` — add the new command, **following the `InjectTickerMsg` JSON-payload pattern** (lines 132-135) to preserve the crate's "Pure serde, no domain dependencies" design principle:
  ```rust
  /// Set Volume Profile settings for a chart. Used by devloop scripts to
  /// drive Slice 2/3 rendering without going through the popup UI. The
  /// settings payload is JSON to avoid pulling `midas-core` into this
  /// crate (matches the InjectTickerMsg precedent).
  SetVpSettings { chart_id: u64, settings_json: serde_json::Value },
  ```
  **Do NOT add `midas-core` as a dep of `midas-devloop-proto`** — that would violate the crate's documented purpose.
- `desktop/win/crates/midas-app/src/devloop/handler.rs` — add a handler arm that:
  1. Deserialises `settings_json` into `midas_core::VolumeProfileSettings` (returning a clear error if malformed — `tracing::error!` + skip).
  2. Mutates `chart.chart_state.volume_profile`, calls `dirty.mark_data()` and `mark_config_dirty()`.
- `.github/workflows/rust.yml` — **NO change required.** Recon R10 (see `00-index.md` addendum) confirmed both `desktop_session_chart_lint` and `desktop_session_chart_tests` jobs already cover the feature gate. Both currently `continue-on-error: true`; the project-wide flip-to-required schedule is tracked separately. **Trade-off acknowledgment**: until that flip lands, new-stack VP regressions surface as yellow warnings rather than red blocks; the kill-switch (D15) is the operational backstop, and S3/S5's screenshot tests are the primary correctness gate.

## Files to create

None.

## Key implementation details

### Schema (`midas-core/src/config/mod.rs`)

```rust
fn default_vp_width_fraction() -> f32 { 0.25 }      // matches today's Viewport behaviour
fn default_vp_value_area_pct() -> f32 { 0.70 }      // RESERVED; not used in v1

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum VolumeProfileAnchor {
    #[default]
    Viewport,
    Daily,
    Weekly,
    Monthly,
    Yearly,
    /// Forward-compat sink: a downgraded binary loading a config with a
    /// future-version anchor falls back here, then default behaviour.
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VolumeProfileSettings {
    #[serde(default)]
    pub anchor: VolumeProfileAnchor,
    #[serde(default = "default_vp_width_fraction")]
    pub width_fraction: f32,
    /// RESERVED for v2 (Slice 6 polish item 4). Default false; render code
    /// ignores it in v1.
    #[serde(default)]
    pub show_value_area: bool,
    /// RESERVED for v2. Default 0.70 (TradingView convention).
    #[serde(default = "default_vp_value_area_pct")]
    pub value_area_pct: f32,
}

impl Default for VolumeProfileSettings {
    fn default() -> Self {
        Self {
            anchor: VolumeProfileAnchor::Viewport,
            width_fraction: default_vp_width_fraction(),
            show_value_area: false,
            value_area_pct: default_vp_value_area_pct(),
        }
    }
}
```

Append to `ChartConfig`:
```rust
/// Volume Profile settings (anchor mode + render knobs). New in plan
/// `volume-profile-anchored`. Defaults preserve pre-feature behaviour.
#[serde(default)]
pub volume_profile: VolumeProfileSettings,
```

Append to `AppConfig` (top-level, NOT per-chart) — the global kill-switch (D15):
```rust
/// Experimental flags. Reserved for risk mitigation: a single config
/// edit can revert behaviour without a binary rollback.
#[serde(default)]
pub experimental: ExperimentalFlags,

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExperimentalFlags {
    /// When true, both render branches force `Anchor = Viewport`
    /// regardless of per-chart `volume_profile.anchor`. Per-chart
    /// settings are preserved on disk (no data loss). Toggle off to
    /// re-enable anchored mode without losing user choices.
    #[serde(default)]
    pub disable_anchored_vp: bool,
}
```
**The leaf crates `midas-chart` and `midas-scene` NEVER see `disable_anchored_vp`** (Architecture Rule 9 — leaf crates cannot reach `AppConfig`). The override is computed at the two `midas-app` assembly sites that already see both `AppConfig` and per-chart state, and propagates through pre-overridden inputs:
- **Legacy stack**: the per-frame `ChartInput` builder in `midas-app` sets `chart_input.effective_vp_anchor = if app_config.experimental.disable_anchored_vp { Viewport } else { state.volume_profile.anchor }`. `build_volume_profile()` reads only `input.effective_vp_anchor` (S2).
- **New stack**: `midas-app::session_chart::scene_builder` computes `scene_anchor` with the same conditional and passes it into `VolumeProfileLayer::new(...)` (S3). The layer never knows the kill-switch exists.

This wording supersedes any earlier draft that suggested a "one-line guard in `build_volume_profile()`" or inside the layer — that v3 framing implied an Architecture Rule 9 violation and was corrected in v3.1.

### Clamp on read AND on write

`width_fraction` clamped to `[0.05, 1.0]` and `value_area_pct` to `[0.10, 0.95]` inside a small `pub fn sanitized(&self) -> Self` method. Call it from `build_config()` (write side) and from `restore_panel()` (read side). Belt & braces — a malformed manual edit doesn't survive even one save→load cycle.

### Devloop command

`midas-devloop-proto` is the shared schema between app + scripts (see CLAUDE.md). Both sides need to compile with the new variant. The handler in `midas-app::devloop::handler` mutates state and marks dirty — same pattern as `InjectTickerMsg`.

### Persistence-failure surfacing

Verify `flush_config()` in `persistence.rs:259-297` already logs save failures via `tracing::error!`. If not, add it (one-line). User shouldn't lose settings silently.

## Testing

```bash
cargo test -p midas-core volume_profile        # schema + roundtrip
cargo test -p midas-chart                      # ChartState compiles + new() initialises
cargo clippy -p midas-core -p midas-chart -p midas-devloop-proto -- -D warnings
cargo build --workspace                        # both feature combos
cargo build --workspace --features session_chart
```

### New tests in `midas-core/src/config/tests.rs`

1. **`vp_settings_roundtrip_preserves_all_fields`** — model after `save_load_roundtrip_preserves_all_fields` (line 97-150). Construct `ChartConfig` with `volume_profile = VolumeProfileSettings { anchor: Daily, width_fraction: 0.65, show_value_area: true, value_area_pct: 0.68 }`. Save → load → assert each field equal.
2. **`vp_settings_legacy_config_loads_with_defaults`** — write a TOML string by hand that contains all the pre-feature fields but **omits** `volume_profile` entirely. `AppConfig::load(path)` must succeed with `volume_profile == VolumeProfileSettings::default()` and `volume_profile.anchor == Viewport`.
3. **`vp_settings_unknown_anchor_falls_back`** — write a TOML with `[charts.0.volume_profile] anchor = "Quarterly"`. Load must succeed with `anchor == Unknown`. Render code in S2/S3 treats `Unknown` exactly like `Viewport`.
4. **`vp_settings_clamps_out_of_range`** — write a TOML with `width_fraction = 5.0` (way over). Load → save → load — second load yields `width_fraction = 1.0` (clamp on write took effect).

### `ChartState` test in `midas-chart/src/state/`

5. **`chart_state_new_initialises_volume_profile_default`** — `let s = ChartState::new(Camera2D::default())`; assert `s.volume_profile == VolumeProfileSettings::default()`.

### Kill-switch tests (D15)

5a. **`kill_switch_default_off`** — `AppConfig::default()` has `experimental.disable_anchored_vp == false`. Round-trip a config without `[experimental]` table → loads with the flag false (additive default).
5b. **`kill_switch_overrides_chart_input_anchor`** — lives in `midas-app`. Build a `ChartInput` from a `ChartState` with `volume_profile.anchor = Daily` AND an `AppConfig` with `experimental.disable_anchored_vp = true`. Assert `chart_input.effective_vp_anchor == VolumeProfileAnchor::Viewport`. Flip the flag off → assert `effective_vp_anchor == Daily`. **This is the legacy-stack enforcement test**; the new stack's parallel test (S3 #15) covers `scene_builder.rs`.

### Devloop test

6. **`devloop_set_vp_settings_mutates_state`** — boot the harness fixture, send `SetVpSettings { chart_id, settings: { anchor: Daily, .. } }`, then `DumpState` and assert the JSON projection shows `volume_profile.anchor == "Daily"`.

## Done when

- All seven tests above pass (1-4 schema, 5 ChartState, 5a/5b kill-switch, 6 devloop).
- `cargo clippy --workspace -- -D warnings` clean on both feature combos.
- An older `data/config.toml` (without the `[charts.N.volume_profile]` table) loads cleanly and yields the default settings.
- A future-version TOML with `anchor = "Quarterly"` loads cleanly and falls back to `Unknown`.
- `flush_config()` failure path logs an error (Recon verifies; add one-line if absent).
- Devloop fixture: `SetVpSettings` round-trips through `DumpState`.
- CI runs both `--workspace` and `--workspace --features session_chart` test jobs.
- `VolumeProfileSettings::sanitized()` clamp tested on read AND on write.
- **Devloop wire-version documented**: `SetVpSettings` requires app build `>= <PR#>`. `midas-devloop-proto`'s `Command` enum is internally-tagged and rejects unknown variants on deserialise (Recon-verified), so old apps + new scripts will error; document in `tools/README.md` (or wherever devloop usage docs live).

## Risks

- **TOML enum variant string casing** — `#[serde(rename_all = "PascalCase")]` produces `anchor = "Daily"` on disk. If we ever want `"daily"` (lowercase) — flag now, change later costs migration code.
- **`VolumeProfileSettings` evolves in v2** (value-area rendering). Adding fields is backwards-compatible only because of `#[serde(default)]` on each.
- **Devloop schema bump** — the `SetVpSettings` variant adds to the wire protocol. Old devloop scripts ignore unknown variants? Verify with the proto's serde config; if strict, flag a follow-up.
- **`ChartState` gains a non-`Copy` field** — confirm there's no `Copy` derive on `ChartState` that breaks. (Verified: only `Clone, Debug`.) New struct is `Clone`, fine.
