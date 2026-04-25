# Slice 4 — Gear `⋮` Icon + Settings Popup Panel + Toolbar Mode Indicator

**Goal:** Ship the toolbar `⋮` glyph next to the existing `VP` button, plus a popup panel with anchor-mode radio + width slider + the two informational notes (collapse_gaps fallback / anchor-too-fine-for-tf). Also: change the `VP` button label to `VP·D / W / M / Y` when anchor ≠ Viewport (mode indicator), with the suffix dropped when a fallback is active (D13).

**Slice split:**
- **S4a** — state + handlers + Recon-cleanup. Mergeable after S1 alone. Done-when: state-only (handler unit tests, DumpState assertions, no popup screenshot).
- **S4b** — popup render + toolbar gear button + screenshot Done-when. Requires S2 OR S3 to be merged so a real chart is on screen behind the popup.

**Depends on:**
- **S4a:** S1 only (mergeable).
- **S4b:** S1 + (S2 OR S3) for visible verification.

## Files to modify (S4a — state + handlers, mergeable after S1)

- `desktop/win/crates/midas-app/src/app.rs`:
  1. Add `pub vp_settings_open: Option<ChartId>` field on `MidasApp`. Mirror placement of `link_picker_open` (line 418 area).
  2. Add Message variants. **Per-knob split** (NOT one omnibus `UpdateVpSettings(ChartId, VolumeProfileSettings)`) — three reasons: (a) `Message` size budget (`app.rs:1141-1144` const-assert ≤ 256 bytes); v2 will grow `VolumeProfileSettings` with value-area styling; (b) iced slider emits a Message per pixel of mouse movement — sending the whole struct on every tick wastes the queue; (c) per-knob mirrors the popup's row-by-row interaction:
     ```rust
     ToggleVpSettingsPanel(ChartId),
     DismissVpSettingsPanel,
     UpdateVpAnchor(ChartId, VolumeProfileAnchor),
     UpdateVpWidthFraction(ChartId, f32),
     // S6 P4 will add: UpdateVpShowValueArea(ChartId, bool), UpdateVpValueAreaPct(ChartId, f32)
     ```
  3. In whichever pane-close handler exists (Recon R7 resolves), reset `vp_settings_open` if it pointed at the closed chart. Mirror the link-picker cleanup pattern.
- `desktop/win/crates/midas-app/src/app/handlers.rs` — add `handle_vp_settings_msg(...)` block:
  - `ToggleVpSettingsPanel(id)`: `vp_settings_open = if vp_settings_open == Some(id) { None } else { Some(id) }`.
  - `DismissVpSettingsPanel`: `vp_settings_open = None`.
  - `UpdateVpAnchor(id, anchor)`: `chart.chart_state.volume_profile.anchor = anchor;` then `chart.chart_state.volume_profile = chart.chart_state.volume_profile.sanitized();` then `dirty.mark_data()` + `mark_config_dirty()` + `tracing::info!(target: "vp", chart=?id, ?anchor, "VP anchor changed")`.
  - `UpdateVpWidthFraction(id, frac)`: same pattern, mutates `width_fraction`. Tracing at `info` for non-slider events; for slider drag, gate the tracing to once per second to avoid log spam.
  - **Also:** when `Message::ToggleVolumeProfile(id)` flips `show_volume_profile` to `false`, also `vp_settings_open = vp_settings_open.filter(|&open| open != id);` (popup auto-dismisses if VP is turned off).

## Files to modify (S4b — popup render + toolbar gear, requires S2 OR S3)

- `desktop/win/crates/midas-app/src/app/views.rs` — three edits (toolbar relabel, gear button, chart-area stack popup; details below).
- `desktop/win/crates/midas-app/src/app/views.rs` — new function `build_vp_settings_panel(...)` (details below).

### S4b views.rs edits
- `desktop/win/crates/midas-app/src/app/views.rs` — three edits:
  1. **Title bar around line 717-728** — change the existing `vp_btn` builder to switch the label based on `vm.volume_profile.anchor` AND whether a fallback is active (D11/D13):
     ```rust
     // Compute fallback flags from the same predicates render uses.
     let fallback_active = vm.collapse_gaps                                // D11
         || timeframe_blocks_anchor(&vm.timeframe, vm.volume_profile.anchor); // D12

     let vp_label = if fallback_active {
         "VP"   // suffix dropped while fallback active — D13 consistency
     } else {
         match vm.volume_profile.anchor {
             VolumeProfileAnchor::Viewport | VolumeProfileAnchor::Unknown => "VP",
             VolumeProfileAnchor::Daily   => "VP·D",
             VolumeProfileAnchor::Weekly  => "VP·W",
             VolumeProfileAnchor::Monthly => "VP·M",
             VolumeProfileAnchor::Yearly  => "VP·Y",
         }
     };
     // ... text(vp_label).size(10) ...
     ```
     The popup itself remains the source of truth for what was selected (radio shows `Daily`); the toolbar label reflects what's effectively rendered. Italic note inside the popup explains the discrepancy.
  2. **Insert `vp_settings_btn` immediately after `vp_btn`** in the `row![...]` at line 764-772:
     ```rust
     let vp_settings_open = self.vp_settings_open == Some(chart_id);
     let vp_settings_btn = if vp_settings_open {
         button(text("⋮").size(12).color(Color::WHITE))
             .on_press(Message::ToggleVpSettingsPanel(chart_id))
             .padding([1, 4])
             .style(button::primary)
     } else {
         button(text("⋮").size(12))
             .on_press(Message::ToggleVpSettingsPanel(chart_id))
             .padding([1, 4])
             .style(button::text)
     };
     row![ticker_input, tf_row, collapse_btn, vp_btn, vp_settings_btn,
          levels_btn, reset_btn, backend_btn]
         .spacing(4).align_y(iced::Alignment::Center).height(24)
     ```
  3. **Chart-area stack — mirror `views.rs:247-271` link-picker block.** When `self.vp_settings_open == Some(chart_id)`:
     ```rust
     chart_layers.push(
         iced::widget::mouse_area(Space::new().width(Fill).height(Fill))
             .on_press(Message::DismissVpSettingsPanel)
             .into(),
     );
     let panel = self.build_vp_settings_panel(chart_id, &chart.chart_state);
     chart_layers.push(
         container(panel)
             .align_x(iced::alignment::Horizontal::Right)
             .align_y(iced::alignment::Vertical::Top)
             .padding([28, 4])    // tuned so it sits just under the title bar gear
             .width(Fill).height(Fill)
             .into(),
     );
     ```
- `desktop/win/crates/midas-app/src/app/views.rs` — new function `build_vp_settings_panel(&self, chart_id: ChartId, state: &ChartState) -> Element<Message>`. See "Panel layout" below.

## Files to create

None (panel builder lives next to `build_link_picker` in `views.rs`).

## Key implementation details

### Glyph

`⋮` (U+22EE) at `text(...).size(12)`, NOT `⚙` (U+2699). Per `plan/feature-header-settings-button.md`: `⚙` renders inconsistently at small sizes on Windows 11 in this iced-loaded font.

### Mandatory `button` rows inside the panel

Every clickable element inside `build_vp_settings_panel` MUST be `iced::widget::button(...)`. NEVER `mouse_area().on_release(...)`. Per `plan/feature-popup-clickable.md` — backdrop steals the press otherwise. The link-picker (`views.rs:1076-1192`) is the template.

### Popup stays open until explicit dismiss (NOT auto-close on selection)

Mirrors the column-selector behaviour, not the link-picker. Users tweak multiple knobs in one session (anchor + width). The only auto-close path is: VP toggle flipped to off (handled in `Message::ToggleVolumeProfile` per above).

### Panel layout

```
┌────────────────────────────────┐
│  Volume Profile                │  <- header text size(12) bold
├────────────────────────────────┤
│  Anchor                        │  <- section label size(10) muted
│  [● Viewport]                  │  <- 5 button rows; selected = button::primary
│  [○ Daily   ]                  │     others = button::text
│  [○ Weekly  ]                  │
│  [○ Monthly ]                  │
│  [○ Yearly  ]                  │
├────────────────────────────────┤
│  Width  [-] 70% [+]            │  <- chip pair if iced 0.14 slider clashes;
│                                │     ship slider first per Open Q 4
├────────────────────────────────┤
│  ⓘ Collapse gaps is on; per-   │  <- italic note, only when collapse_gaps==true
│    period anchors disabled.    │     AND anchor != Viewport
│  ⓘ Anchor too fine for current │  <- italic note, only when timeframe blocks
│    timeframe.                  │     anchor (D12)
└────────────────────────────────┘
```

Container: `container(column.spacing(2)).padding(8).style(panel_container_style)`. Use the same `panel_container_style` the link-picker uses (verify exact name in `views.rs:1076-1192`).

### Anchor row implementation (per-knob — emits `UpdateVpAnchor`)

```rust
fn anchor_row(label: &str, value: VolumeProfileAnchor, current: VolumeProfileAnchor,
              chart_id: ChartId)
    -> Element<'_, Message>
{
    let bullet = if current == value { "●" } else { "○" };
    let style = if current == value { button::primary } else { button::text };
    button(text(format!("{bullet} {label}")).size(12))
        .width(Fill)
        .padding([4, 8])
        .style(style)
        .on_press(Message::UpdateVpAnchor(chart_id, value))
        .into()
}
```

The handler in S4a clones the existing settings, sets `anchor = value`, calls `sanitized()`, and persists. The popup row only sends the per-knob delta — no struct-clone in the view code.

### Width slider / chip pair (per-knob — emits `UpdateVpWidthFraction`)

Try iced 0.14 `slider` first:
```rust
slider(0.05..=1.0, current_settings.width_fraction,
       move |v| Message::UpdateVpWidthFraction(chart_id, v))
.step(0.05)
```
If the dark-theme styling looks bad in dev-loop screenshot, fall back to:
```rust
row![
    button(text("-").size(12)).on_press(...with width_fraction-0.05),
    text(format!("{:.0}%", current_settings.width_fraction * 100.0)).size(12),
    button(text("+").size(12)).on_press(...with width_fraction+0.05),
]
```
Decision threshold: a single visual review during dev loop. Plan ships slider first.

### Informational notes (NOT italic — codebase has zero italic precedent)

Only render when conditions are met. **Use plain `text` with `text::tertiary` styling and a leading `ⓘ` glyph** — the codebase has zero `font::Style::Italic` usage (verified via Recon). The font may not even have an italic variant.

```rust
fn collapse_gaps_note(state: &ChartState) -> Option<Element<'_, Message>> {
    if state.collapse_gaps && !matches!(state.volume_profile.anchor,
            VolumeProfileAnchor::Viewport | VolumeProfileAnchor::Unknown) {
        Some(text("ⓘ Gap-collapse is on; per-period anchors disabled.")
            .size(10).style(text::tertiary).into())
    } else {
        None
    }
}
```

### Pane-close cleanup

Find the pane-close handler (`grep "ClosePane\|on_pane_close\|remove_pane" desktop/win/crates/midas-app/src/app/`). Add:
```rust
if self.vp_settings_open.map(|id| id == closed_chart_id).unwrap_or(false) {
    self.vp_settings_open = None;
}
```

### Reset Chart preservation

`Message::ResetChart` does NOT touch `volume_profile` settings. Verify by reading the `ResetChart` handler today; if it only resets camera/zoom (existing behaviour for `show_volume_profile`), no edit needed. **Add a unit test asserting `chart_state.volume_profile` is unchanged across `ResetChart`.**

## Testing

### Handler unit tests (`desktop/win/crates/midas-app/src/app/handlers.rs::tests` or sibling test module)

1. **`toggle_panel_opens_and_closes_for_same_chart`** — `vp_settings_open == None`. Send `ToggleVpSettingsPanel(0)` → `Some(0)`. Send again → `None`.
2. **`toggle_panel_switches_between_charts`** — open for chart 0, then send `ToggleVpSettingsPanel(1)` → `vp_settings_open == Some(1)`. (One panel at a time.)
3. **`update_anchor_mutates_state_and_marks_dirty`** — send `UpdateVpAnchor(0, Daily)`. Assert: `chart.chart_state.volume_profile.anchor == Daily`, `mark_config_dirty` was called, `dirty.mark_data` was called.
3a. **`update_width_fraction_mutates_and_marks_dirty`** — send `UpdateVpWidthFraction(0, 0.65)`. Assert width_fraction stored, dirty/config-dirty called.
4. **`update_clamps_via_sanitized`** — send `UpdateVpWidthFraction(0, 5.0)`. Assert state has `width_fraction == 1.0` (clamp on write).
5. **`vp_off_dismisses_panel`** — open the panel for chart 0. Send `ToggleVolumeProfile(0)` (which flips `show_volume_profile` to false). Assert `vp_settings_open == None`.
6. **`pane_close_clears_vp_panel`** — open the panel for chart 0. Send the pane-close message for chart 0. Assert `vp_settings_open == None`. (Use whatever the pane-close test pattern is — search for existing tests of pane lifecycle.)
7. **`reset_chart_preserves_vp_settings`** — set `volume_profile.anchor = Daily`, send `Message::ResetChart(0)`. Assert `chart_state.volume_profile.anchor == Daily`.

### Devloop visual

`desktop/win/tools/devloop-vp-popup.sh`:
```bash
cargo run -p midas-app --features dev_harness -- \
    --fixture vp_daily_aapl_5m_3days &
APP=$!
sleep 2
midas-devloop-cli Click --target vp_settings_button --chart-id 0
midas-devloop-cli WaitForIdle --timeout-ms 200
midas-devloop-cli Screenshot --out vp-popup-open.png \
    --diff-ref tests/data/refs/vp-popup-open.png --min-ssim 0.98
midas-devloop-cli Click --x 1 --y 1   # outside popup
midas-devloop-cli WaitForIdle --timeout-ms 200
midas-devloop-cli Screenshot --out vp-popup-closed.png \
    --diff-ref tests/data/refs/vp-popup-closed.png --min-ssim 0.99
kill $APP
```

(Devloop `Click --target` may not exist; fall back to `Click --x 950 --y 30` with a fixed window size set by the fixture. See Slice 5 for window-size determinism.)

### Manual checklist

- Click gear → popup opens at top-right.
- Click outside → popup dismisses.
- Click an anchor row → popup STAYS OPEN; chart updates; restart app → setting persisted.
- Toggle VP off via `VP` button → popup dismisses.
- Open popup, close pane → popup state cleared (no orphan).
- Move width slider → chart updates live (after 2-second config-save debounce, persists).
- Toggle Anchor=Daily on a 1D chart → italic "Anchor too fine" note appears, chart still shows Viewport profile.
- Toggle Anchor=Daily with Gap-collapse on → italic "Gap-collapse is on" note appears, chart still shows Viewport profile.
- VP button label switches `VP → VP·D` when Daily selected.

## Done when

### S4a (state + handlers, mergeable after S1)
- Handler unit tests 1-7 pass.
- `DumpState` shows `vp_settings_open == Some(0)` after `ToggleVpSettingsPanel(0)` and `None` after `Dismiss`.
- `cargo clippy --workspace -- -D warnings` clean (no popup render code yet).
- Pane close clears `vp_settings_open` (test #6).

### S4b (popup render + toolbar gear, requires S2 OR S3)
- Devloop popup screenshot matches reference SSIM ≥ 0.98.
- Manual checklist complete (chart actually updates on anchor change).
- VP toolbar label correctly indicates current mode AND drops the suffix when a fallback is active (D13).
- Informational notes appear under the right conditions and disappear otherwise.
- Italic styling NOT used anywhere (verified — uses plain `text` + `ⓘ` glyph + `text::tertiary`).

## Risks

- **iced 0.14 slider styling** — if it clashes with dark theme, fall back to `[-] N% [+]` chip pair (see Width slider section). Decision in dev-loop visual review.
- **Z-order of popup vs decorators** — if popup ends up under annotations/brackets, push the popup container later in the chart-area stack. Verify with a manual test.
- **Multi-pane gear collisions** — `vp_settings_open: Option<ChartId>` (not `bool`) ensures only one popup is open at a time; opening in pane B closes pane A's. Test #2 covers this.
- **`panel_container_style`** — verify exact name in `views.rs:1076-1192`. If it's a one-off closure, factor it to a shared helper to keep visual consistency.
- **Italic font availability** — verify the iced font load supports italic style at size 10. If not, use plain text with leading `ⓘ` glyph.
- **`Message` size budget** — per-knob split (`UpdateVpAnchor` carries `(ChartId, Anchor)` = ~9 bytes; `UpdateVpWidthFraction` carries `(ChartId, f32)` = 12 bytes incl. discriminant) keeps payloads small. Recon item R11 reads the current `static_assertions` budget and confirms post-S4a fits. If S6 P4 ever wants to send the full `VolumeProfileSettings` struct (e.g., a bulk reset), use `Box<VolumeProfileSettings>` then.
