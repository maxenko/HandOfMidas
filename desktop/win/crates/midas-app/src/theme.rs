//! Dark theme color constants for the Hand of Midas UI.
//!
//! These constants define the color palette used throughout the application.
//! The palette is inspired by professional trading terminals: dark backgrounds,
//! muted borders, and high-contrast text.
//!
//! Some constants are defined here for Phase 4b (shader widget) and beyond;
//! they are intentionally unused in Phase 4a.

#![allow(dead_code)]

use iced::Color;

/// Background color for the main window.
pub const BACKGROUND: Color = Color::from_rgb(0.09, 0.09, 0.11);

/// Background color for the toolbar area.
pub const TOOLBAR_BG: Color = Color::from_rgb(0.12, 0.12, 0.15);

/// Background color for the status bar.
pub const STATUS_BAR_BG: Color = Color::from_rgb(0.10, 0.10, 0.12);

/// Background color for an empty chart panel (no data loaded).
pub const CHART_EMPTY_BG: Color = Color::from_rgb(0.08, 0.10, 0.14);

/// Background color for a chart panel with data loaded.
pub const CHART_LOADED_BG: Color = Color::from_rgb(0.06, 0.08, 0.12);

/// Background color for the active (focused) chart panel.
pub const CHART_ACTIVE_BORDER: Color = Color::from_rgb(0.22, 0.45, 0.85);

/// Border color for inactive chart panels.
pub const CHART_INACTIVE_BORDER: Color = Color::from_rgb(0.20, 0.20, 0.25);

/// Primary text color (high contrast).
pub const TEXT_PRIMARY: Color = Color::from_rgb(0.88, 0.88, 0.92);

/// Secondary text color (lower emphasis).
pub const TEXT_SECONDARY: Color = Color::from_rgb(0.55, 0.55, 0.60);

/// Muted text color (lowest emphasis, e.g. placeholders).
pub const TEXT_MUTED: Color = Color::from_rgb(0.35, 0.35, 0.40);

/// Accent color for active/selected elements.
pub const ACCENT: Color = Color::from_rgb(0.22, 0.55, 0.95);

/// Button background (default state).
pub const BUTTON_BG: Color = Color::from_rgb(0.16, 0.16, 0.20);

/// Button background (hovered state).
pub const BUTTON_HOVER_BG: Color = Color::from_rgb(0.22, 0.22, 0.28);

/// Button background (active/selected state).
pub const BUTTON_ACTIVE_BG: Color = Color::from_rgb(0.18, 0.35, 0.65);

/// Bullish candle color (green).
pub const CANDLE_BULL: Color = Color::from_rgb(0.10, 0.75, 0.40);

/// Bearish candle color (red).
pub const CANDLE_BEAR: Color = Color::from_rgb(0.85, 0.20, 0.25);

/// Success/positive status color.
pub const STATUS_OK: Color = Color::from_rgb(0.15, 0.70, 0.40);

/// Error/negative status color.
pub const STATUS_ERROR: Color = Color::from_rgb(0.85, 0.20, 0.25);

/// Warning status color.
pub const STATUS_WARN: Color = Color::from_rgb(0.90, 0.70, 0.15);
