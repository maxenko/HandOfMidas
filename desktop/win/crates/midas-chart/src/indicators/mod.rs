//! Indicator module -- data-derived overlays computed per chart.
//!
//! Indicators are distinct from annotations:
//! - **Annotations** are user-placed objects persisted in `AnnotationStore`
//!   (horizontal levels, brackets, drawings).
//! - **Indicators** are computed from candle data each frame and are
//!   *not* persisted. They live in `ChartState` as configuration and
//!   produce ephemeral output for the renderer.
//!
//! # Architecture
//!
//! Each indicator has three pieces:
//! 1. [`IndicatorKind`] -- identifies the indicator type.
//! 2. [`IndicatorConfig`] -- per-chart toggle + parameters.
//! 3. [`IndicatorOutput`] -- the computed render data for one frame.
//!
//! The chart state owns a `Vec<IndicatorConfig>` and the compute pass
//! produces a `Vec<IndicatorOutput>` each frame for the renderer.

pub mod gerchik_atr;

pub use gerchik_atr::GerchikAtrConfig;

// ── Indicator kind ──────────────────────────────────────────────────

/// Enumerates all supported indicator types.
///
/// Each variant maps 1:1 to a compute function and a renderer overlay.
#[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum IndicatorKind {
    /// Gerchik ATR -- session range as percentage of ATR.
    GerchikAtr,
    /// Volume Profile histogram overlay.
    VolumeProfile,
}

// ── Indicator config ────────────────────────────────────────────────

/// Per-chart indicator configuration.
///
/// Stored in `ChartState` and serialized to the workspace config file.
/// The `kind` field selects the compute function; `enabled` toggles
/// visibility without removing the entry.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct IndicatorConfig {
    /// Which indicator this entry controls.
    pub kind: IndicatorKind,
    /// Whether the indicator is currently visible.
    pub enabled: bool,
}

impl IndicatorConfig {
    /// Create a new enabled indicator config.
    pub fn new(kind: IndicatorKind) -> Self {
        Self {
            kind,
            enabled: true,
        }
    }

    /// Create a new indicator config with explicit enabled state.
    pub fn with_enabled(kind: IndicatorKind, enabled: bool) -> Self {
        Self { kind, enabled }
    }
}

// ── Indicator output ────────────────────────────────────────────────

/// Computed indicator output for one frame.
///
/// The renderer inspects the variant to choose the drawing method:
/// - `TextBadge` -- a single text label with color (e.g. G.ATR overlay).
/// - `Instances` -- GPU instance data for batch rendering (e.g. VP bars).
#[derive(Clone, Debug)]
pub enum IndicatorOutput {
    /// A text badge rendered at a fixed screen position.
    ///
    /// Used by indicators that produce a single summary value
    /// (e.g. "G.ATR 67%") displayed as a watermark in the chart corner.
    TextBadge {
        /// Display text (e.g. "G.ATR 67%").
        text: String,
        /// RGBA color for the text.
        color: [f32; 4],
    },

    /// GPU instance data for batch rendering.
    ///
    /// Used by indicators that produce geometry (e.g. Volume Profile
    /// histogram bars). The instances are opaque to this module --
    /// the renderer knows the layout based on the `kind` field.
    Instances {
        /// The indicator kind, so the renderer picks the right pipeline.
        kind: IndicatorKind,
        /// Raw instance data as bytes (cast to the appropriate GPU struct
        /// by the renderer). Empty if the indicator produced no output.
        data: Vec<u8>,
    },
}

impl IndicatorOutput {
    /// Create a text badge output.
    pub fn text_badge(text: String, color: [f32; 4]) -> Self {
        Self::TextBadge { text, color }
    }
}

// ── Default indicator set ───────────────────────────────────────────

/// Returns the default set of indicator configs for a new chart.
///
/// Currently enables only Gerchik ATR. Volume Profile is available
/// but disabled by default (it has its own toggle in the UI).
pub fn default_indicators() -> Vec<IndicatorConfig> {
    vec![
        IndicatorConfig::new(IndicatorKind::GerchikAtr),
        IndicatorConfig::with_enabled(IndicatorKind::VolumeProfile, false),
    ]
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indicator_kind_equality() {
        assert_eq!(IndicatorKind::GerchikAtr, IndicatorKind::GerchikAtr);
        assert_ne!(IndicatorKind::GerchikAtr, IndicatorKind::VolumeProfile);
    }

    #[test]
    fn config_new_is_enabled() {
        let cfg = IndicatorConfig::new(IndicatorKind::GerchikAtr);
        assert!(cfg.enabled);
        assert_eq!(cfg.kind, IndicatorKind::GerchikAtr);
    }

    #[test]
    fn config_with_enabled_false() {
        let cfg = IndicatorConfig::with_enabled(IndicatorKind::VolumeProfile, false);
        assert!(!cfg.enabled);
        assert_eq!(cfg.kind, IndicatorKind::VolumeProfile);
    }

    #[test]
    fn default_indicators_contains_gerchik_atr() {
        let defaults = default_indicators();
        let gatr = defaults.iter().find(|c| c.kind == IndicatorKind::GerchikAtr);
        assert!(gatr.is_some(), "default set should include GerchikAtr");
        assert!(gatr.unwrap().enabled, "GerchikAtr should be enabled by default");
    }

    #[test]
    fn default_indicators_volume_profile_disabled() {
        let defaults = default_indicators();
        let vp = defaults.iter().find(|c| c.kind == IndicatorKind::VolumeProfile);
        assert!(vp.is_some(), "default set should include VolumeProfile");
        assert!(!vp.unwrap().enabled, "VolumeProfile should be disabled by default");
    }

    #[test]
    fn text_badge_construction() {
        let output = IndicatorOutput::text_badge("G.ATR 42%".to_string(), [0.2, 0.8, 0.3, 0.18]);
        match &output {
            IndicatorOutput::TextBadge { text, color } => {
                assert_eq!(text, "G.ATR 42%");
                assert_eq!(color[0], 0.2);
            }
            _ => panic!("expected TextBadge variant"),
        }
    }

    #[test]
    fn instances_variant_carries_kind() {
        let output = IndicatorOutput::Instances {
            kind: IndicatorKind::VolumeProfile,
            data: vec![1, 2, 3, 4],
        };
        match &output {
            IndicatorOutput::Instances { kind, data } => {
                assert_eq!(*kind, IndicatorKind::VolumeProfile);
                assert_eq!(data.len(), 4);
            }
            _ => panic!("expected Instances variant"),
        }
    }

    #[test]
    fn indicator_kind_serde_roundtrip() {
        let kind = IndicatorKind::GerchikAtr;
        let json = serde_json::to_string(&kind).unwrap();
        let back: IndicatorKind = serde_json::from_str(&json).unwrap();
        assert_eq!(back, kind);
    }

    #[test]
    fn indicator_config_serde_roundtrip() {
        let cfg = IndicatorConfig::new(IndicatorKind::VolumeProfile);
        let json = serde_json::to_string(&cfg).unwrap();
        let back: IndicatorConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back.kind, cfg.kind);
        assert_eq!(back.enabled, cfg.enabled);
    }
}
