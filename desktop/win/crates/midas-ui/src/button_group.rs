//! Horizontal toggle group of text buttons.
//!
//! [`ButtonGroup`] renders a horizontal row of buttons where exactly one is
//! "selected" at a time. The selected button has a distinct background and text
//! color. Generic over the value type `T` (e.g. `Timeframe`).

use iced::widget::{button, text, Row};
use iced::Element;

use crate::theme::UiTheme;

/// Horizontal toggle group of text buttons.
///
/// Exactly one button is visually "selected" at a time. Pressing any
/// button emits a message carrying the selected item's value.
///
/// Generic over `T` which is the value type (e.g. `Timeframe`).
///
/// # Example
///
/// ```ignore
/// let tf_group = ButtonGroup::new(
///     vec![("1m", Timeframe::M1), ("5m", Timeframe::M5)],
///     panel.timeframe,
///     move |tf| Message::TimeframeSelected(chart_id, tf),
/// )
/// .size(12.0)
/// .view(&ui_theme);
/// ```
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
    /// Horizontal padding override.
    padding_h: Option<f32>,
    /// Vertical padding override.
    padding_v: Option<f32>,
    /// Spacing between buttons.
    spacing: Option<f32>,
}

impl<'a, T: PartialEq + Clone + 'a, Message: Clone + 'a> ButtonGroup<'a, T, Message> {
    /// Create a new button group.
    ///
    /// - `items`: vector of (label, value) pairs.
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
                    theme.button_selected
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
                            ..Default::default()
                        }
                    })
                    .into()
            })
            .collect();

        Row::with_children(buttons).spacing(spacing).into()
    }

    /// Return the number of items in this button group.
    pub fn item_count(&self) -> usize {
        self.items.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, PartialEq)]
    enum Tf {
        M1,
        M5,
        D1,
    }

    #[derive(Debug, Clone)]
    #[allow(dead_code)] // Tf payload used structurally by ButtonGroup; not destructured in tests
    enum Msg {
        Selected(Tf),
    }

    #[test]
    fn button_group_constructs_without_panic() {
        let _group = ButtonGroup::new(
            vec![("1m", Tf::M1), ("5m", Tf::M5), ("1D", Tf::D1)],
            Tf::D1,
            Msg::Selected,
        );
    }

    #[test]
    fn button_group_counts_items() {
        let group = ButtonGroup::new(
            vec![("1m", Tf::M1), ("5m", Tf::M5), ("1D", Tf::D1)],
            Tf::D1,
            Msg::Selected,
        );
        assert_eq!(group.item_count(), 3);
    }

    #[test]
    fn button_group_empty_items() {
        let group: ButtonGroup<'_, Tf, Msg> = ButtonGroup::new(vec![], Tf::D1, Msg::Selected);
        assert_eq!(group.item_count(), 0);
    }

    #[test]
    fn button_group_builder_chains() {
        let group = ButtonGroup::new(vec![("1m", Tf::M1), ("5m", Tf::M5)], Tf::M1, |tf| {
            Msg::Selected(tf)
        })
        .size(10.0)
        .padding_h(6.0)
        .padding_v(2.0)
        .spacing(2.0);

        assert_eq!(group.size, Some(10.0));
        assert_eq!(group.padding_h, Some(6.0));
        assert_eq!(group.padding_v, Some(2.0));
        assert_eq!(group.spacing, Some(2.0));
        assert_eq!(group.item_count(), 2);
    }
}
