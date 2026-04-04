# Plan Evaluation: Grid Component (`midas-grid`)

**Date**: 2026-04-03
**Revised**: 2026-04-03 (fifth evaluation pass — 4-agent full review, all issues resolved)
**Verdict**: SOLID — Ready for implementation

## Summary

A thorough, well-structured plan across 6 documents for extracting a reusable headless grid widget into a new `midas-grid` crate. Codebase claims were verified against actual source files and found accurate. Architecture decisions (headless core, trait-based columns, phased Widget transition) align with industry best practices (TanStack Table, Dear ImGui, Elm architecture). The phased delivery with spike-based risk mitigation is strong.

Five evaluation passes were conducted. The first four passes found and resolved 4 high, 8 medium, and 6 low issues. The fifth pass (4-agent full review) found 2 new high, 7 medium, and 4 low issues; all resolved.

---

## Critical Issues

No critical issues found.

---

## High Severity (all resolved)

### H1. ~~Flash map keyed by fragile `usize` row index~~ (RESOLVED — Pass 3)

Flash state was `HashMap<(usize, ColumnId), FlashState>`, causing flashes to appear on the wrong row after a re-sort. Also used undefined `RowId` type in 02-rendering.md.

**Resolution**: Changed all flash map keys to `(RowKey, ColumnId)` across 00-architecture.md, 02-rendering.md, and 04-implementation-roadmap.md. Changed `FlashCell` message from `row: usize` to `row_key: RowKey`. Replaced all `RowId` references with `RowKey`. The app provides `RowKey` via the same `.row_key()` closure used for multi-selection.

### H2. ~~Phase 3a parallelism overstated~~ (RESOLVED — Pass 3)

Multi-selection and flash state wiring both modify `state.rs` and `message.rs`, which Phase 2 also modifies. The plan claimed "No file-level conflict" for flash, which was incorrect.

**Resolution**: Split Phase 3a into "3a-parallel" (flash pure functions, conditional formatting, persistence — genuinely parallel with Phase 2) and "3a-sequential" (multi-selection, flash wiring into `state.rs`/`message.rs` — must sequence after Phase 2). Added merge protocol documentation for `state.rs`/`message.rs` serialization chokepoints.

### H3. ~~Pre-Phase 2 spike too minimal to validate production complexity~~ (RESOLVED — Pass 3)

The spike used 2 columns and 2 rows; production has 7 columns, dynamic row counts, nested interactive widgets, and resize overlays.

**Resolution**: Added "Post-Spike Checkpoint: Heavy Spike (1 day)" between the minimal spike and Commit 1. The heavy spike validates `Tree::diff()` with dynamic children at production scale before committing to the ~800-1100 line rewrite.

### H4. ~~Cell message type contradiction from `Element::map()` migration~~ (RESOLVED — Pass 4)

Pass 3's `Element::map()` migration changed `grid()` to constrain `C: GridColumn<T, GridMessage>`, but `WatchlistColumn` implements `GridColumn<T, Message>` (the app type). Cell widgets need to emit app-level messages (`Message::ToggleFavorite`), which is impossible in an `Element<GridMessage>` tree. Section 5.3 referenced an undefined `GridMessage::CellMessage(M)` variant.

**Resolution**: Restored generic `M` parameter. The grid is `Grid<'a, T, M, C>` with `C: GridColumn<T, M>`. Two-path message design: cells emit `M` directly, grid chrome maps through a required `on_grid: Fn(GridMessage) -> M` callback. Updated 00-architecture.md (Sections 4.2, 5.3, 7.1, 7.2), 02-rendering.md Section 10 (`GridWidget<M>`, `Shell<M>`, `on_grid` calls), 03-column-data-model.md (grid() signature, usage examples), 04-implementation-roadmap.md (steps 5-7, Phase 2 Widget, API decisions), README.md. In Phase 0-1, `on_grid` is `&dyn Fn`; in Phase 2+, `Widget<M>` stores it as `Box<dyn Fn>`.

### H5. ~~`begin_resize()` signature mismatch~~ (RESOLVED — Pass 5)

`ResizeStarted(ColumnId, f32)` included a cursor position not available on press (iced 0.14's `mouse_area::on_press` does not provide cursor coordinates). `begin_resize(&mut self, column: ColumnId)` correctly omitted it.

**Resolution**: Changed `ResizeStarted(ColumnId, f32)` to `ResizeStarted(ColumnId)` across 00-architecture.md (§4.1 enum, §4.3 update handler), 01-interactions.md (variant table, name mapping table), and 04-implementation-roadmap.md (Phase 1 type table, migration step 6). Added doc comment explaining that cursor position arrives via the first `Resizing(f32)` move event.

### H6. ~~`ColumnId::LastPrice` used as enum variant~~ (RESOLVED — Pass 5)

02-rendering.md used `ColumnId::LastPrice` as if `ColumnId` were an enum. The actual definition is `pub struct ColumnId(pub &'static str)`.

**Resolution**: Changed to `ColumnId("price")` in 02-rendering.md (matching the watchlist column ID from 00-architecture.md).

---

## Medium Severity (all resolved)

### M1. ~~Cell message routing~~ (RESOLVED — Pass 3 → superseded by H4 in Pass 4)

Originally changed to `Element::map()`. Pass 4 found this introduced a type contradiction. Superseded by H4's two-path design (cells emit `M` directly, chrome uses `on_grid` callback).

### M2. ~~`CellContent` enum vestigial artifact~~ (RESOLVED — Pass 3)

01-interactions.md Section 10.1 defined a `CellContent` enum that conflicted with the actual `GridColumn::cell() -> Element` pattern.

**Resolution**: Replaced the enum with a descriptive list of common interactive cell types, with a note that `cell()` return type is the only abstraction needed.

### M3. ~~Vestigial migration note~~ (RESOLVED — Pass 3)

03-column-data-model.md Section 3.5 contained a migration note contradicting the Phase 0 roadmap's "no backward-compatible loading needed" decision.

**Resolution**: Replaced the migration sentence with a note confirming no migration is needed per the Phase 0 decision.

### M4. ~~`on_event()` vs `update()` uncertainty~~ (RESOLVED — Pass 3)

02-rendering.md Section 10 had uncertainty notes about the Widget method name.

**Resolution**: Confirmed `update()` against iced_core 0.14 source. Replaced the uncertainty note with a definitive statement. Updated the spike prototype description to use `update()`.

### M5. ~~O(R*C^2) layout pseudocode~~ (RESOLVED — Pass 3)

02-rendering.md Section 10 used `(0..col).map(...).sum()` per cell, resulting in quadratic column lookups.

**Resolution**: Added pre-computed `column_offsets: Vec<f32>` array before the cell loop. Each cell now uses O(1) index lookup.

### M6. ~~Plan B commit decomposition missing~~ (RESOLVED — Pass 3)

If the Pre-Phase 2 spike fails, the three-commit structure was invalid (Commit 1 was the Widget transition).

**Resolution**: Added "Phase 2 under Plan B" commit decomposition with adapted Commit 1 (`stack![]` drag overlay infrastructure with acceptance criteria), Commit 2 (column reorder with clipping constraint), and Commit 3 (row DnD with clipping constraint).

### M7. ~~Flash naming inconsistency in 01-interactions.md~~ (RESOLVED — Pass 4)

01-interactions.md line 131 used `FlashTick(usize, usize, TickDirection)` with undefined `TickDirection` type and index-based parameters conflicting with canonical `(RowKey, ColumnId)` keys.

**Resolution**: Updated to `FlashCell { column: ColumnId, row_key: RowKey, direction: FlashDirection }` and `FlashTick`. Changed `TickDirection` to `FlashDirection` throughout.

### M8. ~~Sort comparator type mismatch~~ (RESOLVED — Pass 4)

03-column-data-model.md used `c.id().0 == spec.column_id` comparing `&str` with `ColumnId` newtype.

**Resolution**: Changed to `c.id() == spec.column_id` (comparing `ColumnId` with `ColumnId`).

### M9. ~~`on_grid` stored as `Option` but documented as required~~ (RESOLVED — Pass 5)

The `Grid` builder had `on_grid: Option<Box<dyn Fn(GridMessage) -> M + 'a>>` but documentation said it was "required." No behavior defined if caller forgot to set it.

**Resolution**: Made `on_grid` a required parameter of the `grid()` constructor function (not a builder method). The signature is now `grid(columns, rows, state, on_grid)`. The `Grid` struct stores `on_grid: Box<dyn Fn(GridMessage) -> M + 'a>` (not `Option`). Removed the `.on_grid()` builder method. Updated all usage examples in 00-architecture.md (§4.2, §7.1, §7.2), 03-column-data-model.md (§4.1, §6.2), and 04-implementation-roadmap.md (Phase 0 step 7).

### M10. ~~`WatchlistSortBy` phantom reference~~ (RESOLVED — Pass 5)

Migration plan referenced `WatchlistSortBy` message variant that doesn't exist in the codebase. Sort logic is currently inline in `views.rs`.

**Resolution**: Removed `WatchlistSortBy` from 04-implementation-roadmap.md Phase 1 migration step 3. Added note that sort column switching is handled inline in `views.rs` header callbacks. Updated 01-interactions.md Appendix B migration table to reflect "inline sort logic" instead of a named message variant.

### M11. ~~Inconsistent resize handle widths~~ (RESOLVED — Pass 5)

Three documents specified different resize handle widths: 6px (01-interactions.md), 8px (02-rendering.md), 4px (04-roadmap.md Phase 0 migration).

**Resolution**: Standardized Phase 1+ target to 8px (4px each side of column boundary), matching the most detailed spec in 02-rendering.md. Updated 01-interactions.md (detection description, priority table, conflict resolution table) and 04-implementation-roadmap.md (Phase 0 checklist). Phase 0 keeps 4px as migrated; Phase 1 widens to 8px.

### M12. ~~`FlashMap` wrapper vs raw `HashMap`~~ (RESOLVED — Pass 5)

02-rendering.md defined a `FlashMap` wrapper struct with `active_count`, but the roadmap used a raw `HashMap` with `has_active_flashes()`.

**Resolution**: Added implementation note in 02-rendering.md clarifying that `FlashMap` is illustrative, not an implementation target. The canonical runtime type is `HashMap<(RowKey, ColumnId), FlashState>` on `GridState` with `has_active_flashes()`. `FlashState` and `FlashColor` types are implementation targets; the wrapper struct is not.

### M13. ~~Phase 3a/3b merge protocol insufficient~~ (RESOLVED — Pass 5)

The merge protocol was a single sentence. No concrete steps for resolving structural conflicts.

**Resolution**: Expanded into a 6-step numbered workflow: Phase 3a-parallel branches after Phase 1, Phase 2 merges first (critical path), 3a-parallel merges next (conflict-free), 3a-sequential branches from clean base after both merge. Added append-only conventions for `state.rs` fields and `message.rs` variants. Specified merge commits (not squash) to preserve per-commit history.

### M14. ~~`DefaultSortDirection` coupling in Phase 0~~ (RESOLVED — Pass 5)

Phase 0's `toggle_sort()` used "a simple match on `ColumnId`" which would couple `midas-grid` to specific column names.

**Resolution**: Clarified that per-column default direction logic lives in the **app's update handler** (matching on `WatchlistColumn` kind), not inside `GridState::toggle_sort()`. The grid's `toggle_sort()` takes a `default_direction: SortDirection` parameter — the app passes the appropriate direction. Phase 1's `GridColumn::default_sort_direction()` formalizes this into a trait method.

### M15. ~~Widget transition manual-only verification~~ (RESOLVED — Pass 5)

Phase 2 Commit 1 (800-1100 lines) was verified only by "identical visual output" without specific criteria.

**Resolution**: Added 5 explicit intermediate acceptance criteria for Commit 1: all unit tests pass, Phase 0 manual checklist re-run, text_input state persistence, interactive cell event forwarding, and no visual regression in alignment/backgrounds/indicators.

---

## Low Severity (all resolved)

### L1. ~~`ColumnConfig` naming collision~~ (RESOLVED)

Runtime type already named `ColumnState` with explicit collision-avoidance note.

### L2. ~~Sort state type inconsistency~~ (RESOLVED)

`Vec<SortSpec>` already annotated as Phase 4 target shape.

### L3. ~~Accessibility regression from custom Widget~~ (RESOLVED)

Added LOW-severity row to Phase 2 risk table. Tracked as accepted debt per stated non-goal.

### L4. ~~Critical path not explicitly stated~~ (RESOLVED — Pass 3)

**Resolution**: Added "Critical Path" section to 04-implementation-roadmap.md.

### L5. ~~HashMap iteration order in drag handle detection~~ (RESOLVED — Pass 4)

02-rendering.md `mouse_interaction()` used `column_widths.values().next()` which is non-deterministic for `HashMap`.

**Resolution**: Changed to `column_order.first()` + `column_widths.get()` for deterministic lookup.

### L6. ~~FlashState dual definition~~ (RESOLVED — Pass 4)

02-rendering.md and 04-implementation-roadmap.md had conflicting `FlashState` struct definitions (4-field vs 2-field).

**Resolution**: Designated 02-rendering.md Section 4 as canonical source. Updated 04-roadmap to reference it.

### L7. ~~Phase 3a-sequential `RowKey` ordering dependency~~ (RESOLVED — Pass 5)

Flash wiring (step 4) used `RowKey` but `RowKey` is introduced by multi-selection (step 5).

**Resolution**: Reordered steps: multi-selection (introduces `RowKey`) is now step 4, flash wiring (uses `RowKey`) is now step 5.

### L8. ~~No time estimates per phase~~ (RESOLVED — Pass 5)

No calendar time estimates despite detailed complexity estimates.

**Resolution**: Added "Rough Time Estimates" table to Critical Path section with per-phase ranges (single developer). Critical path total: ~17-27 days. Noted as rough ranges, not commitments.

### L9. ~~Phase 0 testing missing `from_config()` edge cases~~ (RESOLVED — Pass 5)

`GridState::from_config()` has non-trivial logic not covered by testing strategy.

**Resolution**: Added 3 explicit `from_config()` edge case tests to Phase 0 testing strategy: unknown column ID (silently dropped), missing column definition (appended), full round-trip with reordered columns.

### L10. ~~Scattered extraction scope understated~~ (RESOLVED — Pass 5)

README described "~400 lines in views.rs" but code is distributed across `views.rs`, `app.rs`, and `watchlist.rs`.

**Resolution**: Updated README motivation to note code is distributed across multiple files.

---

## What's Done Well

- **Codebase alignment**: Every claim verified — `column_widths: [f32; 7]`, `f32::NAN` sentinel, `resizing_column` tuple, message variants, all matched actual source.
- **Best practices alignment**: Headless core follows TanStack Table / Dear ImGui precedent. Trait-based columns with enum dispatch is idiomatic Rust.
- **Pre-Phase 2 spike design**: Runs concurrently with Phase 0, has 5 pass/fail criteria, three-tier fallback (spike pass / `stack![]` / Plan C permanent), plus heavy spike checkpoint.
- **Two-path message design**: Cells emit the app's `M` directly, grid chrome maps through `on_grid` callback. Clean type separation, no wrapper variants, works for both composition functions and custom Widgets.
- **Commit decomposition**: Phase 0 splits into 4 commits. Phase 2 into 3 commits (with Plan B alternative). Phase 3 split into 3a-parallel, 3a-sequential, and 3b.
- **Architecture decisions with alternatives**: Every significant decision includes Y-Statement-complete rationale with explicit trade-offs.
- **Document structure**: Five-document structure with README index, canonical source declarations, cross-references with section numbers, and phase annotations on all type definitions.
- **Risk assessment**: Comprehensive risk tables covering all 5 phases with severity ratings, concrete mitigations, and tiered fallback strategies.
- **Interaction conflict resolution**: Complete priority matrix handling corner cases like right-click during drag, Escape during active interactions.

---

## Recommended Next Steps

All issues resolved across five evaluation passes (6 high, 15 medium, 10 low — all fixed).

1. **Run `plan-execute`** with the grid-component plan path to begin Phase 0 (Foundation)
