//! Theme-aware icon button using Unicode characters as icons.
//!
//! [`IconButton`] renders a single Unicode character (or short string) inside a
//! square-ish button area. Transparent at rest, visible background on hover.
//! Same four visual states as [`TextButton`](crate::TextButton).

use iced::widget::{button, text};
use iced::{Color, Element};

use crate::theme::UiTheme;
use crate::tooltip::Tooltip;

/// Theme-aware icon button using Unicode characters as icons.
///
/// Identical state machine to [`TextButton`](crate::TextButton) but renders a
/// single character/glyph centered in a square-ish button area. Default
/// background is transparent, becoming visible on hover.
///
/// # Example
///
/// ```ignore
/// let close_btn = IconButton::new("\u{00D7}")
///     .on_press(Message::PaneClose(pane))
///     .icon_size(12.0)
///     .tooltip("Close panel")
///     .view(&ui_theme);
/// ```
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
    /// Optional tooltip text.
    tooltip_text: Option<&'a str>,
}

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

    /// Attach tooltip text (rendered via the [`Tooltip`] wrapper).
    pub fn tooltip(mut self, text: &'a str) -> Self {
        self.tooltip_text = Some(text);
        self
    }
}

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
                    ..Default::default()
                }
            });

        if !self.disabled {
            if let Some(msg) = self.on_press {
                btn = btn.on_press(msg);
            }
        }

        let element: Element<'a, Message> = btn.into();

        // If tooltip text was provided, wrap with Tooltip.
        if let Some(tip) = self.tooltip_text {
            Tooltip::new(element, tip).view(theme)
        } else {
            element
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone)]
    enum Msg {
        Close,
    }

    #[test]
    fn icon_button_constructs_without_panic() {
        let _btn = IconButton::<Msg>::new("\u{00D7}");
    }

    #[test]
    fn icon_button_builder_chains() {
        let btn = IconButton::new("\u{00D7}")
            .on_press(Msg::Close)
            .icon_size(12.0)
            .icon_color(Color::WHITE)
            .background(Color::BLACK)
            .padding(6.0)
            .border_radius(2.0)
            .tooltip("Close");

        assert_eq!(btn.icon, "\u{00D7}");
        assert!(btn.on_press.is_some());
        assert_eq!(btn.icon_size, Some(12.0));
        assert_eq!(btn.tooltip_text, Some("Close"));
        assert!(!btn.disabled);
    }

    #[test]
    fn disabled_icon_button() {
        let btn = IconButton::new("+").on_press(Msg::Close).disabled(true);

        assert!(btn.disabled);
    }

    #[test]
    fn builder_methods_are_independent() {
        let btn = IconButton::new("x")
            .icon_color(Color::WHITE)
            .icon_size(20.0)
            .on_press(Msg::Close);

        assert!(btn.icon_color.is_some());
        assert!(btn.icon_size.is_some());
        assert!(btn.on_press.is_some());
    }
}
