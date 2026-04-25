# Group A — Types Extraction (Unblock Slice 9c)

Slice 9c (atomic deletion of `midas-chart`) is blocked because `midas-chart` still owns load-bearing types that the new stack and persistence layer depend on. Group A moves them to dedicated leaf crates first, leaving 9c as a pure-deletion PR.

Five slices: A1 + A1b (annotation types + consumer migration), A2 + A2b (GPU types + consumer migration), A3 (parity baseline). A1 / A2 / A3 are independent; A1b depends on A1 landing, A2b depends on A2 landing.

---

## Slice A1 — Extract annotation types into `midas-annotation-types`

**Goal:** Move the persistent annotation type tree into a new leaf crate so `midas-chart` can be deleted without taking the type definitions with it.

**Depends on:** None.

### Files to create

- `desktop/win/crates/midas-annotation-types/Cargo.toml` (new)
- `desktop/win/crates/midas-annotation-types/src/lib.rs` (new) — re-exports
- `desktop/win/crates/midas-annotation-types/src/annotation.rs` (new) — `Annotation`, `AnnotationId`, `AnnotationKind`, `Presence`, moved from `midas-chart/src/widget/mod.rs:54-176`
- `desktop/win/crates/midas-annotation-types/src/levels.rs` (new) — `HorizontalLevel` + `LevelIcon` + V1/V2 deser path, moved from `midas-chart/src/levels.rs:34-206`
- `desktop/win/crates/midas-annotation-types/src/price_line.rs` (new) — `PriceLine`, `LineStroke`, `LineExtent`, `LineStyle`, moved from `midas-chart/src/widget/price_line.rs:16-58` and `midas-chart/src/widget/level.rs:36-79`
- `desktop/win/crates/midas-annotation-types/src/order_bracket.rs` (new) — `OrderBracket`, `BracketLeg`, `BracketSide`, `BracketStatus`, `LegRole`, `EntryType` + `risk_reward`/`dollar_risk`/`dollar_reward`/`leg_style`/`is_leg_on_wrong_side` helpers, moved from `midas-chart/src/widget/order_bracket/mod.rs`

### Files to modify

- `desktop/win/Cargo.toml` — add `crates/midas-annotation-types` to `workspace.members`; add `midas-annotation-types = { path = "crates/midas-annotation-types", version = "0.1.0" }` to `workspace.dependencies`
- `desktop/win/crates/midas-chart/Cargo.toml` — add dep on `midas-annotation-types.workspace = true`
- `desktop/win/crates/midas-chart/src/widget/mod.rs` — replace type defs with `pub use midas_annotation_types::{Annotation, AnnotationId, AnnotationKind, Presence};` shim (NOT yet deprecated — that lands in A1b)
- `desktop/win/crates/midas-chart/src/levels.rs` — same shim pattern
- `desktop/win/crates/midas-chart/src/widget/price_line.rs` — same shim pattern
- `desktop/win/crates/midas-chart/src/widget/order_bracket/mod.rs` — same shim pattern

**Deprecation timing:** The shim is plain `pub use` in A1 — adding `#[deprecated]` here would immediately red-wall CI because consumers still import through the shim. A1b adds the `#[deprecated(note = "...")]` attribute as its final step, AFTER consumer-side migration is complete (so the deny-warnings clippy stays green).

### Key implementation details

- **Pre-flight serde audit.** Before the move, grep the codebase for non-JSON serializers touching annotation types. Required commands:
  ```bash
  rg "bincode|rmp_serde|postcard|ciborium" -g '!target' --type rust
  rg "Annotation|HorizontalLevel|OrderBracket" desktop/win/crates/midas-store/
  rg "snapshot_fixture|SnapshotFixture" -A 10 | rg "Annotation|HorizontalLevel|OrderBracket"
  ```
  If any non-JSON encoder touches these types, lock variant **order** (not just `#[serde(rename)]`) — bincode-style encoders use the index, not the name. Document findings in the slice's PR description before merging.
- **Serde tag stability is non-negotiable.** The on-disk format at `data/annotations/*.json` carries implicit enum-variant tags. As the FIRST commit in this slice (before any code moves), add explicit `#[serde(rename = "level")]` / `#[serde(rename = "order_bracket")]` / `#[serde(rename = "text_note")]` / `#[serde(rename = "marker")]` attributes on every `AnnotationKind` variant. This commit lands and soaks for at least one CI run before the type move follows.
- **Two-pass deserialize forward-compat** — the existing `serde_json::Value` round-trip at `desktop/win/crates/midas-app/src/annotation_persistence.rs:115-146` (`AnnotationFile` struct at line 27) must keep working. The new crate exports the same `Annotation` shape, so the persistence layer keeps its existing import (just from the new path).
- **No `midas-core` dependency.** This crate stays a pure leaf — it only depends on `serde`, `smallvec`, `glam` (for any geometry primitives). Avoiding the `midas-core` dep keeps it cheap to recompile.
- **`AnnotationStore` does NOT move in this slice.** That's a runtime container, not a type. It stays in `midas-app/src/annotation_store.rs`; only its imports change in A1b.

### Testing

- **New round-trip test:** `desktop/win/crates/midas-annotation-types/tests/serde_roundtrip.rs` — for each `AnnotationKind` variant, build an example, `to_string` → `from_str` → equality check. Catches any tag drift introduced by the move.
- **Fixture replay test:** `desktop/win/crates/midas-annotation-types/tests/fixture_replay.rs` — loads `tests/fixtures/annotations_v1_pre_decorator.json` (already exists in `midas-chart`), parses it, asserts the `AnnotationKind::Level`, `AnnotationKind::OrderBracket` etc. variants survived.
- **Existing tests** in `midas-chart` keep running unchanged via the re-export shims. If any of the 462 tests break, the move is wrong.
- **Migration of `midas-app` imports happens incrementally** in follow-up commits; not gated on this slice. The shims keep things compiling.

### Done when

- `cargo test -p midas-annotation-types` green (new crate's 2 test files).
- `cd desktop/win && cargo test --workspace` green (no regressions in 1619 desktop tests).
- `cd desktop/win && cargo clippy --workspace -- -D warnings` clean.
- Grepping `midas_chart::widget::Annotation` still works (shim in place).
- Adding `midas-annotation-types` to the desktop workspace did not introduce a circular dep (verify with `cargo tree -p midas-annotation-types`).

### Risks & mitigations

- **Risk:** A serde tag drifts silently. **Mitigation:** Explicit `#[serde(rename)]` on every variant landed in the SAME commit that does the move; round-trip test runs in CI.
- **Risk:** `LevelIcon` (helper at `midas-chart/src/levels.rs:34-112`) has hidden dependencies on chart-only types. **Mitigation:** If discovered during the move, leave it in `midas-chart` and only move the data part of `HorizontalLevel`. Helper functions can ride along later.
- **Risk:** `EntryType` enum carries an implicit broker-coupling assumption. **Mitigation:** Per the research, it's sans-IO with no broker dep — verify by checking `cargo tree -p midas-annotation-types` shows no broker / market-data crates.

### Rollback signal

If after the move any of: the file count in `midas-annotation-types/src/` exceeds 8, the crate gains a non-`serde`/`smallvec`/`glam` dependency, or any existing JSON fixture fails round-trip — the split was wrong, fold back into `midas-chart` and reconsider.

---

## Slice A1b — Migrate consumer imports off `midas-chart` annotation shims

**Goal:** Eliminate every `use midas_chart::widget::Annotation` (and similar) site from production code so that slice 9c can delete `midas-chart` without the deletion PR also having to do a workspace-wide search-and-replace.

**Depends on:** A1 (must be merged).

### Files to modify

- Every file under `desktop/win/crates/` that currently imports annotation types via the `midas_chart::widget::*` / `midas_chart::levels::*` paths. Find them with:
  ```bash
  rg "use midas_chart::widget::(Annotation|AnnotationId|AnnotationKind|Presence|PriceLine|LineStyle|LineExtent|LineStroke)" desktop/win/crates/
  rg "use midas_chart::widget::order_bracket::" desktop/win/crates/
  rg "use midas_chart::levels::HorizontalLevel" desktop/win/crates/
  ```
- After all consumers are migrated, edit each shim re-export in `midas-chart` to add `#[deprecated(note = "import from midas_annotation_types directly; midas-chart will be deleted in slice 9c")]` above the `pub use`.

### Key implementation details

- **Pure import-only edits.** This slice rewrites `use` statements; it should not touch behaviour. Each edit is "delete one path, paste another" with no semantic change.
- **Land the deprecation in the same PR as the last import migration.** The deprecation attribute would block the PR otherwise (because the PR's own diff still has at least one shim consumer until the final commit removes it). Sequence within the PR: (1) migrate all consumers across N commits or one big commit, (2) final commit adds the `#[deprecated]` attributes, (3) `cargo clippy --workspace -- -D warnings` confirms no remaining consumers.

### Testing

- All existing tests pass unchanged. This is purely a refactor at the import layer.
- After deprecation lands, `cargo clippy --workspace -- -D warnings` is the gate: if any consumer was missed, the deprecation warning becomes an error.

### Done when

- `rg "use midas_chart::widget::(Annotation|AnnotationId|AnnotationKind|Presence|PriceLine|LineStyle|LineExtent|LineStroke)" desktop/win/crates/` returns zero hits outside `midas-chart` itself.
- `rg "use midas_chart::levels::HorizontalLevel" desktop/win/crates/` returns zero hits outside `midas-chart`.
- The shim re-exports inside `midas-chart` carry `#[deprecated]`.
- `cargo clippy --workspace -- -D warnings` clean across both workspaces.

### Risks & mitigations

- **Risk:** A consumer imports through a re-export chain (e.g., `midas_app::Annotation` → `midas_app::reexport::Annotation` → `midas_chart::widget::Annotation`). Grep won't catch the indirect path. **Mitigation:** After explicit-grep migration, run `cargo clippy --workspace -- -D warnings` with the deprecation in place; any indirect path emits a warning and gets caught.
- **Risk:** Deprecation breaks doctests or examples. **Mitigation:** Doctests are also gated by clippy; treat as regular consumer code.

### Rollback signal

If clippy still fires deprecation warnings after the migration commits, there is a missed consumer. Find it via the warning's file:line, migrate, re-run. Do not silence with `#[allow(deprecated)]`; that defeats the forcing function.

---

## Slice A2 — Extract GPU instance types into `midas-gpu-types`

**Goal:** Move the GPU-buffer-shaped instance structs out of `midas-chart` so `midas-render` can consume them directly without going through the legacy crate.

**Depends on:** None. (Sibling of A1; no shared types.)

### Files to create

- `desktop/win/crates/midas-gpu-types/Cargo.toml` (new) — deps: `bytemuck = { version = "1", features = ["derive"] }`, `glam = { version = "0.29", features = ["bytemuck"] }`
- `desktop/win/crates/midas-gpu-types/src/lib.rs` (new)
- `desktop/win/crates/midas-gpu-types/src/instances.rs` (new) — `CandleInstance`, `VolumeInstance`, `GridLineInstance`, `BadgeInstance`, all derives intact (`Pod`, `Zeroable`, `#[repr(C)]`), moved from `midas-chart/src/instances.rs:20-226`
- `desktop/win/crates/midas-gpu-types/src/labels.rs` (new) — `AxisLabel`, `CrosshairRender`, moved from `midas-chart/src/instances.rs:122-154`
- `desktop/win/crates/midas-gpu-types/src/timeline.rs` (new) — `TimelineLabel`, moved from `midas-chart/src/timeline.rs:46-59`

### Files to modify

- `desktop/win/Cargo.toml` — add `crates/midas-gpu-types` to workspace.members; declare `midas-gpu-types.workspace = true` style entry
- `desktop/win/crates/midas-chart/Cargo.toml` — add `midas-gpu-types.workspace = true`
- `desktop/win/crates/midas-chart/src/instances.rs` — replace type defs with `pub use midas_gpu_types::*;` shim; keep ONLY the layout-test module (it's still useful regression coverage)
- `desktop/win/crates/midas-chart/src/timeline.rs` — same shim pattern for `TimelineLabel`
- `desktop/win/crates/midas-render/Cargo.toml` — add `midas-gpu-types.workspace = true`
- `desktop/win/crates/midas-render/src/**` — switch any `use midas_chart::instances::*` over to `use midas_gpu_types::*`. No behaviour change.
- `desktop/win/crates/midas-app/src/session_chart/primitives_bridge.rs` — switch its 3 imports from `midas_chart::instances::*` to `midas_gpu_types::*`

### Key implementation details

- **Pod/Zeroable layout MUST be byte-identical to today.** `CandleInstance` is 48 bytes, `VolumeInstance` 32, `GridLineInstance` 32, `BadgeInstance` 64. The existing layout tests in `midas-chart/src/instances.rs:228-349` are the canonical guard — port them to `midas-gpu-types/src/instances.rs` (in a `#[cfg(test)] mod tests` block) so they run alongside the type defs.
- **Field order is wire format.** WGSL shaders read these structs by byte offset (`@location(0)`, `@location(1)`, etc., matched to `CandleInstance.x`, `body_top`, `body_bottom` …). Reordering fields breaks the GPU pipeline silently. Move struct definitions verbatim — no "while we're at it" cleanup.
- **No dependency back into `midas-chart`.** This is critical: today `midas-render` depends on `midas-chart`; if `midas-gpu-types` accidentally pulled `midas-chart` for any helper, the dep graph would acquire a duplicate path. `cargo tree -p midas-gpu-types` must show only `bytemuck` and `glam`.
- **`AxisLabel` / `CrosshairRender` / `TimelineLabel` aren't `Pod`** but they are GPU-render adjacent metadata. They go in the new crate too — they should travel with the data they describe.

### Testing

- **Port layout tests verbatim** — every existing assertion in `midas-chart/src/instances.rs:228-349` (size, alignment, field offsets) moves to `midas-gpu-types/src/instances.rs`. Re-run as part of `cargo test -p midas-gpu-types`.
- **GPU smoke** — boot `midas-app --features dev_harness`, render a single chart, screenshot, compare to a recent good baseline. SSIM ≥ 0.995 + diff_fraction ≤ 0.002 (existing slice-0 thresholds). This is the only real regression catch — layout tests prove the bytes line up but not that the shaders read them right.

### Done when

- `cargo test -p midas-gpu-types` green.
- `cd desktop/win && cargo test --workspace` green.
- `cd desktop/win && cargo clippy --workspace -- -D warnings` clean.
- `primitives_bridge.rs` no longer imports from `midas_chart::instances` (verify with grep).
- Single-chart screenshot from dev harness: SSIM vs. reference ≥ 0.995.

### Risks & mitigations

- **Risk:** A struct's field order changes invisibly during the move (e.g., rustfmt reorders). **Mitigation:** Diff-review the moved types side-by-side with the originals; the layout tests catch alignment-padding shifts.
- **Risk:** `glam` version mismatch between root + desktop workspaces. **Mitigation:** Check `desktop/win/Cargo.toml` `[workspace.dependencies] glam` already; reuse it. Don't introduce a second declaration.

### Rollback signal

If a screenshot diff fails after the move with no other code change, OR a layout test gains an unexplained padding byte, revert and investigate before re-attempting.

---

## Slice A2b — Migrate consumer imports off `midas-chart` GPU-instance shims

**Goal:** Eliminate every `use midas_chart::instances::*` and `use midas_chart::timeline::TimelineLabel` from production code so 9c can delete `midas-chart` cleanly. Smaller scope than A1b — only `midas-render` and `session_chart/primitives_bridge.rs` import these.

**Depends on:** A2 (must be merged).

### Files to modify

- `desktop/win/crates/midas-render/**` — switch any remaining `use midas_chart::instances::*` to `use midas_gpu_types::*` (some sites already updated in A2; A2b sweeps any leftovers and removes the `midas-chart` Cargo dep from `midas-render` if it was kept only for instance types)
- `desktop/win/crates/midas-app/src/session_chart/primitives_bridge.rs` — confirm no `midas_chart::instances` imports remain
- `desktop/win/crates/midas-chart/src/instances.rs` — add `#[deprecated]` to the shim `pub use` block
- `desktop/win/crates/midas-chart/src/timeline.rs` — same

### Done when

- `rg "use midas_chart::instances" desktop/win/crates/` returns zero hits outside `midas-chart` itself.
- `rg "use midas_chart::timeline::TimelineLabel" desktop/win/crates/` returns zero hits.
- The instance-type shim in `midas-chart/src/instances.rs` carries `#[deprecated]`.
- `cargo clippy --workspace -- -D warnings` clean.
- Optional bonus: if `midas-render`'s only remaining reason to depend on `midas-chart` was the instance types, drop that Cargo dep entirely. Verify with `cargo tree -p midas-render` showing no `midas-chart` line.

### Risks & mitigations

Same as A1b — indirect re-exports caught by clippy after deprecation lands.

---

## Slice A3 — Pre-9c parity reference

**Goal:** Capture reference images of the chart stack so slice 9c can prove the new stack matches the previously-blessed renders.

**Depends on:** None. Can run today against the legacy stack as it stands.

### Platform constraint — Windows-only

PNG-diff with SSIM is sensitive to font-rendering differences across platforms (DirectX vs `swrast`/`llvmpipe`). The target deployment platform per `desktop/win/CLAUDE.md:6` is Windows 11. Capture the baselines on Windows AND run the comparison test on Windows.

The test is gated `#![cfg(all(target_os = "windows", feature = "chart_parity_tests"))]` so default `cargo test` on a developer's Linux box doesn't fire it. The Windows CI runner from C2 is the canonical execution venue.

### Files to create

- `desktop/win/tests/data/parity-baseline/reference/single-aapl-m1.png` (new, ~80 KB)
- `desktop/win/tests/data/parity-baseline/reference/single-aapl-d1.png` (new, ~80 KB)
- `desktop/win/tests/data/parity-baseline/reference/single-spy-m5.png` (new)
- `desktop/win/tests/data/parity-baseline/reference/grid-20chart-m1.png` (new, ~400 KB)
- `desktop/win/tests/data/parity-baseline/reference/single-aapl-m1-with-bracket.png` (new) — exercises annotation render path AND moved-annotation-types path
- `desktop/win/tests/data/parity-baseline/reference/single-aapl-d1-with-levels.png` (new) — exercises horizontal-level + moved-types path
- `desktop/win/tests/data/parity-baseline/reference/README.md` (new) — capture procedure, fixture names, dev-harness commands, platform requirement
- `desktop/win/tests/parity_baseline.rs` (new) — `#![cfg(all(target_os = "windows", feature = "chart_parity_tests"))]` test that boots the chart stack via the dev harness, renders each fixture, calls `chart_parity::compare_images` against the committed PNG, asserts SSIM ≥ 0.995 AND diff_fraction ≤ 0.002.

The `reference/` naming (rather than `legacy/`) avoids name rot once 9c lands and the legacy stack is gone. The PNGs are not "the legacy renders" — they are "the renders we agreed on".

### Capture procedure (documented in README.md, executed manually on Windows 11)

1. From a Windows 11 box (developer's primary dev machine per the project context):
   ```
   cd desktop/win
   cargo run -p midas-app --features dev_harness
   ```
2. From a second terminal on the same Windows machine, for each fixture name listed in `parity_baseline.rs`:
   - `echo '{"cmd":"load_fixture","name":"<fixture>"}' | nc 127.0.0.1 9898`
   - `echo '{"cmd":"wait_for_idle","timeout_ms":5000}' | nc 127.0.0.1 9898`
   - `echo '{"cmd":"screenshot","out_path":"<absolute Windows path>"}' | nc 127.0.0.1 9898`
3. Move PNGs into `desktop/win/tests/data/parity-baseline/reference/`.
4. Verify each PNG is committed (git check, not LFS — they're ~100 KB each, well under the 1 MB threshold).
5. **Repeat the capture procedure on the same Windows box if any fixture re-rendering is needed.** Cross-machine baseline capture (e.g., one fixture from a different developer's box) introduces drift.

### Key implementation details

- **Use existing fixtures, don't invent new ones.** The dev harness already has fixtures at `desktop/win/tests/data/fixtures/`; pick a stable set that won't churn. If a needed fixture doesn't exist, save it via `SnapshotFixture` BEFORE running the legacy capture so the new-stack test can replay the same input.
- **Compound gate, not just SSIM.** The slice-0 work specifically chose `ssim ≥ 0.995 AND diff_fraction ≤ 0.002` because SSIM alone misses color-channel swaps. Reuse `chart_parity::passes_parity_gate` from `desktop/win/crates/midas-app/src/chart_parity.rs`.
- **The test is feature-gated on `chart_parity_tests`.** Won't run on default `cargo test`. Slice C1 wires this feature into CI.
- **No mid-run input.** Each baseline is a static screenshot — load fixture, wait for idle, capture. No drag, no scroll, no animation. Determinism over coverage.

### Testing

- **Self-test:** before checking in, run `cargo test --features chart_parity_tests parity_baseline` against the *legacy* stack and confirm it passes (legacy-vs-legacy comparison should give SSIM=1.0 within float tolerance). This catches a broken capture procedure.
- **The real test runs in slice 9c's PR**, where new-stack rendering is compared against the same PNGs.

### Done when

- All 6 reference PNGs committed to the repo.
- `parity_baseline.rs` exists, compiles with `--features chart_parity_tests`, and passes against the legacy stack today.
- `tests/data/parity-baseline/reference/README.md` documents the exact `LoadFixture` names and `Screenshot` invocations used.
- Slice C1 (CI) has the `chart_parity_tests` feature wired so this test runs on push.

### Risks & mitigations

- **Risk:** PNGs grow to MB-scale and bloat the repo. **Mitigation:** Use 1280×720 capture (matches default window), PNG quality 9; expected size ~80 KB single-chart, ~400 KB 20-chart grid. Total baseline corpus < 2 MB.
- **Risk:** The legacy stack rendering changes between baseline capture and slice-9c PR. **Mitigation:** Capture in a single sitting after Group A's other slices land; commit the baseline in the same PR as the test code so they age together.

### Rollback signal

If any baseline image fails its own self-test (legacy-vs-legacy SSIM < 0.999), the capture is non-deterministic and the test gate is unreliable; investigate before relying on these baselines for 9c.
