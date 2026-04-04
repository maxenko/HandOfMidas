# 08 — Implementation Order

## Phased Rollout

Each phase is independently shippable. Later phases depend on earlier ones.

---

### Phase 1: Foundation — Migrate Levels into Annotations

**Goal**: Replace `Vec<HorizontalLevel>` with `AnnotationStore` without changing any user-visible behavior.

**Scope**:
- Create `annotations/` module hierarchy
- Define `Annotation`, `AnnotationId`, `AnnotationKind::Level`, `LevelAnnotation`
- Implement `AnnotationStore` with CRUD, iteration, and ID generation
- Migrate `ChartState.levels: Vec<HorizontalLevel>` → `ChartState.annotations: AnnotationStore`
- Update `compute_levels()` to read from `AnnotationStore`
- Update `interaction.rs` hit-testing to use `AnnotationStore`
- Update existing `ChartAction::CreateLevel`, `DragLevel`, `SelectLevel`, `DeleteSelectedLevel`
- Add `annotations: u64` to `DirtyFlags`
- Update persistence (config.toml still, but through AnnotationStore)
- All existing tests pass unchanged

**Verification**: Zero behavior change. All 385+ tests pass. Levels still work identically.

**Estimated files**: 6 new (annotations/mod.rs, types.rs, store.rs, hit_test.rs, render.rs, bracket.rs stub), 5 modified (state.rs, compute.rs, interaction.rs, dirty.rs, scene.rs)

---

### Phase 2: Annotation Persistence — JSON Files

**Goal**: Annotations save/load from dedicated JSON files instead of config.toml.

**Scope**:
- Define `AnnotationFile` serde format
- Implement `save_annotations()` and `load_annotations()` in midas-app
- Add annotation save debounce (shared timer with config save)
- One-time migration: read existing levels from config.toml, write to JSON, remove from config
- Atomic writes (write to .tmp, rename)
- Handle missing/corrupt files gracefully

**Verification**: Close app, reopen. Annotations survive. Delete JSON file, reopen: empty annotations, no crash. Migration from config.toml works once.

**Estimated files**: 2 new (persistence helper, migration), 3 modified (app.rs, persistence.rs, config.rs)

---

### Phase 3: Enhanced Levels

**Goal**: Levels gain labels, line styles, and extend modes.

**Scope**:
- Add `LineStyle` (Solid, Dashed, Dotted) to `LevelAnnotation`
- Add `LevelExtend` (FullWidth, RightFrom, Between)
- Add `label: Option<String>` to levels
- Implement dashed line rendering (multiple short GridLineInstances)
- Add label rendering via iced overlay
- Update level creation UI (default style, no modal dialogs yet)

**Verification**: Create levels with different styles. Dashed lines visible. Labels appear.

---

### Phase 4: Order Brackets — Drawing

**Goal**: Users can draw entry/TP/SL brackets on the chart.

**Scope**:
- Define `OrderBracket`, `BracketLeg`, `BracketSide`, `BracketStatus`
- Add `DrawingBracket` interaction mode with multi-click sequence
- Add preview rendering (ghost lines during drawing)
- Add zone fill rendering (transparent rects between legs)
- Add bracket-specific hit-testing (per-leg)
- Add `DraggingBracketLeg` interaction mode
- R:R ratio computation and display
- Price label badges on Y axis for each leg
- Keyboard shortcut `B` to start drawing

**Verification**: Draw a bracket. Drag individual legs. R:R updates. Delete bracket. Persist to JSON.

**Estimated files**: 2 new (bracket.rs filled out, bracket interaction helpers), 5 modified (interaction.rs, compute.rs, scene.rs, chart_widget.rs, views.rs)

---

### Phase 5: Markers and Notes

**Goal**: Place icons and text notes on the chart.

**Scope**:
- Define `MarkerAnnotation` with `MarkerIcon` enum
- Define `TextNote`
- Implement marker rendering (small rects or SDF circles)
- Implement note rendering (background rect + iced text overlay)
- Add `PlacingMarker` interaction mode
- Add note creation via double-click + text input
- Hit-testing for markers (radius) and notes (bounding box)
- Drag-to-move for notes

**Verification**: Place markers, place notes, drag notes, delete both. Persist to JSON.

---

### Phase 6: Order Bridge — Submit to Broker

**Goal**: Brackets can be submitted as IB bracket orders.

**Scope**:
- Define `OrderAnnotationLink` in midas-app
- Implement "Submit Order" action (bracket → LocalOrder creation)
- Wire fill events from midas-broker broadcast channel
- Auto-create fill markers on fill events
- Bracket status updates (Draft → Pending → Active → Closed)
- Visual state changes per status (dashed → dotted → solid → dimmed)
- Live order modification when dragging legs of active brackets
- Confirmation dialogs for submission, modification, cancellation

**Prerequisite**: midas-broker IB API integration (Phase 1 of broker roadmap) must be complete.

**Verification**: In paper trading mode: draw bracket, submit, see fills, cancel order, see status changes.

---

### Phase 7: Order History

**Goal**: Historical fills appear as markers on the chart.

**Scope**:
- Load fill history from midas-broker database
- Convert fills to locked Marker annotations
- Tag with "fill", "history" for filtering
- Filter toggle in UI: show/hide historical fills
- Buy markers: green triangles. Sell markers: red inverted triangles.
- Tooltip on hover: order details (quantity, price, time, PnL)

**Verification**: Open chart with historical data. Fill markers appear at correct price/time.

---

## Dependency Graph

```
Phase 1: Foundation (migrate levels)
    │
    ├─→ Phase 2: Persistence (JSON files)
    │       │
    │       ├─→ Phase 3: Enhanced Levels (styles, labels)
    │       │
    │       └─→ Phase 4: Order Brackets (drawing)
    │               │
    │               ├─→ Phase 5: Markers & Notes
    │               │
    │               └─→ Phase 6: Order Bridge (requires broker)
    │                       │
    │                       └─→ Phase 7: Order History
    │
    └─→ (all phases depend on Phase 1)
```

Phases 3, 4, 5 can proceed in parallel after Phase 2.
Phase 6 requires Phase 4 + broker integration.
Phase 7 requires Phase 5 + Phase 6.

## Testing Strategy

### Unit Tests (midas-chart)

- `AnnotationStore`: insert, remove, get, iter, next_id, capacity
- `hit_test_annotations`: various annotation types, edge cases
- `compute_annotations()`: level render output, bracket render output
- `BracketLeg` constraint enforcement (TP/SL sides)
- `LineStyle` dashed segment generation
- Round-trip serialization for all annotation types

### Integration Tests (desktop/win/tests/)

- Full pipeline: create annotation → compute scene → verify render data
- Persistence round-trip: create → save → load → verify equality
- Migration: old config.toml with levels → annotation JSON files

### Manual Testing Checklist

- [ ] Draw level, drag, delete
- [ ] Draw bracket (Long), drag each leg, verify R:R
- [ ] Draw bracket (Short), verify TP/SL sides are correct
- [ ] Place marker, place note, drag note
- [ ] Close app, reopen, verify all annotations survived
- [ ] Toggle visibility, lock annotation, verify drag is prevented
- [ ] Zoom/pan: annotations track their anchored positions correctly
- [ ] 20 charts open: annotations don't impact frame rate
