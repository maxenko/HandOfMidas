//! Primitive vocabulary emitted by layers and consumed by the GPU renderer.
//!
//! Per the R2-NB-3 resolution (see `00a-ideal-design.md` →
//! "Scene — composable layers"), scene layers do NOT speak GPU types.
//! They fill a shared [`ScenePrimitives`] AoS-of-Vecs with typed instance
//! records. The renderer walks the typed vectors and batches them into
//! draw calls — but that is outside this crate.
//!
//! Everything here is `Copy` where possible (text-carrying variants hold
//! a `Cow<'static, str>` — `Clone`, not `Copy`) so layer paint functions
//! can write into the buffer with zero per-primitive allocation.

use std::borrow::Cow;

/// One candle body + wick. Rectangle spanning `[x_center - width_px/2,
/// x_center + width_px/2]` on x; `[min(open_px, close_px), max(open_px,
/// close_px)]` on y. The wick line runs from `high_px` to `low_px` at
/// `x_center`. Colours are RGBA8.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct CandleInstance {
    pub x_center: f32,
    pub width_px: f32,
    pub open_px: f32,
    pub high_px: f32,
    pub low_px: f32,
    pub close_px: f32,
    pub color: [u8; 4],
    pub wick_color: [u8; 4],
}

/// Filled axis-aligned rectangle. Used for session bands, volume bars,
/// highlight overlays, and interaction drag-rects.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct QuadInstance {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub color: [u8; 4],
}

/// Straight line segment with a stroke width. Used for gridlines,
/// separators, price lines, crosshair arms, bracket legs.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct LineInstance {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
    pub width_px: f32,
    pub color: [u8; 4],
}

/// Filled badge with an inline label. Used for order-bracket leg
/// badges, holiday markers, decorator chips.
#[derive(Clone, Debug, PartialEq)]
pub struct BadgeInstance {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub color: [u8; 4],
    pub text: Cow<'static, str>,
}

/// Standalone text at a pixel anchor. Used for axis labels, price-line
/// legends, tooltips.
#[derive(Clone, Debug, PartialEq)]
pub struct TextInstance {
    pub x: f32,
    pub y: f32,
    pub color: [u8; 4],
    pub text: Cow<'static, str>,
    pub size_px: f32,
    pub anchor: TextAnchor,
}

/// Anchor-relative text positioning. Interpretation: `(x, y)` is the
/// anchor point; the glyph bounding box is placed so its named vertex /
/// edge sits on the anchor.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum TextAnchor {
    TopLeft,
    TopCenter,
    TopRight,
    MiddleLeft,
    MiddleCenter,
    MiddleRight,
    BottomLeft,
    BottomCenter,
    BottomRight,
}

/// Aggregate output buffer handed to every [`SceneLayer`](crate::SceneLayer)
/// via [`PaintContext::out`](crate::PaintContext). Layers append; they
/// never clear or reorder. [`ChartScene::paint`](crate::ChartScene::paint)
/// clears the buffer ONCE before walking the layer stack.
#[derive(Default, Debug, Clone, PartialEq)]
pub struct ScenePrimitives {
    pub candles: Vec<CandleInstance>,
    pub quads: Vec<QuadInstance>,
    pub lines: Vec<LineInstance>,
    pub badges: Vec<BadgeInstance>,
    pub text: Vec<TextInstance>,
}

impl ScenePrimitives {
    /// Drain every typed vector. Called by
    /// [`ChartScene::paint`](crate::ChartScene::paint) so the caller
    /// hands the same buffer back frame after frame without
    /// reallocating — `Vec::clear` retains capacity.
    #[inline]
    pub fn clear(&mut self) {
        self.candles.clear();
        self.quads.clear();
        self.lines.clear();
        self.badges.clear();
        self.text.clear();
    }

    /// True iff no layer emitted anything.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.candles.is_empty()
            && self.quads.is_empty()
            && self.lines.is_empty()
            && self.badges.is_empty()
            && self.text.is_empty()
    }

    /// Total number of primitives across all typed vectors. Useful for
    /// tests and rough profiling.
    #[inline]
    pub fn total_len(&self) -> usize {
        self.candles.len()
            + self.quads.len()
            + self.lines.len()
            + self.badges.len()
            + self.text.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scene_primitives_default_is_empty() {
        let p = ScenePrimitives::default();
        assert!(p.is_empty());
        assert_eq!(p.total_len(), 0);
    }

    #[test]
    fn scene_primitives_clear_retains_capacity() {
        let mut p = ScenePrimitives::default();
        p.quads.push(QuadInstance {
            x: 0.0,
            y: 0.0,
            w: 10.0,
            h: 10.0,
            color: [0; 4],
        });
        assert!(!p.is_empty());
        let cap_before = p.quads.capacity();
        p.clear();
        assert!(p.is_empty());
        // Vec::clear keeps capacity; assert we did not drop it.
        assert!(p.quads.capacity() >= cap_before);
    }

    #[test]
    fn candle_instance_is_copy() {
        fn takes_copy<T: Copy>(_: T) {}
        takes_copy(CandleInstance {
            x_center: 0.0,
            width_px: 1.0,
            open_px: 0.0,
            high_px: 0.0,
            low_px: 0.0,
            close_px: 0.0,
            color: [0; 4],
            wick_color: [0; 4],
        });
    }

    #[test]
    fn total_len_sums_all_vectors() {
        let mut p = ScenePrimitives::default();
        p.quads.push(QuadInstance {
            x: 0.0,
            y: 0.0,
            w: 1.0,
            h: 1.0,
            color: [0; 4],
        });
        p.lines.push(LineInstance {
            x0: 0.0,
            y0: 0.0,
            x1: 1.0,
            y1: 1.0,
            width_px: 1.0,
            color: [0; 4],
        });
        p.badges.push(BadgeInstance {
            x: 0.0,
            y: 0.0,
            w: 1.0,
            h: 1.0,
            color: [0; 4],
            text: "".into(),
        });
        assert_eq!(p.total_len(), 3);
    }
}
