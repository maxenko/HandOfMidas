# Feature: Annotation Decorator System

Multi-file plan. This index is the entry point — read it first, then jump into the file that matches what you're working on.

## Overview

Replace the hand-rolled per-annotation labeling in `compute_level()` and `compute_bracket()` with a **composable decorator system** that lets any chart annotation attach rich flex-laid-out badges and interactive buttons to a generic price-level primitive. This lands four things the current system cannot express:

1. **SVG-style dash patterns** — arbitrary `[on, off, on, off, ...]` sequences instead of the fixed `Dashed`/`Dotted` enums. Unlocks sparse dots, dash-dot, dash-dot-dot, and custom rhythms.
2. **Composable badges** — a single right-aligned "tag" with a triangular left point, multiple colored segments (role glyph, quantity, price), optional sub-shapes (a black circle around a count digit), and dividers.
3. **Hover-revealed interactive buttons** — close buttons, quick-create-TP/SL carets, edit affordances that only appear when the user is hovering the line or its decorator group. Clicks emit sans-IO `DecoratorAction` variants routed through `ChartAction` to the app layer.
4. **A `PriceLine` primitive** — the shared geometry type that levels, bracket legs, alert lines, and any future horizontal-price annotation compose on top of. Collapses the three near-duplicate "line at a price" structs that exist today.

The system is backed by a new **SDF (signed-distance-field) GPU pipeline** in `midas-render` that rasterizes all non-axis-aligned shapes (pill, rounded-rect, pointed-left, chevron, circle) from a single instance buffer — one new draw call per frame for all decorator shapes across all charts.

This plan does not change the broker layer, the DuckDB `midas-store` crate, or the annotation placement tools. It's a refactor + extension of the render and interaction layers of `midas-chart`, with new GPU infrastructure in `midas-render`, plus a migration of the two existing horizontal-price annotation kinds onto the new primitive.

## Goals

Concrete outcomes. When this plan is done:

1. **`LineStyle` expresses any SVG-style dash pattern.** `Solid` and `Pattern(SmallVec<[f32; 6]>)` replace the rigid `Dashed { dash_len, gap_len }` / `Dotted { dot_spacing }` variants. Every preset in [02-design-decisions.md](02-design-decisions.md) (dotted, dashed, dash-dot, dash-dot-dot, sparse) round-trips through a single `segmented_line()` walker.
2. **Three near-duplicate "line at a price" structs collapse to one `PriceLine`.** The second `widget::level::HorizontalLevel` is deleted. `BracketLeg`'s top-level `color`/`line_width`/`style`/`label`/`timestamp` fields move into `line: PriceLine`. Grep proves there is one canonical representation of a horizontal price line.
3. **The screenshot tag designs from the conversation ship.** Draft brackets render the `[X] ← P | 5000 | 45.01 [▲/▼]` layout at the right edge, with the close button and `▲`/`▼` quick-create buttons revealed on hover. TP legs render the `T | (2) | 100% | 46.40` layout with the black circle around the position count.
4. **Hover-reveal interactive buttons work, and clicks route sans-IO through `ChartAction::DecoratorClick`.** The app layer handles each `DecoratorAction` variant explicitly. `midas-chart` emits data only — no IO leaks across the crate boundary.
5. **One new GPU draw call per frame covers all decorator shapes across all charts.** The new `BadgePipeline` in `midas-render` renders `Rect`, `Rounded`, `Pill`, `PointLeft`, `PointRight`, `DoublePoint`, `Chevron`, and `Circle` via SDF in a single shader, slotted between `candle_bodies` and `crosshair` in the draw order.

## Non-Goals (highlights — full list of eleven in 07)

The decorator system does **not**: (1) rasterize text on the GPU, (2) animate or transition decorator visibility, (3) support keyboard navigation of decorator items, (4) add tooltips, (5) allow decorator drag-and-drop, (6) theme decorator colors per chart, (7) handle `LevelExtent::Between` interaction with decorator anchors, (8) migrate `TextNote`/`Marker` annotation kinds to decorators, (9) auto-stack cross-annotation decorators to prevent overlap, (10) change the broker order round-trip flow, or (11) add undo/redo for decorator actions. See [07-risks-testing.md](07-risks-testing.md) for the one-paragraph rationale on each.

## File map

| File | Purpose | Read when |
|---|---|---|
| **[00-index.md](00-index.md)** | You are here. Overview, goals, nav. | First. |
| **[01-research.md](01-research.md)** | Codebase analysis — the current types, plumbing audit, GPU layout, persistence surfaces. Grounds every later claim. | Before designing anything. |
| **[02-design-decisions.md](02-design-decisions.md)** | Eight consequential decisions with Context / Options / Recommendation / Confidence. | Before reviewing. |
| **[03-data-model.md](03-data-model.md)** | Full type surface: `PriceLine`, `DecoratorGroup`, `Badge`, `Button`, `DecoratorAction`, `BadgeInstance`, `HitZoneKind::Decorator`. Reference material. | When writing types. |
| **[04-rendering.md](04-rendering.md)** | SDF pipeline, `badge.wgsl` shader sketch, both `ChartScene` types, z-order, integration point. | When writing the GPU side. |
| **[05-interaction.md](05-interaction.md)** | Hover two-pass compute, visibility rules, first-frame edge case, click routing, breadcrumbs. | When writing the event-loop side. |
| **[06-implementation.md](06-implementation.md)** | **The execution plan and authoritative dependency graph.** Eleven slices (0, 1, 2, 2.5, 3, 4, 5, 6, 7, 8a-i, 8a-ii, 8b, 9) with Goal / Depends on / Size / Files / Key details / Testing / Done when. The dep graph in 00-index is a conceptual preview; 06 is canonical. | When implementing. |
| **[07-risks-testing.md](07-risks-testing.md)** | Risks (9), testing strategy with perf targets, non-goals, review notes. | Before merging. |

## Top-level dependency graph (conceptual)

This is a **conceptual** preview. The authoritative graph with file references, per-edge
rationale, parallel-window notes, and the execution schedule table lives in
[06-implementation.md](06-implementation.md). When they drift, 06 wins.

```
Slice 0  (SDF shader spike)   ‖   Slice 1  (LineStyle::Pattern)
                                        │
                                        ↓
                           Slice 2  (PriceLine + types + BadgeInstance)
                                        │
                                        ↓
                           Slice 2.5 (hover-recompute benchmark gate)
                                        │
                            ┌───────────┴───────────┐
                            ↓                       ↓
                   Slice 3 (compute)         Slice 4 (SDF GPU pipeline)
                            │                       │
                            └───────────┬───────────┘
                                        ↓
                           Slice 5  (hover two-pass + recompute)
                                        │
                              ┌─────────┴─────────┐
                              ↓                   ↓
                      Slice 6 (actions)   Slice 7 (level migration)
                                    │   │
                                    └─┬─┘
                                      ↓
                           Slice 8a-i  (bracket data model + shim)
                                      │
                                      ↓
                           Slice 8a-ii (visual decorator emissions)  ← screenshot payoff
                                      │
                                      ↓
                           Slice 8b  (button migration + 5-variant deletion)
                                      │
                                      ↓
                           Slice 9   (cleanup + archive)
```

Three parallel windows after Slice 2 lands: (0 ‖ 1) at the start, (3 ‖ 4) and (6 ‖ 7)
afterward. Critical path length with two engineers is roughly 10 steps and ~20.5
calendar days at S/M/L = 1/2/4 day sizes.

## Reading order for new contributors

1. **[00-index.md](00-index.md)** (this file) — 5 min
2. **[01-research.md](01-research.md)** — 10 min. Grounds you in what exists today.
3. **[02-design-decisions.md](02-design-decisions.md)** — 15 min. The "why this shape and not another."
4. Skim **[03-data-model.md](03-data-model.md)** and **[04-rendering.md](04-rendering.md)** — 10 min each. Reference when you need type details.
5. **[06-implementation.md](06-implementation.md)** — pick the slice you're working on.
6. **[05-interaction.md](05-interaction.md)** if your slice touches hover/click routing.
7. **[07-risks-testing.md](07-risks-testing.md)** before merging your slice.

Implementers using `plan-execute`: start at [06-implementation.md](06-implementation.md) Slice 0.

## Status

This plan supersedes the earlier single-file `plan/decorator-system.md`, which has been removed. It has survived two plan-eval passes:

- **Pass 1** (single-file): 2 critical + 6 high findings. Fixed by splitting the flat file into this eight-file directory and reworking the dependency graph, hit-zone payload, `ChartScene` wiring, and hover persistence.
- **Pass 2** (multi-file): 0 critical + 3 high findings (broken cross-file links, Slice 3 ‖ Slice 4 integration ownership gap, Slice 7 hit-test priority during the 5–6 transition window). Fixed by a cleanup pass, moving the placeholder→`BadgeInstance` emission flip into Slice 4's Files list, emitting the level price badge with `action: None` during Slice 7 with a one-line follow-up in Slice 6, and adopting a private `ItemPath` newtype for the decorator hit-zone payload.

See the `## Status` section at the bottom of [06-implementation.md](06-implementation.md) for per-slice execution status as work lands.
