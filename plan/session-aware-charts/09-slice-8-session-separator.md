# Slice 8 — RTH-close vertical separator

**Goal.** Draw a thin vertical line at the regular session close (16:00 ET for XNYS) to echo the TradingView convention. Reuses existing `detect_session_boundaries()` with a boundary-kind tag.

## Scope

### Boundary-kind tagging

`desktop/win/crates/midas-chart/src/compute/mod.rs::detect_session_boundaries()`:

```rust
pub struct SessionBoundary {
    pub x: f32,
    pub color: [f32; 4],
    pub kind: BoundaryKind,     // NEW
}

pub enum BoundaryKind {
    DayBreak,       // existing: generic gap > 1.5x candle duration
    RthClose,       // new: regular-session close (16:00 ET on XNYS)
}
```

### Classification

When walking candles to detect gaps:

```rust
let from_session = data.session(i - 1);
let to_session = data.session(i);

let kind = match (from_session, to_session) {
    (Regular, PostMarket) | (Regular, Closed) => BoundaryKind::RthClose,
    _ => BoundaryKind::DayBreak,
};
```

### Differentiated color

Existing `SESSION_BOUNDARY_COLOR` (`[0.3, 0.3, 0.5, 0.30]`) stays for DayBreak. Add `RTH_CLOSE_COLOR = [0.2, 0.4, 0.7, 0.50]` (bluer, slightly more opaque — the TradingView convention).

Both map to `GridLineInstance` via `build_grid_instances`.

## Files touched

- `desktop/win/crates/midas-chart/src/compute/mod.rs` — BoundaryKind, classification.
- `desktop/win/crates/midas-chart/src/style.rs` — RTH_CLOSE_COLOR constant.
- `desktop/win/crates/midas-chart/src/instances.rs` — if SessionBoundary struct lives there.

## Tests

- `rth_close_separator_detected`: CandleBuffer with Regular..Regular..Regular → PostMarket transition at 16:00 ET; assert `detect_session_boundaries` returns a boundary with `kind: RthClose`.
- `day_break_unchanged`: gap between last PostMarket and next PreMarket stays `DayBreak` (not `RthClose`).
- `separator_color_matches`: RthClose gets the RTH_CLOSE_COLOR; DayBreak gets SESSION_BOUNDARY_COLOR.

## Acceptance

- Tests pass.
- Clippy / fmt clean.
- Visual smoke: RTH close at 16:00 ET renders as a slightly more prominent vertical than other day breaks.

## Commit

Single commit: `feat(chart): distinguish RTH-close separator from generic day-break`.
