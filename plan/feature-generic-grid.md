# Feature: Generic Grid Helpers and Reusable Column-Selector Popup

## Goal

Extract the duplicated header/body/popup plumbing shared by the Watchlist
(`view_watchlist_body`) and the Order Blotter (`view_order_blotter_body`) into
stateless helper functions inside `midas-grid`, so both panels — and any future
table panel — share the same renderer. In particular, the column-show/hide popup
that currently lives inline in `view_order_blotter_body` becomes reusable so the
Watchlist can adopt it in a follow-up with trivial wiring.

## Non-goals

- **Do not** rescue the generic `midas_grid::grid()` builder. Its borrow-graph
  problem (local `Vec<Row>` cannot outlive the view fn) is real and its fix is
  Phase 2 (custom `Widget<M>` impl) — out of scope here. The comment at
  `midas-grid/src/widget.rs:25` stays accurate.
- **Do not** reshape `GridState` or the `GridColumn` trait. Additions only —
  no renames, no field removals, no trait-method changes. Persistence format
  for `column_widths` is frozen.
- **Do not** move `WatchlistRow` or `DisplayRow` (or their sort/filter code)
  into `midas-grid`. Row data and its domain transformations stay in
  `midas-app` where they belong.
- **Do not** add a column selector to the Watchlist in this work. That is a
  three-line follow-up explicitly called out below.

## Architecture

Four stateless helpers added to `midas-grid` under a new `helpers/` module
directory. All take owned or prebuilt `M`-typed messages — none capture borrows
of app state — so the lifetime trap that sinks the `grid()` builder never
appears.

### Relationship to existing `grid_header` / `grid_body`

`midas-grid/src/header.rs` and `midas-grid/src/body.rs` already export
whole-row `grid_header` / `grid_body` used by the (currently unusable)
`grid()` builder. Those stay put — they are the Phase-2 Widget-impl path. The
new `grid_header_cell` / `grid_body_row` in `helpers/` are for hand-built
panels today. To keep these straight, add a one-line module doc to each of
the four files pointing at its counterpart:
- `header.rs`/`body.rs` — "Whole-row, for the `grid()` builder. Hand-built
  panels use `helpers::grid_header_cell` / `grid_body_row`."
- `helpers/header_cell.rs`/`helpers/body_row.rs` — "Per-cell/per-row, for
  hand-built panels. The `grid()` builder uses `header::grid_header` /
  `body::grid_body`."

### 1. `ResizeHandle<M>` — value type

A tiny struct wiring the drag-start message for a column divider. Lives in
`midas-grid::helpers` alongside the other helpers.

```rust
#[derive(Debug, Clone)]
pub struct ResizeHandle<M: Clone> {
    /// Emitted on mouse-press on the handle strip.
    pub on_press: M,
    /// Height of the hit strip, logical px. Default 26.0.
    pub height: f32,
    /// Width of the hit strip, logical px. Default 4.0.
    pub width: f32,
}
```

The non-`pub` fields that end up internal-only until slice 4/5 consume them get
`#[allow(dead_code)]` (see slice 2 note below).

### 2. `grid_header_cell<M: Clone>(...)` + `HeaderStyle`

A `HeaderStyle` struct mirrors the existing `GridStyle` convention (see
`midas-grid/src/style.rs:15`) and keeps the signature from sprawling:

```rust
pub struct HeaderStyle {
    pub padding: [u16; 2],
    pub border_width: f32,
    pub border_color: Color,
}
impl Default for HeaderStyle { /* blotter values: [6,8], 0.5, GRID_HEADER_BORDER_COLOR */ }

pub fn grid_header_cell<'a, M: Clone + 'a>(
    label: &str,
    width: f32,
    sort_indicator: &str,
    sort_msg: Option<M>,
    resize: Option<ResizeHandle<M>>,
    style: &HeaderStyle,
) -> Element<'a, M>
```

Semantics:
- If `sort_msg.is_some()`, the label + indicator are wrapped in a `mouse_area`
  that emits `sort_msg` on release.
- If `resize.is_some()`, a 4-px right-edge drag strip is layered on top via
  `stack!` so it does not push width.
- Watchlist passes a `HeaderStyle { padding: [2,4], border_width: 1.0, .. }`;
  blotter takes `HeaderStyle::default()`.

### 3. `grid_body_row<'a, M: Clone + 'a>(...)`

```rust
pub fn grid_body_row<'a, M: Clone + 'a>(
    cells: Vec<Element<'a, M>>,
    selected: bool,
    alt_bg: bool,
    on_click: Option<M>,
) -> Element<'a, M>
```

Semantics:
- Background resolution reads from `GridStyle::default()`:
  `selected → GridStyle::default().selected_bg`,
  else `alt_bg → ALT_ROW_BG`, else `TRANSPARENT`.
- No new `SELECTED_ROW_BG` const is introduced. `GridStyle::selected_bg`
  (`midas-grid/src/style.rs:25,39`) is already the single source of truth; the
  helper reuses it. Only `ALT_ROW_BG` is new: `rgba(1.0, 1.0, 1.0, 0.02)`
  (matches blotter today), added to `midas-grid::style`.
- A future variant taking `&GridStyle` can be added when a panel needs a
  non-default palette; until then the helper just calls
  `GridStyle::default()` internally.
- `on_click.is_some()` → wrap in `mouse_area` with `on_release(on_click)`.

### 4. `column_selector_popup<'a, M: Clone + 'a>(...)` + `ColumnEntry`

Fold the `mandatory` set into each entry so illegal states (a mandatory ID not
in the entries slice) become unrepresentable:

```rust
pub struct ColumnEntry<'a> {
    pub id: ColumnId,
    pub label: &'a str,
    pub mandatory: bool,
}

pub fn column_selector_popup<'a, M, F>(
    entries: &[ColumnEntry<'_>],
    hidden: &HashSet<ColumnId>,
    on_toggle: F,
    on_dismiss: M,
) -> Element<'a, M>
where
    M: Clone + 'a,
    F: Fn(ColumnId) -> M + 'a,
```

Semantics:
- Returns **only** the popup container (checklist with `☑`/`☐` rows, top-right
  aligned dark panel). Caller pushes it into its own `stack![]` alongside a
  backdrop that emits `on_dismiss`. The helper does **not** render the
  backdrop itself.
- Entries with `mandatory == true` render disabled (no `mouse_area` wrapper)
  but still show a ticked box; matches today's Symbol-is-mandatory blotter
  behaviour.
- Entry ordering: `entries` slice order. Caller controls.

### Why helpers and not a builder?

Every signature above takes `M`-typed values the caller has already
constructed. No `Fn(&Row) -> M` callback that needs to outlive the borrow of
a local `Vec<Row>`. This is exactly why the current `grid()` wrapper can't be
used: it wants `&'a [T]` for locally-built `T`s. These helpers don't.

## Call-site migration

### `view_order_blotter_body` (richer case — do it first)

Current scope: 408 lines (views.rs:2333–2741). Post-refactor target: ≤ ~250.

Replace:
- Header-cell block (views.rs:2422–2490) → per-column `grid_header_cell(...)`
  with `ResizeHandle` for all but the last column. **Gear cell:** if
  `feature-header-settings-button.md` has already landed, there is no gear
  cell — the gear lives in the title bar. If it has not landed, preserve the
  inline gear cell for now; `feature-header-settings-button.md` will remove
  it as a separate PR.
- Body-row loop (views.rs:2503–2628) → `grid_body_row(cells, selected,
  alt_bg, Some(row_click_msg))`. Cell-content construction stays here (it's
  domain-specific: `symbol_badge`, status colour mapping, etc.). The helper
  just wraps them.
- Column-selector popup layer (views.rs:2665–2719) → one call to
  `column_selector_popup(&entries, &panel.hidden_columns, toggle_fn,
  dismiss_msg)`, where `entries` is a `Vec<ColumnEntry>` built from
  `ALL_COL_DEFS` marking `Symbol` as mandatory. Pushed into the stack exactly
  where the inline popup used to go. Backdrop stays as its own layer.

### `view_watchlist_body`

Current scope: 379 lines (views.rs:1226–1604). Post-refactor target: ≤ ~260.

Replace:
- Header-cell loop (views.rs:1347–1423) → `grid_header_cell(...)` per column.
  The quirky `col_idx = i + 1` offset (legacy `COL_DRAG` space) is preserved
  at the call site — the helper does not know about it; caller still builds
  the `WatchlistColumnResizeStart(wl_id, col_idx, 0.0)` message itself.
- Body-row loop (views.rs:1427–1521) → `grid_body_row(...)` wrapping the
  existing `grid_data_cell`-built cells. The existing local `grid_data_cell`
  helper in `midas-app` stays put — it's a padding wrapper, not grid chrome.

### What stays where

| Stays in `midas-app` | Moves to `midas-grid` |
|---|---|
| `WatchlistRow`, `DisplayRow` | (nothing domain-shaped) |
| Sort comparator invocation | Helper pieces, `grid_header_cell`, `grid_body_row`, `column_selector_popup`, `ResizeHandle`, `HeaderStyle`, `ColumnEntry` |
| `symbol_badge`, per-column cell content | `ALT_ROW_BG` constant (selected-bg already in `GridStyle`) |
| `grid_data_cell` padding wrapper | |
| The `ALL_COL_DEFS` slice literals (domain columns) | |

### LOC measurement

Reviewers should see the win. In each migration commit:

```bash
git diff --stat desktop/win/crates/midas-app/src/app/views.rs \
               desktop/win/crates/midas-grid/
```

Expected net change across slices 4+5: views.rs shrinks ~250 lines, midas-grid
grows ~200. The saving is real (~50 lines net) but the bigger win is
deduplication — slice 5 adds near-zero code to migrate the watchlist because
slice 4 already built the helpers.

## Side benefit (NOT in scope here)

After this lands, adding a column selector to the watchlist is a three-part
diff in a separate PR:

1. One new `Option<WatchlistId>` field on `MidasApp` for open-popup tracking.
2. One gear button in the watchlist title bar (paralleling the blotter's).
3. One call to `column_selector_popup` in `view_watchlist_body`'s stack layer.

Call this out in the PR description as a follow-up. **Do not ship it here.**

## Dependencies on other plans

- **`feature-row-selection.md`** — **lands first.** This plan lands *after*
  row-selection so Slice 4's migration embeds the final 3-arg
  `OrderBlotterRowSelected` message shape and passes
  `panel.selected_row == Some(r.order_id)` straight into `grid_body_row`'s
  `selected: bool` param. No later API churn.
- **`feature-popup-clickable.md`** — **superseded by Slice 1 of this plan.**
  The cross-plan eval recommends skipping popup-clickable entirely: Slice 1
  delivers the fixed popup layering (helper returns a single container
  meant to be stacked *above* the backdrop). If popup-clickable ships first,
  its fix is simply replaced by the helper in Slice 4; if this plan lands
  first, popup-clickable should not be executed at all.
- **`feature-header-settings-button.md`** — no API coupling, but both plans
  touch adjacent blotter code. Either order works; see the Slice 4
  gear-cell instruction above for how the call-site migration handles both
  possibilities.

## Build order (5 vertical slices)

Each slice is compile-green, tests pass, commit-shaped.

**Parallelization.** Slices 1, 2, 3 can run in parallel — they are independent
leaf helpers. Slices 4 and 5 run in parallel after 1/2/3 land. The shared
workspace is `midas-grid/src/helpers/` (new module directory); parallel agents
writing separate files (`popup.rs`, `header_cell.rs`, `body_row.rs`) avoid
collisions.

**Dead-code note (all of slices 1–3).** These slices add public helpers with
no in-tree consumers until slices 4–5. Public items generally escape
`dead_code`, but `ResizeHandle` fields and any private helpers get
`#[allow(dead_code)]` to stay clippy-clean under `-D warnings`. Unit tests in
each slice exercise the public surface to keep it alive.

### Slice 1 — `column_selector_popup` + `ColumnEntry` + `ALT_ROW_BG` (~2 h)
- Add `helpers/` module directory under `midas-grid/src/`, with
  `helpers/mod.rs` re-exported from `lib.rs`.
- Implement `column_selector_popup` in `helpers/popup.rs`. Introduce
  `ColumnEntry { id, label, mandatory }`.
- Add `ALT_ROW_BG` const to `midas-grid::style` (not used until slice 3).
  **Do not** add a new `SELECTED_ROW_BG` const — `GridStyle::selected_bg`
  (already in `style.rs`) is the single source of truth.
- Unit tests covering: no-hidden-columns, all-hidden, mandatory-only, and
  popup-not-open states. Smoke-test the returned element via
  `iced::Element::as_widget()` layout — the iced widget tree is mostly opaque
  at test time, so a build-and-drop test is the realistic bar for lifetime
  regressions.
- **Rollback blast radius.** A helper bug shipped in Slice 1 affects **both**
  panels once Slices 4 AND 5 land — two-panel regression, not one. The four
  unit tests above are the safety net; run them before tagging the slice.
- Call sites: untouched.

### Slice 2 — `grid_header_cell` + `HeaderStyle` + `ResizeHandle` (~2 h)
- Add to `midas-grid::helpers::header_cell`.
- `ResizeHandle` lives in `helpers/mod.rs` (shared value type).
- Unit test: build a sortable header cell with a resize handle, assert it
  builds without panicking. Build a non-sortable empty-label cell (watchlist
  COL_DELETE case), assert same. Build once with `HeaderStyle::default()` and
  once with the watchlist style for coverage.
- Call sites: untouched.

### Slice 3 — `grid_body_row` (~1.5 h)
- Add to `midas-grid::helpers::body_row`.
- Unit test: `selected=true`, `alt_bg=false` → uses
  `GridStyle::default().selected_bg`. `selected=false, alt_bg=true` → uses
  `ALT_ROW_BG`. `on_click=None` → no `mouse_area` wrap. Verify structurally;
  a non-panic smoke test is the realistic bar.
- Call sites: untouched.

### Slice 4 — Migrate `view_order_blotter_body` (~3 h)
- Replace header loop with `grid_header_cell` + `HeaderStyle::default()`.
- Replace body loop with `grid_body_row`.
- Replace popup layer with `column_selector_popup` (build `Vec<ColumnEntry>`
  from `ALL_COL_DEFS`).
- Handle the gear cell per the coordination note in "Call-site migration".
- Run the full app; visually verify: header borders, sort arrows, resize
  drag, hidden-column filter, column-selector popup open/close/toggle, row
  selection background, alternating row bg.
- `cargo test --workspace` green.
- `cargo clippy --workspace -- -D warnings` green.
- LOC: views.rs should lose ~150 lines net on this slice alone.

### Slice 5 — Migrate `view_watchlist_body` (~2.5 h)
- Replace header loop with `grid_header_cell` + watchlist `HeaderStyle`.
- Replace body loop with `grid_body_row`. Keep the `grid_data_cell` padding
  wrapper — pass the already-wrapped elements into the helper as the
  `cells` vector.
- Visually verify: ticker drag-handle click still starts drag, favourite
  toggle still works, delete button still works, sort arrows, resize drag,
  selection highlight, link picker overlay (unchanged).
- `cargo test --workspace` green.
- `cargo clippy --workspace -- -D warnings` green.
- LOC: views.rs should lose ~100 lines net on this slice.

**Total budget: ~11 hours.**

## Risks

- **Lifetime escapes.** Helpers take `&str` labels and `Vec<Element<'a, M>>`
  cells so `'a` flows through naturally; `on_toggle` is `+ 'a` bounded so it
  can borrow app state. If any call-site hits E0597/E0495, fall back to
  hand-built at that site — don't fight the borrow checker for hours.
- **Event ordering regressions.** `mouse_area` nesting order controls event
  capture. Slice 4/5 verify manually: header click-to-sort, resize drag on
  the 4-px strip, row click through the selection wrapper. Fix is `stack!`
  z-order or swapping `on_press` ↔ `on_release`.
- **Config format drift.** `column_widths` serde shape unchanged. Legacy
  `COL_DRAG` entries in saved watchlist configs stay readable; the helper
  just never renders a cell for them. Reload both configs after slice 5.
- **Style param sprawl.** Addressed by `HeaderStyle` (slice 2). If a third
  consumer wants a fourth knob, add a field to `HeaderStyle` — do not add
  positional params.
- **Popup-click layering.** Baked into the helper from slice 1, independent
  of the superseded `feature-popup-clickable.md`. Backdrop is the caller's
  concern; the helper returns only the popup container.
