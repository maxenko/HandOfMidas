# Slice 9 — EH toggle UI

**Goal.** Per-chart toggle for extended-hours display. Two surfaces: bottom-right chip on the chart, plus settings-menu checkbox. State persisted via `ChartViewStore`.

## Scope

### `ChartPanel` state

```rust
pub struct ChartPanel {
    // ...existing fields...
    pub show_extended_hours: bool,    // NEW; default true on intraday, ignored on D1+
}
```

### `ChartViewStore` persistence

`ChartViewStore` already persists per-(symbol, timeframe) records. Add field:

```rust
pub struct ChartViewRecord {
    // ...existing...
    pub show_extended_hours: Option<bool>,
}
```

Serde `#[serde(default)]` makes legacy records `None` → defaults applied at load.

### Bottom-right chip

`desktop/win/crates/midas-app/src/app/views.rs` or the chart widget renderer — add a small "EH" / "RTH" chip button next to the existing timeframe chip.

```rust
// Pseudo-code — depends on chart-chrome renderer.
if chart.show_extended_hours {
    button("EH").tinted_blue().on_click(Message::ToggleEh(chart_id))
} else {
    button("RTH").tinted_gray().on_click(Message::ToggleEh(chart_id))
}
```

### Settings menu

Chart settings → "Session" section → checkbox "Show extended-hours trading". Writes `Message::ToggleEh(chart_id)`.

### Message + handler

```rust
// Messages enum addition
Message::ToggleEh(ChartId),

// handlers.rs
Message::ToggleEh(chart_id) => {
    if let Some(chart) = self.charts.get_mut(&chart_id) {
        chart.show_extended_hours = !chart.show_extended_hours;
        chart.chart_state.dirty.mark_all();
        // Persist to ChartViewStore.
        self.chart_view_store.update(&chart.symbol, chart.timeframe, |rec| {
            rec.show_extended_hours = Some(chart.show_extended_hours);
        });
    }
    Task::none()
}
```

### Wire to `ChartInput`

When the chart widget builds its `ChartInput` for `compute_chart_scene`, it passes `chart.show_extended_hours` through.

## Files touched

- `desktop/win/crates/midas-app/src/app.rs` — ChartPanel field, MidasApp::new default.
- `desktop/win/crates/midas-app/src/app/handlers.rs` — ToggleEh handler, ChartInput wiring.
- `desktop/win/crates/midas-app/src/app/views.rs` — chip button rendering.
- `desktop/win/crates/midas-app/src/chart_view.rs` — ChartViewRecord field.
- `desktop/win/crates/midas-app/src/app/persistence.rs` — serde_default.

## Tests

- `toggle_eh_changes_chart_input`: create chart, send `Message::ToggleEh`, assert `chart.show_extended_hours` flips and the next `view()` pass produces `ChartInput::show_extended_hours` matching.
- `eh_persists_across_chart_reopen`: toggle EH, close chart, reopen same (symbol, tf); assert state restored.
- `default_eh_true_for_intraday`: new chart on M1 → `show_extended_hours = true`.
- `default_eh_preserved_on_d1`: default value preserved but effectively inert for D1.

## Acceptance

- Tests pass.
- Clippy / fmt clean.
- Manual: run app, open AAPL M1 chart, EH chip toggles; bands + session tints appear/disappear accordingly.

## Commit

Single commit: `feat(app): per-chart extended-hours toggle UI`.
