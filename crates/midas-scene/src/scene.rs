//! [`ChartScene`] — sorted stack of [`SceneLayer`] boxes + projection state.
//!
//! Per R2-NB-5 layers sort by `(LayerZ, insertion_idx)` so same-Z
//! siblings render in insertion order. Construction goes through a
//! builder so projection pieces are validated (all three of
//! axis / price_range / viewport are required).

use midas_axis::{PriceRange, TimeAxis, Viewport};

use crate::layer::{LayerZ, SceneLayer};
use crate::paint::PaintContext;
use crate::primitives::ScenePrimitives;
use crate::ThemePalette;

/// Declarative layer-toggle set for a scene. Callers build a scene
/// with every visual feature they want; disabled layers simply aren't
/// added, so runtime cost is zero for off layers.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct LayerConfig {
    pub candles: bool,
    pub volume: bool,
    pub grid: bool,
    pub session_bands: bool,
    pub session_separators: bool,
    pub holidays: bool,
    pub annotations: bool,
    pub crosshair: bool,
}

impl LayerConfig {
    /// All layers on. Default for the main chart.
    pub const fn all_on() -> Self {
        Self {
            candles: true,
            volume: true,
            grid: true,
            session_bands: true,
            session_separators: true,
            holidays: true,
            annotations: true,
            crosshair: true,
        }
    }

    /// Candles + grid + crosshair only. Spartan analytical view.
    pub const fn minimal() -> Self {
        Self {
            candles: true,
            volume: false,
            grid: true,
            session_bands: false,
            session_separators: false,
            holidays: false,
            annotations: false,
            crosshair: true,
        }
    }

    /// Thumbnail preset — candles + session bands only (per R2-G-6).
    pub const fn thumbnail() -> Self {
        Self {
            candles: true,
            volume: false,
            grid: false,
            session_bands: true,
            session_separators: false,
            holidays: false,
            annotations: false,
            crosshair: false,
        }
    }
}

impl Default for LayerConfig {
    fn default() -> Self {
        Self::all_on()
    }
}

/// Finished scene: sorted layer stack + projection state. Produced by
/// [`ChartSceneBuilder`].
pub struct ChartScene {
    axis: Box<dyn TimeAxis>,
    price_range: PriceRange,
    viewport: Viewport,
    palette: ThemePalette,
    layers: Vec<Box<dyn SceneLayer>>,
}

impl std::fmt::Debug for ChartScene {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChartScene")
            .field("price_range", &self.price_range)
            .field("viewport", &self.viewport)
            .field("palette", &self.palette)
            .field("axis_policy", &self.axis.policy())
            .field("axis_width_px", &self.axis.width_px())
            .field(
                "layers",
                &self
                    .layers
                    .iter()
                    .map(|l| (l.id(), l.z()))
                    .collect::<Vec<_>>(),
            )
            .finish()
    }
}

impl ChartScene {
    /// Entry point for building a scene. See
    /// [`ChartSceneBuilder`].
    pub fn builder() -> ChartSceneBuilder {
        ChartSceneBuilder::default()
    }

    /// Paint every layer in z-order into `out`. Clears `out` first so
    /// the caller can reuse the same buffer across frames.
    pub fn paint(&self, out: &mut ScenePrimitives) {
        out.clear();
        let mut ctx = PaintContext {
            axis: self.axis.as_ref(),
            viewport: self.viewport,
            price_range: self.price_range,
            palette: &self.palette,
            out,
        };
        for layer in &self.layers {
            layer.paint(&mut ctx);
        }
    }

    #[inline]
    pub fn viewport(&self) -> Viewport {
        self.viewport
    }

    #[inline]
    pub fn price_range(&self) -> PriceRange {
        self.price_range
    }

    #[inline]
    pub fn palette(&self) -> &ThemePalette {
        &self.palette
    }

    #[inline]
    pub fn axis(&self) -> &dyn TimeAxis {
        self.axis.as_ref()
    }

    /// Layer count. Useful for tests asserting the builder sorted /
    /// preserved order.
    #[inline]
    pub fn layer_count(&self) -> usize {
        self.layers.len()
    }

    /// Observe layers in z-order. Each item is a reference to the
    /// boxed layer — callers typically just want the `z()` / `id()`.
    pub fn layers(&self) -> impl Iterator<Item = &dyn SceneLayer> {
        self.layers.iter().map(|b| b.as_ref())
    }
}

/// Fluent builder. Validates axis / price_range / viewport are set;
/// sorts the collected layers by `(LayerZ, insertion_idx)`.
#[derive(Default)]
pub struct ChartSceneBuilder {
    axis: Option<Box<dyn TimeAxis>>,
    price_range: Option<PriceRange>,
    viewport: Option<Viewport>,
    palette: Option<ThemePalette>,
    layers: Vec<(LayerZ, usize, Box<dyn SceneLayer>)>,
    next_insertion_idx: usize,
}

impl ChartSceneBuilder {
    pub fn axis<A: TimeAxis + 'static>(mut self, axis: A) -> Self {
        self.axis = Some(Box::new(axis));
        self
    }

    /// Alternative when the axis is already a boxed trait object.
    pub fn axis_boxed(mut self, axis: Box<dyn TimeAxis>) -> Self {
        self.axis = Some(axis);
        self
    }

    pub fn price_range(mut self, range: PriceRange) -> Self {
        self.price_range = Some(range);
        self
    }

    pub fn viewport(mut self, vp: Viewport) -> Self {
        self.viewport = Some(vp);
        self
    }

    pub fn palette(mut self, palette: ThemePalette) -> Self {
        self.palette = Some(palette);
        self
    }

    pub fn layer<L: SceneLayer + 'static>(mut self, layer: L) -> Self {
        let idx = self.next_insertion_idx;
        self.next_insertion_idx += 1;
        self.layers.push((layer.z(), idx, Box::new(layer)));
        self
    }

    pub fn build(self) -> Result<ChartScene, SceneError> {
        let Some(axis) = self.axis else {
            return Err(SceneError::MissingAxis);
        };
        let Some(price_range) = self.price_range else {
            return Err(SceneError::MissingPriceRange);
        };
        let Some(viewport) = self.viewport else {
            return Err(SceneError::MissingViewport);
        };
        let palette = self.palette.unwrap_or_else(ThemePalette::dark_default);
        let mut layers = self.layers;
        layers.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
        let layers = layers.into_iter().map(|(_, _, l)| l).collect();
        Ok(ChartScene {
            axis,
            price_range,
            viewport,
            palette,
            layers,
        })
    }
}

/// Errors surfaced by [`ChartSceneBuilder::build`].
#[derive(Debug, thiserror::Error)]
pub enum SceneError {
    #[error("builder missing axis")]
    MissingAxis,
    #[error("builder missing price_range")]
    MissingPriceRange,
    #[error("builder missing viewport")]
    MissingViewport,
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use chrono::TimeZone;
    use midas_axis::{ContinuousAxis, PriceRange, Viewport};
    use midas_calendar::Timestamp;

    use super::*;
    use crate::layer::{LayerId, LayerZ, SceneLayer};
    use crate::layers::{LevelLayer, LevelView, PriceLineLayer, PriceLineView};
    use crate::paint::PaintContext;
    use crate::primitives::{LineInstance, ScenePrimitives};

    fn ts(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> Timestamp {
        chrono::Utc.with_ymd_and_hms(y, mo, d, h, mi, s).unwrap()
    }

    fn axis() -> ContinuousAxis {
        ContinuousAxis::new(ts(2024, 1, 1, 0, 0, 0), ts(2024, 1, 2, 0, 0, 0), 1000.0).unwrap()
    }

    fn pr() -> PriceRange {
        PriceRange::new(90.0, 110.0).unwrap()
    }

    fn vp() -> Viewport {
        Viewport::new(1000.0, 400.0)
    }

    /// Test helper: a layer that records its paint-invocation order
    /// into a shared counter and emits one `LineInstance` whose
    /// `color[0]` encodes the recorded order.
    struct RecordingLayer {
        z: LayerZ,
        tag: u8,
        counter: Arc<AtomicUsize>,
        order: Arc<AtomicUsize>,
    }

    impl SceneLayer for RecordingLayer {
        fn id(&self) -> LayerId {
            LayerId("recording")
        }
        fn z(&self) -> LayerZ {
            self.z
        }
        fn paint(&self, ctx: &mut PaintContext<'_>) {
            let n = self.counter.fetch_add(1, Ordering::SeqCst);
            self.order.store(n, Ordering::SeqCst);
            ctx.out.lines.push(LineInstance {
                x0: 0.0,
                y0: 0.0,
                x1: 0.0,
                y1: 0.0,
                width_px: self.tag as f32,
                color: [n as u8, self.tag, 0, 0],
            });
        }
    }

    #[test]
    fn layer_config_presets() {
        let a = LayerConfig::all_on();
        assert!(a.candles && a.volume && a.grid && a.crosshair);

        let m = LayerConfig::minimal();
        assert!(m.candles && m.grid && m.crosshair);
        assert!(!m.volume && !m.session_bands && !m.annotations);

        let t = LayerConfig::thumbnail();
        assert!(t.candles && t.session_bands);
        assert!(!t.volume && !t.grid && !t.crosshair && !t.annotations);
    }

    #[test]
    fn build_errors_on_missing_axis() {
        let err = ChartScene::builder()
            .price_range(pr())
            .viewport(vp())
            .build()
            .unwrap_err();
        assert!(matches!(err, SceneError::MissingAxis));
    }

    #[test]
    fn build_errors_on_missing_price_range() {
        let err = ChartScene::builder()
            .axis(axis())
            .viewport(vp())
            .build()
            .unwrap_err();
        assert!(matches!(err, SceneError::MissingPriceRange));
    }

    #[test]
    fn build_errors_on_missing_viewport() {
        let err = ChartScene::builder()
            .axis(axis())
            .price_range(pr())
            .build()
            .unwrap_err();
        assert!(matches!(err, SceneError::MissingViewport));
    }

    #[test]
    fn build_sorts_layers_by_layer_z() {
        // Insert in reverse-z order; expect build to sort ascending.
        let counter = Arc::new(AtomicUsize::new(0));
        let order_a = Arc::new(AtomicUsize::new(usize::MAX));
        let order_b = Arc::new(AtomicUsize::new(usize::MAX));
        let order_c = Arc::new(AtomicUsize::new(usize::MAX));

        let crosshair = RecordingLayer {
            z: LayerZ::CROSSHAIR,
            tag: 10,
            counter: counter.clone(),
            order: order_a.clone(),
        };
        let grid = RecordingLayer {
            z: LayerZ::GRID,
            tag: 1,
            counter: counter.clone(),
            order: order_b.clone(),
        };
        let bands = RecordingLayer {
            z: LayerZ::SESSION_BAND,
            tag: 0,
            counter: counter.clone(),
            order: order_c.clone(),
        };

        let scene = ChartScene::builder()
            .axis(axis())
            .price_range(pr())
            .viewport(vp())
            .layer(crosshair)
            .layer(grid)
            .layer(bands)
            .build()
            .unwrap();

        let mut out = ScenePrimitives::default();
        scene.paint(&mut out);

        // `bands` should paint first (0), `grid` second (1),
        // `crosshair` third (2).
        assert_eq!(order_c.load(Ordering::SeqCst), 0, "band painted first");
        assert_eq!(order_b.load(Ordering::SeqCst), 1, "grid painted second");
        assert_eq!(order_a.load(Ordering::SeqCst), 2, "crosshair painted last");
    }

    #[test]
    fn same_z_layers_preserve_insertion_order() {
        // Two PriceLineLayer instances at the same z; check that the
        // first inserted still paints first. We assert on the Vec
        // position of each layer's emitted `LineInstance`.
        let a = PriceLineLayer::new(vec![PriceLineView {
            id: 1,
            price: 100.0,
            label: Cow::Borrowed("A"),
            color: [1, 0, 0, 255],
        }]);
        let b = PriceLineLayer::new(vec![PriceLineView {
            id: 2,
            price: 101.0,
            label: Cow::Borrowed("B"),
            color: [2, 0, 0, 255],
        }]);

        let scene = ChartScene::builder()
            .axis(axis())
            .price_range(pr())
            .viewport(vp())
            .layer(a)
            .layer(b)
            .build()
            .unwrap();

        let mut out = ScenePrimitives::default();
        scene.paint(&mut out);
        // `a` emits first → out.lines[0].color[0] == 1; `b` second → out.lines[1].color[0] == 2.
        assert_eq!(out.lines[0].color[0], 1);
        assert_eq!(out.lines[1].color[0], 2);
    }

    #[test]
    fn same_z_layers_across_kinds_preserve_insertion_order() {
        // PriceLineLayer (z=PriceLine=6) vs. LevelLayer (z=Level=8).
        // This crosses z-ordinals; `PriceLine` should still paint first
        // (lower z). Then within Level, if we had two, they'd keep
        // insertion order — this test demonstrates both.
        let p = PriceLineLayer::new(vec![PriceLineView {
            id: 1,
            price: 100.0,
            label: Cow::Borrowed("P"),
            color: [9, 0, 0, 255],
        }]);
        let l1 = LevelLayer::new(vec![LevelView {
            id: 1,
            price: 101.0,
            label: Cow::Borrowed("L1"),
            color: [7, 0, 0, 255],
        }]);
        let l2 = LevelLayer::new(vec![LevelView {
            id: 2,
            price: 102.0,
            label: Cow::Borrowed("L2"),
            color: [8, 0, 0, 255],
        }]);

        // Insert Level2 BEFORE Level1 but both after PriceLine —
        // Level1 and Level2 share `LayerZ::LEVEL` so the insertion
        // order of `.layer(l2).layer(l1)` determines paint order.
        let scene = ChartScene::builder()
            .axis(axis())
            .price_range(pr())
            .viewport(vp())
            .layer(p)
            .layer(l2)
            .layer(l1)
            .build()
            .unwrap();

        let mut out = ScenePrimitives::default();
        scene.paint(&mut out);
        // Price line goes first (z=6), then l2 (insertion 1 among same-z), then l1 (insertion 2).
        assert_eq!(out.lines[0].color[0], 9, "price line paints first");
        assert_eq!(out.lines[1].color[0], 8, "l2 paints before l1");
        assert_eq!(out.lines[2].color[0], 7, "l1 paints last");
    }

    #[test]
    fn paint_clears_previous_primitives() {
        let scene = ChartScene::builder()
            .axis(axis())
            .price_range(pr())
            .viewport(vp())
            .layer(PriceLineLayer::new(vec![PriceLineView {
                id: 1,
                price: 100.0,
                label: Cow::Borrowed("x"),
                color: [0, 0, 0, 255],
            }]))
            .build()
            .unwrap();
        let mut out = ScenePrimitives::default();
        // Pre-seed with garbage; paint must wipe it.
        out.lines.push(LineInstance {
            x0: 999.0,
            y0: 999.0,
            x1: 999.0,
            y1: 999.0,
            width_px: 1.0,
            color: [1, 2, 3, 4],
        });
        scene.paint(&mut out);
        assert_eq!(out.lines.len(), 1);
        assert_ne!(out.lines[0].x0, 999.0);
    }

    #[test]
    fn chart_scene_accessors() {
        let scene = ChartScene::builder()
            .axis(axis())
            .price_range(pr())
            .viewport(vp())
            .palette(ThemePalette::light_default())
            .build()
            .unwrap();
        assert_eq!(scene.viewport(), vp());
        assert_eq!(scene.price_range(), pr());
        assert_eq!(*scene.palette(), ThemePalette::light_default());
        assert_eq!(scene.layer_count(), 0);
    }
}
