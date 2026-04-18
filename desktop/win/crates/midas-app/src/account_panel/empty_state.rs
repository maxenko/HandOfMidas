//! Shared empty-state placeholder used by Account-panel tabs while
//! their real content waits on later slices (Positions / History /
//! Recents). Keeping it here keeps Slice 1 self-contained; Slice 6
//! can re-theme it globally without touching the tab views.

use iced::widget::{container, text};
use iced::{Color, Element, Fill};

use midas_ui::UiTheme;

/// Centered, muted placeholder label.
///
/// `Message` is generic so each tab's own message type can flow
/// through — the placeholder itself never emits anything.
pub fn empty_state<'a, Message: 'a>(msg: &'a str, theme: &UiTheme) -> Element<'a, Message> {
    // Muted = 40% alpha over the themed primary text colour.
    let muted = Color {
        a: 0.40,
        ..theme.text_primary
    };
    container(text(msg).size(14).color(muted))
        .center_x(Fill)
        .center_y(Fill)
        .into()
}
