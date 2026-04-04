# Grid Component Plan

Professional-grade, headless grid/table widget for Hand of Midas.
Built on iced 0.14, designed for trading workflows (watchlists, blotters, scanners).

**Date**: 2026-04-01
**Status**: Planning complete, all eval issues resolved (3 passes, 4-agent review), ready for implementation
**Research**: [`research/grid-component/`](../../../../research/grid-component/README.md)

---

## Motivation

The current watchlist grid is implemented inline across `views.rs`, `app.rs`, and
`watchlist.rs` (~400 lines of layout, styling, resize, sort, and selection logic,
distributed across non-contiguous sections) with no reuse path. As the application grows
to include order blotters, scanner results, position tables, and trade history views,
duplicating this code per panel is unsustainable. This plan extracts a generic, trait-based
grid component into its own crate (`midas-grid`) that any panel can consume.

---

## Plan Documents

| # | Document | Focus |
|---|---|---|
| 00 | [Architecture](00-architecture.md) | Crate placement, headless core (TanStack pattern), `GridColumn` trait, `GridState`, message architecture, component hosting, dependency graph, public API sketch |
| 01 | [Interactions & UX](01-interactions.md) | Column resize, column reorder (drag preview), row selection (single/multi/range), row drag-and-drop, sorting, keyboard navigation, scrolling, context menus, trading-specific interactions (flash-on-tick, symbol linking), conflict resolution |
| 02 | [Rendering](02-rendering.md) | Rendering strategy decision (pure iced vs wgpu vs hybrid), 7-layer architecture, virtual scrolling, flash-on-tick animation, conditional formatting, drag visuals, performance budget, theme integration, iced Widget implementation |
| 03 | [Column & Data Model](03-column-data-model.md) | `GridColumn<T, M>` trait design, `ColumnWidth` system (Fixed/Auto/Flex), column configuration & persistence, data access pattern, pre-built column types (Text, Numeric, Button, Toggle, DragHandle), watchlist column definitions, multi-sort, type safety |
| 04 | [Implementation Roadmap](04-implementation-roadmap.md) | 5-phase plan (Foundation → Core Interactions → Drag & Drop → Polish & Performance → Advanced), file structure, per-phase types/files/tests/acceptance criteria, risk assessment, migration path from current watchlist |

---

## Goals

1. **Replace the current inline watchlist grid** in `views.rs` with a reusable, trait-based grid component
2. **Support arbitrary cell content** — any iced widget (text, button, toggle, input, canvas) can live in a cell
3. **Full column interactions** — resize by dragging dividers, reorder by dragging headers, click-to-sort with indicators
4. **Full row interactions** — single/multi selection, drag-and-drop reorder with visual feedback
5. **Trading-grade features** — flash-on-tick, conditional formatting, symbol linking, keyboard navigation

## Non-Goals

- **Inline cell editing** — Cells display widgets but there is no built-in "edit mode" (post-Phase 4)
- **Tree grid / grouped rows** — Flat row lists only; hierarchical grouping is out of scope
- **Accessibility / screen reader support** — Not targeted for Phase 0-4
- **Real-time streaming integration** — The grid receives pre-sorted data slices; it does not subscribe to data feeds
- **Computed / formula columns** — User-defined formulas are out of scope (post-Phase 4)
- **Variable row heights** — All rows have uniform height for layout simplicity and virtual scrolling
- **Horizontal scrolling** — Phases 0-3 assume all columns fit the panel viewport; if resize would cause overflow, columns proportionally shrink to fit. Phase 4 introduces horizontal scrolling alongside column pinning (see `01-interactions.md` Sections 8.2-8.5).

---

## Key Architecture Decisions

1. **New crate `midas-grid`** — Independent of `midas-core`, depends only on `iced` + `serde`
2. **Headless core** — `GridState` is a plain struct owned by the app; grid widget is stateless
3. **Specs-only sorting** — Grid emits `SortToggled` message; app sorts its own data
4. **Trait-based columns** — `GridColumn<T, M>` trait with `header()`, `cell()`, `compare()`
5. **Cell = any iced widget** — `cell()` returns `Element<'a, M>`, hosting text/buttons/inputs/canvas
6. **Hybrid rendering** — iced widgets for cell content, custom overlays for drag/flash/resize
7. **Message mapping** — Cells emit app's `M` directly; grid chrome maps via `on_grid` callback (`Fn(GridMessage) -> M`)
8. **Three-state sort cycle** — Phase 1 introduces Asc -> Desc -> None (clear), replacing the current two-state toggle so users can return a column to its unsorted state

## Implementation Phases

| Phase | Scope | Key Deliverable | Depends On |
|---|---|---|---|
| **0: Foundation** | Core types, basic grid, fixed header + scrollable body | Grid replaces current watchlist with same features | — |
| **1: Core Interactions** | Column resize, sort, row selection, interactive cells | Feature parity + resize + sort | Phase 0 |
| **2: Drag & Drop** | Column reorder, row drag, drag previews, drop indicators | Full DnD for columns and rows | Phase 1 |
| **3a: Polish (Widget-independent)** | Flash-on-tick, conditional formatting, multi-selection, column persistence | Trading-quality visuals and data interactions | Phase 1 |
| **3b: Polish (Widget-dependent)** | Virtual scrolling, keyboard navigation | Performance and keyboard-driven workflows | Phase 2 |
| **4: Advanced** | Context menus, column presets, pinning, multi-sort, copy/paste | Professional trading-grade grid | Phase 2 + 3b |
