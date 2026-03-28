# midas-ui: Custom Widget Crate Plan

## Overview

`midas-ui` is a minimal custom widget library for the Hand of Midas charting
application. It provides six composable widgets built directly on iced 0.14's
widget API, themed to match the existing dark trading-terminal aesthetic defined
in `midas-app/src/theme.rs` and `midas-render/src/color.rs`.

The crate has zero dependencies beyond `iced` itself. All colors, spacings, and
font sizes flow through a single `UiTheme` struct so that swapping to a light
theme or adjusting the palette requires no per-widget changes.

---

## File Structure

```
crates/midas-ui/
├── Cargo.toml
├── plan.md               <- this file
└── src/
    ├── lib.rs            <- re-exports, prelude
    ├── theme.rs          <- UiTheme struct, default dark palette
    ├── label.rs          <- Label widget
    ├── editable_label.rs <- EditableLabel widget
    ├── button.rs         <- TextButton widget
    ├── icon_button.rs    <- IconButton widget
    ├── button_group.rs   <- ButtonGroup widget
    └── tooltip.rs        <- Tooltip wrapper
```

### Cargo.toml

```toml
[package]
name = "midas-ui"
version = "0.1.0"
edition = "2021"
publish = false

[dependencies]
iced = { workspace = true }
```

The workspace root `Cargo.toml` must add `"crates/midas-ui"` to the `members`
list, and `midas-app` must add `midas-ui = { path = "../midas-ui" }` to its
`[dependencies]`.

### src/lib.rs

```rust
//! Minimal custom widget library for the Hand of Midas charting application.
//!
//! All widgets are built on iced 0.14 primitives and share a [`UiTheme`] for
//! consistent styling across the UI.

pub mod theme;
pub mod label;
pub mod editable_label;
pub mod button;
pub mod icon_button;
pub mod button_group;
pub mod tooltip;

pub use theme::UiTheme;
pub use label::Label;
pub use editable_label::EditableLabel;
pub use button::TextButton;
pub use icon_button::IconButton;
pub use button_group::ButtonGroup;
pub use tooltip::Tooltip;
```

---

## Design Principles

1. **Composition over custom rendering.** Each widget is composed from iced
   primitives (`container`, `text`, `text_input`, `mouse_area`, `row`) rather
   than implementing `Widget::draw()` from scratch. This avoids reimplementing
   layout, text shaping, and hit testing, while still giving full control over
   styling via iced's closure-based style functions.

2. **Theme-driven.** Every color, padding, and font size comes from a `UiTheme`
   reference. Widgets never contain hardcoded color values.

3. **Builder pattern.** Every widget has a `::new()` constructor returning the
   widget with sensible defaults, followed by chainable `.size()`, `.color()`,
   etc. methods for overrides. This matches the iced widget API convention.

4. **State machines for interactive widgets.** `EditableLabel`, `TextButton`,
   `IconButton`, and `ButtonGroup` track visual states (normal, hover, pressed,
   disabled). State transitions happen through iced's event system: mouse enter/
   leave, press/release, focus/blur.

5. **Composability.** `ButtonGroup` contains `TextButton` instances.
   `Tooltip` wraps any arbitrary `Element`. The title bar is assembled by
   composing these widgets in a `row![]`.

6. **Minimal surface area.** The crate exposes exactly six public widget types
   plus `UiTheme`. No utility functions, no internal abstractions beyond what
   the widgets themselves need.

---

## UiTheme (theme.rs)

### Purpose

Central palette and spacing configuration consumed by all widgets in the crate.
Mirrors the constants currently in `midas-app/src/theme.rs` but structured as a
data type that can be passed to widget constructors.

### Struct Definition

```rust
use iced::Color;

/// Centralized theme controlling all midas-ui widget colors and spacing.
///
/// Constructed once and passed by reference to widget constructors. The
/// [`Default`] implementation returns the dark trading-terminal palette
/// matching `midas-app/src/theme.rs`.
#[derive(Debug, Clone)]
pub struct UiTheme {
    // -- Text colors --
    /// Primary text (high contrast). Used for labels, active button text.
    pub text_primary: Color,
    /// Secondary text (lower emphasis). Unfocused titles, descriptions.
    pub text_secondary: Color,
    /// Muted text (lowest emphasis). Placeholders, disabled text.
    pub text_muted: Color,

    // -- Surface colors --
    /// Background for elevated surfaces (title bars, toolbars).
    pub surface: Color,
    /// Background for secondary surfaces (status bar, unfocused title bars).
    pub surface_dim: Color,

    // -- Button colors --
    /// Default button background.
    pub button_bg: Color,
    /// Hovered button background.
    pub button_hover: Color,
    /// Pressed button background.
    pub button_pressed: Color,
    /// Selected/active button background (used in ButtonGroup).
    pub button_selected: Color,
    /// Disabled button background.
    pub button_disabled: Color,
    /// Default button text color.
    pub button_text: Color,
    /// Selected button text color.
    pub button_selected_text: Color,

    // -- Accent --
    /// Accent color for focused elements, active borders.
    pub accent: Color,

    // -- Editable label --
    /// Subtle hover background to hint editability.
    pub editable_hover_bg: Color,
    /// Border color for the editing state text input.
    pub editable_border: Color,

    // -- Tooltip --
    /// Tooltip background color.
    pub tooltip_bg: Color,
    /// Tooltip text color.
    pub tooltip_text: Color,

    // -- Spacing (in logical pixels) --
    /// Default horizontal padding inside buttons.
    pub button_padding_h: f32,
    /// Default vertical padding inside buttons.
    pub button_padding_v: f32,
    /// Default border radius for buttons.
    pub button_border_radius: f32,
    /// Spacing between items in a ButtonGroup.
    pub button_group_spacing: f32,
    /// Default font size for button text.
    pub button_font_size: f32,
    /// Default font size for labels.
    pub label_font_size: f32,
    /// Default font size for tooltip text.
    pub tooltip_font_size: f32,
    /// Tooltip show delay in milliseconds.
    pub tooltip_delay_ms: u64,
}
```

### Default Implementation

```rust
impl Default for UiTheme {
    fn default() -> Self {
        Self {
            // Text — matches midas-app/src/theme.rs constants
            text_primary:    Color::from_rgb(0.88, 0.88, 0.92),
            text_secondary:  Color::from_rgb(0.55, 0.55, 0.60),
            text_muted:      Color::from_rgb(0.35, 0.35, 0.40),

            // Surfaces
            surface:         Color::from_rgb(0.12, 0.12, 0.15),
            surface_dim:     Color::from_rgb(0.10, 0.10, 0.12),

            // Buttons
            button_bg:       Color::from_rgb(0.16, 0.16, 0.20),
            button_hover:    Color::from_rgb(0.22, 0.22, 0.28),
            button_pressed:  Color::from_rgb(0.12, 0.12, 0.16),
            button_selected: Color::from_rgb(0.18, 0.35, 0.65),
            button_disabled: Color::from_rgb(0.13, 0.13, 0.16),
            button_text:     Color::from_rgb(0.88, 0.88, 0.92),
            button_selected_text: Color::WHITE,

            // Accent
            accent:          Color::from_rgb(0.22, 0.55, 0.95),

            // Editable label
            editable_hover_bg: Color::from_rgba(1.0, 1.0, 1.0, 0.05),
            editable_border:   Color::from_rgb(0.22, 0.55, 0.95),

            // Tooltip
            tooltip_bg:   Color::from_rgb(0.20, 0.20, 0.24),
            tooltip_text: Color::from_rgb(0.88, 0.88, 0.92),

            // Spacing
            button_padding_h:     8.0,
            button_padding_v:     4.0,
            button_border_radius: 3.0,
            button_group_spacing: 1.0,
            label_font_size:      13.0,
            button_font_size:     12.0,
            tooltip_font_size:    11.0,
            tooltip_delay_ms:     500,
        }
    }
}
```

### Integration Note

The theme values are sourced from the existing constants in
`crates/midas-app/src/theme.rs`:

| UiTheme field      | Existing constant       |
|--------------------|-------------------------|
| `text_primary`     | `theme::TEXT_PRIMARY`   |
| `text_secondary`   | `theme::TEXT_SECONDARY` |
| `text_muted`       | `theme::TEXT_MUTED`     |
| `surface`          | `theme::TOOLBAR_BG`    |
| `surface_dim`      | `theme::STATUS_BAR_BG` |
| `button_bg`        | `theme::BUTTON_BG`     |
| `button_hover`     | `theme::BUTTON_HOVER_BG` |
| `button_selected`  | `theme::BUTTON_ACTIVE_BG` |
| `accent`           | `theme::ACCENT`        |

---

## Widget 1: Label (label.rs)

### Purpose

Static, non-interactive text display. Thin wrapper around `iced::widget::text`
that reads font size and color from `UiTheme` by default, with builder overrides.

### States

| State  | Description              |
|--------|--------------------------|
| Normal | Only state. Displays text. |

### Struct Definition

```rust
use iced::{Color, Element};

/// Static text label with theme-driven defaults.
///
/// Wraps `iced::widget::text` with builder methods for size, color, and
/// font weight. Reads defaults from the provided [`UiTheme`].
pub struct Label<'a> {
    /// The text content to display.
    content: &'a str,
    /// Font size in logical pixels. Defaults to `theme.label_font_size`.
    size: Option<f32>,
    /// Text color. Defaults to `theme.text_primary`.
    color: Option<Color>,
    /// Whether the text should be bold.
    bold: bool,
}
```

### Constructor and Builder Methods

```rust
impl<'a> Label<'a> {
    /// Create a new label displaying the given text.
    pub fn new(content: &'a str) -> Self {
        Self {
            content,
            size: None,
            color: None,
            bold: false,
        }
    }

    /// Override the font size (logical pixels).
    pub fn size(mut self, size: f32) -> Self {
        self.size = Some(size);
        self
    }

    /// Override the text color.
    pub fn color(mut self, color: Color) -> Self {
        self.color = Some(color);
        self
    }

    /// Set the text to bold weight.
    pub fn bold(mut self) -> Self {
        self.bold = true;
        self
    }

    /// Convert to an iced Element using the given theme for defaults.
    pub fn view(self, theme: &UiTheme) -> Element<'a, Message> {
        let size = self.size.unwrap_or(theme.label_font_size);
        let color = self.color.unwrap_or(theme.text_primary);

        let mut txt = text(self.content).size(size).color(color);
        if self.bold {
            txt = txt.font(iced::Font {
                weight: iced::font::Weight::Bold,
                ..Default::default()
            });
        }
        txt.into()
    }
}
```

### Implementation Strategy

Label is **not** a custom `Widget` impl. It is a builder that produces an
`Element` via `.view(theme)`. This avoids unnecessary complexity for a widget
that has no interaction, no state, and no custom rendering.

### Example Usage

```rust
let label = Label::new("AAPL")
    .size(14.0)
    .bold()
    .view(&ui_theme);
```

---

## Widget 2: EditableLabel (editable_label.rs)

### Purpose

Displays text like a `Label` in its default state, but transitions to a
`text_input` when clicked. Primary use case: the ticker symbol in the chart
title bar. Shows "AAPL" as static text; click to edit; Enter to confirm;
Escape to cancel.

### States

| State    | Visual                                            | Trigger to enter        | Trigger to exit                       |
|----------|---------------------------------------------------|-------------------------|---------------------------------------|
| Display  | Static text, identical to Label                   | Initial state / confirm / cancel | Click or double-click on the text |
| Hover    | Static text with subtle background highlight      | Mouse enters widget     | Mouse leaves widget                   |
| Editing  | text_input with cursor, current text pre-filled   | Click on Display/Hover  | Enter (confirm) or Escape (cancel)    |

### Struct Definition

```rust
/// Inline-editable label that looks like static text until clicked.
///
/// # Messages
///
/// The widget emits two message types through caller-provided closures:
/// - `on_confirm`: fired when the user presses Enter with the new text.
/// - `on_cancel`: fired when the user presses Escape (optional).
///
/// Internally it manages its own editing state via an iced widget `Id`.
pub struct EditableLabel<'a, Message> {
    /// The currently committed display text (e.g. "AAPL").
    display_text: &'a str,
    /// The current value of the text input while editing.
    /// Managed externally in the parent's state so iced can diff it.
    edit_text: &'a str,
    /// Whether the widget is currently in editing mode.
    is_editing: bool,
    /// Font size (logical pixels).
    size: Option<f32>,
    /// Text color for display mode.
    color: Option<Color>,
    /// Bold weight for display mode.
    bold: bool,
    /// Closure called when editing text changes (mirrors text_input::on_input).
    on_input: Box<dyn Fn(String) -> Message + 'a>,
    /// Closure called when the user presses Enter.
    on_confirm: Box<dyn Fn(String) -> Message + 'a>,
    /// Closure called to request entering edit mode (parent sets is_editing).
    on_edit_start: Message,
    /// Closure called when the user presses Escape.
    on_cancel: Option<Message>,
}
```

### State Management Pattern

EditableLabel does **not** hold its own editing state internally. Instead, the
parent component (e.g. `ChartPanel` or `MidasApp`) owns three pieces of state:

```rust
// In the parent's state struct:
pub struct ChartPanel {
    pub symbol: String,            // committed display value
    pub symbol_edit_text: String,  // current text_input buffer
    pub symbol_editing: bool,      // whether we are in edit mode
    // ... other fields
}
```

This follows iced's unidirectional data flow: the widget is a pure function of
these three values, and it emits messages that the parent's `update()` handles.

### Required Messages in the Parent

```rust
enum Message {
    // ... existing variants ...

    /// The editable label was clicked; enter edit mode for the given chart.
    SymbolEditStart(ChartId),
    /// The text input contents changed while editing.
    SymbolEditChanged(ChartId, String),
    /// Enter was pressed; confirm the new symbol.
    SymbolEditConfirm(ChartId, String),
    /// Escape was pressed; cancel editing.
    SymbolEditCancel(ChartId),
}
```

### Constructor and Builder Methods

```rust
impl<'a, Message> EditableLabel<'a, Message> {
    /// Create a new editable label.
    ///
    /// - `display_text`: the committed value shown when not editing.
    /// - `edit_text`: the current text_input buffer (owned by parent).
    /// - `is_editing`: whether the widget should show the text_input.
    /// - `on_input`: called on every keystroke while editing.
    /// - `on_confirm`: called when Enter is pressed.
    /// - `on_edit_start`: message emitted when the label is clicked.
    pub fn new(
        display_text: &'a str,
        edit_text: &'a str,
        is_editing: bool,
        on_input: impl Fn(String) -> Message + 'a,
        on_confirm: impl Fn(String) -> Message + 'a,
        on_edit_start: Message,
    ) -> Self {
        Self {
            display_text,
            edit_text,
            is_editing,
            size: None,
            color: None,
            bold: false,
            on_input: Box::new(on_input),
            on_confirm: Box::new(on_confirm),
            on_edit_start,
            on_cancel: None,
        }
    }

    /// Set an optional cancel message (Escape key).
    pub fn on_cancel(mut self, msg: Message) -> Self {
        self.on_cancel = Some(msg);
        self
    }

    /// Override font size.
    pub fn size(mut self, size: f32) -> Self {
        self.size = Some(size);
        self
    }

    /// Override text color.
    pub fn color(mut self, color: Color) -> Self {
        self.color = Some(color);
        self
    }

    /// Set bold weight.
    pub fn bold(mut self) -> Self {
        self.bold = true;
        self
    }
}
```

### View Method

```rust
impl<'a, Message: Clone + 'a> EditableLabel<'a, Message> {
    /// Render the widget using the given theme.
    pub fn view(self, theme: &UiTheme) -> Element<'a, Message> {
        let size = self.size.unwrap_or(theme.label_font_size);
        let color = self.color.unwrap_or(theme.text_primary);

        if self.is_editing {
            // Editing mode: show a text_input
            let confirm_text = self.edit_text.to_owned();
            let on_confirm = self.on_confirm;

            text_input("", self.edit_text)
                .on_input(self.on_input)
                .on_submit((on_confirm)(confirm_text))
                .size(size)
                .width(iced::Length::Shrink)
                .style(move |_theme, status| {
                    text_input::Style {
                        background: iced::Background::Color(
                            theme.surface,
                        ),
                        border: iced::Border {
                            color: theme.editable_border,
                            width: 1.0,
                            radius: 2.0.into(),
                        },
                        // ... icon/placeholder/value/selection colors
                        ..Default::default()
                    }
                })
                .into()
        } else {
            // Display mode: static text wrapped in mouse_area for click
            let label = text(self.display_text).size(size).color(color);
            let label = if self.bold {
                label.font(iced::Font {
                    weight: iced::font::Weight::Bold,
                    ..Default::default()
                })
            } else {
                label
            };

            // Wrap in mouse_area to detect click -> enter edit mode
            // Wrap in container for hover background
            let display = container(label)
                .padding([2, 4])
                .style(move |_theme| container::Style {
                    // Hover background applied via mouse_area interaction
                    // (see state machine below)
                    ..Default::default()
                });

            mouse_area(display)
                .on_press(self.on_edit_start)
                .into()
        }
    }
}
```

### State Machine Diagram

```
                click
  DISPLAY  ─────────────>  EDITING
     ^                        │
     │   Enter (confirm)      │
     ├────────────────────────┘
     │   Escape (cancel)      │
     └────────────────────────┘

  Mouse enter/leave on DISPLAY toggles HOVER styling.
  HOVER is a visual sub-state of DISPLAY, not a separate mode.
```

### Hover Behavior

The hover highlight (subtle background change) is handled via iced's
`mouse_area` or by applying a `container` style that responds to mouse
position. Since iced 0.14's `container::Style` is set via a closure that
receives the iced Theme (not mouse state), the hover effect can be implemented
with `mouse_area::on_enter` and `mouse_area::on_exit` messages that toggle a
`hovered: bool` flag in the parent state. However, to keep the parent state
minimal, an alternative approach is to use iced's built-in `button` widget
styled to look like plain text (transparent background, no border) with its
hover style set to `editable_hover_bg`. This is simpler and avoids extra state:

```rust
// Alternative: use a button styled as text for hover effect
button(label)
    .on_press(self.on_edit_start)
    .padding([2, 4])
    .style(move |_theme, status| {
        button::Style {
            background: match status {
                button::Status::Hovered => Some(theme.editable_hover_bg.into()),
                _ => None,
            },
            text_color: color,
            border: iced::Border::default(),
            shadow: iced::Shadow::default(),
        }
    })
    .into()
```

This is the **recommended approach**: use `button` in display mode (styled to
look like text but with hover feedback) and `text_input` in editing mode.

### Focus Management

When entering edit mode, the text_input should automatically receive keyboard
focus. This is achieved by returning a `Task::widget(text_input::focus(id))`
from the parent's update handler for `SymbolEditStart`. The text_input widget
ID should be derived deterministically from the `ChartId`:

```rust
// In the parent's update():
Message::SymbolEditStart(chart_id) => {
    if let Some(panel) = self.charts.get_mut(&chart_id) {
        panel.symbol_editing = true;
        panel.symbol_edit_text = panel.symbol.clone();
    }
    // Focus the text_input
    let id = text_input::Id::new(format!("symbol-edit-{}", chart_id));
    text_input::focus(id)
}

Message::SymbolEditConfirm(chart_id, new_symbol) => {
    if let Some(panel) = self.charts.get_mut(&chart_id) {
        panel.symbol = new_symbol.trim().to_uppercase();
        panel.symbol_editing = false;
    }
    // Trigger data reload...
    Task::none()
}

Message::SymbolEditCancel(chart_id) => {
    if let Some(panel) = self.charts.get_mut(&chart_id) {
        panel.symbol_editing = false;
    }
    Task::none()
}
```

### Escape Key Handling

iced 0.14's `text_input` does not have a built-in `on_escape` callback.
Escape must be handled via the global keyboard subscription that already exists
in the app (`Message::KeyPressed`). When a `KeyPressed(Escape)` is received
and any chart's `symbol_editing` is `true`, emit the cancel:

```rust
Key::Named(Named::Escape) => {
    // Cancel any active editable label
    for (&id, panel) in self.charts.iter_mut() {
        if panel.symbol_editing {
            panel.symbol_editing = false;
            break;
        }
    }
}
```

Alternatively, the EditableLabel in editing mode can be wrapped in an
`iced::widget::keyed_column` or use `iced::keyboard::on_key_press` subscription
to intercept Escape before it propagates.

### Example Usage

```rust
let editable = EditableLabel::new(
    &panel.symbol,
    &panel.symbol_edit_text,
    panel.symbol_editing,
    move |s| Message::SymbolEditChanged(chart_id, s),
    move |s| Message::SymbolEditConfirm(chart_id, s),
    Message::SymbolEditStart(chart_id),
)
.on_cancel(Message::SymbolEditCancel(chart_id))
.size(13.0)
.bold()
.view(&ui_theme);
```

---

## Widget 3: TextButton (button.rs)

### Purpose

A button with text content. Wraps iced's `button` widget with theme-driven
styling for all four visual states.

### States

| State    | Visual                          | Transition in              | Transition out             |
|----------|---------------------------------|----------------------------|----------------------------|
| Normal   | `button_bg` background          | Initial / mouse leave      | Mouse enters               |
| Hover    | `button_hover` background       | Mouse enters               | Mouse leaves / press       |
| Pressed  | `button_pressed` background     | Mouse down                 | Mouse up                   |
| Disabled | `button_disabled` bg, muted text| `.disabled(true)` called   | `.disabled(false)` called  |

### Struct Definition

```rust
use iced::{Color, Element, Length};

/// Theme-aware text button with four visual states.
///
/// Built on top of `iced::widget::button` with a custom style closure
/// that reads colors from [`UiTheme`].
pub struct TextButton<'a, Message> {
    /// Button label text.
    content: &'a str,
    /// Message emitted on press (None if disabled).
    on_press: Option<Message>,
    /// Font size override.
    size: Option<f32>,
    /// Text color override.
    text_color: Option<Color>,
    /// Background color override (normal state).
    background: Option<Color>,
    /// Horizontal padding override.
    padding_h: Option<f32>,
    /// Vertical padding override.
    padding_v: Option<f32>,
    /// Border radius override.
    border_radius: Option<f32>,
    /// Width constraint.
    width: Option<Length>,
    /// Whether the button is disabled (overrides on_press to None).
    disabled: bool,
}
```

### Constructor and Builder Methods

```rust
impl<'a, Message: Clone> TextButton<'a, Message> {
    /// Create a new text button with the given label.
    pub fn new(content: &'a str) -> Self {
        Self {
            content,
            on_press: None,
            size: None,
            text_color: None,
            background: None,
            padding_h: None,
            padding_v: None,
            border_radius: None,
            width: None,
            disabled: false,
        }
    }

    /// Set the message emitted when the button is pressed.
    pub fn on_press(mut self, msg: Message) -> Self {
        self.on_press = Some(msg);
        self
    }

    /// Set the message emitted when pressed, or None to disable.
    pub fn on_press_maybe(mut self, msg: Option<Message>) -> Self {
        self.on_press = msg;
        self
    }

    /// Override font size.
    pub fn size(mut self, size: f32) -> Self {
        self.size = Some(size);
        self
    }

    /// Override text color.
    pub fn text_color(mut self, color: Color) -> Self {
        self.text_color = Some(color);
        self
    }

    /// Override the normal-state background color.
    pub fn background(mut self, color: Color) -> Self {
        self.background = Some(color);
        self
    }

    /// Override horizontal padding.
    pub fn padding_h(mut self, px: f32) -> Self {
        self.padding_h = Some(px);
        self
    }

    /// Override vertical padding.
    pub fn padding_v(mut self, px: f32) -> Self {
        self.padding_v = Some(px);
        self
    }

    /// Override border radius.
    pub fn border_radius(mut self, r: f32) -> Self {
        self.border_radius = Some(r);
        self
    }

    /// Set the width constraint.
    pub fn width(mut self, w: Length) -> Self {
        self.width = Some(w);
        self
    }

    /// Mark the button as disabled.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}
```

### View Method and Styling

```rust
impl<'a, Message: Clone + 'a> TextButton<'a, Message> {
    /// Render the button using the given theme.
    pub fn view(self, theme: &UiTheme) -> Element<'a, Message> {
        let font_size = self.size.unwrap_or(theme.button_font_size);
        let txt_color = if self.disabled {
            theme.text_muted
        } else {
            self.text_color.unwrap_or(theme.button_text)
        };
        let bg = self.background.unwrap_or(theme.button_bg);
        let pad_h = self.padding_h.unwrap_or(theme.button_padding_h);
        let pad_v = self.padding_v.unwrap_or(theme.button_padding_v);
        let radius = self.border_radius.unwrap_or(theme.button_border_radius);
        let hover_bg = theme.button_hover;
        let pressed_bg = theme.button_pressed;
        let disabled_bg = theme.button_disabled;
        let disabled = self.disabled;

        let label = text(self.content).size(font_size).color(txt_color);

        let mut btn = button(label)
            .padding([pad_v, pad_h])
            .style(move |_iced_theme, status| {
                let background = if disabled {
                    disabled_bg
                } else {
                    match status {
                        button::Status::Hovered => hover_bg,
                        button::Status::Pressed => pressed_bg,
                        _ => bg,
                    }
                };
                button::Style {
                    background: Some(background.into()),
                    text_color: txt_color,
                    border: iced::Border {
                        radius: radius.into(),
                        ..Default::default()
                    },
                    shadow: iced::Shadow::default(),
                }
            });

        if !self.disabled {
            if let Some(msg) = self.on_press {
                btn = btn.on_press(msg);
            }
        }

        if let Some(w) = self.width {
            btn = btn.width(w);
        }

        btn.into()
    }
}
```

### iced Integration

TextButton does not implement the `Widget` trait directly. It wraps iced's
`button` widget and applies a custom style closure. The four visual states
(normal, hover, pressed, disabled) are handled entirely by iced's built-in
`button::Status` enum, which is passed to the style closure:

- `button::Status::Active` -> normal background
- `button::Status::Hovered` -> lighter background
- `button::Status::Pressed` -> darker background
- Disabled is handled by not calling `.on_press()`, which prevents iced from
  ever entering the Hovered/Pressed states, and by using `disabled_bg` in the
  style closure's Active branch.

### Example Usage

```rust
let btn = TextButton::new("Split H")
    .on_press(Message::PaneSplit(Axis::Horizontal, pane))
    .size(11.0)
    .padding_h(6.0)
    .view(&ui_theme);
```

---

## Widget 4: IconButton (icon_button.rs)

### Purpose

A button with icon content instead of (or in addition to) text. Icons are
Unicode characters rendered at a configurable size. Same four visual states as
TextButton.

### States

Same as TextButton: Normal, Hover, Pressed, Disabled.

### Icon Representation

Icons are Unicode characters or short strings. No bitmap/image support in V1.
Common icons:

| Icon       | Character | Unicode    |
|------------|-----------|------------|
| Close      | `\u{00D7}` (multiplication sign) or `\u{2715}` | U+00D7 / U+2715 |
| Settings   | `\u{2699}` (gear)                    | U+2699   |
| Expand     | `\u{25B6}` (right triangle)          | U+25B6   |
| Collapse   | `\u{25BC}` (down triangle)           | U+25BC   |
| Plus       | `+`                                  | U+002B   |
| Minus      | `\u{2212}`                           | U+2212   |
| Fullscreen | `\u{26F6}` (square four corners)     | U+26F6   |

### Struct Definition

```rust
/// Theme-aware icon button using Unicode characters as icons.
///
/// Identical state machine to [`TextButton`] but renders a single
/// character/glyph centered in a square-ish button area.
pub struct IconButton<'a, Message> {
    /// The icon character(s) to display.
    icon: &'a str,
    /// Message emitted on press.
    on_press: Option<Message>,
    /// Icon font size (logical pixels). Defaults to 14.0.
    icon_size: Option<f32>,
    /// Icon color override.
    icon_color: Option<Color>,
    /// Background color override (normal state).
    background: Option<Color>,
    /// Padding (uniform on all sides for a square feel).
    padding: Option<f32>,
    /// Border radius override.
    border_radius: Option<f32>,
    /// Whether the button is disabled.
    disabled: bool,
    /// Optional tooltip text (rendered separately via Tooltip wrapper).
    tooltip_text: Option<&'a str>,
}
```

### Constructor and Builder Methods

```rust
impl<'a, Message: Clone> IconButton<'a, Message> {
    /// Create a new icon button with the given Unicode icon string.
    pub fn new(icon: &'a str) -> Self {
        Self {
            icon,
            on_press: None,
            icon_size: None,
            icon_color: None,
            background: None,
            padding: None,
            border_radius: None,
            disabled: false,
            tooltip_text: None,
        }
    }

    /// Set the message emitted when pressed.
    pub fn on_press(mut self, msg: Message) -> Self {
        self.on_press = Some(msg);
        self
    }

    /// Override icon size.
    pub fn icon_size(mut self, size: f32) -> Self {
        self.icon_size = Some(size);
        self
    }

    /// Override icon color.
    pub fn icon_color(mut self, color: Color) -> Self {
        self.icon_color = Some(color);
        self
    }

    /// Override background color (normal state).
    pub fn background(mut self, color: Color) -> Self {
        self.background = Some(color);
        self
    }

    /// Override uniform padding.
    pub fn padding(mut self, px: f32) -> Self {
        self.padding = Some(px);
        self
    }

    /// Override border radius.
    pub fn border_radius(mut self, r: f32) -> Self {
        self.border_radius = Some(r);
        self
    }

    /// Mark as disabled.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Attach tooltip text (requires wrapping with Tooltip at call site).
    pub fn tooltip(mut self, text: &'a str) -> Self {
        self.tooltip_text = Some(text);
        self
    }
}
```

### View Method

```rust
impl<'a, Message: Clone + 'a> IconButton<'a, Message> {
    /// Render the icon button using the given theme.
    pub fn view(self, theme: &UiTheme) -> Element<'a, Message> {
        let icon_sz = self.icon_size.unwrap_or(14.0);
        let color = if self.disabled {
            theme.text_muted
        } else {
            self.icon_color.unwrap_or(theme.text_secondary)
        };
        let bg = self.background.unwrap_or(Color::TRANSPARENT);
        let pad = self.padding.unwrap_or(4.0);
        let radius = self.border_radius.unwrap_or(theme.button_border_radius);
        let hover_bg = theme.button_hover;
        let pressed_bg = theme.button_pressed;
        let disabled = self.disabled;

        let icon_text = text(self.icon)
            .size(icon_sz)
            .color(color)
            .align_x(iced::alignment::Horizontal::Center);

        let mut btn = button(icon_text)
            .padding(pad)
            .style(move |_iced_theme, status| {
                let background = if disabled {
                    Color::TRANSPARENT
                } else {
                    match status {
                        button::Status::Hovered => hover_bg,
                        button::Status::Pressed => pressed_bg,
                        _ => bg,
                    }
                };
                button::Style {
                    background: Some(background.into()),
                    text_color: color,
                    border: iced::Border {
                        radius: radius.into(),
                        ..Default::default()
                    },
                    shadow: iced::Shadow::default(),
                }
            });

        if !self.disabled {
            if let Some(msg) = self.on_press {
                btn = btn.on_press(msg);
            }
        }

        let element: Element<'a, Message> = btn.into();

        // If tooltip text was provided, wrap with Tooltip
        if let Some(tip) = self.tooltip_text {
            Tooltip::new(element, tip).view(theme)
        } else {
            element
        }
    }
}
```

### Difference from TextButton

- Default background is `Color::TRANSPARENT` (icon buttons are often
  borderless until hovered).
- Uniform padding (square feel) vs. asymmetric h/v padding.
- Smaller default size, centered text alignment.
- Built-in `.tooltip()` convenience that integrates with the Tooltip widget.

### Example Usage

```rust
// Close button (multiplication sign)
let close_btn = IconButton::new("\u{00D7}")
    .on_press(Message::PaneClose(pane))
    .icon_size(12.0)
    .icon_color(theme.text_muted)
    .tooltip("Close panel")
    .view(&ui_theme);
```

---

## Widget 5: ButtonGroup (button_group.rs)

### Purpose

Horizontal row of text buttons where exactly one is "selected" (toggle group).
Use case: timeframe selector (1m | 5m | 15m | 1H | 4H | 1D | 1W). The
selected button has a distinct background and text color.

### States

The group itself has no state. Each child button has:
- **Selected**: `button_selected` background, `button_selected_text` color.
- **Normal/Hover/Pressed**: same as TextButton.

### Struct Definition

```rust
/// Horizontal toggle group of text buttons.
///
/// Exactly one button is visually "selected" at a time. Pressing any
/// button emits a message carrying the selected item's value.
///
/// Generic over `T` which is the value type (e.g. `Timeframe`).
pub struct ButtonGroup<'a, T, Message> {
    /// (label, value) pairs for each button in the group.
    items: Vec<(&'a str, T)>,
    /// The currently selected value. Compared via PartialEq to determine
    /// which button gets the selected style.
    selected: T,
    /// Closure that maps a selected value to a Message.
    on_select: Box<dyn Fn(T) -> Message + 'a>,
    /// Font size override for all buttons.
    size: Option<f32>,
    /// Padding override.
    padding_h: Option<f32>,
    padding_v: Option<f32>,
    /// Spacing between buttons.
    spacing: Option<f32>,
}
```

### Constructor and Builder Methods

```rust
impl<'a, T: PartialEq + Clone + 'a, Message: Clone + 'a>
    ButtonGroup<'a, T, Message>
{
    /// Create a new button group.
    ///
    /// - `items`: slice of (label, value) pairs.
    /// - `selected`: the currently selected value.
    /// - `on_select`: closure mapping a value to a message.
    pub fn new(
        items: Vec<(&'a str, T)>,
        selected: T,
        on_select: impl Fn(T) -> Message + 'a,
    ) -> Self {
        Self {
            items,
            selected,
            on_select: Box::new(on_select),
            size: None,
            padding_h: None,
            padding_v: None,
            spacing: None,
        }
    }

    /// Override font size for all buttons.
    pub fn size(mut self, size: f32) -> Self {
        self.size = Some(size);
        self
    }

    /// Override horizontal padding for all buttons.
    pub fn padding_h(mut self, px: f32) -> Self {
        self.padding_h = Some(px);
        self
    }

    /// Override vertical padding for all buttons.
    pub fn padding_v(mut self, px: f32) -> Self {
        self.padding_v = Some(px);
        self
    }

    /// Override spacing between buttons.
    pub fn spacing(mut self, px: f32) -> Self {
        self.spacing = Some(px);
        self
    }
}
```

### View Method

```rust
impl<'a, T: PartialEq + Clone + 'a, Message: Clone + 'a>
    ButtonGroup<'a, T, Message>
{
    /// Render the button group as a horizontal row using the given theme.
    pub fn view(self, theme: &UiTheme) -> Element<'a, Message> {
        let font_size = self.size.unwrap_or(theme.button_font_size);
        let pad_h = self.padding_h.unwrap_or(theme.button_padding_h);
        let pad_v = self.padding_v.unwrap_or(theme.button_padding_v);
        let spacing = self.spacing.unwrap_or(theme.button_group_spacing);

        let buttons: Vec<Element<'a, Message>> = self
            .items
            .into_iter()
            .map(|(label, value)| {
                let is_selected = value == self.selected;
                let msg = (self.on_select)(value);

                let txt_color = if is_selected {
                    theme.button_selected_text
                } else {
                    theme.button_text
                };
                let normal_bg = if is_selected {
                    theme.button_selected
                } else {
                    theme.button_bg
                };
                let hover_bg = if is_selected {
                    theme.button_selected  // No change on hover when selected
                } else {
                    theme.button_hover
                };
                let pressed_bg = if is_selected {
                    theme.button_selected
                } else {
                    theme.button_pressed
                };
                let radius = theme.button_border_radius;

                let label_widget = text(label).size(font_size).color(txt_color);

                button(label_widget)
                    .on_press(msg)
                    .padding([pad_v, pad_h])
                    .style(move |_iced_theme, status| {
                        let bg = match status {
                            button::Status::Hovered => hover_bg,
                            button::Status::Pressed => pressed_bg,
                            _ => normal_bg,
                        };
                        button::Style {
                            background: Some(bg.into()),
                            text_color: txt_color,
                            border: iced::Border {
                                radius: radius.into(),
                                ..Default::default()
                            },
                            shadow: iced::Shadow::default(),
                        }
                    })
                    .into()
            })
            .collect();

        Row::with_children(buttons).spacing(spacing).into()
    }
}
```

### Visual Layout

```
 ┌────┬────┬─────┬────┬────┬────┬────┐
 │ 1m │ 5m │ 15m │ 1H │ 4H │[1D]│ 1W │
 └────┴────┴─────┴────┴────┴────┴────┘
                        ^^^^
                      selected
                  (button_selected bg)
```

Spacing between buttons is `button_group_spacing` (default 1px) to create a
near-seamless visual strip. The selected button stands out through its
`button_selected` background color (a blue accent) and white text.

### Example Usage

```rust
use midas_core::Timeframe;

let timeframes = vec![
    ("1m",  Timeframe::M1),
    ("5m",  Timeframe::M5),
    ("15m", Timeframe::M15),
    ("1H",  Timeframe::H1),
    ("4H",  Timeframe::H4),
    ("1D",  Timeframe::D1),
    ("1W",  Timeframe::W1),
];

let tf_group = ButtonGroup::new(
    timeframes,
    panel.timeframe,
    move |tf| Message::TimeframeSelected(chart_id, tf),
)
.size(12.0)
.padding_h(8.0)
.padding_v(4.0)
.view(&ui_theme);
```

---

## Widget 6: Tooltip (tooltip.rs)

### Purpose

Shows a text popup on hover after a configurable delay. Wraps any arbitrary
iced `Element` and adds a tooltip below/above it.

### Implementation Strategy

iced 0.14 ships with `iced::widget::tooltip` which provides exactly this
behavior. The midas-ui `Tooltip` is a thin wrapper that applies theme-driven
styling (background color, text color, font size, padding) so callers do not
need to repeat style boilerplate.

### Struct Definition

```rust
use iced::widget::tooltip::Position;

/// Theme-styled tooltip wrapper.
///
/// Wraps any `Element` and shows a text popup on hover. Delegates to
/// `iced::widget::tooltip` with colors from [`UiTheme`].
pub struct Tooltip<'a, Message> {
    /// The widget to attach the tooltip to.
    content: Element<'a, Message>,
    /// Tooltip text to display.
    tip_text: &'a str,
    /// Tooltip position relative to the content.
    position: Position,
    /// Gap between the content and the tooltip popup (px).
    gap: Option<f32>,
}
```

### Constructor and Builder Methods

```rust
impl<'a, Message: 'a> Tooltip<'a, Message> {
    /// Create a tooltip wrapping the given content element.
    pub fn new(content: Element<'a, Message>, tip_text: &'a str) -> Self {
        Self {
            content,
            tip_text,
            position: Position::Bottom,
            gap: None,
        }
    }

    /// Set the tooltip position.
    pub fn position(mut self, pos: Position) -> Self {
        self.position = pos;
        self
    }

    /// Set the gap between content and tooltip.
    pub fn gap(mut self, px: f32) -> Self {
        self.gap = Some(px);
        self
    }
}
```

### View Method

```rust
impl<'a, Message: 'a> Tooltip<'a, Message> {
    /// Render the tooltip-wrapped widget using the given theme.
    pub fn view(self, theme: &UiTheme) -> Element<'a, Message> {
        let tip = text(self.tip_text)
            .size(theme.tooltip_font_size)
            .color(theme.tooltip_text);

        let gap = self.gap.unwrap_or(4.0);
        let bg = theme.tooltip_bg;
        let radius = theme.button_border_radius;

        iced::widget::tooltip(self.content, tip, self.position)
            .gap(gap)
            .style(move |_theme| container::Style {
                background: Some(bg.into()),
                border: iced::Border {
                    radius: radius.into(),
                    ..Default::default()
                },
                ..Default::default()
            })
            .into()
    }
}
```

### Note on Delay

iced 0.14's built-in tooltip does not support a show delay. The
`tooltip_delay_ms` field in `UiTheme` is reserved for a future custom
implementation if the instant-show behavior proves undesirable. For V1, the
tooltip appears immediately on hover, which is consistent with iced's built-in
behavior.

### Example Usage

```rust
let close_btn = IconButton::new("\u{00D7}")
    .on_press(Message::PaneClose(pane))
    .view(&ui_theme);

let with_tip = Tooltip::new(close_btn, "Close panel")
    .position(Position::Bottom)
    .view(&ui_theme);
```

---

## Integration: Chart Panel Title Bar

### Current Implementation

The app currently uses `pane_grid::TitleBar` (see `app.rs:868-913`). It shows
the symbol + timeframe as static text and a close button. The title bar is a
built-in iced construct that sits above the pane body.

### New Implementation

Replace the `pane_grid::TitleBar` contents with midas-ui widgets. We continue
to use `pane_grid::TitleBar` as the container (it provides the drag handle
behavior for pane reordering) but populate it with our custom widgets.

### Layout

```
┌─────────────────────────────────────────────────────────────────────┐
│ [EditableLabel: "AAPL"] [ButtonGroup: 1m|5m|15m|1H|4H|1D|1W] ──── [x] │
│                                                            (space)      │
└─────────────────────────────────────────────────────────────────────┘
```

- `EditableLabel` for the symbol (click to change ticker)
- `ButtonGroup` for the timeframe selector (per-panel, not global toolbar)
- `Space::new().width(Fill)` to push the close button to the right
- `IconButton` for the close button (x)

### State Changes Required in ChartPanel

```rust
pub struct ChartPanel {
    pub symbol: String,
    pub timeframe: Timeframe,
    pub data: Option<Arc<CandleBuffer>>,
    pub chart_state: ChartState,
    pub load_state: LoadState,

    // NEW fields for EditableLabel
    pub symbol_edit_text: String,  // text_input buffer while editing
    pub symbol_editing: bool,      // whether currently in edit mode
}
```

### New Message Variants

```rust
enum Message {
    // ... existing variants ...

    // Per-panel symbol editing
    SymbolEditStart(ChartId),
    SymbolEditChanged(ChartId, String),
    SymbolEditConfirm(ChartId, String),
    SymbolEditCancel(ChartId),

    // Per-panel timeframe selection (replaces global TimeframeSelected)
    PanelTimeframeSelected(ChartId, Timeframe),
}
```

### Updated view_pane_title_bar

```rust
fn view_pane_title_bar(
    &self,
    chart_id: ChartId,
    pane: pane_grid::Pane,
    is_focused: bool,
) -> pane_grid::TitleBar<'_, Message> {
    let ui_theme = UiTheme::default(); // or store as field on MidasApp
    let panel = self.charts.get(&chart_id);

    // -- Symbol: EditableLabel --
    let symbol_widget = if let Some(panel) = panel {
        EditableLabel::new(
            if panel.symbol.is_empty() { "---" } else { &panel.symbol },
            &panel.symbol_edit_text,
            panel.symbol_editing,
            move |s| Message::SymbolEditChanged(chart_id, s),
            move |s| Message::SymbolEditConfirm(chart_id, s),
            Message::SymbolEditStart(chart_id),
        )
        .on_cancel(Message::SymbolEditCancel(chart_id))
        .size(13.0)
        .bold()
        .color(if is_focused {
            ui_theme.text_primary
        } else {
            ui_theme.text_secondary
        })
        .view(&ui_theme)
    } else {
        Label::new("Empty").color(ui_theme.text_muted).view(&ui_theme)
    };

    // -- Timeframe: ButtonGroup --
    let timeframes = vec![
        ("1m",  Timeframe::M1),
        ("5m",  Timeframe::M5),
        ("15m", Timeframe::M15),
        ("1H",  Timeframe::H1),
        ("4H",  Timeframe::H4),
        ("1D",  Timeframe::D1),
        ("1W",  Timeframe::W1),
    ];
    let current_tf = panel.map(|p| p.timeframe).unwrap_or(Timeframe::D1);
    let tf_group = ButtonGroup::new(
        timeframes,
        current_tf,
        move |tf| Message::PanelTimeframeSelected(chart_id, tf),
    )
    .size(10.0)
    .padding_h(6.0)
    .padding_v(2.0)
    .view(&ui_theme);

    // -- Close: IconButton --
    let close_btn = if self.workspace.pane_count() > 1 {
        IconButton::new("\u{00D7}")
            .on_press(Message::PaneClose(pane))
            .icon_size(12.0)
            .icon_color(ui_theme.text_muted)
            .padding(2.0)
            .view(&ui_theme)
    } else {
        Space::new().width(Length::Shrink).height(Length::Shrink).into()
    };

    // -- Compose --
    let title_row = row![
        symbol_widget,
        Space::new().width(8),
        tf_group,
        Space::new().width(Fill),
        close_btn,
    ]
    .spacing(4)
    .align_y(iced::Alignment::Center);

    let bg_color = if is_focused {
        ui_theme.surface
    } else {
        ui_theme.surface_dim
    };

    pane_grid::TitleBar::new(title_row)
        .padding([3, 6])
        .style(move |_theme| container::Style {
            background: Some(bg_color.into()),
            ..Default::default()
        })
}
```

### Title Bar Inside the Chart Panel (Alternative)

The specification calls for the title bar to be rendered INSIDE the chart
panel, above the chart area but within the same pane. Two approaches:

**Approach A: Keep pane_grid::TitleBar (recommended for V1).**
Continue using `pane_grid::TitleBar` but populate it with our custom widgets.
This preserves iced's built-in drag-to-reorder behavior. The title bar already
renders inside the pane visually; it appears as the top strip of each pane in
the grid. This is the approach shown above.

**Approach B: Remove TitleBar, embed in pane body.**
Remove the `pane_grid::TitleBar` and instead compose the title row directly
inside `view_pane_body()` as a `column![title_row, chart_content]`. This gives
maximum control but loses iced's built-in pane drag behavior (would need to
reimplement drag detection on the title bar area). Not recommended for V1.

**Recommendation: Use Approach A.** It is simpler, preserves drag behavior, and
still places the title bar visually inside the pane. The only difference from
Approach B is that the title bar is managed by iced's pane_grid widget rather
than by our own column layout — but visually the result is identical.

---

## Implementation Order

Implement the widgets in dependency order:

1. **theme.rs** — `UiTheme` struct and `Default` impl. Zero dependencies
   beyond `iced::Color`. All other modules import this.

2. **label.rs** — `Label`. Simplest widget. Only depends on `UiTheme`.
   Good for validating the builder pattern and `.view(theme)` convention.

3. **button.rs** — `TextButton`. Introduces the four-state style closure
   pattern. This becomes the reference implementation for state handling.

4. **icon_button.rs** — `IconButton`. Variant of TextButton, can reference
   its style closure pattern. Tests Unicode rendering.

5. **tooltip.rs** — `Tooltip`. Wraps `iced::widget::tooltip`. Needed by
   IconButton's `.tooltip()` convenience method.

6. **button_group.rs** — `ButtonGroup`. Composes TextButton-style buttons
   in a row. Tests composability.

7. **editable_label.rs** — `EditableLabel`. Most complex widget with two
   visual modes. Depends on Label's text rendering and the button style
   pattern for hover. Implement last because it requires parent-state
   changes in `midas-app`.

---

## Testing Strategy

### Unit Tests (per module)

Each module should have a `#[cfg(test)] mod tests` section verifying:

- **theme.rs**: All default colors have components in `[0.0, 1.0]`. All
  spacing values are positive. `UiTheme::default()` does not panic.

- **label.rs**: `Label::new("test")` does not panic. Builder methods return
  `Self` (chain correctly). Since `view()` requires an iced renderer we
  cannot easily unit-test the output Element, but we can test that
  construction with various combinations of overrides does not panic.

- **button.rs / icon_button.rs**: Same construction tests. Verify that
  `.disabled(true)` causes `on_press` to be None in the output. Verify
  builder methods are independent (setting size does not reset color).

- **button_group.rs**: Verify construction with empty items does not panic.
  Verify selected item detection logic (PartialEq comparison).

- **editable_label.rs**: Verify construction in both `is_editing = true` and
  `is_editing = false` modes. Verify that the correct branch (text vs
  text_input) is taken.

### Integration Tests (in midas-app)

After wiring the widgets into `view_pane_title_bar`:

1. **Visual inspection**: Run the app, verify the title bar renders correctly
   with symbol, timeframe buttons, and close button.

2. **EditableLabel flow**: Click the symbol text, verify text_input appears.
   Type a new symbol, press Enter, verify the label updates and data reloads.
   Press Escape, verify the edit cancels.

3. **ButtonGroup selection**: Click different timeframe buttons, verify the
   selected button changes style and the panel's timeframe updates.

4. **IconButton close**: Click the close button, verify the pane closes.

5. **Theme consistency**: Compare the widget colors against the existing
   toolbar to ensure visual harmony.

---

## Open Questions for Implementation

1. **Theme lifetime**: Should `UiTheme` be stored as a field on `MidasApp`
   and passed into views, or reconstructed each frame via `Default::default()`?
   Storing it is cleaner if we later support runtime theme switching.
   **Recommendation**: Store as `pub ui_theme: UiTheme` on `MidasApp`.

2. **Generic Message type**: The widgets are generic over `Message`. To avoid
   requiring `Message: Clone` everywhere, consider whether any widget needs
   to clone the message. `ButtonGroup` does (it calls `on_select` per item
   in a loop), so `Message: Clone` is required there. TextButton and
   IconButton take `Option<Message>` which requires `Clone` for
   `button.on_press()`. This matches iced's own convention.

3. **Accessibility**: iced 0.14 does not have a built-in accessibility layer.
   No action needed for V1, but structure the widgets so that accessibility
   traits can be added later without API changes.

4. **Font selection**: The current app uses iced's default font. If we want
   a monospace font for the symbol label (common in trading terminals), we
   would need to load a custom font via `iced::font::load()`. Out of scope
   for V1 but the `UiTheme` struct could later include `Font` fields.
