//! Theme-aware text button with four visual states.
//!
//! [`TextButton`] wraps `iced::widget::button` with a custom style closure
//! that reads all colors from a [`UiTheme`]. The four visual states (normal,
//! hover, pressed, disabled) are driven by iced's built-in `button::Status`.

use iced::widget::{button, text};
use iced::{Color, Element, Length};

use crate::theme::UiTheme;

/// Theme-aware text button with four visual states.
///
/// Built on top of `iced::widget::button` with a custom style closure
/// that reads colors from [`UiTheme`].
///
/// # Example
///
/// ```ignore
/// let btn = TextButton::new("Split H")
///     .on_press(Message::PaneSplit(Axis::Horizontal, pane))
///     .size(11.0)
///     .padding_h(6.0)
///     .view(&ui_theme);
/// ```
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
                    ..Default::default()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone)]
    enum Msg {
        Clicked,
    }

    #[test]
    fn text_button_constructs_without_panic() {
        let _btn = TextButton::<Msg>::new("test");
    }

    #[test]
    fn text_button_builder_chains() {
        let btn = TextButton::new("OK")
            .on_press(Msg::Clicked)
            .size(14.0)
            .text_color(Color::WHITE)
            .background(Color::BLACK)
            .padding_h(10.0)
            .padding_v(5.0)
            .border_radius(4.0)
            .width(Length::Fixed(100.0));

        assert_eq!(btn.content, "OK");
        assert!(btn.on_press.is_some());
        assert_eq!(btn.size, Some(14.0));
        assert!(!btn.disabled);
    }

    #[test]
    fn disabled_button_clears_interaction() {
        let btn = TextButton::new("Disabled")
            .on_press(Msg::Clicked)
            .disabled(true);

        // When disabled, the view method will not set on_press.
        assert!(btn.disabled);
        // The on_press is set but will be ignored in view().
        assert!(btn.on_press.is_some());
    }

    #[test]
    fn builder_methods_are_independent() {
        let btn = TextButton::new("test")
            .text_color(Color::WHITE)
            .size(20.0)
            .on_press(Msg::Clicked);

        assert!(btn.text_color.is_some());
        assert!(btn.size.is_some());
        assert!(btn.on_press.is_some());
    }
}
