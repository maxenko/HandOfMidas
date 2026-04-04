# Plan Evaluation: Grid Component (`midas-grid`)

**Date**: 2026-04-03
**Revised**: 2026-04-03 (second evaluation pass)
**Verdict**: SOLID — Ready for implementation

## Summary

A thorough, well-structured plan across 6 documents for extracting a reusable headless grid widget into a new `midas-grid` crate. Codebase claims were verified against actual source files and found accurate. Architecture decisions (headless core, trait-based columns, phased Widget transition) align with industry best practices (TanStack Table, Dear ImGui, Elm architecture). The phased delivery with spike-based risk mitigation is strong.

Two evaluation passes were conducted. The first pass (9 issues) found 6 already resolved and fixed 3. The second pass (4-agent full review) found 2 medium issues in 02-rendering.md Section 10 (pseudocode contradicted iced's Widget contract; `on_event` vs `Element::map()` contradiction) and fixed both.

---

## Critical Issues

No critical issues found.

---

## High Severity

No high-severity issues found.

---

## Medium Severity (all resolved)

### 1. ~~`ActiveInteraction` enum vs. separate `Option<>` fields~~ (RESOLVED — Pass 1)

**Resolution**: The roadmap already consistently uses the `ActiveInteraction` enum throughout. No contradictory language remains.

### 2. ~~`RowKey` phase timing contradiction~~ (RESOLVED — Pass 1)

**Resolution**: Updated 03-column-data-model.md Section 4.4 — changed "Phase 1" to "Phase 3a" in `RowKeyFn` and `.row_key()` doc comments.

### 3. ~~`SelectionState` defined three different ways~~ (RESOLVED — Pass 1)

**Resolution**: 01-interactions.md Section 4.3 already has the phase annotation clarifying `BTreeSet<usize>` is the all-phases-complete target.

### 4. ~~`ResizeState` field names/types inconsistent~~ (RESOLVED — Pass 1)

**Resolution**: Updated 01-interactions.md — changed `original_width` to `start_width` to match canonical definition.

### 5. ~~Section 10 `draw()` pseudocode contradicts iced Widget contract~~ (RESOLVED — Pass 2)

**Resolution**: Rewrote Section 10 to follow iced's Table widget pattern. `GridWidget` now stores pre-built cell `Element`s, declares them via `children()`, reconciles via `diff()`, lays out child `Node`s in `layout()`, and draws via `tree.children[i]` + `layout.children()[i]`. No more ad-hoc element creation in `draw()`.

### 6. ~~`on_event` closure vs `Element::map()` contradiction~~ (RESOLVED — Pass 2)

**Resolution**: Removed `on_event` field from `GridWidget` struct. Widget now operates on `GridMessage` internally. `update()` publishes `GridMessage` directly via `shell.publish()`. At the call site, `Element::map()` converts to the app's message type, following iced's idiomatic pattern.

### 7. ~~Phase 3a/Phase 2 parallelism understates shared-file risk~~ (RESOLVED — Pass 1)

**Resolution**: 04-implementation-roadmap.md already has the sequencing constraint for `SelectionState` replacement.

---

## Low Severity (all resolved)

### 8. ~~`ColumnConfig` naming collision~~ (RESOLVED)

Runtime type already named `ColumnState` with explicit collision-avoidance note.

### 9. ~~Sort state type inconsistency~~ (RESOLVED)

`Vec<SortSpec>` already annotated as Phase 4 target shape.

### 10. ~~`Widget::update()` method name~~ (RESOLVED)

Added method name note to 02-rendering.md clarifying iced 0.14 may use `on_event()`.

### 11. ~~Accessibility regression from custom Widget~~ (RESOLVED — Pass 2)

Added LOW-severity row to Phase 2 risk table acknowledging the custom Widget bypasses iced's default accessibility infrastructure. Tracked as accepted debt per the stated non-goal.

---

## What's Done Well

- **Codebase alignment**: Every claim verified — `column_widths: [f32; 7]`, `f32::NAN` sentinel, `resizing_column` tuple, `DEFAULT_COLUMN_WIDTHS` values, message variants, all matched actual source.
- **Best practices alignment**: Headless core follows TanStack Table / Dear ImGui precedent. Trait-based columns with enum dispatch is idiomatic Rust. The `cell()` lifetime design avoids data cloning (more sophisticated than iced's built-in Table which requires `T: Clone`).
- **Pre-Phase 2 spike design**: Runs concurrently with Phase 0, has 5 pass/fail criteria, three-tier fallback (spike pass / `stack![]` / Plan C permanent).
- **Commit decomposition**: Phase 0 splits into 4 commits with 3a/3b isolation. Phase 2 into 3 commits. Phase 3 split into 3a (parallelizable) and 3b (widget-dependent).
- **Architecture decisions with alternatives**: Every significant decision includes Y-Statement-complete rationale with explicit trade-offs.
- **Document structure**: Five-document structure with README index, canonical source declarations, cross-references with section numbers, and phase annotations on all type definitions.
- **Risk assessment**: Comprehensive risk tables covering all 5 phases with severity ratings, concrete mitigations, and tiered fallback strategies.

---

## Recommended Next Steps

All issues resolved across two evaluation passes.

1. **Run `plan-execute`** with the grid-component plan path to begin Phase 0 (Foundation)
