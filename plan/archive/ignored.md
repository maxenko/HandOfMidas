# Ignored Plan Items

## Clean up RightClickLevel action (remove x, y coordinates)
- **Plan source**: architecture-improvements.md, Item 4
- **Status**: Not applicable
- **Reason**: The `x, y` fields carry the cursor position at click time, needed to position the level editor popup. This cannot be recovered "at view time" as the plan suggests because the cursor moves between the click event and the next view rebuild. The coordinates are part of the interaction context, not a pure UI concern. A rename from `RightClickLevel` to `EditLevel` would be cosmetic and not worth the churn across 4 files.

## Unify normal/collapsed code paths
- **Plan source**: architecture-improvements.md, Item 2
- **Status**: Not applicable
- **Reason**: Deferred — this is a significant refactoring of compute.rs that benefits from Item 3's integration test coverage being in place first. The plan eval recommended implementing this last. Tracked for a future session.

## Add interaction sequence tests
- **Plan source**: architecture-improvements.md, Item 3
- **Status**: Not applicable
- **Reason**: Deferred — new test authoring is valuable but independent of the code cleanup in Item 1. Tracked for a future session.

## Deduplicate crosshair label computation
- **Plan source**: architecture-improvements.md, Item 5
- **Status**: Not applicable
- **Reason**: The plan explicitly states "accept the duplication as a cost of the clean sans-IO/overlay split. Not worth restructuring data flow for this alone." Accepted as-is.
