# Feature: Settings UI

## Overview

A modal/dockable Settings panel exposing user-tweakable preferences that
today are either hardcoded constants, theme defaults, or layout defaults.
This document is a **seed list** — items get added here whenever a knob
is identified during feature work but doesn't justify shipping its own
preference UI immediately. When the list is large enough to warrant the
panel, this becomes the implementation plan.

Settings persist via the existing TOML `AppConfig` (`midas-core/src/config/`)
under a new `[settings]` section. Defaults stay as the current hardcoded
values so existing configs continue to work without migration.

## Candidate settings

### Watchlist

- **Chart thumbnail overflow behavior** — when the watchlist pane is
  narrower than the sum of its column widths, the Chart cell either:
  - **Squeeze** *(current default)*: the sparkline is compressed
    horizontally to fit the visible portion of the cell. The full
    mountain shape is preserved; X-axis is just denser.
  - **Clip**: the sparkline renders at its natural full width and the
    overflowing portion is cropped at the pane edge.
  - Implementation cost: ~one line in `thumbnail_widget::render`.
    Squeeze = `set_viewport(clip_bounds)` (current). Clip = capture
    the original widget bounds in `prepare()`, set viewport to those,
    set scissor to `clip_bounds`.
  - Both behaviors require `midas_ui::clip_layer` wrapping the
    watchlist body so a renderer layer with the pane bounds exists —
    that part is unconditional and already in place.
  - Why it's a setting, not a fixed default: which one looks better
    depends on how narrow the user makes the column. Squeeze is
    elegant at small widths; clip preserves the "real" sparkline at
    medium widths. No clearly-better answer.

- **Sparkline default timeframe** — currently driven by per-symbol
  `ThumbnailIntervalCycle`. A global default for first-time view of a
  newly added ticker would help.

- **Show/hide Chart column entirely** — for users who only want
  numeric watchlist columns.

### Account panel

- **Default tab on first open** *(currently Orders)*. Could be
  Positions for users who lead with their open exposure.
- **Disconnect-banner auto-dismiss timeout** — banner currently stays
  forever until user clicks ×. Some users want it to fade after N
  seconds.

### Charts

- **Vertical-line density preference** — the auto-spacing rules in
  `midas-chart::compute::build_grid_instances` already filter overly-
  dense session boundaries. Some users may want a manual cap (max
  vertical lines per 1000px) or a stricter "labeled-lines-only" mode.
- **Top-of-chart border** — the divider added in the recent vlines
  arc; could be toggleable per user.
- **GATR baseline visibility** — show/hide the gray reference line.

### General

- **Panel padding density** — compact / normal / comfortable.
- **Font size scale** — global multiplier (reuses `UiTheme::*_font_size`).
- **Tab strip thickness / underline color** — currently white 3px,
  recently bumped from blue 2px. Some users may want it back.

## Out of scope (not settings)

- Per-symbol annotations / brackets — already first-class state.
- Workspace layout — pane geometry persists automatically via iced
  `pane_grid` config and doesn't need a settings UI.
- Theme palette — out of scope until a second theme exists.

## When to actually build

Build the panel when **either**:
1. The candidate list above passes ~6 items the user actively wants
   exposed (right now: 1–2 strong, rest speculative); or
2. A specific user-visible knob ships AND can't be left as a
   compile-time default (e.g. compliance / accessibility need).

Until then, when a new knob comes up, append it here and ship the
default behavior in code.
