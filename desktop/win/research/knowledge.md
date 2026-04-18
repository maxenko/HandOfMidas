# Knowledge Base — Hand of Midas Desktop (Windows)

Hard-won lessons from debugging. Check here before fighting the same battles.

---

## iced 0.14 Layout

### Never use `height(Fill)` on children inside a Row that sits in a Column

**Symptom**: Content appears vertically centered instead of top-aligned. The header row expands to fill the entire parent Column, pushing the scrollable body to the middle.

**Root cause**: iced's layout engine resolves `Fill` height on Row children against the parent Column's available space. A single `Space::new().width(4).height(Fill)` resize handle spacer inside a header `Row` makes the Row's height equal to the full Column height — because the Row's height is the max of its children.

**Fix**: Use a fixed pixel height matching the row height:
```rust
// WRONG — expands the Row to fill the Column
Space::new().width(4).height(Fill)

// RIGHT — fixed height matching the header
Space::new().width(4).height(26)
```

**Applies to**: Any `Space`, `mouse_area`, or widget with `height(Fill)` inside a `Row` that is a direct child of a `Column` where sibling elements use `height(Fill)` (like a scrollable body area). The `Fill`-height Row and the `Fill`-height scrollable compete for space, and the Row wins because it resolves first.

**Discovered**: 2026-04-04, grid component Phase 0 layout debugging.
