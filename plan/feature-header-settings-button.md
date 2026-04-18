# Orders Panel — Move Column-Selector Gear to Title Bar

## Goal

Move the orders-panel column-selector trigger (the `⋮` gear button) out of the far-right
end of the column header row and into the pane title bar, positioned directly to the
**left of the close `X`** and adjacent (to the right of) the existing `[S]` symbol-link
button. The gear must always be visible regardless of how many columns the blotter shows
or how far the horizontal scroll has been dragged.

## Motivation

With the gear sitting at the tail of the header row (see
`midas-app/src/app/views.rs` ~L2494), users with many visible columns must scroll the
grid horizontally to reach the settings button. Relocating it to the title bar — which
is a fixed-width strip that never scrolls — makes it reachable in one click at all
times. The title bar sits **outside** the body's scrollable region, so the gear is
always reachable even when the grid is horizontally scrolled. This is the primary
reason for the move, not just aesthetics.

## Scope

Pure view relocation. No new state fields, no new messages, no handler changes. Only
`view_order_blotter_title_bar` and `view_order_blotter_body` are modified.

### Changes in `view_order_blotter_title_bar` (`views.rs` ~L2261–L2329)

- Add a new `gear_btn` definition **next to** the existing `link_btn` and `close_btn`
  definitions.
- Use the same `hover_text_button_style` that `close_btn` uses so the two icon-only
  buttons match visually.
- Emit `Message::OrderBlotterOpenColumnSelector(blotter_id)` on press. (The `blotter_id`
  is already in scope in this function.)
- Extend the `.controls(...)` row so the order reads **left to right**:
  `link_btn` → spacer(4) → `gear_btn` → spacer(2) → `close_btn`. Keep the existing
  `.spacing(2)` and `align_y(Center)`.

```rust
// Sketch — drop in alongside link_btn / close_btn.
let gear_btn: Element<'_, Message> = button(text("⋮").size(12))
    .on_press(Message::OrderBlotterOpenColumnSelector(blotter_id))
    .padding([2, 6])
    .style(hover_text_button_style)
    .into();
```

### Changes in `view_order_blotter_body` (`views.rs` ~L2333+)

- **Delete** the trailing gear cell in the header row (currently ~L2492–L2498: the
  `gear_btn` `button(...)` followed by
  `header_cells.push(container(gear_btn).width(28)...)`).
- **Delete** the matching tail spacer in the body rows (~L2610–L2612:
  `cells.push(container(Space::new()).width(28).into());`).
- Nothing else in the body needs to move — rows already align to the header via shared
  column widths from `panel.grid_state.column_width`.

### Icon choice

Keep `⋮` (U+22EE) for consistency with the currently shipped glyph and to match the
muscle memory users have already built. `⚙` (U+2699) renders inconsistently at
small sizes across the fonts iced loads on Windows 11 — verified against the existing
`text(...).size(10..14)` sites — so we stick with `⋮`.

### Visual spec

- **Icon:** `⋮` at `size(12)` (matches the `[S]` button's 10–12 range).
- **Padding:** `[2, 6]` (matches `close_btn`).
- **Color:** `theme::TEXT_SECONDARY` (matches the old gear).
- **Style fn:** `hover_text_button_style` (matches `close_btn`).

## Popup anchoring

The column-selector popup is rendered inside `view_order_blotter_body`, anchored with
`align_x(Right).align_y(Top).padding([32, 6])` against the body `Fill` area (views.rs
~L2711–L2719).

The popup stays inside the body stack (the popup must overlay the grid, not the title
bar), but **popup anchoring tuning is required**: the trigger moves from the
column-header row (inside the body stack) to the title bar (above the body). The
existing `padding([32, 6])` was sized to clear the in-body column header and will leave
a visible gap in the new layout. Reduce to `padding([4, 6])` or similar; verify
visually. Do **not** restructure the overlay stack — this is a one-line tuning change.

## Non-goals

- **Watchlist title bar is untouched.** Watchlists have no column selector; this plan is
  orders-panel-only.
- **Popup contents unchanged.** No edits to the checklist rows, the backdrop, or the
  `OrderBlotterToggleColumn` / `OrderBlotterDismissColumnSelector` message flow.
- **No reusable-popup extraction.** That belongs to `feature-generic-grid.md`. Keep
  this PR narrowly focused on button relocation.

## Dependencies

Standalone. No dependency on the generic-grid refactor. If `feature-generic-grid.md`
lands later, the generic grid component can simply expose a trigger-injection slot and
replace this call site without disturbing the `Message` wiring or `MidasApp` state.

**Coordination with `feature-generic-grid.md`:** If that plan lands **after** this one,
its Slice 4 migration instruction to relocate the gear becomes a no-op — the gear is
already in the title bar. If it lands **first**, this plan's header-deletion portion
still applies but against the post-refactor code.

## Test plan

- `cargo clippy -p midas-app -- -D warnings` — ensure no warnings from the new button
  or the removed cells.
- Manual: open an orders pane, confirm gear is visible in the title bar regardless of
  column count / horizontal scroll position. Click it — popup opens. Click outside —
  popup dismisses. Click a column row — toggles as before.
- Manual: resize the blotter pane narrow enough that the grid scrolls horizontally;
  confirm the gear never moves or disappears.
- Manual: confirm the `[S]` link-picker button, gear button, and `X` close button all
  sit on the same baseline with consistent padding.
