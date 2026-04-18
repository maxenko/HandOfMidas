//! Horizontal tab bar with underline indicator and optional count badges.
//!
//! [`Tabs`] renders a horizontal row of text tabs where exactly one is
//! "active" at a time. The active tab gets a colored underline beneath its
//! label; inactive tabs brighten their text on hover. Each tab can optionally
//! carry a small count badge next to its label (e.g. "Positions 3").
//!
//! Generic over `T` (the value type — typically a section enum) and `Message`.
//! Mirrors the [`crate::ButtonGroup`] API: parent-owned selection, builder
//! pattern, terminal `.view(theme)`.

use iced::alignment::Vertical;
use iced::widget::{button, column, container, row, text, Row, Space};
use iced::{Border, Color, Element, Length, Padding};

use crate::theme::UiTheme;

/// Vertical gap between the label and the underline bar.
const LABEL_TO_UNDERLINE_GAP: f32 = 4.0;

/// One item in a [`Tabs`] bar.
///
/// Construct with [`TabItem::new`], optionally chain [`TabItem::with_badge`]
/// to attach a count.
pub struct TabItem<'a, T> {
    /// Label text shown on the tab.
    label: &'a str,
    /// Value emitted when this tab is selected.
    value: T,
    /// Optional count badge displayed next to the label.
    badge: Option<usize>,
}

impl<'a, T> TabItem<'a, T> {
    /// Create a tab item with the given label and value.
    pub fn new(label: &'a str, value: T) -> Self {
        Self {
            label,
            value,
            badge: None,
        }
    }

    /// Attach a count badge to this tab (e.g. unread count).
    pub fn with_badge(mut self, count: usize) -> Self {
        self.badge = Some(count);
        self
    }

    /// Read back the badge value (used in tests; useful for callers too).
    pub fn badge(&self) -> Option<usize> {
        self.badge
    }
}

/// Horizontal row of tabs with an underline marking the active one.
///
/// # Example
///
/// ```ignore
/// #[derive(Clone, Copy, PartialEq)]
/// enum Section { Positions, Orders, History }
///
/// let tabs = Tabs::new(
///     vec![
///         TabItem::new("Positions", Section::Positions).with_badge(3),
///         TabItem::new("Orders", Section::Orders),
///         TabItem::new("Trade History", Section::History),
///     ],
///     model.section,
///     Message::SectionSelected,
/// )
/// .view(&ui_theme);
/// ```
pub struct Tabs<'a, T, Message> {
    items: Vec<TabItem<'a, T>>,
    selected: T,
    on_select: Box<dyn Fn(T) -> Message + 'a>,
    size: Option<f32>,
    padding_h: Option<f32>,
    padding_v: Option<f32>,
    spacing: Option<f32>,
    underline_height: Option<f32>,
}

impl<'a, T: PartialEq + Clone + 'a, Message: Clone + 'a> Tabs<'a, T, Message> {
    /// Create a new tab bar.
    ///
    /// - `items`: tab items (use [`TabItem::new`] / [`TabItem::with_badge`]).
    /// - `selected`: the currently active value.
    /// - `on_select`: closure mapping a tab value to a message.
    pub fn new(
        items: Vec<TabItem<'a, T>>,
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
            underline_height: None,
        }
    }

    /// Override font size for tab labels.
    pub fn size(mut self, size: f32) -> Self {
        self.size = Some(size);
        self
    }

    /// Override horizontal padding inside each tab.
    pub fn padding_h(mut self, px: f32) -> Self {
        self.padding_h = Some(px);
        self
    }

    /// Override vertical padding inside each tab.
    pub fn padding_v(mut self, px: f32) -> Self {
        self.padding_v = Some(px);
        self
    }

    /// Override spacing between adjacent tabs.
    pub fn spacing(mut self, px: f32) -> Self {
        self.spacing = Some(px);
        self
    }

    /// Override the active-tab underline height.
    pub fn underline_height(mut self, px: f32) -> Self {
        self.underline_height = Some(px);
        self
    }

    /// Number of items in this tab bar.
    pub fn item_count(&self) -> usize {
        self.items.len()
    }

    /// Render the tab bar using the given theme.
    pub fn view(self, theme: &UiTheme) -> Element<'a, Message> {
        let font_size = self.size.unwrap_or(theme.button_font_size);
        let pad_h = self.padding_h.unwrap_or(theme.button_padding_h);
        let pad_v = self.padding_v.unwrap_or(theme.button_padding_v);
        let spacing = self.spacing.unwrap_or(theme.tab_spacing);
        let underline_h = self.underline_height.unwrap_or(theme.tab_underline_height);

        let active_color = theme.tab_text_active;
        let inactive_color = theme.tab_text_inactive;
        let hover_color = theme.text_primary;
        let underline_color = theme.tab_underline;
        let badge_bg = theme.tab_badge_bg;
        let badge_text_color = theme.tab_badge_text;

        let buttons: Vec<Element<'a, Message>> = self
            .items
            .into_iter()
            .map(|item| {
                let is_selected = item.value == self.selected;
                let badge = item.badge;
                let label = item.label;
                let msg = (self.on_select)(item.value);

                // Label text — no .color(), so it inherits from the button's text_color
                // which the style closure varies per Status.
                let label_text = text(label).size(font_size);

                // Optional badge sits to the right of the label.
                let label_row: Element<'a, Message> = match badge {
                    None => label_text.into(),
                    Some(n) => {
                        let badge_size = (font_size - 2.0).max(8.0);
                        let badge_widget =
                            container(text(n.to_string()).size(badge_size).color(badge_text_color))
                                .padding([2.0, 6.0])
                                .style(move |_| container::Style {
                                    background: Some(badge_bg.into()),
                                    border: Border {
                                        radius: 4.0.into(),
                                        ..Default::default()
                                    },
                                    ..Default::default()
                                });
                        row![label_text, badge_widget]
                            .spacing(6.0)
                            .align_y(Vertical::Center)
                            .into()
                    }
                };

                // Underline — same height in both states; transparent when inactive
                // so layout is stable across selection changes.
                let this_underline_color = if is_selected {
                    underline_color
                } else {
                    Color::TRANSPARENT
                };
                let underline = container(Space::new())
                    .width(Length::Fill)
                    .height(Length::Fixed(underline_h))
                    .style(move |_| container::Style {
                        background: Some(this_underline_color.into()),
                        ..Default::default()
                    });

                // Inner column auto-shrinks to label_row width, so the underline's
                // Length::Fill resolves to label width — not the button's outer width
                // and not the row's full width.
                let inner = column![label_row, underline].spacing(LABEL_TO_UNDERLINE_GAP);

                // Transparent button background; text color shifts on hover for
                // inactive tabs (active tab keeps its color regardless).
                // Zero bottom padding so the underline sits flush with the
                // widget's bottom edge — parent can then butt the next
                // element right up against it.
                button(inner)
                    .width(Length::Shrink)
                    .padding(Padding {
                        top: pad_v,
                        right: pad_h,
                        bottom: 0.0,
                        left: pad_h,
                    })
                    .on_press(msg)
                    .style(move |_iced_theme, status| {
                        let text_color = if is_selected {
                            active_color
                        } else if matches!(status, button::Status::Hovered) {
                            hover_color
                        } else {
                            inactive_color
                        };
                        button::Style {
                            background: Some(Color::TRANSPARENT.into()),
                            text_color,
                            border: Border::default(),
                            ..Default::default()
                        }
                    })
                    .into()
            })
            .collect();

        // Left-align: wrap tabs in a Fill-wide row with a trailing Space
        // flex so the buttons hug the left edge regardless of what the
        // parent container does. Without this, some parent layouts cause
        // the tab buttons to spread evenly across the available width.
        row![
            Row::with_children(buttons)
                .spacing(spacing)
                .width(Length::Shrink),
            Space::new().width(Length::Fill),
        ]
        .width(Length::Fill)
        .align_y(Vertical::Bottom)
        .into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, PartialEq)]
    enum Section {
        Positions,
        Orders,
        History,
    }

    #[derive(Debug, Clone)]
    #[allow(dead_code)]
    enum Msg {
        Selected(Section),
    }

    #[test]
    fn tabs_constructs_without_panic() {
        let _tabs = Tabs::new(
            vec![
                TabItem::new("Positions", Section::Positions),
                TabItem::new("Orders", Section::Orders),
            ],
            Section::Positions,
            Msg::Selected,
        );
    }

    #[test]
    fn tabs_counts_items() {
        let tabs = Tabs::new(
            vec![
                TabItem::new("Positions", Section::Positions),
                TabItem::new("Orders", Section::Orders),
                TabItem::new("Trade History", Section::History),
            ],
            Section::Positions,
            Msg::Selected,
        );
        assert_eq!(tabs.item_count(), 3);
    }

    #[test]
    fn tabs_empty_items() {
        let tabs: Tabs<'_, Section, Msg> = Tabs::new(vec![], Section::Positions, Msg::Selected);
        assert_eq!(tabs.item_count(), 0);
    }

    #[test]
    fn tabs_builder_chains() {
        let tabs = Tabs::new(
            vec![TabItem::new("Positions", Section::Positions)],
            Section::Positions,
            Msg::Selected,
        )
        .size(13.0)
        .padding_h(12.0)
        .padding_v(8.0)
        .spacing(20.0)
        .underline_height(3.0);

        assert_eq!(tabs.size, Some(13.0));
        assert_eq!(tabs.padding_h, Some(12.0));
        assert_eq!(tabs.padding_v, Some(8.0));
        assert_eq!(tabs.spacing, Some(20.0));
        assert_eq!(tabs.underline_height, Some(3.0));
    }

    #[test]
    fn tab_item_constructs_without_badge() {
        let item = TabItem::new("Positions", Section::Positions);
        assert_eq!(item.badge, None);
        assert_eq!(item.label, "Positions");
    }

    #[test]
    fn tab_item_with_badge_records_count() {
        let item = TabItem::new("Positions", Section::Positions).with_badge(3);
        assert_eq!(item.badge, Some(3));
        assert_eq!(item.badge(), Some(3));
    }

    #[test]
    fn tabs_mixed_items_count() {
        let items = vec![
            TabItem::new("Positions", Section::Positions).with_badge(3),
            TabItem::new("Orders", Section::Orders),
            TabItem::new("History", Section::History).with_badge(0),
        ];
        let tabs = Tabs::new(items, Section::Orders, Msg::Selected);
        assert_eq!(tabs.item_count(), 3);
    }
}
