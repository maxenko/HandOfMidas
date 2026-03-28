//! Static text label with theme-driven defaults.
//!
//! [`Label`] is a thin builder around `iced::widget::text` that reads font
//! size and color from a [`UiTheme`] by default, with chainable overrides.

use iced::widget::text;
use iced::{Color, Element};

use crate::theme::UiTheme;

/// Static text label with theme-driven defaults.
///
/// Wraps `iced::widget::text` with builder methods for size, color, and
/// font weight. Reads defaults from the provided [`UiTheme`].
///
/// # Example
///
/// ```ignore
/// let label = Label::new("AAPL")
///     .size(14.0)
///     .bold()
///     .view(&ui_theme);
/// ```
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

    /// Convert to an iced [`Element`] using the given theme for defaults.
    pub fn view<Message: 'a>(self, theme: &UiTheme) -> Element<'a, Message> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn label_constructs_without_panic() {
        let _label = Label::new("test");
    }

    #[test]
    fn label_builder_methods_chain() {
        let label = Label::new("AAPL").size(14.0).color(Color::WHITE).bold();

        assert_eq!(label.content, "AAPL");
        assert_eq!(label.size, Some(14.0));
        assert!(label.bold);
    }

    #[test]
    fn label_defaults_are_none() {
        let label = Label::new("hello");
        assert!(label.size.is_none());
        assert!(label.color.is_none());
        assert!(!label.bold);
    }

    #[test]
    fn label_builder_methods_are_independent() {
        // Setting size should not reset color.
        let label = Label::new("test").color(Color::WHITE).size(20.0);
        assert!(label.color.is_some());
        assert!(label.size.is_some());
    }
}
