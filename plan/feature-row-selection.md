# Feature: Order Blotter Row Selection Highlight

## Landing order

This plan lands BEFORE `feature-generic-grid.md` so Slice 4's migration targets
the final 3-arg `OrderBlotterRowSelected` message shape. Lands AFTER/PARALLEL
with `feature-header-settings-button.md` (no coupling).

## Goal

Clicking a row in the Order Blotter panel highlights that row visually (matching
the watchlist's selected-row styling), in addition to the existing symbol-link
broadcast. Visual feedback parity with the watchlist.

## Non-goals

- Multi-select (Ctrl/Shift click).
- Keyboard navigation (arrow keys).
- Selection persistence across restarts — session-scoped only.
- Highlighting every row for a symbol on click. Single-row selection keyed by
  `order_id` (Uuid); multi-row-by-symbol is a separate future feature.
- Any change to the watchlist.

## Design

### Selection key: `Uuid`, not symbol

Rows are keyed by broker-assigned `Uuid`. A bracket produces three rows (entry
+ TP + SL) sharing a symbol; keying selection by `Uuid` highlights exactly the
clicked row. "Highlight all legs" is a separate future feature built on
`parent_id`.

### State

Add one field to `OrderBlotterPanel`
(`desktop/win/crates/midas-app/src/order_blotter/panel.rs`):

```rust
/// Currently selected order row, keyed by broker-assigned order UUID.
/// `None` = nothing selected. Session state only — not persisted.
pub selected_row: Option<uuid::Uuid>,
```

Init `None` in `new()` and `from_config()`. `to_config()` unchanged —
`OrderBlotterConfig` gets no field; selection resets on restart, by design.

### Message wiring

Extend `Message::OrderBlotterRowSelected` in `app.rs` to carry the Uuid
alongside the existing symbol:

```rust
// Before
OrderBlotterRowSelected(OrderBlotterId, String),
// After
OrderBlotterRowSelected(OrderBlotterId, uuid::Uuid, String),
```

One message, not two: splitting would risk selection-set and broadcast-gated
handlers drifting out of sync.

In `handlers.rs` (`handle_order_blotter_msg`, `OrderBlotterRowSelected` arm),
replace the existing two-step `self.order_blotters.get(...)` with a single
`get_mut` that writes selection FIRST, then decides whether to broadcast:

```rust
let Some(panel) = self.order_blotters.get_mut(&blotter_id) else {
    return Task::none();
};
panel.selected_row = Some(order_id);
let link = panel.symbol_link;
if matches!(link, LinkMode::Unlinked) {
    return Task::none();
}
self.broadcast_symbol_to_link_group(link, &symbol)
```

Selection write happens BEFORE the `Unlinked` short-circuit so selection is
never skipped for unlinked panels. A mutable handle is required — do not keep
the original immutable `get(...)` shape.

### Render

In `view_order_blotter_body` (`app/views.rs`), the body-row loop currently
computes `bg` from row parity only. Selection needs the raw `Uuid` at render
time — `DisplayRow` only stores a shortened `order_id: String` and
`order_id_sort_key: u128`. Add the raw Uuid:

```rust
// In order_blotter/columns.rs::DisplayRow
pub order_uuid: Uuid,  // full Uuid, used for selection + message payload
```

Populate in `DisplayRow::from_row` (`order_uuid: row.order_id`). The existing
string+sort-key fields stay as-is — renaming is churn for no value.

Then in the row loop:

```rust
let is_selected = panel.selected_row == Some(r.order_uuid);
let bg = if is_selected {
    Color::from_rgba(0.2, 0.35, 0.55, 0.6)   // match watchlist selection tint
} else if row_idx % 2 == 0 {
    Color::from_rgba(1.0, 1.0, 1.0, 0.02)
} else {
    Color::TRANSPARENT
};
```

The `mouse_area::on_release` message becomes:

```rust
.on_release(Message::OrderBlotterRowSelected(
    blotter_id,
    r.order_uuid,
    sym_for_click,
));
```

### Sort interaction

Selection is keyed by `order_id`, a stable identifier independent of row order,
so re-sorting preserves the highlight without extra work — the render loop
re-checks `panel.selected_row == Some(r.order_uuid)` per row and the match
follows the row to its new visual index.

Blotter uses direct `panel.selected_row == Some(uuid)` matching per row rather
than the watchlist's `midas_grid::Selection` index-based bridge — the blotter
doesn't use `midas_grid` today. If it later adopts the shared `grid_body_row`
helper from `feature-generic-grid.md`, the `selected: bool` param is populated
from the same match — no migration needed.

### Link-broadcast interaction

Row click always sets `panel.selected_row`; additionally broadcasts to the
link group when `symbol_link != Unlinked` (unchanged). Clicking the
already-selected row re-asserts selection and re-broadcasts (no toggle-off).
Toggle-off is out of scope for v1.

### Row removal / blotter clear

If a selected order is pruned, the stale Uuid matches no row — nothing
highlights, benign. No eager cleanup; next click overwrites it.

## Files touched

Numbered build order — each step compiles only after the previous lands:

1. `order_blotter/panel.rs` — add `selected_row: Option<uuid::Uuid>` + init in
   `new()`/`from_config()`.
2. `order_blotter/columns.rs` — add `order_uuid: Uuid` to `DisplayRow`;
   populate in `from_row`.
3. `app.rs` — extend `Message::OrderBlotterRowSelected` variant with `Uuid`.
4. `app/handlers.rs` — rewrite the arm as `get_mut` + unconditional
   `selected_row` write, then `Unlinked` short-circuit, then broadcast.
5. `app/views.rs` — `view_order_blotter_body`: compute `is_selected`, adjust
   `bg`, include `r.order_uuid` in the `on_release` payload.

All paths under `desktop/win/crates/midas-app/src/`.

## Tests

Manual verification (mirrors how watchlist selection was validated):

1. Orders pane with 3+ rows; click a row — background tints blue, others
   unchanged.
2. Click a different row — tint moves.
3. Re-sort via column header — highlight follows the `order_id`, not the
   visual index.
4. With link colour set, confirm symbol still broadcasts.
5. With `Unlinked`, confirm click still highlights locally.
6. Restart — selection cleared (non-goal to persist).

Add one unit test near `handlers.rs` asserting `OrderBlotterRowSelected`
writes `selected_row` on the target panel regardless of link mode — pins the
"selection always applies" invariant.

## Risks

- `Message` payload change ripples to every call site — two known constructors
  (view) and one matcher (handler + the `..` catch-all at `app.rs:2356`);
  compiler flags the rest.
- `DisplayRow` grows by 16 bytes; negligible at tens-of-rows scale.
