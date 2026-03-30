//! Horizontal price levels.
//!
//! A `HorizontalLevel` represents a user-drawn horizontal line at a specific
//! price. These are persisted with the chart state and serialized to config.

/// Icon displayed next to a level label on the chart.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum LevelIcon {
    /// No icon.
    #[default]
    None,
    /// Upward arrow (bullish signal).
    ArrowUp,
    /// Downward arrow (bearish signal).
    ArrowDown,
    /// Star marker.
    Star,
    /// Flag marker.
    Flag,
    /// Warning/alert marker.
    Warning,
}

impl LevelIcon {
    /// Unicode character representation for rendering on the chart.
    pub fn as_char(&self) -> Option<char> {
        match self {
            LevelIcon::None => None,
            LevelIcon::ArrowUp => Some('\u{25B2}'),   // ▲
            LevelIcon::ArrowDown => Some('\u{25BC}'), // ▼
            LevelIcon::Star => Some('\u{2726}'),      // ✦
            LevelIcon::Flag => Some('\u{2691}'),      // ⚑
            LevelIcon::Warning => Some('\u{26A0}'),   // ⚠
        }
    }

    /// Display name for the icon (used in UI selectors).
    pub fn display_name(&self) -> &'static str {
        match self {
            LevelIcon::None => "None",
            LevelIcon::ArrowUp => "Arrow Up",
            LevelIcon::ArrowDown => "Arrow Down",
            LevelIcon::Star => "Star",
            LevelIcon::Flag => "Flag",
            LevelIcon::Warning => "Warning",
        }
    }

    /// All available icon variants for UI selection.
    pub fn all() -> &'static [LevelIcon] {
        &[
            LevelIcon::None,
            LevelIcon::ArrowUp,
            LevelIcon::ArrowDown,
            LevelIcon::Star,
            LevelIcon::Flag,
            LevelIcon::Warning,
        ]
    }

    /// Convert from a string identifier (used in config persistence).
    pub fn from_str_id(s: &str) -> Self {
        match s {
            "arrow_up" => LevelIcon::ArrowUp,
            "arrow_down" => LevelIcon::ArrowDown,
            "star" => LevelIcon::Star,
            "flag" => LevelIcon::Flag,
            "warning" => LevelIcon::Warning,
            _ => LevelIcon::None,
        }
    }

    /// Convert to a string identifier (used in config persistence).
    pub fn to_str_id(&self) -> &'static str {
        match self {
            LevelIcon::None => "none",
            LevelIcon::ArrowUp => "arrow_up",
            LevelIcon::ArrowDown => "arrow_down",
            LevelIcon::Star => "star",
            LevelIcon::Flag => "flag",
            LevelIcon::Warning => "warning",
        }
    }
}

/// Compute a smart price step size based on the current price level.
///
/// Returns `(coarse_step, fine_step)` where coarse is for arrow key clicks
/// and fine is for Shift+arrow or scroll wheel.
pub fn price_step_for(price: f64) -> (f64, f64) {
    if price.abs() >= 200.0 {
        (0.05, 0.05)
    } else {
        (0.01, 0.01)
    }
}

/// A user-defined horizontal price level.
///
/// Represents a horizontal line drawn at a specific price on the chart.
/// Supports serialization for persistence across sessions.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct HorizontalLevel {
    /// Unique identifier for this level within the chart.
    pub id: u64,
    /// Price at which the horizontal line is drawn.
    pub price: f64,
    /// RGBA color of the line (linear space).
    pub color: [f32; 4],
    /// Line width in logical pixels.
    pub line_width: f32,
    /// Optional text label displayed on the chart next to the line.
    #[serde(default)]
    pub label: Option<String>,
    /// Optional icon displayed next to the label.
    #[serde(default)]
    pub icon: LevelIcon,
    /// Whether this level is locked (prevents drag and delete).
    #[serde(default)]
    pub locked: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_clone_and_debug() {
        let level = HorizontalLevel {
            id: 1,
            price: 150.0,
            color: [1.0, 0.0, 0.0, 1.0],
            line_width: 1.0,
            label: None,
            icon: LevelIcon::None,
            locked: false,
        };
        let cloned = level.clone();
        assert_eq!(cloned.id, 1);
        assert_eq!(cloned.price, 150.0);
        // Debug should not panic.
        let _ = format!("{:?}", cloned);
    }

    #[test]
    fn level_serde_round_trip() {
        let level = HorizontalLevel {
            id: 42,
            price: 175.50,
            color: [0.0, 1.0, 0.5, 0.8],
            line_width: 2.0,
            label: Some("Resistance".into()),
            icon: LevelIcon::ArrowUp,
            locked: true,
        };
        let json = serde_json::to_string(&level).expect("serialize");
        let decoded: HorizontalLevel = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(decoded.id, 42);
        assert!((decoded.price - 175.50).abs() < f64::EPSILON);
        assert_eq!(decoded.line_width, 2.0);
        assert_eq!(decoded.label.as_deref(), Some("Resistance"));
        assert_eq!(decoded.icon, LevelIcon::ArrowUp);
        assert!(decoded.locked);
    }

    #[test]
    fn level_serde_defaults_for_new_fields() {
        // Simulate loading old config without label/icon/locked fields.
        let json = r#"{"id":1,"price":100.0,"color":[1,0,0,1],"line_width":1.0}"#;
        let decoded: HorizontalLevel = serde_json::from_str(json).expect("deserialize");
        assert_eq!(decoded.label, None);
        assert_eq!(decoded.icon, LevelIcon::None);
        assert!(!decoded.locked);
    }

    #[test]
    fn level_icon_round_trip() {
        for icon in LevelIcon::all() {
            let id = icon.to_str_id();
            let restored = LevelIcon::from_str_id(id);
            assert_eq!(&restored, icon);
        }
    }

    #[test]
    fn price_step_for_various_prices() {
        let (c, f) = price_step_for(250.0);
        assert_eq!(c, 0.05);
        assert_eq!(f, 0.05);

        let (c, f) = price_step_for(50.0);
        assert_eq!(c, 0.01);
        assert_eq!(f, 0.01);

        let (c, f) = price_step_for(199.99);
        assert_eq!(c, 0.01);
        assert_eq!(f, 0.01);

        let (c, f) = price_step_for(200.0);
        assert_eq!(c, 0.05);
        assert_eq!(f, 0.05);
    }
}
