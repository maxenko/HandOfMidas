//! Core [`SceneLayer`] trait + [`LayerId`] / [`LayerZ`] identifiers.

use crate::paint::PaintContext;

/// Human-readable layer identifier. Not load-bearing for ordering — the
/// z-order is driven by [`LayerZ`] — but useful for diagnostics, per-
/// layer debug dumps, and UI toggles that target a specific layer.
///
/// Wraps a `&'static str`; construction is `const`, equality is a
/// pointer compare for interned strings.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct LayerId(pub &'static str);

impl std::fmt::Display for LayerId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

/// Z-ordinal newtype with named canonical constants.
///
/// Per R2-NB-5 every layer carries a compile-time ordinal; render order
/// follows painter's algorithm — lower ordinal = drawn first = sits
/// beneath higher ordinals.
///
/// The newtype deliberately uses a wide numeric space (100-unit
/// increments between canonical layers) so a new layer inserted
/// "between Candle and HolidayMarker" is just `LayerZ(450)` — no
/// renumber of existing slots, no file-wide shotgun edit.
///
/// ## Canonical slots
///
/// | Constant                       | Value |
/// | ------------------------------ | ----- |
/// | [`LayerZ::SESSION_BAND`]       | 0     |
/// | [`LayerZ::GRID`]               | 100   |
/// | [`LayerZ::SESSION_SEPARATOR`]  | 200   |
/// | [`LayerZ::VOLUME`]             | 300   |
/// | [`LayerZ::CANDLE`]             | 400   |
/// | [`LayerZ::HOLIDAY_MARKER`]     | 500   |
/// | [`LayerZ::PRICE_LINE`]         | 600   |
/// | [`LayerZ::ORDER_BRACKET`]      | 700   |
/// | [`LayerZ::LEVEL`]              | 800   |
/// | [`LayerZ::DECORATOR`]          | 900   |
/// | [`LayerZ::CROSSHAIR`]          | 1000  |
///
/// ## Adding a new layer
///
/// Pick a value between two existing slots — e.g. an indicator overlay
/// between `CANDLE` (400) and `HOLIDAY_MARKER` (500) would use
/// `LayerZ(450)`. No modifications to other layers; their constants
/// don't shift.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LayerZ(pub i16);

impl LayerZ {
    /// Trading-hour tint (darkest background).
    pub const SESSION_BAND: LayerZ = LayerZ(0);
    /// Gridlines on top of bands.
    pub const GRID: LayerZ = LayerZ(100);
    /// Thin vertical rule at session transitions.
    pub const SESSION_SEPARATOR: LayerZ = LayerZ(200);
    /// Bottom-pane volume bars.
    pub const VOLUME: LayerZ = LayerZ(300);
    /// Candle bodies and wicks.
    pub const CANDLE: LayerZ = LayerZ(400);
    /// Holiday day markers.
    pub const HOLIDAY_MARKER: LayerZ = LayerZ(500);
    /// User + order-bracket price lines.
    pub const PRICE_LINE: LayerZ = LayerZ(600);
    /// Bracket badges / drag handles.
    pub const ORDER_BRACKET: LayerZ = LayerZ(700);
    /// Named level annotations.
    pub const LEVEL: LayerZ = LayerZ(800);
    /// Decorator-tree interactive elements.
    pub const DECORATOR: LayerZ = LayerZ(900);
    /// Always-on-top crosshair.
    pub const CROSSHAIR: LayerZ = LayerZ(1000);
}

/// Implementors render one visual concern into the scene. The trait
/// is `Send + Sync` because a scene may be walked from a render thread
/// while the app thread prepares the next frame. Paint is `&self` —
/// layers mutate state only through dedicated `update_*` methods on
/// the concrete type (called by the scene driver, not by the renderer).
pub trait SceneLayer: Send + Sync {
    /// Human-readable identifier. Used for layer-level diagnostics.
    fn id(&self) -> LayerId;

    /// Compile-time z-ordinal. Drives render order.
    fn z(&self) -> LayerZ;

    /// Emit primitives into `ctx.out`. Pure function of
    /// `(ctx, self-state)`; never touches global state.
    fn paint(&self, ctx: &mut PaintContext<'_>);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layer_z_canonical_values_match_spec() {
        assert_eq!(LayerZ::SESSION_BAND, LayerZ(0));
        assert_eq!(LayerZ::GRID, LayerZ(100));
        assert_eq!(LayerZ::SESSION_SEPARATOR, LayerZ(200));
        assert_eq!(LayerZ::VOLUME, LayerZ(300));
        assert_eq!(LayerZ::CANDLE, LayerZ(400));
        assert_eq!(LayerZ::HOLIDAY_MARKER, LayerZ(500));
        assert_eq!(LayerZ::PRICE_LINE, LayerZ(600));
        assert_eq!(LayerZ::ORDER_BRACKET, LayerZ(700));
        assert_eq!(LayerZ::LEVEL, LayerZ(800));
        assert_eq!(LayerZ::DECORATOR, LayerZ(900));
        assert_eq!(LayerZ::CROSSHAIR, LayerZ(1000));
    }

    #[test]
    fn layer_z_is_totally_ordered() {
        assert!(LayerZ::SESSION_BAND < LayerZ::GRID);
        assert!(LayerZ::GRID < LayerZ::SESSION_SEPARATOR);
        assert!(LayerZ::CANDLE < LayerZ::HOLIDAY_MARKER);
        assert!(LayerZ::CROSSHAIR > LayerZ::DECORATOR);
    }

    #[test]
    fn layer_z_allows_insertion_between_canonical_slots() {
        // Demonstration: a future indicator layer between CANDLE and
        // HOLIDAY_MARKER needs no renumber of existing slots.
        let indicator = LayerZ(450);
        assert!(indicator > LayerZ::CANDLE);
        assert!(indicator < LayerZ::HOLIDAY_MARKER);
    }

    #[test]
    fn layer_id_display() {
        assert_eq!(format!("{}", LayerId("candles")), "candles");
    }
}
