//! Core [`SceneLayer`] trait + [`LayerId`] / [`LayerZ`] identifiers.

use crate::error::SceneError;
use crate::input::{EventStatus, Hit, InputEvent, Point};
use crate::paint::PaintContext;
use midas_axis::PriceRange;

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
/// | [`LayerZ::VOLUME_PROFILE`]     | 350   |
/// | [`LayerZ::CANDLE`]             | 400   |
/// | [`LayerZ::INDICATOR`]          | 450   |
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
    /// Horizontal volume-profile histogram (by-price, left-anchored).
    /// Sits between [`Self::VOLUME`] (time-strip) and [`Self::CANDLE`]
    /// (wick + body) so the profile paints UNDER the candles. Slots at
    /// 350 per slice 7 of the chart-transition plan — picked midway
    /// between 300 (`VOLUME`) and 400 (`CANDLE`) so the wide-numeric
    /// insertion rule (R2-NB-5) holds.
    pub const VOLUME_PROFILE: LayerZ = LayerZ(350);
    /// Candle bodies and wicks.
    pub const CANDLE: LayerZ = LayerZ(400);
    /// Computed overlay indicators that read `CandleSeries` and emit
    /// derived lines / badges (ATR band, Gerchik ATR watermark).
    /// Slots at 450 — picked midway between 400 (`CANDLE`) and 500
    /// (`HOLIDAY_MARKER`) per the wide-numeric insertion rule (R2-NB-5),
    /// so indicators paint above candles but below the holiday markers.
    /// Added by slice 6 of the chart-transition plan.
    pub const INDICATOR: LayerZ = LayerZ(450);
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

    /// Opt-in hook for interactive layers (tools, draggable
    /// annotations). Default returns `None` — a layer that doesn't
    /// override this is treated as passive. Per plan D4 there is NO
    /// blanket `impl<T: SceneLayer> InteractiveLayer for T`; the
    /// blanket-impl shortcut would block downstream crates from
    /// opting specific layers in under the orphan rule.
    ///
    /// Interactive layers override `as_interactive` to return
    /// `Some(self)` so the scene's input dispatcher can reach them.
    fn as_interactive(&mut self) -> Option<&mut dyn InteractiveLayer> {
        None
    }
}

// ── Interactive layer trait ───────────────────────────────────────────

/// Opt-in interactivity for a [`SceneLayer`]. Implemented by tools
/// (bracket placement, level draw, measure) and by layers whose
/// primitives respond to drag/hover (draggable price lines, TP/SL
/// handles).
///
/// Contract:
///
/// - [`update`] receives events the scene dispatched top-down (or
///   directly from the drag-focus layer). Return `Captured` to claim
///   the event, `Ignored` to let it bubble.
/// - [`hit_test`] is pure — must not mutate layer state. The scene
///   calls it before routing a `MouseDown`/`MouseMove` so the winning
///   layer can establish drag-focus.
/// - [`cancel`] resets any in-flight tool state to `Idle`. Called by
///   `ChartScene::on_destroy` (window close) and by Escape handling.
/// - [`update`] may emit errors via `ctx.emit_error(SceneError)` —
///   signature lives on the tool-context once slice 4's `ToolCtx` is
///   specced. Slice 1 ships the slot on [`crate::scene::ChartScene`]
///   and a pair of no-op helpers so tool impls can land with forward-
///   compatible error plumbing.
///
/// [`update`]: InteractiveLayer::update
/// [`hit_test`]: InteractiveLayer::hit_test
/// [`cancel`]: InteractiveLayer::cancel
pub trait InteractiveLayer: SceneLayer + Send + Sync {
    /// React to an input event. Non-capturing layers implement this
    /// by returning `Ignored`. Tools that pin state across frames
    /// mutate through `&mut self` here.
    fn update(&mut self, ev: InputEvent, ctx: &mut ToolContext<'_>) -> EventStatus;

    /// Pure hit-test. Return `Some(Hit)` if the layer wants to claim
    /// hover / cursor shape at `pt`. `price_range` is passed because
    /// layers whose primitives are priced (price lines, bracket legs)
    /// need to convert screen y → price to decide the hit.
    fn hit_test(&self, pt: Point, price_range: &PriceRange) -> Option<Hit>;

    /// Reset in-flight tool state. Called on Escape and on
    /// `ChartScene::on_destroy`. Must be idempotent.
    fn cancel(&mut self);
}

/// Context threaded to [`InteractiveLayer::update`]. Carries the
/// scene's per-frame projection state + a channel for emitting errors
/// that surface on [`crate::scene::ChartScene::last_error`].
///
/// Slice 1 ships the minimum shape; slice 4 extends with `ToolEffect`
/// emission (cross-slice coupling pre-agreed in slice 4's spec).
pub struct ToolContext<'a> {
    pub price_range: &'a PriceRange,
    pub last_error: &'a mut Option<SceneError>,
}

impl<'a> ToolContext<'a> {
    /// Park an error on the scene's `last_error` slot. Tool impls
    /// call this rather than returning `Result` from `update`.
    pub fn emit_error(&mut self, err: SceneError) {
        tracing::warn!(error = ?err, "tool emitted SceneError");
        *self.last_error = Some(err);
    }
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
        assert_eq!(LayerZ::VOLUME_PROFILE, LayerZ(350));
        assert_eq!(LayerZ::CANDLE, LayerZ(400));
        assert_eq!(LayerZ::INDICATOR, LayerZ(450));
        assert_eq!(LayerZ::HOLIDAY_MARKER, LayerZ(500));
        assert_eq!(LayerZ::PRICE_LINE, LayerZ(600));
        assert_eq!(LayerZ::ORDER_BRACKET, LayerZ(700));
        assert_eq!(LayerZ::LEVEL, LayerZ(800));
        assert_eq!(LayerZ::DECORATOR, LayerZ(900));
        assert_eq!(LayerZ::CROSSHAIR, LayerZ(1000));
    }

    #[test]
    fn volume_profile_slots_between_volume_and_candle() {
        // Slice 7: VP histogram paints UNDER candles but OVER the
        // time-strip volume bars. Integer-literal assertion guards
        // against accidental renumber.
        assert!(LayerZ::VOLUME < LayerZ::VOLUME_PROFILE);
        assert!(LayerZ::VOLUME_PROFILE < LayerZ::CANDLE);
        assert_eq!(LayerZ::VOLUME_PROFILE.0, 350);
    }

    #[test]
    fn indicator_slots_between_candle_and_holiday_marker() {
        // Slice 6: indicator overlays (ATR band, G.ATR badge) paint
        // OVER candles so price labels don't disappear under wicks,
        // and UNDER the holiday markers so non-trading-day shading
        // still wins. Integer-literal assertion guards against
        // accidental renumber.
        assert!(LayerZ::CANDLE < LayerZ::INDICATOR);
        assert!(LayerZ::INDICATOR < LayerZ::HOLIDAY_MARKER);
        assert_eq!(LayerZ::INDICATOR.0, 450);
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
