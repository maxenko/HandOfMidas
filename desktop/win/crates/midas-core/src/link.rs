//! Chart linking color and mode types.
//!
//! Charts can be linked by color so that a symbol or timeframe change in one
//! chart is automatically propagated to every other chart sharing the same
//! `LinkColor`. `LinkMode` captures whether a chart is unlinked, linked to a
//! specific color group, or listening to all groups.

use std::fmt;

// ── LinkColor ───────────────────────────────────────────────────────

/// One of eight color channels that charts can subscribe to for linked
/// symbol/timeframe propagation.
#[derive(
    Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Debug, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum LinkColor {
    Blue,
    Red,
    Orange,
    Green,
    Purple,
    Violet,
    Teal,
    Brown,
}

impl LinkColor {
    /// All eight link colors in declaration order.
    pub const ALL: [LinkColor; 8] = [
        LinkColor::Blue,
        LinkColor::Red,
        LinkColor::Orange,
        LinkColor::Green,
        LinkColor::Purple,
        LinkColor::Violet,
        LinkColor::Teal,
        LinkColor::Brown,
    ];

    /// Human-readable display name for the color.
    pub const fn display_name(&self) -> &'static str {
        match self {
            LinkColor::Blue => "Blue",
            LinkColor::Red => "Red",
            LinkColor::Orange => "Orange",
            LinkColor::Green => "Green",
            LinkColor::Purple => "Purple",
            LinkColor::Violet => "Violet",
            LinkColor::Teal => "Teal",
            LinkColor::Brown => "Brown",
        }
    }
}

impl fmt::Display for LinkColor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.display_name())
    }
}

// ── LinkMode ────────────────────────────────────────────────────────

/// Describes how a chart participates in the linking system.
///
/// - `Unlinked` — chart ignores all link broadcasts (default).
/// - `Color(c)` — chart sends and receives on color channel `c`.
/// - `ListenAll` — chart receives broadcasts from *every* color channel
///   but does not broadcast its own changes.
#[derive(Copy, Clone, Eq, PartialEq, Debug, Default)]
pub enum LinkMode {
    /// Chart is not linked to any group.
    #[default]
    Unlinked,
    /// Chart is linked to a specific color group.
    Color(LinkColor),
    /// Chart listens to all color groups (receive-only).
    ListenAll,
}

impl LinkMode {
    /// Returns true if this is the default unlinked mode.
    pub fn is_unlinked(&self) -> bool {
        matches!(self, LinkMode::Unlinked)
    }
}

impl serde::Serialize for LinkMode {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let s = match self {
            Self::Unlinked => "unlinked",
            Self::ListenAll => "listen_all",
            Self::Color(c) => match c {
                LinkColor::Blue => "blue",
                LinkColor::Red => "red",
                LinkColor::Orange => "orange",
                LinkColor::Green => "green",
                LinkColor::Purple => "purple",
                LinkColor::Violet => "violet",
                LinkColor::Teal => "teal",
                LinkColor::Brown => "brown",
            },
        };
        serializer.serialize_str(s)
    }
}

impl<'de> serde::Deserialize<'de> for LinkMode {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        match s.as_str() {
            "unlinked" => Ok(Self::Unlinked),
            "listen_all" => Ok(Self::ListenAll),
            "blue" => Ok(Self::Color(LinkColor::Blue)),
            "red" => Ok(Self::Color(LinkColor::Red)),
            "orange" => Ok(Self::Color(LinkColor::Orange)),
            "green" => Ok(Self::Color(LinkColor::Green)),
            "purple" => Ok(Self::Color(LinkColor::Purple)),
            "violet" => Ok(Self::Color(LinkColor::Violet)),
            "teal" => Ok(Self::Color(LinkColor::Teal)),
            "brown" => Ok(Self::Color(LinkColor::Brown)),
            other => Err(serde::de::Error::custom(format!(
                "unknown link mode: '{other}'"
            ))),
        }
    }
}

impl fmt::Display for LinkMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LinkMode::Unlinked => f.write_str("Unlinked"),
            LinkMode::Color(c) => write!(f, "Color({})", c),
            LinkMode::ListenAll => f.write_str("ListenAll"),
        }
    }
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_color_all_has_8_elements() {
        assert_eq!(LinkColor::ALL.len(), 8);
        // Verify no duplicates.
        let mut seen = std::collections::HashSet::new();
        for c in LinkColor::ALL {
            assert!(seen.insert(c), "duplicate color: {:?}", c);
        }
    }

    #[test]
    fn link_mode_default_is_unlinked() {
        assert_eq!(LinkMode::default(), LinkMode::Unlinked);
    }

    #[test]
    fn serde_roundtrip_each_variant() {
        // Mode variants — all serialize as flat strings now
        let cases: Vec<(LinkMode, &str)> = vec![
            (LinkMode::Unlinked, r#""unlinked""#),
            (LinkMode::Color(LinkColor::Blue), r#""blue""#),
            (LinkMode::Color(LinkColor::Red), r#""red""#),
            (LinkMode::ListenAll, r#""listen_all""#),
        ];
        for (mode, expected_json) in &cases {
            let json = serde_json::to_string(mode).unwrap();
            assert_eq!(&json, *expected_json, "unexpected JSON for {mode:?}");
            let back: LinkMode = serde_json::from_str(&json).unwrap();
            assert_eq!(*mode, back, "roundtrip failed for {mode:?} (json: {json})");
        }

        // All 8 colors through Color variant
        for color in LinkColor::ALL {
            let mode = LinkMode::Color(color);
            let json = serde_json::to_string(&mode).unwrap();
            // Custom impl produces a flat string, not a sub-object
            assert!(json.starts_with('"'), "expected flat string for {color:?}, got: {json}");
            let back: LinkMode = serde_json::from_str(&json).unwrap();
            assert_eq!(mode, back, "roundtrip failed for {color:?} (json: {json})");
        }

        // Standalone color roundtrip
        for color in LinkColor::ALL {
            let json = serde_json::to_string(&color).unwrap();
            let back: LinkColor = serde_json::from_str(&json).unwrap();
            assert_eq!(color, back, "color roundtrip failed for {color:?}");
        }
    }

    #[test]
    fn display_names_correct() {
        let expected = [
            (LinkColor::Blue, "Blue"),
            (LinkColor::Red, "Red"),
            (LinkColor::Orange, "Orange"),
            (LinkColor::Green, "Green"),
            (LinkColor::Purple, "Purple"),
            (LinkColor::Violet, "Violet"),
            (LinkColor::Teal, "Teal"),
            (LinkColor::Brown, "Brown"),
        ];
        for (color, name) in expected {
            assert_eq!(color.display_name(), name);
            assert_eq!(color.to_string(), name);
        }
    }

    #[test]
    fn toml_roundtrip_all_link_modes() {
        #[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
        struct W {
            mode: LinkMode,
        }

        let cases = [
            LinkMode::Unlinked,
            LinkMode::Color(LinkColor::Blue),
            LinkMode::Color(LinkColor::Red),
            LinkMode::ListenAll,
        ];
        for mode in cases {
            let w = W { mode };
            let s = toml::to_string(&w).unwrap();
            let back: W = toml::from_str(&s).unwrap();
            assert_eq!(w, back, "TOML roundtrip failed for {mode:?}");
        }
    }

    #[test]
    fn unknown_link_mode_rejected() {
        let result = serde_json::from_str::<LinkMode>(r#""bogus""#);
        assert!(result.is_err());
    }

    #[test]
    fn is_unlinked_returns_true_only_for_unlinked() {
        assert!(LinkMode::Unlinked.is_unlinked());
        assert!(!LinkMode::ListenAll.is_unlinked());
        assert!(!LinkMode::Color(LinkColor::Blue).is_unlinked());
    }
}
