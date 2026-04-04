# Annotations & Order Overlays — Implementation Plan

> Chart drawings, order brackets, notes, markers, and broker integration
> Builds on existing `HorizontalLevel` system in midas-chart

---

## Plan Files

| # | File | Scope |
|---|---|---|
| 1 | [01-architecture.md](01-architecture.md) | Module placement, crate boundaries, type hierarchy, dependency flow |
| 2 | [02-core-types.md](02-core-types.md) | Annotation, Anchor, AnnotationKind, style types, ID scheme |
| 3 | [03-order-brackets.md](03-order-brackets.md) | OrderBracket, BracketLeg, entry/TP/SL, shaded zones, status |
| 4 | [04-interaction.md](04-interaction.md) | Drawing modes, hit-testing, drag, snap, keyboard shortcuts |
| 5 | [05-rendering.md](05-rendering.md) | Layer order, GPU pipeline reuse, text labels, iced overlay |
| 6 | [06-persistence.md](06-persistence.md) | File format, save/load, separation from config.toml |
| 7 | [07-order-bridge.md](07-order-bridge.md) | Annotation-to-broker mapping, lifecycle sync, fill markers |
| 8 | [08-implementation-order.md](08-implementation-order.md) | Phased rollout, migration of HorizontalLevel, what ships when |

---

## Design Principles

1. **midas-chart stays sans-IO.** All annotation types and logic are pure Rust — no GPU, no iced, no broker dependencies.
2. **midas-chart doesn't know about orders.** `BracketStatus` is a visual style enum. The app layer maps it to broker order state.
3. **Annotations are user data, not preferences.** They persist separately from config.toml.
4. **Module hierarchy now, crate extraction later.** Clean API boundary inside midas-chart so mechanical extraction to `midas-overlay` is trivial if needed.
5. **Extend existing patterns.** InteractionMode state machine, ChartScene output, GridPipeline reuse, generation-counter dirty tracking.

## Non-Goals (v1)

- Multi-chart synchronized annotations (draw on one, appears on all same-symbol charts)
- Undo/redo stack (will add later, but types are designed to support it)
- Complex drawing tools (trend lines, Fibonacci, rectangles, channels) — architecture supports them but they ship later
- Real-time P&L calculation on bracket overlays
