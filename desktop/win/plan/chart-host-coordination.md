# Chart Host Coordination

Refactor the chart component coordination model so that `ChartState` acts as
a stateful host that tools query, rather than having the interaction layer
manually orchestrate crosshair visibility for each tool.

---

## Problem

Crosshair/tool coordination is **hardcoded and scattered** across 7+ sites in
`interaction.rs`. Each site checks specific tool state (`level_tool.is_placing()`,
`level_tool.is_dragging()`) and manually calls crosshair methods. Adding a new
annotation tool (trendline, fib, rectangle) requires touching every one of
these sites.

### Current coordination sites in `interaction.rs`

| Location | What it does |
|---|---|
| `handle_mouse_moved` ~L250 | `if level_tool.is_placing() -> crosshair.enter_preview()` |
| `handle_mouse_moved` ~L278 | `if level_tool.is_dragging() -> crosshair.suppress()` |
| `handle_mouse_pressed` ~L560 | Placing left-click: `crosshair.force_hide()` |
| `handle_mouse_pressed` ~L571 | Placing right-click: `level_tool.suspend_placing()` |
| `handle_mouse_released` ~L681 | `try_resume_placing() -> crosshair.resume_preview()` |
| `handle_mouse_released` ~L695 | Same for right button release |
| `handle_key_pressed` ~L838 | Escape: `level_tool.cancel() + crosshair.force_hide()` |

Each new tool would need its own branch in **all** of these sites.

---

## Design

### Core idea

Crosshair asks the chart host "should I be active?" via a single query method
on `ChartState`, rather than being manually commanded by tool-specific
if/else chains.

### CursorClaim enum

```rust
/// What the active tool needs from the crosshair.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CursorClaim {
    /// No tool claims the cursor. Normal crosshair rules apply.
    None,
    /// A placement tool wants a preview line. Crosshair visible
    /// regardless of mouse button, Y may be snapped.
    Preview,
    /// An active drag/edit wants the crosshair hidden entirely.
    Suppress,
}
```

Priority: `Suppress > Preview > None`. If multiple tools are active
(shouldn't happen, but defensively), highest priority wins.

### Host query method

```rust
impl ChartState {
    /// Query the collective cursor requirement of all active tools.
    ///
    /// The interaction layer calls this instead of checking each tool
    /// individually. New tools only need to be added here.
    pub fn active_cursor_claim(&self) -> CursorClaim {
        // Suppress beats Preview beats None.
        if self.level_tool.is_dragging() {
            return CursorClaim::Suppress;
        }
        if self.level_tool.is_placing() {
            return CursorClaim::Preview;
        }
        // Future tools:
        // if self.trendline_tool.is_editing() { return CursorClaim::Suppress; }
        // if self.fib_tool.is_placing() { return CursorClaim::Preview; }
        CursorClaim::None
    }
}
```

### Why not a trait?

A `ChartTool` trait with `fn cursor_claim(&self) -> CursorClaim` would be
the natural next step if we had 4+ tools with a common interface. Today we
have one tool (LevelTool) and each tool has fundamentally different event
handling (OHLC snap vs anchor points vs bounding boxes). The shared surface
is just this one query.

Concrete methods + centralized query is the right abstraction for now. When
we add a third annotation tool and see the pattern repeat, extract the trait
then.

---

## Changes

### 1. Add `CursorClaim` enum

**File:** `crates/midas-chart/src/state.rs`

Add the enum and the `active_cursor_claim()` method on `ChartState`.
Re-export from `lib.rs`.

### 2. Refactor `handle_mouse_moved` top section

**File:** `crates/midas-chart/src/interaction.rs`

**Before** (two separate early-return blocks, ~50 lines):
```rust
if state.level_tool.is_placing() {
    // ... 20 lines of placing-specific crosshair logic
    return vec![...];
}
if state.level_tool.is_dragging() {
    // ... 20 lines of dragging-specific crosshair logic
    return vec![...];
}
// Normal crosshair path (~15 lines)
```

**After** (unified via `active_cursor_claim`):
```rust
match state.active_cursor_claim() {
    CursorClaim::Preview => {
        // Placing-specific: snap + enter_preview (logic stays, but
        // the dispatch is generic — any future Preview tool lands here)
        return handle_preview_move(state, x, y, alt_held, data, is_collapsed);
    }
    CursorClaim::Suppress => {
        // Drag-specific: compute drag price + suppress crosshair
        return handle_suppressed_move(state, x, y, alt_held, data, is_collapsed);
    }
    CursorClaim::None => {
        // Normal crosshair tracking (existing code, unchanged)
    }
}
```

The tool-specific logic inside each branch doesn't change — we're just
making the **dispatch** generic. The placing branch still does OHLC snap,
the dragging branch still computes grab_offset, etc.

Note: `handle_preview_move` and `handle_suppressed_move` are extracted
helper functions, not new abstractions. They contain the same code that's
currently inline.

### 3. Simplify press/release crosshair coordination

**File:** `crates/midas-chart/src/interaction.rs`

Two distinct concerns in the release paths:

1. **Tool state transitions** — `try_resume_placing()` is a state mutation
   that restores the level tool to Placing mode after a pan/scale
   interruption. This is tool-specific and must remain as-is.
2. **Crosshair coordination** — the `if is_placing() { resume_preview() }`
   check that follows. This is the generic part.

The crosshair coordination (concern 2) can be replaced with a post-event
`active_cursor_claim()` check, eliminating the duplicated if-blocks in both
middle-release and right-release paths:

```rust
// Tool-specific state transitions stay:
state.level_tool.try_resume_placing();

// Generic crosshair sync (replaces duplicated if-blocks):
match state.active_cursor_claim() {
    CursorClaim::Preview => state.crosshair.resume_preview(),
    CursorClaim::Suppress => state.crosshair.suppress(),
    CursorClaim::None => { /* crosshair already handled by on_left_release */ }
}
```

### 4. Escape key — generic tool cancellation

Currently:
```rust
if state.level_tool.is_active() {
    state.level_tool.cancel();
    state.crosshair.force_hide();
    // ...
}
```

After: the level-tool-specific cancel stays (each tool has its own cleanup),
but the crosshair call becomes generic:
```rust
// Cancel whichever tool is active
if state.level_tool.is_active() {
    state.level_tool.cancel();
}
// Crosshair follows from the now-idle tool state
state.crosshair.force_hide();
```

When more tools exist, this becomes a loop or a sequence of cancel calls.

### 5. Deprecated field cleanup (opportunistic)

The `#[deprecated]` fields (`crosshair_pos`, `left_mouse_down`) and their
`#[allow(deprecated)]` sync blocks can be removed if all consumers have been
migrated. Check:

- `views.rs` — already uses `crosshair.render_pos()`
- `chart_widget.rs` — already uses `crosshair.render_pos()` / `force_hide()`
- `app.rs` — needs audit (3 sites)
- `compute.rs` — uses `ChartInput.crosshair_pos` (derived from render_pos)

If all sites are migrated, remove the deprecated fields entirely. This
eliminates ~15 `#[allow(deprecated)]` blocks in `interaction.rs` alone.

---

## What Does NOT Change

- **`CrosshairTool`** — its API is already correct (enter_preview, suppress,
  on_mouse_move, etc.). No modifications needed.
- **`LevelTool`** — already exposes `is_placing()`, `is_dragging()`. No
  modifications needed.
- **`ChartAction` enum** — no new variants.
- **Sans-IO boundary** — events in, actions out. No framework coupling.
- **Tool-specific event handling** — each tool still handles its own events
  (OHLC snap, grab_offset, etc.). We're only making the crosshair dispatch
  generic.

---

## Estimated Scope

~80-120 lines changed across 2 files (`state.rs`, `interaction.rs`).
0 new files. No new dependencies. All existing tests should pass with
minor updates (test helpers may need to account for `CursorClaim`).

---

## Future: Adding a New Annotation Tool

With this architecture, adding e.g. a `TrendlineTool` requires:

1. Create `trendline_tool.rs` with its own state machine
2. Add `pub trendline_tool: TrendlineTool` to `ChartState`
3. Add lines to `active_cursor_claim()`:
   ```rust
   if self.trendline_tool.is_dragging_anchor() { return CursorClaim::Suppress; }
   if self.trendline_tool.is_placing() { return CursorClaim::Preview; }
   ```
4. Add tool-specific event handling in `interaction.rs` (press/release for
   anchors, etc.)
5. Add tool-specific data computation inside `handle_preview_move` and/or
   `handle_suppressed_move` (e.g., anchor-point snapping instead of OHLC
   snapping). The crosshair dispatch is generic, but each tool's data
   computation is inherently tool-specific.

The crosshair mode dispatch and press/release coordination are automatic.
Tool-specific data computation (step 5) still requires a branch per tool
inside the helper functions — this is expected and correct, since each tool
has fundamentally different behavior (OHLC snap vs anchor points vs bounding
boxes).
