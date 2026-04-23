//! # midas-scene
//!
//! Slice S5 of the session-aware-charts stack: sans-IO composable scene
//! layers. Each visual concern — candles, session bands, session
//! separators, gridlines, volume, annotations, crosshair — is an
//! independent [`SceneLayer`] implementation with its own state and a
//! compile-time [`LayerZ`] z-ordinal. A [`ChartScene`] is a sorted
//! `Vec<Box<dyn SceneLayer>>` plus the projection state (axis, price
//! range, viewport, palette).
//!
//! ## Rendering model
//!
//! Layers emit into a shared [`ScenePrimitives`] vocabulary — `Copy`
//! (or nearly so) plain-data vectors of candles, quads, lines, badges,
//! and text. No GPU types, no iced types, no wgpu types. The GPU
//! renderer consumes [`ScenePrimitives`] and batches them into draw
//! calls post-hoc. This keeps every layer testable by asserting on the
//! emitted primitive counts and positions.
//!
//! ## Ideal-design references
//!
//! - `plan/session-aware-charts/00a-ideal-design.md`:
//!   - R2-NB-3 `PaintContext`: sans-IO primitive emitter.
//!   - R2-NB-5 `LayerZ`: compile-time z-ordinals; builder sorts by
//!     `(LayerZ, insertion_idx)`.
//!   - R2-NM-4 annotation split: concrete `OrderBracketLayer`,
//!     `PriceLineLayer`, `LevelLayer`, `DecoratorLayer` — no god-enum.
//!   - R2-NM-7 Camera deletion: `InteractionState` captures pan/zoom/
//!     drag/hover.

pub mod interaction;
pub mod layer;
pub mod layers;
pub mod paint;
pub mod primitives;
pub mod scene;

pub use crate::interaction::{BracketLeg, DragSession, HoverTarget, InteractionState};
pub use crate::layer::{LayerId, LayerZ, SceneLayer};
pub use crate::layers::{
    CandleLayer, CandleStyle, CrosshairLayer, DecoratorLayer, GridLayer, GridStyle,
    HolidayMarkerLayer, LevelLayer, LevelView, OrderBracketLayer, OrderBracketView, PriceLineLayer,
    PriceLineView, SeparatorStyle, SessionBandLayer, SessionBoundary, SessionPalette,
    SessionSeparatorLayer, SharedCandleSeries, Side, VolumeLayer, VolumeStyle,
};
pub use crate::paint::PaintContext;
pub use crate::primitives::{
    BadgeInstance, CandleInstance, LineInstance, QuadInstance, ScenePrimitives, TextAnchor,
    TextInstance,
};
pub use crate::scene::{ChartScene, ChartSceneBuilder, LayerConfig, SceneError};

// Convenience re-exports — downstream crates pull the full scene surface
// from `midas-scene` without a second `midas-axis` / `midas-calendar`
// import just to reach these.
pub use midas_axis::{PriceRange, Viewport};
pub use midas_calendar::Timestamp;

/// Theme palette for a rendered scene. All colours are RGBA8. Layers
/// receive an immutable borrow via [`PaintContext`] and decide per-
/// primitive which entry to pick (e.g. `CandleLayer` picks between
/// `candle_up` and `candle_down` based on OHLC direction; session bands
/// pick between `band_pre` / `band_regular` / `band_post` / `band_closed`
/// from [`midas_calendar::SessionKind`]).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ThemePalette {
    pub candle_up: [u8; 4],
    pub candle_down: [u8; 4],
    pub candle_wick: [u8; 4],
    pub grid: [u8; 4],
    pub separator: [u8; 4],
    pub band_pre: [u8; 4],
    pub band_post: [u8; 4],
    pub band_regular: [u8; 4],
    pub band_closed: [u8; 4],
    pub crosshair: [u8; 4],
    pub text: [u8; 4],
}

impl ThemePalette {
    /// Dark-theme preset. Matches the visual direction of the existing
    /// `midas-app` chart surface: near-black background with pastel
    /// accents for session tint.
    pub const fn dark_default() -> Self {
        Self {
            candle_up: [0x3d, 0xd5, 0x98, 0xff],
            candle_down: [0xf2, 0x5d, 0x5d, 0xff],
            candle_wick: [0xc8, 0xc8, 0xc8, 0xff],
            grid: [0x2a, 0x2a, 0x2a, 0xff],
            separator: [0x44, 0x44, 0x66, 0xff],
            band_pre: [0x1a, 0x1a, 0x2e, 0x66],
            band_post: [0x2e, 0x1a, 0x1a, 0x66],
            band_regular: [0x0d, 0x0d, 0x12, 0x00],
            band_closed: [0x05, 0x05, 0x08, 0xcc],
            crosshair: [0xff, 0xff, 0xff, 0x88],
            text: [0xf0, 0xf0, 0xf0, 0xff],
        }
    }

    /// Light-theme preset.
    pub const fn light_default() -> Self {
        Self {
            candle_up: [0x26, 0xa6, 0x6e, 0xff],
            candle_down: [0xd2, 0x3f, 0x3f, 0xff],
            candle_wick: [0x40, 0x40, 0x40, 0xff],
            grid: [0xe0, 0xe0, 0xe0, 0xff],
            separator: [0xb0, 0xb0, 0xd0, 0xff],
            band_pre: [0xe8, 0xe8, 0xfa, 0xff],
            band_post: [0xfa, 0xe8, 0xe8, 0xff],
            band_regular: [0xff, 0xff, 0xff, 0x00],
            band_closed: [0xf0, 0xf0, 0xf0, 0xcc],
            crosshair: [0x30, 0x30, 0x30, 0xaa],
            text: [0x10, 0x10, 0x10, 0xff],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dark_and_light_palettes_differ() {
        let d = ThemePalette::dark_default();
        let l = ThemePalette::light_default();
        assert_ne!(d.text, l.text);
        assert_ne!(d.grid, l.grid);
    }

    #[test]
    fn palette_is_copy() {
        fn takes_copy<T: Copy>(_: T) {}
        takes_copy(ThemePalette::dark_default());
    }
}
