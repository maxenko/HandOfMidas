//! `column_selector_popup` — reusable show/hide column checklist popup.
//!
//! Returns only the popup container. The caller is responsible for:
//! - pushing a full-surface backdrop layer that emits `on_dismiss` on press
//! - aligning / padding this popup (e.g. top-right with `[4, 6]` padding)
//!
//! See [`column_selector_popup`] for the call shape.

use std::collections::HashSet;

use iced::widget::{button, column as iced_column, container, row, text};
use iced::{Color, Element};

use crate::column::ColumnId;

/// One entry in the column-selector popup.
///
/// Mandatory entries render without an `on_press` handler and are greyed
/// out — the box is shown as checked but clicking is a no-op. This makes
/// illegal state (a mandatory column that's somehow hidden) unrepresentable
/// at the API surface.
#[derive(Debug, Clone, Copy)]
pub struct ColumnEntry<'a> {
    /// Stable identifier for the column.
    pub id: ColumnId,
    /// Human-readable label shown in the popup.
    pub label: &'a str,
    /// If `true`, the entry renders as non-interactive and always-checked.
    pub mandatory: bool,
}

/// Build the column-selector popup.
///
/// Returns only the dark checklist panel itself (not a backdrop). The
/// caller pushes a backdrop layer beneath it that emits `on_dismiss`.
///
/// `entries` provides the order of checklist rows. `hidden` is the set
/// of currently-hidden column IDs; an entry is shown as checked when
/// `entry.mandatory || !hidden.contains(&entry.id)`.
///
/// `on_toggle` is invoked with the column ID when a non-mandatory row
/// is clicked; the app typically toggles membership of `hidden` in its
/// `update()`.
pub fn column_selector_popup<'a, M, F>(
    entries: &[ColumnEntry<'_>],
    hidden: &HashSet<ColumnId>,
    on_toggle: F,
    _on_dismiss: M,
) -> Element<'a, M>
where
    M: Clone + 'a,
    F: Fn(ColumnId) -> M + 'a,
{
    // Muted grey for mandatory entries. Keeps them visually distinct
    // from the active checklist rows.
    const MUTED: Color = Color::from_rgb(0.55, 0.55, 0.60);

    let mut list = iced_column![].spacing(2).padding(8);

    for entry in entries {
        let is_mandatory = entry.mandatory;
        let checked = is_mandatory || !hidden.contains(&entry.id);
        let check_mark = if checked { "\u{2611}" } else { "\u{2610}" };

        let row_el: Element<'a, M> = if is_mandatory {
            // Mandatory — render greyed, no press handler, no mouse
            // interaction change. Matches today's blotter Symbol row.
            container(
                row![
                    text(check_mark.to_string()).size(14).color(MUTED),
                    text(entry.label.to_string()).size(12).color(MUTED),
                ]
                .spacing(6)
                .align_y(iced::Alignment::Center),
            )
            .padding([4, 8])
            .into()
        } else {
            // Interactive — use a `button` widget so iced correctly
            // consumes the press event (the root cause of the popup-
            // clickable bug when using `mouse_area` on top of a
            // backdrop `mouse_area`).
            let inner = row![
                text(check_mark.to_string()).size(14),
                text(entry.label.to_string()).size(12),
            ]
            .spacing(6)
            .align_y(iced::Alignment::Center);

            button(inner)
                .on_press(on_toggle(entry.id))
                .padding([4, 8])
                .style(popup_row_button_style)
                .into()
        };

        list = list.push(row_el);
    }

    container(list)
        .style(|_| container::Style {
            background: Some(Color::from_rgb(0.12, 0.13, 0.16).into()),
            border: iced::Border {
                color: Color::from_rgba(1.0, 1.0, 1.0, 0.15),
                width: 1.0,
                radius: 4.0.into(),
            },
            ..Default::default()
        })
        .padding(2)
        .into()
}

/// Transparent-by-default button style for popup rows.
///
/// Shows a subtle hover/press background but no border so the rows
/// read as checklist entries, not chunky buttons.
fn popup_row_button_style(
    _theme: &iced::Theme,
    status: iced::widget::button::Status,
) -> iced::widget::button::Style {
    use iced::widget::button;
    let background = match status {
        button::Status::Hovered => Some(iced::Background::Color(Color::from_rgba(
            1.0, 1.0, 1.0, 0.06,
        ))),
        button::Status::Pressed => Some(iced::Background::Color(Color::from_rgba(
            1.0, 1.0, 1.0, 0.10,
        ))),
        _ => None,
    };
    button::Style {
        background,
        text_color: Color::from_rgb(0.88, 0.88, 0.92),
        border: iced::Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: 2.0.into(),
        },
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const COL_A: ColumnId = ColumnId("a");
    const COL_B: ColumnId = ColumnId("b");
    const COL_C: ColumnId = ColumnId("c");

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum TestMsg {
        Toggle(ColumnId),
        Dismiss,
    }

    fn entries() -> Vec<ColumnEntry<'static>> {
        vec![
            ColumnEntry {
                id: COL_A,
                label: "Alpha",
                mandatory: true,
            },
            ColumnEntry {
                id: COL_B,
                label: "Beta",
                mandatory: false,
            },
            ColumnEntry {
                id: COL_C,
                label: "Gamma",
                mandatory: false,
            },
        ]
    }

    #[test]
    fn builds_with_no_hidden_columns() {
        let entries = entries();
        let hidden: HashSet<ColumnId> = HashSet::new();
        let _el: Element<'_, TestMsg> =
            column_selector_popup(&entries, &hidden, TestMsg::Toggle, TestMsg::Dismiss);
    }

    #[test]
    fn builds_with_all_hidden() {
        let entries = entries();
        let mut hidden: HashSet<ColumnId> = HashSet::new();
        // Mandatory COL_A stays checked regardless of the hidden set —
        // ensures the helper doesn't crash on "everything hidden".
        hidden.insert(COL_A);
        hidden.insert(COL_B);
        hidden.insert(COL_C);
        let _el: Element<'_, TestMsg> =
            column_selector_popup(&entries, &hidden, TestMsg::Toggle, TestMsg::Dismiss);
    }

    #[test]
    fn builds_with_mandatory_only() {
        let entries = vec![ColumnEntry {
            id: COL_A,
            label: "Alpha",
            mandatory: true,
        }];
        let hidden: HashSet<ColumnId> = HashSet::new();
        let _el: Element<'_, TestMsg> =
            column_selector_popup(&entries, &hidden, TestMsg::Toggle, TestMsg::Dismiss);
    }

    #[test]
    fn builds_without_panic_across_hidden_shapes() {
        // "popup-not-open" doesn't apply at the helper level — the helper
        // is always called when the popup is open. This is the "builds
        // cleanly across input shapes" regression net.
        let entries = entries();
        for hidden in [
            HashSet::new(),
            HashSet::from([COL_B]),
            HashSet::from([COL_B, COL_C]),
        ] {
            let _el: Element<'_, TestMsg> =
                column_selector_popup(&entries, &hidden, TestMsg::Toggle, TestMsg::Dismiss);
        }
    }
}
