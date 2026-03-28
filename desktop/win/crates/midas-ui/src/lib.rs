//! Minimal custom widget library for the Hand of Midas charting application.
//!
//! All widgets are built on iced 0.14 primitives and share a [`UiTheme`] for
//! consistent styling across the UI. Each widget follows the builder pattern:
//! construct with `::new()`, customize with chainable methods, then call
//! `.view(theme)` to produce an iced [`Element`].
//!
//! # Widgets
//!
//! - [`Label`] -- Static text display with theme-driven defaults.
//! - [`EditableLabel`] -- Inline-editable label (click to edit, Enter to confirm).
//! - [`TextButton`] -- Themed text button with 4 visual states.
//! - [`IconButton`] -- Unicode icon button, transparent at rest.
//! - [`ButtonGroup`] -- Horizontal toggle group of buttons.
//! - [`Tooltip`] -- Theme-styled tooltip wrapper.

pub mod button;
pub mod button_group;
pub mod editable_label;
pub mod icon_button;
pub mod label;
pub mod theme;
pub mod tooltip;

pub use button::TextButton;
pub use button_group::ButtonGroup;
pub use editable_label::EditableLabel;
pub use icon_button::IconButton;
pub use label::Label;
pub use theme::UiTheme;
pub use tooltip::Tooltip;
