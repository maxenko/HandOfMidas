//! Inline-editable label that looks like static text until clicked.
//!
//! [`EditableLabel`] displays text like a [`Label`](crate::Label) in its
//! default state, but transitions to an `iced::widget::text_input` when
//! clicked. Primary use case: the ticker symbol in the chart title bar.

use iced::widget::{button, text, text_input};
use iced::{Color, Element};

use crate::theme::UiTheme;

/// Inline-editable label that looks like static text until clicked.
///
/// # Messages
///
/// The widget emits messages through caller-provided closures:
/// - `on_input`: fired on every keystroke while editing.
/// - `on_confirm`: fired when the user presses Enter with the new text.
/// - `on_edit_start`: fired when the label is clicked to enter edit mode.
/// - `on_cancel`: (optional) fired when Escape should cancel editing.
///
/// The parent component owns the editing state (`is_editing`, `edit_text`).
///
/// # Example
///
/// ```ignore
/// let editable = EditableLabel::new(
///     &panel.symbol,
///     &panel.symbol_edit_text,
///     panel.symbol_editing,
///     move |s| Message::SymbolEditChanged(chart_id, s),
///     move |s| Message::SymbolEditConfirm(chart_id, s),
///     Message::SymbolEditStart(chart_id),
/// )
/// .on_cancel(Message::SymbolEditCancel(chart_id))
/// .size(13.0)
/// .bold()
/// .view(&ui_theme);
/// ```
pub struct EditableLabel<'a, Message> {
    /// The currently committed display text (e.g. "AAPL").
    display_text: &'a str,
    /// The current value of the text input while editing.
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
    /// Message emitted to request entering edit mode.
    on_edit_start: Message,
    /// Optional message emitted when the user presses Escape.
    on_cancel: Option<Message>,
}

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

    /// Returns whether the widget is currently in editing mode.
    pub fn is_editing(&self) -> bool {
        self.is_editing
    }
}

impl<'a, Message: Clone + 'a> EditableLabel<'a, Message> {
    /// Render the widget using the given theme.
    ///
    /// In editing mode, renders a `text_input`. In display mode, renders a
    /// styled `button` that looks like text (transparent background, hover
    /// highlight) to get free hover feedback from iced.
    pub fn view(self, theme: &UiTheme) -> Element<'a, Message> {
        let size = self.size.unwrap_or(theme.label_font_size);
        let color = self.color.unwrap_or(theme.text_primary);

        if self.is_editing {
            // Editing mode: show a text_input.
            let confirm_text = self.edit_text.to_owned();
            let on_confirm = self.on_confirm;
            let surface_bg = theme.surface;
            let border_color = theme.editable_border;

            text_input("", self.edit_text)
                .on_input(self.on_input)
                .on_submit((on_confirm)(confirm_text))
                .size(size)
                .width(iced::Length::Shrink)
                .style(move |_theme, _status| text_input::Style {
                    background: iced::Background::Color(surface_bg),
                    border: iced::Border {
                        color: border_color,
                        width: 1.0,
                        radius: 2.0.into(),
                    },
                    icon: color,
                    placeholder: Color::from_rgb(0.4, 0.4, 0.4),
                    value: color,
                    selection: Color::from_rgba(0.22, 0.55, 0.95, 0.3),
                })
                .into()
        } else {
            // Display mode: button styled as text with hover background.
            let editable_hover_bg = theme.editable_hover_bg;

            let label = text(self.display_text).size(size).color(color);
            let label = if self.bold {
                label.font(iced::Font {
                    weight: iced::font::Weight::Bold,
                    ..Default::default()
                })
            } else {
                label
            };

            button(label)
                .on_press(self.on_edit_start)
                .padding([2, 4])
                .style(move |_theme, status| {
                    let background = match status {
                        button::Status::Hovered => Some(editable_hover_bg.into()),
                        _ => None,
                    };
                    button::Style {
                        background,
                        text_color: color,
                        ..Default::default()
                    }
                })
                .into()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone)]
    enum Msg {
        EditStart,
        EditChanged(String),
        EditConfirm(String),
        EditCancel,
    }

    fn make_display_label<'a>() -> EditableLabel<'a, Msg> {
        EditableLabel::new(
            "AAPL",
            "",
            false,
            Msg::EditChanged,
            Msg::EditConfirm,
            Msg::EditStart,
        )
    }

    fn make_editing_label<'a>() -> EditableLabel<'a, Msg> {
        EditableLabel::new(
            "AAPL",
            "MSFT",
            true,
            Msg::EditChanged,
            Msg::EditConfirm,
            Msg::EditStart,
        )
    }

    #[test]
    fn display_mode_constructs_without_panic() {
        let _label = make_display_label();
    }

    #[test]
    fn editing_mode_constructs_without_panic() {
        let _label = make_editing_label();
    }

    #[test]
    fn is_editing_flag() {
        let display = make_display_label();
        assert!(!display.is_editing());

        let editing = make_editing_label();
        assert!(editing.is_editing());
    }

    #[test]
    fn builder_methods_chain() {
        let label = make_display_label()
            .on_cancel(Msg::EditCancel)
            .size(14.0)
            .color(Color::WHITE)
            .bold();

        assert!(label.on_cancel.is_some());
        assert_eq!(label.size, Some(14.0));
        assert!(label.bold);
    }

    #[test]
    fn display_and_editing_modes_differ() {
        let display = make_display_label();
        let editing = make_editing_label();

        // The core distinction: one is editing, the other is not.
        assert!(!display.is_editing());
        assert!(editing.is_editing());

        // Both should have their display_text set correctly.
        assert_eq!(display.display_text, "AAPL");
        assert_eq!(editing.display_text, "AAPL");
        assert_eq!(editing.edit_text, "MSFT");
    }
}
