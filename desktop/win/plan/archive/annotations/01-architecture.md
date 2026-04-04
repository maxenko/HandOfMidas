# 01 — Architecture

## Decision: Module Hierarchy Inside midas-chart

Annotations live inside `midas-chart` as a module hierarchy, not a separate crate.

### Rationale

- Annotations need `Camera2D.price_to_y()`, `time_to_x()` — already in midas-chart
- Hit-testing operates in chart pixel coordinates — same coordinate space as existing level hit-tests
- Annotations are part of `ChartScene` output — the compute pipeline already produces them
- Existing `HorizontalLevel` already lives in midas-chart and follows this exact pattern
- There is exactly one consumer of annotation logic: `compute_chart_scene()`

### Escape Hatch

The module exposes a clean public API: types + store + query. If annotations outgrow midas-chart (>2000 lines, >10 types, or need their own persistence crate), extraction to `midas-overlay` is a mechanical `cargo new` + move + re-export.

## Module Layout

```
midas-chart/src/
├── annotations/
│   ├── mod.rs              # Re-exports, AnnotationId, Anchor enum
│   ├── types.rs            # Annotation struct, AnnotationKind enum, style types
│   ├── store.rs            # AnnotationStore: Vec<Annotation> + CRUD + spatial index
│   ├── bracket.rs          # OrderBracket, BracketLeg, BracketSide, BracketStatus
│   ├── note.rs             # TextNote — price/time anchored text
│   ├── marker.rs           # Marker — icon/stamp at a point (fills, signals, flags)
│   ├── hit_test.rs         # Unified hit-testing: point → Option<(AnnotationId, HitZone)>
│   └── render.rs           # AnnotationRender variants → GPU-ready data
│
├── levels.rs               # HorizontalLevel (unchanged short-term; migrates to annotations/ in phase 1)
├── compute.rs              # gains compute_annotations() call
├── interaction.rs          # gains DrawingBracket, DraggingBracketLeg, DraggingNote modes
├── state.rs                # ChartState.annotations: AnnotationStore
├── scene.rs                # ChartScene.annotations: Vec<AnnotationRender>
├── input.rs                # ChartInput unchanged (annotations come from state, not input)
└── dirty.rs                # DirtyFlags gains annotations: u64 generation counter
```

## Dependency Graph

```
                    ┌──────────────┐
                    │  midas-app   │  ← Order bridge: annotation ↔ broker
                    └──────┬───────┘
                           │
              ┌────────────┼────────────┐
              │            │            │
       ┌──────┴───┐  ┌────┴────┐  ┌───┴────────┐
       │midas-render│ │midas-chart│ │midas-broker│
       │ (GPU draw)│  │(sans-IO) │  │ (IB API)  │
       └──────┬───┘  └────┬────┘  └───┬────────┘
              │            │            │
              └────────────┼────────────┘
                           │
                    ┌──────┴───────┐
                    │  midas-core  │  ← ChartId, Timeframe, shared enums
                    └──────────────┘
```

**Key boundary**: midas-chart and midas-broker never depend on each other. The bridge is always midas-app.

## Data Flow

### Creation

```
User double-clicks price axis → ChartEvent::DoubleClick
    → handle_event() → ChartAction::CreateLevel { price }
    → state.apply_action() → annotations.insert(HorizontalLevel { ... })
    → dirty.annotations += 1

User activates bracket tool, clicks entry price → ChartEvent::MousePressed
    → handle_event() → InteractionMode::DrawingBracket { phase: Entry }
    → clicks TP → phase: TakeProfit
    → clicks SL → phase: StopLoss
    → ChartAction::CreateBracket { entry, tp, sl }
    → state.apply_action() → annotations.insert(OrderBracket { ... })
```

### Rendering

```
compute_chart_scene(&input)
    → compute_annotations(state.annotations, camera, viewport)
    → Vec<AnnotationRender> (GPU-ready rects, lines, text positions)
    → ChartScene { annotations: Vec<AnnotationRender>, ... }

ChartRenderer::render_prepare()
    → annotation_pipeline.update_instances(annotation_lines)
    → (text labels handled by iced overlay, like date labels)

ChartRenderer::render_draw_calls()
    → draw order: grid → volume → VP → wicks → bodies → annotations → crosshair
```

### Order Bridge (midas-app only)

```
User right-clicks bracket → context menu → "Submit Order"
    → app creates LocalOrder via midas-broker API
    → app sets annotation.external_id = Some(order.id.to_string())
    → app sets bracket.status = BracketStatus::Pending

Broker fills order
    → app receives fill event on broadcast channel
    → app looks up annotation by external_id
    → app updates bracket.status = BracketStatus::Active
    → app creates Marker annotation at fill price/time with "filled" icon
```

## What Stays in midas-chart vs midas-app

| Concern | Where | Why |
|---|---|---|
| Annotation types & enums | midas-chart | Pure data, framework-agnostic |
| AnnotationStore CRUD | midas-chart | Sans-IO collection management |
| Hit-testing | midas-chart | Needs camera coordinate transforms |
| Interaction modes | midas-chart | Part of InteractionMode state machine |
| GPU render data computation | midas-chart | Part of compute_chart_scene() |
| Annotation ↔ order mapping | midas-app | Needs both chart and broker |
| JSON persistence (save/load) | midas-app | I/O belongs in the app shell |
| Context menus, toolbars | midas-app | UI framework dependent |
| Annotation type definitions for serde | midas-chart | Types derive Serialize/Deserialize |
