# Order-Blotter Column Selector — Clickable Rows Fix

> **Status: SUPERSEDED** — cross-plan evaluation concluded this bug is fixed by
> `feature-generic-grid.md` Slice 1, which ships the `column_selector_popup`
> helper with the fix baked in. Executing this plan separately would be
> wasted work: its inline edit at `views.rs:~2687–2697` is deleted in
> generic-grid's Slice 4 migration. Retained as a diagnosis record.
>
> If `feature-generic-grid.md` is deferred beyond a week, revisit this plan.

## Problem

The order-blotter column-selector popup in `midas-app/src/app/views.rs` (~L2665) renders
a checklist of columns, but clicking a row fails to toggle the column. The popup simply
dismisses. Each row is built as `mouse_area(container(row![...])).on_release(ToggleColumn)`,
sitting on top of a full-viewport backdrop `mouse_area(...).on_press(DismissColumnSelector)`.

The link-picker in the same file (`build_link_picker`, L1053) uses iced `button` widgets and
works correctly, which is a strong signal for the fix shape.

## Root Cause

Two things conspire — but (2) is the decisive one:

1. **Backdrop consumes mouse-down first.** The backdrop layer is pushed into the `stack`
   *before* the popup container, but iced's stack event routing delivers press events
   top-down relative to layer order and z-order. The popup layer (pushed last) renders on
   top, yet its inner rows use `on_release` only. When the user presses inside a row, the
   `mouse_area` wrapping the row has no `on_press` handler, so it does not capture/consume
   the press. The press propagates down to the backdrop (which is a full-viewport
   `Space` beneath) and fires `DismissColumnSelector`. That closes the popup before the
   release can resolve on the row.

2. **iced 0.14 `mouse_area::on_release` requires the press to have been captured by the
   same widget.** Without `on_press` set, the release never binds to the row. Even if the
   backdrop weren't there, rows would be unreliable.

Verification path: the link-picker uses `button(...)` per row. `button` captures press
natively (consuming the event so it never reaches a layer below) and emits `on_press`
cleanly. That is exactly why the link-picker works and this popup does not.

## Fix

Replace each non-mandatory row in the column-selector with an iced `button`, matching the
link-picker pattern.

**File:** `desktop/win/crates/midas-app/src/app/views.rs` — lines ~2677–2697 (the `else`
branch of `is_mandatory`).

Change:

```rust
mouse_area(
    container(
        row![text(check_mark).size(14), text(label).size(12)]
            .spacing(6)
            .align_y(iced::Alignment::Center),
    )
    .padding([4, 8]),
)
.on_release(Message::OrderBlotterToggleColumn(blotter_id, col_id))
.into()
```

to:

```rust
button(
    row![text(check_mark).size(14), text(label).size(12)]
        .spacing(6)
        .align_y(iced::Alignment::Center),
)
.on_press(Message::OrderBlotterToggleColumn(blotter_id, col_id))
.padding([4, 8])
.width(Fill)
.style(button::text)
.into()
```

`button` consumes the press event, so the backdrop beneath never sees it — the popup stays
open and the toggle message fires. `width(Fill)` ensures the entire row is a hit target,
matching the visual affordance. `button::text` keeps the row chromeless so the checkmark
and label look identical to today.

No changes to message plumbing, no changes to the backdrop, no changes to the mandatory
Symbol row (still a plain non-interactive `row![...]`).

## Verification

Run `cargo run -p midas-app --features dev_harness`, open an order blotter via the
devloop fixtures, click the column-selector gear to open the popup, then click any
non-Symbol row: the checkmark glyph flips (`☑`↔`☐`) and the popup remains open. Clicking
outside the popup still dismisses it. Max validates visually once deployed.

## Non-goals

- No redesign of the popup layout, styling, or the checklist widget structure.
- No generic popup abstraction — `feature-generic-grid.md` owns that.
- No change to backdrop dismissal semantics (`on_press` on the backdrop stays).
- No change to `Message::OrderBlotterToggleColumn` or its handler.
- No change to the link-picker or any other popup site in the file.
