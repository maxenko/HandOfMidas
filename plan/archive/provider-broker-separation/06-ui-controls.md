# 06 -- UI Controls: Toolbar Dropdowns and Enhanced Status Bar

> Parent: [00-index.md](./00-index.md)
> Depends on: 04 (ProviderRegistry), 05 (App Integration)
> Implements: Phase 6 of the provider/broker separation plan

## Overview

This document specifies two UI changes that make provider/broker selection
visible and interactive:

1. **Toolbar** -- two `pick_list` dropdowns appended to the right side of the
   existing toolbar row, separated by a spacer from the layout/split/add buttons.
2. **Status bar** -- a connection indicator dot and the active provider name
   prepended to the left side of the existing status bar.

Both changes are pure view-layer modifications in
`midas-app/src/app/views.rs` plus two new `Message` variants in `app.rs`.
No new crates or dependencies are required -- iced 0.14 ships `pick_list`
in `iced::widget`.

---

## 1. Enhanced Toolbar Layout

### Before

```
[1] [1|1] [1/1] [2x2]  [Split H] [Split V]  [+] [Watchlist]
```

### After

```
[1] [1|1] [1/1] [2x2]  [Split H] [Split V]  [+] [Watchlist]  ── spacer ──  [Data: ▾] [Broker: ▾]
```

The left side (layout presets, split buttons, add chart, watchlist) is unchanged.
A `Space::new().width(Fill)` pushes the two dropdowns to the far right. The
dropdowns are sized to fit their content and separated by 8px spacing.

### Toolbar View Code

```rust
fn view_toolbar(&self) -> Element<'_, Message> {
    // ── Left side: existing buttons (unchanged) ──────────────────

    let layout_buttons = row![
        button(text("1").size(12))
            .on_press(Message::LayoutPreset(LayoutPresetKind::Single))
            .padding([4, 8])
            .style(hover_text_button_style),
        button(text("1|1").size(12))
            .on_press(Message::LayoutPreset(LayoutPresetKind::SplitH))
            .padding([4, 8])
            .style(hover_text_button_style),
        button(text("1/1").size(12))
            .on_press(Message::LayoutPreset(LayoutPresetKind::SplitV))
            .padding([4, 8])
            .style(hover_text_button_style),
        button(text("2x2").size(12))
            .on_press(Message::LayoutPreset(LayoutPresetKind::Grid2x2))
            .padding([4, 8])
            .style(hover_text_button_style),
    ]
    .spacing(2);

    let split_buttons = row![
        button(text("Split H").size(11))
            .on_press_maybe(
                self.workspace
                    .focus
                    .map(|p| Message::PaneSplit(pane_grid::Axis::Horizontal, p))
            )
            .padding([4, 6])
            .style(hover_text_button_style),
        button(text("Split V").size(11))
            .on_press_maybe(
                self.workspace
                    .focus
                    .map(|p| Message::PaneSplit(pane_grid::Axis::Vertical, p))
            )
            .padding([4, 6])
            .style(hover_text_button_style),
    ]
    .spacing(2);

    let add_btn = button(text("+").size(14))
        .on_press(Message::AddChart)
        .padding([4, 10])
        .style(hover_text_button_style);

    let wl_btn = button(text("Watchlist").size(12))
        .on_press(Message::AddWatchlist)
        .padding([4, 10])
        .style(hover_text_button_style);

    // ── Right side: provider dropdowns (new) ─────────────────────

    let data_label = text("Data:").size(11).color(theme::TEXT_SECONDARY);

    let data_options: Vec<String> = self.providers.data_provider_names();
    let data_selected = data_options.get(self.providers.active_data_idx)
        .cloned();
    let data_picker = pick_list(
        data_options,
        data_selected,
        |name| {
            // Find the index of the selected name.
            // The pick_list returns the value, not the index, so
            // the update handler maps name -> idx via the registry.
            Message::DataProviderSelected(name)
        },
    )
    .text_size(11)
    .padding([2, 6])
    .style(dark_pick_list_style);

    let broker_label = text("Broker:").size(11).color(theme::TEXT_SECONDARY);

    let broker_options: Vec<String> = self.providers.order_broker_names();
    let broker_selected = match self.providers.active_broker_idx {
        Some(idx) => broker_options.get(idx).cloned(),
        None => Some("None".to_string()),
    };
    let has_brokers = broker_options.len() > 1; // "None" is always present
    let broker_picker = pick_list(
        broker_options,
        broker_selected,
        |name| Message::OrderBrokerSelected(name),
    )
    .text_size(11)
    .padding([2, 6])
    .style(dark_pick_list_style);

    // ── Assemble ─────────────────────────────────────────────────

    let toolbar_row = row![
        layout_buttons,
        split_buttons,
        add_btn,
        wl_btn,
        Space::new().width(Fill),  // <-- pushes dropdowns right
        data_label,
        data_picker,
        Space::with_width(8),
        broker_label,
        broker_picker,
    ]
    .spacing(4)
    .padding(6)
    .align_y(iced::Alignment::Center);

    container(toolbar_row)
        .width(Fill)
        .style(|_theme| container::Style {
            background: Some(theme::TOOLBAR_BG.into()),
            ..Default::default()
        })
        .into()
}
```

---

## 2. Data Provider pick_list

### Options Source

```rust
// ProviderRegistry method:
pub fn data_provider_names(&self) -> Vec<String>
```

For v1 this returns `["Test Data"]` (caching is transparent, not a separate
entry). Future providers (IB, Polygon) register themselves and appear
automatically.

### Selected Value

The currently active provider name, looked up via:

```rust
self.providers.data_provider_names()[self.providers.active_data_idx]
```

### On Change

The pick_list produces a `String` (the selected name). The `Message` variant
carries the name rather than an index, because the index could change if
providers are registered/unregistered dynamically. The `update()` handler
resolves name to index:

```rust
Message::DataProviderSelected(name: String)
```

### Update Handler

```rust
Message::DataProviderSelected(name) => {
    if let Some(idx) = self.providers.find_data_provider_index(&name) {
        if idx != self.providers.active_data_idx {
            self.providers.active_data_idx = idx;
            self.status_message = format!("Switched to: {name}");
            self.mark_config_dirty();
            // Reload all charts from the new provider.
            return self.reload_all_charts();
        }
    }
    Task::none()
}
```

### Width

Auto (fits content). The pick_list's width is determined by the longest
option string. With `["Test Data"]` for v1, this is approximately 80px at
11pt font. No explicit width constraint is needed.

---

## 3. Broker pick_list

### Options Source

```rust
// ProviderRegistry method:
pub fn order_broker_names(&self) -> Vec<String>
```

Always starts with `"None"` as the first element. For v1, returns only
`["None"]`. When an IB broker is registered: `["None", "Interactive Brokers"]`.

### Selected Value

- `active_broker_idx == None` --> selected is `"None"`
- `active_broker_idx == Some(i)` --> selected is `broker_names[i]`

### On Change

```rust
Message::OrderBrokerSelected(name: String)
```

### Update Handler

```rust
Message::OrderBrokerSelected(name) => {
    if name == "None" {
        self.providers.active_broker_idx = None;
        self.status_message = "Broker disconnected".to_string();
    } else if let Some(idx) = self.providers.find_broker_index(&name) {
        self.providers.active_broker_idx = Some(idx);
        self.status_message = format!("Broker: {name}");
    }
    self.mark_config_dirty();
    Task::none()
}
```

### Disabled State

When only `"None"` is available (no brokers registered), the pick_list is
still rendered but is effectively a no-op -- selecting "None" when "None" is
already selected does nothing. A future enhancement could gray out the text,
but for v1 the single-option pick_list is sufficient visual indication.

---

## 4. Enhanced Status Bar

### Before

```
[status_message]                                [symbol | tf | pane_count | overlay | HH:MM:SS]
```

### After

```
[dot] [provider_name] | [status_message]          [symbol | tf | pane_count | overlay | HH:MM:SS]
```

The left side gains two new elements before the existing `status_message`:
1. A colored dot indicating connection state
2. The active data provider's display name

### Status Bar View Code

```rust
fn view_status_bar(&self) -> Element<'_, Message> {
    // ── Connection indicator ─────────────────────────────────────
    let (dot_char, dot_color) = self.connection_indicator();

    let connection_dot = text(dot_char)
        .size(12)
        .color(dot_color);

    let provider_name = text(self.providers.active_data_provider_name())
        .size(12)
        .color(theme::TEXT_SECONDARY);

    let separator = text(" | ")
        .size(12)
        .color(theme::TEXT_MUTED);

    // ── Right side: existing info (unchanged) ────────────────────
    let active_info = if let Some(id) = self.active_chart_id() {
        if let Some(chart) = self.charts.get(&id) {
            let sym = if chart.symbol.is_empty() {
                "---"
            } else {
                &chart.symbol
            };
            format!("{sym} | {}", chart.timeframe.display_name())
        } else {
            "---".to_string()
        }
    } else {
        "No chart".to_string()
    };
    let pane_count = self.workspace.pane_count();
    let overlay_indicator = if self.show_frame_overlay {
        " | F11: overlay ON"
    } else {
        ""
    };

    let status_row = row![
        connection_dot,
        Space::with_width(4),
        provider_name,
        separator,
        text(&self.status_message)
            .size(12)
            .color(theme::TEXT_SECONDARY),
        Space::new().width(Fill),
        text(format!(
            "{active_info} | {pane_count} pane(s){overlay_indicator} | {}",
            self.current_time
        ))
        .size(12)
        .color(theme::TEXT_MUTED),
    ]
    .padding([4, 8])
    .align_y(iced::Alignment::Center);

    container(status_row)
        .width(Fill)
        .style(|_theme| container::Style {
            background: Some(theme::STATUS_BAR_BG.into()),
            ..Default::default()
        })
        .into()
}
```

---

## 5. Connection Indicator

The connection dot represents the aggregate health of the active data provider
and active broker. Data providers report via `is_connected()` on the
`DataProvider` trait; brokers report via `connection_state()` on `OrderBroker`.

### Color Mapping

| State | Dot | Color | Hex | Constant |
|---|---|---|---|---|
| Connected / Ready | `\u{25CF}` (filled circle) | Green | `#26B368` | `theme::STATUS_OK` |
| Connecting / Reconnecting | `\u{25CF}` (filled circle) | Yellow | `#E6B30F` | `theme::STATUS_WARN` |
| Disconnected | `\u{25CB}` (hollow circle) | Gray | `#595960` | `theme::TEXT_MUTED` |

### Logic

```rust
impl MidasApp {
    /// Compute the connection indicator character and color.
    ///
    /// Priority: if the active broker is connecting/reconnecting, show yellow.
    /// Otherwise, show green if both data provider and broker (if any) are
    /// connected, gray if either is disconnected.
    fn connection_indicator(&self) -> (&'static str, Color) {
        let data_connected = self.providers
            .active_data_provider()
            .map_or(false, |p| p.is_connected());

        let broker_state = self.providers
            .active_broker()
            .map(|b| b.connection_state());

        match broker_state {
            // Broker is connecting or reconnecting -- yellow dot.
            Some(ConnectionState::Connecting)
            | Some(ConnectionState::Reconnecting { .. }) => {
                ("\u{25CF}", theme::STATUS_WARN)
            }
            // Broker is connected/ready and data is connected -- green.
            Some(ConnectionState::Connected { .. })
            | Some(ConnectionState::Ready)
                if data_connected =>
            {
                ("\u{25CF}", theme::STATUS_OK)
            }
            // No broker active, data is connected -- green.
            None if data_connected => {
                ("\u{25CF}", theme::STATUS_OK)
            }
            // Everything else -- gray/disconnected.
            _ => {
                ("\u{25CB}", theme::TEXT_MUTED)
            }
        }
    }
}
```

### Provider-Specific Behavior

- **TestProvider**: `is_connected()` always returns `true` (deterministic data
  needs no network). The dot is always green when TestProvider is active and
  no broker is configured.
- **CachingProvider**: Delegates `is_connected()` to its inner provider.
  If the inner provider is connected, the cache layer is transparent.
- **Future IbDataProvider**: `is_connected()` returns `true` only when the
  TWS Gateway connection is in `Connected` or `Ready` state.
- **OrderBroker (future)**: Reports `ConnectionState` directly, which the
  indicator maps to dot color as shown above.

---

## 6. pick_list Dark Theme Styling

iced 0.14's `pick_list` accepts a style closure
`Fn(&Theme, pick_list::Status) -> pick_list::Style`. The dark theme style
matches the toolbar aesthetic -- dark background, subtle border, light text.

### Style Function

```rust
/// Dark theme style for toolbar pick_list dropdowns.
///
/// Matches the toolbar's dark color scheme: dark background, subtle border,
/// white text. Hover and opened states brighten slightly for feedback.
fn dark_pick_list_style(
    _theme: &iced::Theme,
    status: pick_list::Status,
) -> pick_list::Style {
    let base = pick_list::Style {
        text_color: Color::from_rgb(0.88, 0.88, 0.92),    // TEXT_PRIMARY
        placeholder_color: Color::from_rgb(0.35, 0.35, 0.40), // TEXT_MUTED
        handle_color: Color::from_rgb(0.55, 0.55, 0.60),  // TEXT_SECONDARY
        background: iced::Background::Color(
            Color::from_rgb(0.16, 0.16, 0.20)              // BUTTON_BG
        ),
        border: iced::Border {
            color: Color::from_rgb(0.25, 0.25, 0.30),
            width: 1.0,
            radius: 3.0.into(),
        },
    };

    match status {
        pick_list::Status::Active => base,
        pick_list::Status::Hovered => pick_list::Style {
            background: iced::Background::Color(
                Color::from_rgb(0.22, 0.22, 0.28)          // BUTTON_HOVER_BG
            ),
            border: iced::Border {
                color: Color::from_rgb(0.30, 0.30, 0.38),
                ..base.border
            },
            ..base
        },
        pick_list::Status::Opened => pick_list::Style {
            background: iced::Background::Color(
                Color::from_rgb(0.20, 0.20, 0.25)
            ),
            border: iced::Border {
                color: Color::from_rgb(0.22, 0.55, 0.95),  // ACCENT
                width: 1.5,
                ..base.border
            },
            ..base
        },
    }
}
```

### Menu (Dropdown List) Styling

iced 0.14's pick_list also has a `.menu_style()` method for the dropdown
overlay. The menu style should be consistent:

```rust
/// Dark theme style for the pick_list dropdown menu overlay.
fn dark_pick_list_menu_style(_theme: &iced::Theme) -> pick_list::menu::Style {
    pick_list::menu::Style {
        background: iced::Background::Color(
            Color::from_rgb(0.14, 0.14, 0.18)
        ),
        text_color: Color::from_rgb(0.88, 0.88, 0.92),
        selected_text_color: Color::WHITE,
        selected_background: iced::Background::Color(
            Color::from_rgb(0.22, 0.55, 0.95)  // ACCENT
        ),
        border: iced::Border {
            color: Color::from_rgb(0.25, 0.25, 0.30),
            width: 1.0,
            radius: 3.0.into(),
        },
    }
}
```

The dropdown invocation becomes:

```rust
pick_list(options, selected, on_change)
    .text_size(11)
    .padding([2, 6])
    .style(dark_pick_list_style)
```

> **Note on menu_style:** iced 0.14's exact API for menu styling may require
> `.menu_style(dark_pick_list_menu_style)` or a combined style. Verify against
> the iced 0.14 `pick_list` source during implementation. The color values
> above are correct regardless of the exact API surface.

---

## 7. Message Variants

Two new variants are added to the `Message` enum in `app.rs`:

```rust
pub enum Message {
    // ... existing variants ...

    // -- Provider selection --
    /// User selected a data provider from the toolbar dropdown.
    /// Carries the provider's display name (resolved to index in update).
    DataProviderSelected(String),
    /// User selected an order broker from the toolbar dropdown.
    /// Carries the broker's display name (resolved to index in update).
    OrderBrokerSelected(String),
}
```

### Why String, Not usize

The pick_list widget returns the selected *value*, not its index. Using
`String` aligns with the pick_list API naturally. The `update()` handler
calls `registry.find_data_provider_index(&name)` to resolve the name to an
index. This also makes the message self-describing in debug logs.

### Reload Flow

When `DataProviderSelected` triggers and the provider actually changes:

1. `self.providers.active_data_idx = new_idx;`
2. All existing charts are reloaded from the new provider.
3. Reload is done by iterating `self.charts.keys()` and issuing async
   `get_candles` calls for each, funneling results back through the existing
   `Message::DataLoaded(chart_id, Result<...>)` variant.

```rust
/// Reload all charts from the currently active data provider.
///
/// Returns a batched Task that fires `DataLoaded` for each chart.
fn reload_all_charts(&mut self) -> Task<Message> {
    let chart_specs: Vec<(ChartId, String, Timeframe)> = self
        .charts
        .iter()
        .filter(|(_, panel)| !panel.symbol.is_empty())
        .map(|(id, panel)| (*id, panel.symbol.clone(), panel.timeframe))
        .collect();

    let provider = self.providers.active_data_provider();
    let Some(provider) = provider else {
        return Task::none();
    };

    let tasks: Vec<Task<Message>> = chart_specs
        .into_iter()
        .map(|(chart_id, symbol, tf)| {
            let provider = Arc::clone(&provider);
            let days = Self::days_for_timeframe(tf);
            Task::perform(
                async move {
                    let result = provider.get_candles(&symbol, tf, days).await;
                    (chart_id, result.map(Arc::new).map_err(|e| e.to_string()))
                },
                |(chart_id, result)| Message::DataLoaded(chart_id, result),
            )
        })
        .collect();

    Task::batch(tasks)
}
```

---

## 8. Files Modified

| File | Change |
|---|---|
| `midas-app/src/app.rs` | Add `DataProviderSelected(String)`, `OrderBrokerSelected(String)` to `Message`. Add `reload_all_charts()`. Add update handlers. |
| `midas-app/src/app/views.rs` | Modify `view_toolbar()` to add spacer + two pick_lists. Modify `view_status_bar()` to add connection dot + provider name. Add `connection_indicator()` method. Add `dark_pick_list_style()` function. |
| `midas-app/src/theme.rs` | No changes needed -- existing constants (`STATUS_OK`, `STATUS_WARN`, `TEXT_MUTED`) cover all dot colors. |

---

## 9. Accessibility Notes

- The pick_list dropdowns are keyboard-navigable by default in iced 0.14
  (arrow keys to browse, Enter to select, Escape to close).
- The connection dot uses both color *and* shape (filled vs hollow) to
  distinguish connected from disconnected, supporting color-blind users.
- Provider names are descriptive strings, not abbreviations.

---

## 10. Visual Reference

```
+===========================================================================+
| [1] [1|1] [1/1] [2x2]  [Split H] [Split V] [+] [WL]     Data:[Test Data v] Broker:[None v] |
+===========================================================================+
|                                                                           |
|                          Chart content area                               |
|                                                                           |
+===========================================================================+
| ● Test Data | AAPL: 2500 candles at 1d                  AAPL | 1d | 4 pane(s) | 14:32:07 |
+===========================================================================+
```

When caching is active (transparent -- same provider name shown):

```
| ● Test Data | AAPL: 2500 candles at 1d                  AAPL | 1d | 4 pane(s) | 14:32:07 |
```

When no data is loaded yet:

```
| ○ Test Data | Ready                                          No chart | 0 pane(s) | 14:32:07 |
```
