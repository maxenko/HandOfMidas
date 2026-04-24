//! Slice 4 of chart-transition: inline price-edit popup for level
//! annotations.
//!
//! The popup is a value-type (no widget-tree state beyond what iced's
//! `text_input` holds) — the host stores the current edit target in
//! its own state and passes that into [`LevelEditPopup::view`] each
//! frame.
//!
//! ## UX scope
//!
//! - One text field, "Price", that accepts a floating-point number.
//! - Commit on Enter → emit `Message::UpdateLevelPrice(id, price)`.
//! - Cancel on Escape / "Cancel" button → host closes the popup
//!   without emitting.
//! - Delete button → emit `Message::DeleteLevel(id)` so the
//!   context-menu "Delete" action can route through the same popup.
//!
//! The fuller editor (label / colour / thickness / lock toggle)
//! already lives in the legacy chart path
//! (`Message::LevelEditorPriceChanged` + siblings in `app.rs`); this
//! popup is deliberately slimmer — an inline nudge for the new
//! session-chart stack. A richer editor lands in the slice-5a
//! decorator port.

#![cfg(feature = "session_chart")]

use iced::widget::{button, column, row, text, text_input};
use iced::{Element, Length};

/// Per-popup state: the id + current text-field buffer. The parent
/// widget owns one instance behind an `Option<LevelEditPopup>` that it
/// sets on `OpenContextMenu { Edit }` and clears on commit / cancel.
#[derive(Clone, Debug, PartialEq)]
pub struct LevelEditPopup {
    /// The level annotation being edited.
    pub annotation_id: u64,
    /// The symbol the level belongs to — needed to route the commit
    /// back through `AnnotationStore::update_level(symbol, id, ...)`.
    pub symbol: String,
    /// Text-field buffer. Starts as the current price rendered to 4
    /// decimals; mutates on user input.
    pub price_text: String,
}

impl LevelEditPopup {
    /// Build a fresh popup targeting `(symbol, id)` with the current
    /// price pre-filled in the text field.
    pub fn new(annotation_id: u64, symbol: impl Into<String>, current_price: f64) -> Self {
        Self {
            annotation_id,
            symbol: symbol.into(),
            price_text: format!("{current_price:.4}"),
        }
    }

    /// Parse the current text-field buffer. Returns `None` on invalid
    /// input so the host can render a disabled commit button.
    pub fn parsed_price(&self) -> Option<f64> {
        self.price_text.trim().parse::<f64>().ok()
    }

    /// Build the iced element. Message constructors are generic over
    /// the host's `Message` type so unit tests can parameterise and
    /// the app binary can pass its real `crate::app::Message`.
    pub fn view<'a, M: 'a + Clone>(
        &'a self,
        on_text_change: impl 'a + Fn(String) -> M,
        on_commit: impl 'a + Fn(u64, f64) -> M,
        on_delete: impl 'a + Fn(u64) -> M,
        on_cancel: M,
    ) -> Element<'a, M> {
        let id = self.annotation_id;
        let commit_enabled = self.parsed_price().is_some();

        let price_field = text_input("price", &self.price_text)
            .on_input(on_text_change)
            .padding(4)
            .width(Length::Fixed(120.0));

        let mut commit_btn = button(text("OK").size(11)).padding([2, 8]);
        if commit_enabled {
            let price = self.parsed_price().expect("checked by `commit_enabled`");
            commit_btn = commit_btn.on_press(on_commit(id, price));
        }

        let cancel_btn = button(text("Cancel").size(11))
            .on_press(on_cancel)
            .padding([2, 8]);

        let delete_btn = button(text("Delete").size(11))
            .on_press(on_delete(id))
            .padding([2, 8]);

        column![
            text(format!("Edit level #{id}")).size(12),
            row![text("Price").size(11), price_field].spacing(6),
            row![commit_btn, cancel_btn, delete_btn].spacing(6),
        ]
        .spacing(6)
        .padding(8)
        .into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_formats_price_to_four_decimals() {
        let p = LevelEditPopup::new(1, "AAPL", 100.1);
        assert_eq!(p.price_text, "100.1000");
    }

    #[test]
    fn parsed_price_round_trips() {
        let p = LevelEditPopup::new(1, "AAPL", 100.12345);
        // Stored format is 4 decimals; parsed back is 100.1235.
        assert_eq!(p.parsed_price(), Some(100.1235));
    }

    #[test]
    fn parsed_price_rejects_non_numeric() {
        let mut p = LevelEditPopup::new(1, "AAPL", 100.0);
        p.price_text = "abc".into();
        assert!(p.parsed_price().is_none());
    }

    #[test]
    fn parsed_price_trims_whitespace() {
        let mut p = LevelEditPopup::new(1, "AAPL", 100.0);
        p.price_text = "  101.5  ".into();
        assert_eq!(p.parsed_price(), Some(101.5));
    }

    #[test]
    fn popup_carries_symbol() {
        let p = LevelEditPopup::new(42, "aapl", 100.0);
        assert_eq!(p.symbol, "aapl");
        assert_eq!(p.annotation_id, 42);
    }
}
