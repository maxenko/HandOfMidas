//! Centralized theme struct controlling all midas-ui widget colors and spacing.
//!
//! The [`UiTheme`] struct holds every color, padding, font size, and border
//! radius used by the widgets in this crate. Construct one via [`Default`] for
//! the dark trading-terminal palette, or build a custom instance for alternate
//! themes.

use iced::Color;

/// Centralized theme controlling all midas-ui widget colors and spacing.
///
/// Constructed once and passed by reference to widget constructors. The
/// [`Default`] implementation returns the dark trading-terminal palette
/// matching `midas-app/src/theme.rs`.
#[derive(Debug, Clone)]
pub struct UiTheme {
    // -- Text colors --
    /// Primary text (high contrast). Used for labels, active button text.
    pub text_primary: Color,
    /// Secondary text (lower emphasis). Unfocused titles, descriptions.
    pub text_secondary: Color,
    /// Muted text (lowest emphasis). Placeholders, disabled text.
    pub text_muted: Color,

    // -- Surface colors --
    /// Background for elevated surfaces (title bars, toolbars).
    pub surface: Color,
    /// Background for secondary surfaces (status bar, unfocused title bars).
    pub surface_dim: Color,

    // -- Button colors --
    /// Default button background.
    pub button_bg: Color,
    /// Hovered button background.
    pub button_hover: Color,
    /// Pressed button background.
    pub button_pressed: Color,
    /// Selected/active button background (used in ButtonGroup).
    pub button_selected: Color,
    /// Disabled button background.
    pub button_disabled: Color,
    /// Default button text color.
    pub button_text: Color,
    /// Selected button text color.
    pub button_selected_text: Color,

    // -- Accent --
    /// Accent color for focused elements, active borders.
    pub accent: Color,

    // -- Editable label --
    /// Subtle hover background to hint editability.
    pub editable_hover_bg: Color,
    /// Border color for the editing state text input.
    pub editable_border: Color,

    // -- Tooltip --
    /// Tooltip background color.
    pub tooltip_bg: Color,
    /// Tooltip text color.
    pub tooltip_text: Color,

    // -- Tabs --
    /// Color of the underline beneath the active tab.
    pub tab_underline: Color,
    /// Height of the active-tab underline in logical pixels.
    pub tab_underline_height: f32,
    /// Text color for the active tab label.
    pub tab_text_active: Color,
    /// Text color for inactive tab labels.
    pub tab_text_inactive: Color,
    /// Background color for the count badge next to a tab label.
    pub tab_badge_bg: Color,
    /// Text color inside the count badge.
    pub tab_badge_text: Color,
    /// Spacing in logical pixels between adjacent tabs.
    pub tab_spacing: f32,

    // -- Spacing (in logical pixels) --
    /// Default horizontal padding inside buttons.
    pub button_padding_h: f32,
    /// Default vertical padding inside buttons.
    pub button_padding_v: f32,
    /// Default border radius for buttons.
    pub button_border_radius: f32,
    /// Spacing between items in a ButtonGroup.
    pub button_group_spacing: f32,
    /// Default font size for button text.
    pub button_font_size: f32,
    /// Default font size for labels.
    pub label_font_size: f32,
    /// Default font size for tooltip text.
    pub tooltip_font_size: f32,
    /// Tooltip show delay in milliseconds (reserved for future use).
    pub tooltip_delay_ms: u64,
}

impl Default for UiTheme {
    fn default() -> Self {
        Self {
            // Text -- matches midas-app/src/theme.rs constants
            text_primary: Color::from_rgb(0.88, 0.88, 0.92),
            text_secondary: Color::from_rgb(0.55, 0.55, 0.60),
            text_muted: Color::from_rgb(0.35, 0.35, 0.40),

            // Surfaces
            surface: Color::from_rgb(0.12, 0.12, 0.15),
            surface_dim: Color::from_rgb(0.10, 0.10, 0.12),

            // Buttons
            button_bg: Color::from_rgb(0.16, 0.16, 0.20),
            button_hover: Color::from_rgb(0.22, 0.22, 0.28),
            button_pressed: Color::from_rgb(0.12, 0.12, 0.16),
            button_selected: Color::from_rgb(0.18, 0.35, 0.65),
            button_disabled: Color::from_rgb(0.13, 0.13, 0.16),
            button_text: Color::from_rgb(0.88, 0.88, 0.92),
            button_selected_text: Color::WHITE,

            // Accent
            accent: Color::from_rgb(0.22, 0.55, 0.95),

            // Editable label
            editable_hover_bg: Color::from_rgba(1.0, 1.0, 1.0, 0.05),
            editable_border: Color::from_rgb(0.22, 0.55, 0.95),

            // Tooltip
            tooltip_bg: Color::from_rgb(0.20, 0.20, 0.24),
            tooltip_text: Color::from_rgb(0.88, 0.88, 0.92),

            // Tabs
            tab_underline: Color::from_rgb(0.22, 0.55, 0.95),
            tab_underline_height: 2.0,
            tab_text_active: Color::from_rgb(0.88, 0.88, 0.92),
            tab_text_inactive: Color::from_rgb(0.55, 0.55, 0.60),
            tab_badge_bg: Color::from_rgb(0.18, 0.18, 0.22),
            tab_badge_text: Color::from_rgb(0.70, 0.70, 0.75),
            tab_spacing: 16.0,

            // Spacing
            button_padding_h: 8.0,
            button_padding_v: 4.0,
            button_border_radius: 3.0,
            button_group_spacing: 1.0,
            label_font_size: 13.0,
            button_font_size: 12.0,
            tooltip_font_size: 11.0,
            tooltip_delay_ms: 500,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_creates_valid_theme() {
        let theme = UiTheme::default();

        // All color components should be in [0.0, 1.0].
        let colors = [
            theme.text_primary,
            theme.text_secondary,
            theme.text_muted,
            theme.surface,
            theme.surface_dim,
            theme.button_bg,
            theme.button_hover,
            theme.button_pressed,
            theme.button_selected,
            theme.button_disabled,
            theme.button_text,
            theme.button_selected_text,
            theme.accent,
            theme.editable_hover_bg,
            theme.editable_border,
            theme.tooltip_bg,
            theme.tooltip_text,
            theme.tab_underline,
            theme.tab_text_active,
            theme.tab_text_inactive,
            theme.tab_badge_bg,
            theme.tab_badge_text,
        ];
        for color in &colors {
            assert!(
                (0.0..=1.0).contains(&color.r),
                "red component out of range: {}",
                color.r
            );
            assert!(
                (0.0..=1.0).contains(&color.g),
                "green component out of range: {}",
                color.g
            );
            assert!(
                (0.0..=1.0).contains(&color.b),
                "blue component out of range: {}",
                color.b
            );
            assert!(
                (0.0..=1.0).contains(&color.a),
                "alpha component out of range: {}",
                color.a
            );
        }
    }

    #[test]
    fn default_spacing_values_are_positive() {
        let theme = UiTheme::default();
        assert!(theme.button_padding_h > 0.0);
        assert!(theme.button_padding_v > 0.0);
        assert!(theme.button_border_radius > 0.0);
        assert!(theme.button_group_spacing > 0.0);
        assert!(theme.button_font_size > 0.0);
        assert!(theme.label_font_size > 0.0);
        assert!(theme.tooltip_font_size > 0.0);
        assert!(theme.tooltip_delay_ms > 0);
        assert!(theme.tab_underline_height > 0.0);
        assert!(theme.tab_spacing > 0.0);
    }

    #[test]
    fn theme_is_cloneable() {
        let theme = UiTheme::default();
        let cloned = theme.clone();
        assert_eq!(cloned.text_primary.r, theme.text_primary.r);
        assert_eq!(cloned.button_font_size, theme.button_font_size);
    }
}
