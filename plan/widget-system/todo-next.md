# Widget System — Remaining Phases

Phases 1A through 6 are implemented. These two phases remain blocked on external dependencies.

---

## Phase 7: Order Bridge — Submit to Broker

**Blocked on**: midas-broker Phase 1 (IB API, `LocalOrder`, `BrokerCommand`/`BrokerEvent`)

Connects order brackets to the broker. Design in:
- [`02-storage-and-sync.md` Section 5](02-storage-and-sync.md) — BrokerEvent → AnnotationStore
- [`04-interaction-system.md` Section 4](04-interaction-system.md) — bracket interaction + broker wiring
- [`06-implementation-roadmap.md` Section 8](06-implementation-roadmap.md) — Phase 7 tasks

### Tasks
1. `OrderAnnotationLink` — maps `AnnotationId` to broker order IDs (lives in midas-app)
2. Submit action — UI button on bracket label creates `BrokerCommand::PlaceBracketOrder`, updates status to `Pending`
3. Fill event wiring — subscribe to `BrokerEvent::OrderFilled/Cancelled`, update `BracketStatus`, auto-create fill `MarkerAnnotation`
4. Live modification — drag leg while `Active` triggers confirmation dialog, then `BrokerCommand::ModifyOrder`
5. Tests (~12) — submit, fill status update, cancel, fill marker creation, modification rejection. All use `MockBrokerAdapter`

### Success criteria
- Paper trading: draw bracket, submit, see fills, cancel, see status changes
- Fill markers at correct price/time
- Survives restart (persistence from Phase 6)

---

## Phase 8: Polish and Advanced Features

**Blocked on**: all prior phases merged + user demand

| Feature | Description | Reference |
|---------|-------------|-----------|
| Undo/Redo | Action log with inverse ops, stack depth 50, Ctrl+Z/Y | [06-implementation-roadmap.md Section 9](06-implementation-roadmap.md) |
| Annotation Templates | Save/load presets (e.g., "My trading levels"), JSON in `data/templates/` | [06-implementation-roadmap.md Section 9](06-implementation-roadmap.md) |
| Link Groups | Color-coded chart grouping for symbol routing | [06-implementation-roadmap.md Section 9](06-implementation-roadmap.md) |
| Import/Export | Export to JSON/CSV, import from JSON, future TradingView import | [06-implementation-roadmap.md Section 9](06-implementation-roadmap.md) |
| Multi-Select | Shift+click, box select, bulk delete/move/color | [06-implementation-roadmap.md Section 9](06-implementation-roadmap.md) |

### Estimated scope
~5-8 new files, ~8-12 modified, ~20 new tests.
