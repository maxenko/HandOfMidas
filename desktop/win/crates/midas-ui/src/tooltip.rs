//! Theme-styled tooltip wrapper.
//!
//! [`Tooltip`] wraps any iced `Element` and shows a text popup on hover.
//! It delegates to `iced::widget::tooltip` with colors from [`UiTheme`].

use iced::widget::{container, text, tooltip::Position};
use iced::Element;

use crate::theme::UiTheme;

/// Theme-styled tooltip wrapper.
///
/// Wraps any [`Element`] and shows a text popup on hover. Delegates to
/// `iced::widget::tooltip` with colors from [`UiTheme`].
///
/// # Example
///
/// ```ignore
/// let with_tip = Tooltip::new(some_element, "Help text")
///     .position(Position::Bottom)
///     .view(&ui_theme);
/// ```
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tooltip_default_position_is_bottom() {
        let content: Element<'_, ()> = iced::widget::text("inner").into();
        let tt = Tooltip::new(content, "tip");
        // Position::Bottom is the default.
        assert!(matches!(tt.position, Position::Bottom));
    }

    #[test]
    fn tooltip_builder_chains() {
        let content: Element<'_, ()> = iced::widget::text("inner").into();
        let tt = Tooltip::new(content, "help")
            .position(Position::Top)
            .gap(8.0);

        assert!(matches!(tt.position, Position::Top));
        assert_eq!(tt.gap, Some(8.0));
    }
}
