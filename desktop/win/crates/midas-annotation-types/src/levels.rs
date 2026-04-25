//! Horizontal price levels.
//!
//! Moved from `midas-chart/src/levels.rs` (Slice A1). Carries the data
//! type (`HorizontalLevel`), its display-side `LevelIcon` enum, the
//! `price_step_for` price-step heuristic, and the V1/V2 untagged
//! deserialize migration path.
//!
//! The `to_decorators()` rendering helper stays behind in `midas-chart`
//! — it depends on chart-only `Badge`/`DecoratorGroup`/`PRICELINE_WIDTH`
//! types. It is exposed there as the [`HorizontalLevelExt`] extension
//! trait so existing `level.to_decorators(locked)` call sites keep
//! resolving via a trait import. Per the plan's risk-mitigation
//! guidance: "If discovered during the move, leave [helpers with chart
//! deps] in `midas-chart` and only move the data part of
//! `HorizontalLevel`. Helper functions can ride along later."
//!
//! ## Lock semantics
//!
//! Per the upstream plan, `locked` conceptually lives on the
//! `Annotation` wrapper (`crate::annotation::Annotation`) rather than
//! on the inner `HorizontalLevel`. That is true for every level that
//! flows through the `AnnotationStore`. The app's pre-decorator
//! `LevelStore` path still stores bare levels outside of an
//! `Annotation` wrapper (it predates the unified widget system), so a
//! sibling `StoredLevel { level, locked }` wrapper type lives in
//! `midas-app/src/level_store/mod.rs` and owns the lock flag for that
//! path. `HorizontalLevel::to_decorators(locked)` (in `midas-chart`)
//! takes `locked` as an explicit argument so both paths can drive the
//! same decorator emission from a wrapper-side flag.

use crate::line_style::LineStyle;
use crate::price_line::{LineExtent, LineStroke, PriceLine};
use serde::de::{Deserializer, Error as DeError};
use serde::{Deserialize, Serialize};

/// Icon displayed next to a level label on the chart.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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
/// Wraps a `PriceLine` geometry primitive plus the level's domain metadata
/// (`label`, `icon`). The `locked` flag is carried on the `Annotation`
/// wrapper for the widget path and on a `StoredLevel` wrapper for the
/// pre-decorator `LevelStore` path — see the module doc.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct HorizontalLevel {
    /// Unique identifier for this level within the chart.
    pub id: u64,
    /// Line geometry (price, extent, stroke: color/width/style).
    pub line: PriceLine,
    /// Optional text label displayed on the chart next to the line.
    pub label: Option<String>,
    /// Optional icon displayed next to the label.
    pub icon: LevelIcon,
}

// ── Two-version deserialize strategy ───────────────────────────────
//
// `#[serde(untagged)]` usually picks the first variant that deserializes
// successfully, which is a silent-branch-selection risk when v1 and v2
// overlap on fields like `id`/`label`/`icon`. We mitigate the ambiguity
// by marking V2 with `deny_unknown_fields` AND requiring the nested
// `line` field — a V1 payload has `price`/`color`/`line_width` at the
// top level, which V2 rejects as unknown fields, deterministically
// falling through to V1. V2 always has the `line` key, so V1 rejects
// it via its own flat schema. The discriminator is therefore the
// presence of `line`, and untagged selection becomes unambiguous. This
// approach is cross-format (works for both JSON via `serde_json` and
// TOML via `toml`) without needing to buffer through an intermediate
// `Value` type, and avoids pulling in `serde-value` as a new dep.
//
// If either branch grows new overlapping fields in the future, the
// `deny_unknown_fields` + mandatory `line` pair must be preserved or
// the discriminator breaks.

#[derive(Deserialize)]
#[serde(untagged)]
enum HorizontalLevelRepr {
    V2(HorizontalLevelV2),
    V1(HorizontalLevelV1),
}

/// V2 (post-Slice-7) shape used to drive `Deserialize`. Private.
/// `deny_unknown_fields` + the mandatory nested `line` field is the
/// discriminator against the v1 shape.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HorizontalLevelV2 {
    id: u64,
    line: PriceLine,
    #[serde(default)]
    label: Option<String>,
    #[serde(default)]
    icon: LevelIcon,
}

/// V1 (pre-Slice-7) flat shape. Fallback branch. The `locked` field is
/// accepted for migration compatibility but dropped from the in-memory
/// type — a one-time warn is logged if it was `true`, since the caller
/// needs to set `Annotation.locked` or `StoredLevel.locked` on the
/// wrapper manually.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HorizontalLevelV1 {
    id: u64,
    price: f64,
    color: [f32; 4],
    #[serde(default = "default_line_width_v1")]
    line_width: f32,
    #[serde(default)]
    style: LineStyle,
    #[serde(default)]
    label: Option<String>,
    #[serde(default)]
    icon: LevelIcon,
    // Accept-and-drop: v1 `extend` (LevelExtend) is no longer a
    // distinct type. We accept whatever the old JSON/TOML encoded and
    // fold the result into `LineExtent::FullWidth`. Callers that need
    // a non-default extent must re-save the level.
    #[serde(default, deserialize_with = "deserialize_ignored_extend")]
    #[allow(dead_code)]
    extend: (),
    #[serde(default)]
    locked: bool,
}

fn default_line_width_v1() -> f32 {
    1.0
}

/// Accept any shape for the legacy `extend` field and discard it.
/// Used by `HorizontalLevelV1` so v1 payloads with `"extend":"FullWidth"`
/// or `"extend":{"RightFrom":{"timestamp":123}}` both parse.
fn deserialize_ignored_extend<'de, D>(deserializer: D) -> Result<(), D::Error>
where
    D: Deserializer<'de>,
{
    let _ = serde::de::IgnoredAny::deserialize(deserializer)?;
    Ok(())
}

impl From<HorizontalLevelV2> for HorizontalLevel {
    fn from(v: HorizontalLevelV2) -> Self {
        Self {
            id: v.id,
            line: v.line,
            label: v.label,
            icon: v.icon,
        }
    }
}

impl From<HorizontalLevelV1> for HorizontalLevel {
    fn from(v: HorizontalLevelV1) -> Self {
        if v.locked {
            tracing::warn!(
                level_id = v.id,
                "v1 HorizontalLevel had locked=true; drop this field — set \
                 Annotation.locked or StoredLevel.locked on the wrapper instead"
            );
        }
        Self {
            id: v.id,
            line: PriceLine {
                price: v.price,
                extent: LineExtent::default(),
                stroke: LineStroke {
                    color: v.color,
                    width: v.line_width,
                    style: v.style,
                },
            },
            label: v.label,
            icon: v.icon,
        }
    }
}

impl<'de> Deserialize<'de> for HorizontalLevel {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let repr = HorizontalLevelRepr::deserialize(deserializer).map_err(|e| {
            DeError::custom(format!(
                "HorizontalLevel: neither v2 nor v1 shape matched: {e}"
            ))
        })?;
        match repr {
            HorizontalLevelRepr::V2(v2) => Ok(v2.into()),
            HorizontalLevelRepr::V1(v1) => Ok(v1.into()),
        }
    }
}
