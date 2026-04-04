# Reduce File Sizes: Extract Inline Tests to Separate Files

Analysis of all 10 crates in the workspace. Files with `#[cfg(test)]` modules that should be extracted to dedicated test files.

## Summary

| Crate | Files with Tests | Total Test Lines | Total Test Functions | Extraction Candidates |
|-------|-----------------|-----------------|---------------------|----------------------|
| midas-broker (root) | 10 | 1,667 | 128 | 10 |
| midas-core (root) | 1 | 73 | 6 | 1 |
| midas-core (desktop) | 4 | 1,191 | 57 | 4 |
| midas-data | 3 | 1,014 | 78 | 3 |
| midas-feed | 2 | 445 | 29 | 2 |
| midas-chart | 10 | 3,850 | 186 | 10 |
| midas-render | 3 | 249 | 26 | 2 |
| midas-app | 2 | 355 | 28 | 2 |
| midas-ui | 7 | 377 | 26 | 6 |
| midas-indicators | 1 | 155 | 12 | 1 |
| **TOTAL** | **43** | **9,376** | **576** | **41** |

---

## Tier 1 - Critical (>200 test lines)

These files are the most bloated by inline tests. Extract first.

### `desktop/win/crates/midas-chart/src/interaction.rs`
- **Total lines:** 2,170 | **Test lines:** 1,204 (55%) | **Tests:** 40
- Target: `tests/interaction.rs`

### `desktop/win/crates/midas-chart/src/compute.rs`
- **Total lines:** 1,994 | **Test lines:** 858 (43%) | **Tests:** 29
- Target: `tests/compute.rs`

### `desktop/win/crates/midas-core/src/config.rs`
- **Total lines:** 1,013 | **Test lines:** 735 (73%) | **Tests:** 17
- Target: `tests/config.rs`

### `crates/midas-broker/src/orders/state.rs`
- **Total lines:** 618 | **Test lines:** 408 (66%) | **Tests:** 47
- Target: `tests/order_state.rs`

### `desktop/win/crates/midas-data/src/binary.rs`
- **Total lines:** 802 | **Test lines:** 384 (48%) | **Tests:** 22
- Target: `tests/binary.rs`

### `desktop/win/crates/midas-data/src/candle.rs`
- **Total lines:** 726 | **Test lines:** 368 (51%) | **Tests:** 37
- Target: `tests/candle.rs`

### `desktop/win/crates/midas-chart/src/state.rs`
- **Total lines:** 806 | **Test lines:** 335 (42%) | **Tests:** 21
- Target: `tests/state.rs`

### `crates/midas-broker/src/testdata/mod.rs`
- **Total lines:** 605 | **Test lines:** 320 (53%) | **Tests:** 25
- Target: `tests/testdata_provider.rs`

### `desktop/win/crates/midas-chart/src/level_tool.rs`
- **Total lines:** 534 | **Test lines:** 315 (59%) | **Tests:** 14
- Target: `tests/level_tool.rs`

### `desktop/win/crates/midas-feed/src/csv.rs`
- **Total lines:** 671 | **Test lines:** 301 (45%) | **Tests:** 17
- Target: `tests/csv.rs`

### `desktop/win/crates/midas-chart/src/volume_profile.rs`
- **Total lines:** 503 | **Test lines:** 275 (55%) | **Tests:** 14
- Target: `tests/volume_profile.rs`

### `desktop/win/crates/midas-data/src/lod.rs`
- **Total lines:** 452 | **Test lines:** 262 (58%) | **Tests:** 19
- Target: `tests/lod.rs`

### `desktop/win/crates/midas-chart/src/crosshair_tool.rs`
- **Total lines:** 435 | **Test lines:** 233 (54%) | **Tests:** 23
- Target: `tests/crosshair_tool.rs`

### `desktop/win/crates/midas-app/src/level_store.rs`
- **Total lines:** 402 | **Test lines:** 229 (57%) | **Tests:** 16
- Target: `tests/level_store.rs`

### `desktop/win/crates/midas-chart/src/camera.rs`
- **Total lines:** 362 | **Test lines:** 218 (60%) | **Tests:** 13
- Target: `tests/camera.rs`

### `desktop/win/crates/midas-chart/src/dirty.rs`
- **Total lines:** 368 | **Test lines:** 205 (56%) | **Tests:** 16
- Target: `tests/dirty.rs`

### `crates/midas-broker/src/persist/order_repo.rs`
- **Total lines:** 483 | **Test lines:** 203 (42%) | **Tests:** 9
- Target: `tests/order_repo.rs`

### `desktop/win/crates/midas-core/src/timeframe.rs`
- **Total lines:** 415 | **Test lines:** 201 (48%) | **Tests:** 19
- Target: `tests/timeframe.rs`

### `crates/midas-broker/src/engine.rs`
- **Total lines:** 395 | **Test lines:** 195 (49%) | **Tests:** 8
- Target: `tests/engine.rs`

---

## Tier 2 - Moderate (50-200 test lines)

### `desktop/win/crates/midas-core/src/candle_data.rs`
- **Total lines:** 222 | **Test lines:** 159 (72%) | **Tests:** 9
- Target: `tests/candle_data.rs`

### `desktop/win/crates/midas-indicators/src/atr.rs`
- **Total lines:** 375 | **Test lines:** 155 (41%) | **Tests:** 12
- Target: `tests/atr.rs`

### `desktop/win/crates/midas-feed/src/testdata.rs`
- **Total lines:** 954 | **Test lines:** 144 (15%) | **Tests:** 12
- Target: `tests/testdata.rs`

### `desktop/win/crates/midas-chart/src/instances.rs`
- **Total lines:** 338 | **Test lines:** 126 (37%) | **Tests:** 10
- Target: `tests/instances.rs`

### `desktop/win/crates/midas-app/src/layout.rs`
- **Total lines:** 421 | **Test lines:** 126 (30%) | **Tests:** 12
- Target: `tests/layout.rs`

### `crates/midas-broker/src/orders/types.rs`
- **Total lines:** 401 | **Test lines:** 105 (26%) | **Tests:** 10
- Target: `tests/order_types.rs`

### `desktop/win/crates/midas-render/src/color.rs`
- **Total lines:** 229 | **Test lines:** 105 (46%) | **Tests:** 8
- Target: `tests/color.rs`

### `desktop/win/crates/midas-core/src/id.rs`
- **Total lines:** 163 | **Test lines:** 96 (59%) | **Tests:** 12
- Target: `tests/id.rs`

### `desktop/win/crates/midas-render/src/lib.rs`
- **Total lines:** 127 | **Test lines:** 93 (73%) | **Tests:** 10
- Target: `tests/lib.rs`

### `crates/midas-broker/src/config.rs`
- **Total lines:** 317 | **Test lines:** 92 (29%) | **Tests:** 7
- Target: `tests/config.rs`

### `desktop/win/crates/midas-ui/src/editable_label.rs`
- **Total lines:** 272 | **Test lines:** 81 (30%) | **Tests:** 5
- Target: `tests/editable_label.rs`

### `desktop/win/crates/midas-chart/src/levels.rs`
- **Total lines:** 203 | **Test lines:** 81 (40%) | **Tests:** 5
- Target: `tests/levels.rs`

### `crates/midas-broker/src/ib_strings.rs`
- **Total lines:** 158 | **Test lines:** 79 (50%) | **Tests:** 8
- Target: `tests/ib_strings.rs`

### `crates/midas-broker/src/commands.rs`
- **Total lines:** 190 | **Test lines:** 75 (39%) | **Tests:** 4
- Target: `tests/commands.rs`

### `crates/midas-core/src/lib.rs`
- **Total lines:** 282 | **Test lines:** 73 (26%) | **Tests:** 6
- Target: `tests/lib.rs`

### `desktop/win/crates/midas-ui/src/theme.rs`
- **Total lines:** 199 | **Test lines:** 73 (37%) | **Tests:** 3
- Target: `tests/theme.rs`

### `crates/midas-broker/src/connection.rs`
- **Total lines:** 117 | **Test lines:** 62 (53%) | **Tests:** 5
- Target: `tests/connection.rs`

### `crates/midas-broker/src/events.rs`
- **Total lines:** 232 | **Test lines:** 60 (26%) | **Tests:** 4
- Target: `tests/events.rs`

### `crates/midas-broker/src/db.rs`
- **Total lines:** 181 | **Test lines:** 61 (34%) | **Tests:** 3
- Target: `tests/db.rs`

### `desktop/win/crates/midas-ui/src/button_group.rs`
- **Total lines:** 222 | **Test lines:** 59 (27%) | **Tests:** 4
- Target: `tests/button_group.rs`

### `desktop/win/crates/midas-ui/src/button.rs`
- **Total lines:** 239 | **Test lines:** 56 (23%) | **Tests:** 4
- Target: `tests/button.rs`

### `desktop/win/crates/midas-ui/src/icon_button.rs`
- **Total lines:** 225 | **Test lines:** 51 (23%) | **Tests:** 4
- Target: `tests/icon_button.rs`

### `desktop/win/crates/midas-render/src/pipelines/mod.rs`
- **Total lines:** 195 | **Test lines:** 51 (26%) | **Tests:** 8
- Target: `tests/pipelines.rs`

---

## Tier 3 - Skip (< 35 test lines)

These are too small to bother extracting.

| File | Test Lines | Tests |
|------|-----------|-------|
| `desktop/win/crates/midas-ui/src/label.rs` | 34 | 4 |
| `crates/midas-broker/src/testdata/adapter.rs` | 34 | 3 |
| `desktop/win/crates/midas-ui/src/tooltip.rs` | 23 | 2 |
| `desktop/win/crates/midas-chart/src/lib.rs` | 9 | 1 |

---

## Extraction Approach

For each file, the extraction follows this pattern:

1. **Create `tests/<name>.rs`** in the crate root (sibling to `src/`)
2. **Move** the entire `#[cfg(test)] mod tests { ... }` block into the new file
3. **Add imports** at the top of the test file: `use <crate_name>::*;` plus any needed deps
4. **Make items `pub(crate)`** if tests access private internals (or keep as unit tests in a `src/tests/` submodule instead)
5. **Remove** the `#[cfg(test)]` block from the source file
6. **Run `cargo test`** to verify nothing broke

### Unit vs Integration Tests

- Tests that access **private fields/methods**: keep as `#[cfg(test)] mod tests` in `src/<name>/tests.rs` submodule
- Tests that only use the **public API**: move to `tests/<name>.rs` integration tests

### Impact

Extracting all Tier 1 + Tier 2 candidates removes **~9,300 lines** of test code from implementation files, making the source files dramatically easier to navigate.

### Top 5 Highest-Impact Extractions

| File | Lines Saved | % Reduction |
|------|------------|-------------|
| `midas-chart/src/interaction.rs` | 1,204 | 55% |
| `midas-chart/src/compute.rs` | 858 | 43% |
| `midas-core (desktop)/src/config.rs` | 735 | 73% |
| `midas-broker/src/orders/state.rs` | 408 | 66% |
| `midas-data/src/binary.rs` | 384 | 48% |
