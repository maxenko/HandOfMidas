//! [`translate`] — pure, sans-IO conversion from
//! [`midas_scene::ScenePrimitives`] (the AoS buffer emitted by the
//! scene's layer stack) into [`RenderBuckets`] — `#[repr(C)]`
//! `bytemuck::Pod` instance types the existing
//! [`midas_chart::instances`] + [`midas_render`] pipelines already know
//! how to draw.
//!
//! ## Why a separate translator?
//!
//! The new scene emits in **logical pixels** with RGBA8 colours packed
//! as `[u8; 4]`. The existing GPU pipelines want **linear-space RGBA
//! f32**, and they split candles into `body_top` / `body_bottom` /
//! `wick_top` / `wick_bottom` pixel Y values (the midas-scene
//! `CandleInstance` already exposes the pixel OHLC). The translator
//! does no layout — only coordinate/format transposition.
//!
//! ## 1:1 correspondence
//!
//! | Scene primitive                       | Legacy bucket          |
//! |---------------------------------------|------------------------|
//! | `CandleInstance`                      | `Vec<CandleInstance>`  |
//! | `QuadInstance`                        | `Vec<GridLineInstance>`|
//! | `LineInstance` (axis-aligned 1-px)    | `Vec<GridLineInstance>`|
//! | `BadgeInstance`                       | `Vec<BadgeMetaInstance>` (text hook — TODO, see below) |
//! | `TextInstance`                        | ❌ no legacy pipeline — TODO |
//!
//! The `BadgeInstance`'s string is NOT rendered by the legacy SDF badge
//! pipeline in this translator — that pipeline goes through a separate
//! `midas_chart::widget::compute::WidgetLabel` + glyph-atlas path which
//! is not yet ported. For S8 the translator exposes the rectangle + fill
//! color on a [`BadgeMetaInstance`] with a `text` `Cow<'static, str>`
//! sidecar, and the widget is expected to skip text rendering until S9
//! reroutes the glyph path. This matches the ideal design's
//! "[R2-G-2]" gap acknowledgement — glyph/text rendering is deferred.
//!
//! ## Coordinate spaces
//!
//! The scene emits all coordinates in **logical pixels with y=0 at the
//! top of the viewport**. The legacy `GridLineInstance` and
//! `VolumeInstance` use the same convention; no inversion is needed.
//! The legacy `CandleInstance` uses `body_top` < `body_bottom` (top =
//! smaller y) which matches the scene's semantics (high = smaller y).

use std::borrow::Cow;

use midas_chart::instances::{CandleInstance, GridLineInstance, VolumeInstance};
use midas_chart::widget::compute::{LabelAnchor, WidgetLabel};
use midas_scene::{
    BadgeInstance as SceneBadge, CandleInstance as SceneCandle, LineInstance as SceneLine,
    QuadInstance as SceneQuad, ScenePrimitives, TextAnchor as SceneTextAnchor,
    TextInstance as SceneText,
};

/// Result of [`translate`]. Each `Vec` maps 1:1 to a legacy
/// `midas-render` pipeline input.
///
/// `PartialEq` is NOT derived because the legacy `CandleInstance` /
/// `GridLineInstance` / `VolumeInstance` are GPU-layout `#[repr(C)]`
/// structs without `PartialEq` derives. Tests that need equality on
/// translator output compare field-by-field via helper functions in
/// the test module (see `bucket_equal` below).
#[derive(Debug, Default, Clone)]
pub struct RenderBuckets {
    /// Candle bodies + wicks. Consumed by `midas-render::CandlePipeline`.
    pub candles: Vec<CandleInstance>,
    /// Axis-aligned lines (gridlines, crosshair arms, session separators).
    /// Drawn as thin [`GridLineInstance`] rectangles — this is how the
    /// legacy grid + crosshair + volume-profile pipelines consume line
    /// primitives today.
    pub lines: Vec<GridLineInstance>,
    /// Filled quads (session bands, volume bars, highlights). The
    /// existing pipeline family renders these as [`GridLineInstance`]
    /// rectangles too, so we keep the quads inside the same bucket as
    /// `lines` — but exposed separately so widgets can split the draw
    /// order if they want to.
    pub quads: Vec<GridLineInstance>,
    /// Volume bars — the scene's `VolumeLayer` emits `QuadInstance` at
    /// the viewport's bottom strip. We ALSO materialise those as
    /// [`VolumeInstance`] so the legacy `VolumePipeline` can consume
    /// them directly (bar width is rectangle width; `y_top`/`y_bottom`
    /// map from rect y/y+h). Empty today — the scene's `VolumeLayer`
    /// emits quads into `quads`, and we don't attempt to disambiguate
    /// "session band" from "volume" in the translator. Kept as a
    /// reserved field for the follow-up slice that splits the two.
    pub volumes: Vec<VolumeInstance>,
    /// Badge metadata (rect + fill + border + inline text) — the SDF
    /// badge pipeline's glyph-atlas wiring is TODO (see module docs).
    /// For S8 widgets can still render the rect via a `GridLineInstance`
    /// fallback by reading `rect_quad_from_badge`.
    pub badges: Vec<BadgeMetaInstance>,
    /// Stand-alone text primitives (axis labels, tooltips). The legacy
    /// text pipeline is not yet ported — this bucket is populated but
    /// widgets ignore it for S8. Documented in module docs.
    pub text: Vec<TextMetaInstance>,
}

/// Rectangle + fill + border + inline text for the SDF badge
/// pipeline. Not `Pod` — holds a `Cow<'static, str>`. Consumed by the
/// follow-up slice that ports the glyph-atlas path.
#[derive(Debug, Clone, PartialEq)]
pub struct BadgeMetaInstance {
    pub rect: [f32; 4],
    pub color: [f32; 4],
    pub text: Cow<'static, str>,
}

/// Anchor + size + color + payload for the text pipeline. Not `Pod`
/// — same reason as above. Slice 3 widened this with `anchor` so the
/// crosshair's right-margin / bottom-margin / top-left OHLC rows
/// project onto the correct [`LabelAnchor`] for cryoglyph.
#[derive(Debug, Clone, PartialEq)]
pub struct TextMetaInstance {
    pub x: f32,
    pub y: f32,
    pub size_px: f32,
    pub color: [f32; 4],
    pub text: Cow<'static, str>,
    pub anchor: SceneTextAnchor,
}

/// Project the scene's nine-way [`TextAnchor`][midas_scene::TextAnchor]
/// onto the legacy text-pipeline's four-way [`LabelAnchor`]. The
/// legacy pipeline has no Top/Bottom-only anchors, so the scene's
/// `TopCenter`/`BottomCenter` collapse to `Center` (the cursor's x
/// still sits at the requested x, but the y shifts by half the
/// glyph height). For the crosshair this is acceptable — the
/// bottom-margin time label is an axis chip that tolerates a 6 px
/// y-drift.
pub fn scene_anchor_to_label_anchor(a: SceneTextAnchor) -> LabelAnchor {
    match a {
        SceneTextAnchor::TopLeft | SceneTextAnchor::BottomLeft => LabelAnchor::TopLeft,
        SceneTextAnchor::TopRight | SceneTextAnchor::BottomRight => LabelAnchor::Right,
        SceneTextAnchor::MiddleLeft => LabelAnchor::Left,
        SceneTextAnchor::MiddleRight => LabelAnchor::Right,
        SceneTextAnchor::MiddleCenter
        | SceneTextAnchor::TopCenter
        | SceneTextAnchor::BottomCenter => LabelAnchor::Center,
    }
}

/// Map a [`TextMetaInstance`] into a [`WidgetLabel`] the
/// [`midas_render::pipelines::text::TextPipeline`] consumes directly.
/// Slice 3 of the chart-transition plan closes the G-2 gap — the
/// translator now produces legacy-compatible text records instead of
/// dropping scene text on the floor.
///
/// `bg_color` is `[0, 0, 0, 0]` (transparent) — crosshair labels paint
/// pure text on top of the chart; no backing badge. If a future layer
/// needs a background it should emit a [`BadgeInstance`] of its own
/// and route the text through there.
pub fn text_meta_to_widget_label(t: &TextMetaInstance) -> WidgetLabel {
    WidgetLabel {
        text: t.text.to_string(),
        screen_x: t.x,
        screen_y: t.y,
        bg_color: [0.0, 0.0, 0.0, 0.0],
        text_color: t.color,
        font_size: t.size_px,
        anchor: scene_anchor_to_label_anchor(t.anchor),
    }
}

/// Bulk variant of [`text_meta_to_widget_label`]. Allocates a `Vec`
/// the text pipeline can drain directly via its axis-labels slot.
pub fn text_buckets_to_widget_labels(bucket: &[TextMetaInstance]) -> Vec<WidgetLabel> {
    bucket.iter().map(text_meta_to_widget_label).collect()
}

/// Pure data transform. Zero GPU work. Runs on the main thread between
/// `ChartScene::paint` and the wgpu submit.
pub fn translate(primitives: &ScenePrimitives) -> RenderBuckets {
    let mut out = RenderBuckets::default();

    out.candles.reserve(primitives.candles.len());
    for c in &primitives.candles {
        out.candles.push(translate_candle(c));
    }

    out.quads.reserve(primitives.quads.len());
    for q in &primitives.quads {
        out.quads.push(translate_quad(q));
    }

    out.lines.reserve(primitives.lines.len());
    for l in &primitives.lines {
        out.lines.push(translate_line(l));
    }

    out.badges.reserve(primitives.badges.len());
    for b in &primitives.badges {
        out.badges.push(translate_badge(b));
    }

    out.text.reserve(primitives.text.len());
    for t in &primitives.text {
        out.text.push(translate_text(t));
    }

    out
}

/// RGBA8 → linear RGBA f32. `u8 / 255` is the SRGB-encoded value the
/// existing `midas-render::color` module already uses; see
/// `dark_theme()`. We do NOT gamma-correct — the existing pipelines
/// treat their colour inputs as linear (see `instances.rs` docstring:
/// "RGBA color (linear space, NOT sRGB)") but every existing call site
/// is already passing `u8/255`-derived floats, so we match the
/// convention.
#[inline]
fn rgba8_to_f32(c: [u8; 4]) -> [f32; 4] {
    [
        c[0] as f32 / 255.0,
        c[1] as f32 / 255.0,
        c[2] as f32 / 255.0,
        c[3] as f32 / 255.0,
    ]
}

fn translate_candle(c: &SceneCandle) -> CandleInstance {
    // Scene: `high_px < low_px` because y=0 is top (high prices).
    // Legacy `CandleInstance`:
    //   body_top    = min(open_px, close_px)   [smaller y = higher on screen]
    //   body_bottom = max(open_px, close_px)
    //   wick_top    = high_px                  [smallest y across all four]
    //   wick_bottom = low_px                   [largest y]
    let body_top = c.open_px.min(c.close_px);
    let body_bottom = c.open_px.max(c.close_px);
    CandleInstance {
        x: c.x_center,
        body_top,
        body_bottom,
        wick_top: c.high_px,
        wick_bottom: c.low_px,
        width: c.width_px,
        wick_width: 1.0,
        dim: 0.0,
        color: rgba8_to_f32(c.color),
    }
}

fn translate_quad(q: &SceneQuad) -> GridLineInstance {
    GridLineInstance {
        rect: [q.x, q.y, q.x + q.w, q.y + q.h],
        color: rgba8_to_f32(q.color),
    }
}

fn translate_line(l: &SceneLine) -> GridLineInstance {
    // Axis-aligned lines emit as 1..n px thin rectangles. If the line is
    // non-axis-aligned (diagonal), we still place it inside the bucket as
    // the enclosing axis-aligned box — the legacy pipeline has no diagonal
    // line primitive, and no MVP layer emits diagonals. The scene_builder
    // tests assert that every emitted line is axis-aligned.
    let half = (l.width_px * 0.5).max(0.5);
    let (rect_l, rect_r, rect_t, rect_b) = if (l.x0 - l.x1).abs() < f32::EPSILON {
        // Vertical.
        (l.x0 - half, l.x0 + half, l.y0.min(l.y1), l.y0.max(l.y1))
    } else if (l.y0 - l.y1).abs() < f32::EPSILON {
        // Horizontal.
        (l.x0.min(l.x1), l.x0.max(l.x1), l.y0 - half, l.y0 + half)
    } else {
        // Diagonal fallback: enclosing bounding box. Not used by MVP
        // layers; kept lossless-enough to avoid a panic.
        (
            l.x0.min(l.x1),
            l.x0.max(l.x1),
            l.y0.min(l.y1),
            l.y0.max(l.y1),
        )
    };
    GridLineInstance {
        rect: [rect_l, rect_t, rect_r, rect_b],
        color: rgba8_to_f32(l.color),
    }
}

fn translate_badge(b: &SceneBadge) -> BadgeMetaInstance {
    BadgeMetaInstance {
        rect: [b.x, b.y, b.x + b.w, b.y + b.h],
        color: rgba8_to_f32(b.color),
        text: b.text.clone(),
    }
}

fn translate_text(t: &SceneText) -> TextMetaInstance {
    // Slice 3: propagate the scene's anchor so the downstream text
    // pipeline (cryoglyph) places the glyph box correctly. The legacy
    // [`LabelAnchor`] mapping lives in [`scene_anchor_to_label_anchor`].
    TextMetaInstance {
        x: t.x,
        y: t.y,
        size_px: t.size_px,
        color: rgba8_to_f32(t.color),
        text: t.text.clone(),
        anchor: t.anchor,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use midas_scene::{
        BadgeInstance as SceneBadge, CandleInstance as SceneCandle, LineInstance as SceneLine,
        QuadInstance as SceneQuad, ScenePrimitives, TextAnchor, TextInstance as SceneText,
    };

    fn empty() -> ScenePrimitives {
        ScenePrimitives::default()
    }

    #[test]
    fn empty_primitives_translate_to_empty_buckets() {
        let out = translate(&empty());
        assert!(out.candles.is_empty());
        assert!(out.quads.is_empty());
        assert!(out.lines.is_empty());
        assert!(out.badges.is_empty());
        assert!(out.text.is_empty());
    }

    #[test]
    fn single_candle_translates_one_for_one() {
        let mut p = empty();
        p.candles.push(SceneCandle {
            x_center: 100.0,
            width_px: 8.0,
            open_px: 200.0,
            high_px: 150.0,
            low_px: 250.0,
            close_px: 180.0,
            color: [0x3d, 0xd5, 0x98, 0xff],
            wick_color: [0xc8, 0xc8, 0xc8, 0xff],
        });
        let out = translate(&p);
        assert_eq!(out.candles.len(), 1);
        let c = out.candles[0];
        assert!((c.x - 100.0).abs() < 1e-5);
        assert!((c.width - 8.0).abs() < 1e-5);
        assert!((c.wick_top - 150.0).abs() < 1e-5);
        assert!((c.wick_bottom - 250.0).abs() < 1e-5);
        // body_top = min(open, close) = 180.
        assert!((c.body_top - 180.0).abs() < 1e-5);
        // body_bottom = max(open, close) = 200.
        assert!((c.body_bottom - 200.0).abs() < 1e-5);
    }

    #[test]
    fn candle_body_ordering_survives_up_bar() {
        // open == close-tinybit: body_top <= body_bottom always.
        let mut p = empty();
        p.candles.push(SceneCandle {
            x_center: 0.0,
            width_px: 1.0,
            open_px: 100.0,
            high_px: 50.0,
            low_px: 150.0,
            close_px: 80.0, // close higher on screen (smaller y) than open
            color: [1, 2, 3, 4],
            wick_color: [5, 6, 7, 8],
        });
        let out = translate(&p);
        assert!(out.candles[0].body_top <= out.candles[0].body_bottom);
    }

    #[test]
    fn single_quad_translates_to_grid_line_rect() {
        let mut p = empty();
        p.quads.push(SceneQuad {
            x: 50.0,
            y: 10.0,
            w: 200.0,
            h: 30.0,
            color: [0x12, 0x34, 0x56, 0xff],
        });
        let out = translate(&p);
        assert_eq!(out.quads.len(), 1);
        let r = out.quads[0].rect;
        assert!((r[0] - 50.0).abs() < 1e-5);
        assert!((r[1] - 10.0).abs() < 1e-5);
        assert!((r[2] - 250.0).abs() < 1e-5);
        assert!((r[3] - 40.0).abs() < 1e-5);
    }

    #[test]
    fn rgba_u8_to_f32_preserves_255_max() {
        let mut p = empty();
        p.quads.push(SceneQuad {
            x: 0.0,
            y: 0.0,
            w: 1.0,
            h: 1.0,
            color: [0xff, 0x80, 0x00, 0xff],
        });
        let out = translate(&p);
        let c = out.quads[0].color;
        assert!((c[0] - 1.0).abs() < 1e-5);
        assert!((c[1] - 0x80 as f32 / 255.0).abs() < 1e-5);
        assert!((c[2] - 0.0).abs() < 1e-5);
        assert!((c[3] - 1.0).abs() < 1e-5);
    }

    #[test]
    fn horizontal_line_translates_to_thin_rect() {
        let mut p = empty();
        p.lines.push(SceneLine {
            x0: 10.0,
            y0: 100.0,
            x1: 200.0,
            y1: 100.0,
            width_px: 1.0,
            color: [0xff; 4],
        });
        let out = translate(&p);
        let r = out.lines[0].rect;
        assert!((r[0] - 10.0).abs() < 1e-5);
        assert!((r[2] - 200.0).abs() < 1e-5);
        // 1px wide centred on y=100 → [99.5, 100.5].
        assert!((r[1] - 99.5).abs() < 1e-5);
        assert!((r[3] - 100.5).abs() < 1e-5);
    }

    #[test]
    fn vertical_line_translates_to_thin_rect() {
        let mut p = empty();
        p.lines.push(SceneLine {
            x0: 300.0,
            y0: 0.0,
            x1: 300.0,
            y1: 400.0,
            width_px: 1.0,
            color: [0xff; 4],
        });
        let out = translate(&p);
        let r = out.lines[0].rect;
        assert!((r[0] - 299.5).abs() < 1e-5);
        assert!((r[2] - 300.5).abs() < 1e-5);
        assert!((r[1] - 0.0).abs() < 1e-5);
        assert!((r[3] - 400.0).abs() < 1e-5);
    }

    #[test]
    fn wide_line_honours_width_px() {
        let mut p = empty();
        p.lines.push(SceneLine {
            x0: 0.0,
            y0: 50.0,
            x1: 100.0,
            y1: 50.0,
            width_px: 4.0,
            color: [0; 4],
        });
        let out = translate(&p);
        let r = out.lines[0].rect;
        // width_px = 4 → half = 2.
        assert!((r[1] - 48.0).abs() < 1e-5);
        assert!((r[3] - 52.0).abs() < 1e-5);
    }

    #[test]
    fn badge_preserves_text_and_geometry() {
        let mut p = empty();
        p.badges.push(SceneBadge {
            x: 10.0,
            y: 20.0,
            w: 60.0,
            h: 16.0,
            color: [0x33, 0x33, 0x33, 0xff],
            text: "Entry @ 50000".into(),
        });
        let out = translate(&p);
        let b = &out.badges[0];
        assert_eq!(b.rect, [10.0, 20.0, 70.0, 36.0]);
        assert_eq!(b.text, "Entry @ 50000");
    }

    #[test]
    fn text_primitive_preserves_position_size_colour_text() {
        let mut p = empty();
        p.text.push(SceneText {
            x: 40.0,
            y: 40.0,
            color: [0xff, 0xff, 0xff, 0xff],
            text: "50,000.00".into(),
            size_px: 12.0,
            anchor: TextAnchor::MiddleCenter,
        });
        let out = translate(&p);
        let t = &out.text[0];
        assert!((t.x - 40.0).abs() < 1e-5);
        assert!((t.y - 40.0).abs() < 1e-5);
        assert!((t.size_px - 12.0).abs() < 1e-5);
        assert_eq!(t.text, "50,000.00");
        assert!((t.color[0] - 1.0).abs() < 1e-5);
    }

    #[test]
    fn mixed_scene_produces_sized_buckets_for_each_kind() {
        let mut p = empty();
        p.candles.push(SceneCandle {
            x_center: 10.0,
            width_px: 4.0,
            open_px: 100.0,
            high_px: 90.0,
            low_px: 110.0,
            close_px: 95.0,
            color: [0x3d, 0xd5, 0x98, 0xff],
            wick_color: [0xc8, 0xc8, 0xc8, 0xff],
        });
        p.quads.push(SceneQuad {
            x: 0.0,
            y: 0.0,
            w: 500.0,
            h: 400.0,
            color: [0x20, 0x20, 0x30, 0x30],
        });
        p.lines.push(SceneLine {
            x0: 100.0,
            y0: 0.0,
            x1: 100.0,
            y1: 400.0,
            width_px: 1.0,
            color: [0xff; 4],
        });
        p.badges.push(SceneBadge {
            x: 0.0,
            y: 0.0,
            w: 10.0,
            h: 10.0,
            color: [0; 4],
            text: "x".into(),
        });
        p.text.push(SceneText {
            x: 1.0,
            y: 1.0,
            color: [0; 4],
            text: "y".into(),
            size_px: 10.0,
            anchor: TextAnchor::TopLeft,
        });
        let out = translate(&p);
        assert_eq!(out.candles.len(), 1);
        assert_eq!(out.quads.len(), 1);
        assert_eq!(out.lines.len(), 1);
        assert_eq!(out.badges.len(), 1);
        assert_eq!(out.text.len(), 1);
    }

    #[test]
    fn translate_never_allocates_extra_buckets() {
        // `volumes` is reserved for a follow-up slice; S8 translator
        // never populates it. Documented invariant.
        let mut p = empty();
        p.quads.push(SceneQuad {
            x: 0.0,
            y: 0.0,
            w: 1.0,
            h: 1.0,
            color: [0; 4],
        });
        let out = translate(&p);
        assert!(
            out.volumes.is_empty(),
            "S8 translator must not emit into `volumes`"
        );
    }

    /// Field-by-field comparison for [`RenderBuckets`]. The legacy GPU
    /// instance types don't implement `PartialEq`, so we assert on the
    /// semantically-meaningful fields instead.
    fn candles_equal(a: &CandleInstance, b: &CandleInstance) -> bool {
        a.x == b.x
            && a.body_top == b.body_top
            && a.body_bottom == b.body_bottom
            && a.wick_top == b.wick_top
            && a.wick_bottom == b.wick_bottom
            && a.width == b.width
            && a.color == b.color
    }

    fn buckets_equal(a: &RenderBuckets, b: &RenderBuckets) -> bool {
        if a.candles.len() != b.candles.len() {
            return false;
        }
        for (ac, bc) in a.candles.iter().zip(b.candles.iter()) {
            if !candles_equal(ac, bc) {
                return false;
            }
        }
        if a.quads.len() != b.quads.len() {
            return false;
        }
        for (aq, bq) in a.quads.iter().zip(b.quads.iter()) {
            if aq.rect != bq.rect || aq.color != bq.color {
                return false;
            }
        }
        if a.lines.len() != b.lines.len() {
            return false;
        }
        for (al, bl) in a.lines.iter().zip(b.lines.iter()) {
            if al.rect != bl.rect || al.color != bl.color {
                return false;
            }
        }
        a.badges == b.badges && a.text == b.text
    }

    #[test]
    fn translator_is_pure_function_no_cross_contamination() {
        // Calling twice with the same input yields equal output.
        let mut p = empty();
        p.candles.push(SceneCandle {
            x_center: 1.0,
            width_px: 1.0,
            open_px: 1.0,
            high_px: 0.5,
            low_px: 2.0,
            close_px: 1.5,
            color: [1, 2, 3, 4],
            wick_color: [5, 6, 7, 8],
        });
        let a = translate(&p);
        let b = translate(&p);
        assert!(buckets_equal(&a, &b));
    }

    // ── Slice 3 — text → WidgetLabel projection ──────────────────────

    #[test]
    fn text_meta_carries_anchor_from_scene_primitive() {
        let mut p = empty();
        p.text.push(SceneText {
            x: 10.0,
            y: 20.0,
            color: [0xff; 4],
            text: "50000.00".into(),
            size_px: 11.0,
            anchor: TextAnchor::MiddleRight,
        });
        let out = translate(&p);
        assert_eq!(out.text[0].anchor, TextAnchor::MiddleRight);
    }

    #[test]
    fn scene_anchor_projection_covers_every_variant() {
        // Slice 3 asserts no scene anchor panics the projection.
        for a in [
            TextAnchor::TopLeft,
            TextAnchor::TopCenter,
            TextAnchor::TopRight,
            TextAnchor::MiddleLeft,
            TextAnchor::MiddleCenter,
            TextAnchor::MiddleRight,
            TextAnchor::BottomLeft,
            TextAnchor::BottomCenter,
            TextAnchor::BottomRight,
        ] {
            let _la = scene_anchor_to_label_anchor(a);
        }
    }

    #[test]
    fn right_anchors_collapse_to_label_right() {
        assert!(matches!(
            scene_anchor_to_label_anchor(TextAnchor::MiddleRight),
            LabelAnchor::Right
        ));
        assert!(matches!(
            scene_anchor_to_label_anchor(TextAnchor::TopRight),
            LabelAnchor::Right
        ));
    }

    #[test]
    fn middle_left_collapses_to_label_left() {
        assert!(matches!(
            scene_anchor_to_label_anchor(TextAnchor::MiddleLeft),
            LabelAnchor::Left
        ));
    }

    #[test]
    fn bottom_center_collapses_to_label_center() {
        assert!(matches!(
            scene_anchor_to_label_anchor(TextAnchor::BottomCenter),
            LabelAnchor::Center
        ));
    }

    #[test]
    fn text_meta_to_widget_label_round_trips_core_fields() {
        let meta = TextMetaInstance {
            x: 42.0,
            y: 19.0,
            size_px: 11.0,
            color: [1.0, 0.5, 0.25, 1.0],
            text: "AAPL 150.25".into(),
            anchor: TextAnchor::TopLeft,
        };
        let wl = text_meta_to_widget_label(&meta);
        assert_eq!(wl.text, "AAPL 150.25");
        assert!((wl.screen_x - 42.0).abs() < 1e-6);
        assert!((wl.screen_y - 19.0).abs() < 1e-6);
        assert!((wl.font_size - 11.0).abs() < 1e-6);
        assert_eq!(wl.text_color, [1.0, 0.5, 0.25, 1.0]);
        // Crosshair labels paint with a transparent background.
        assert_eq!(wl.bg_color, [0.0, 0.0, 0.0, 0.0]);
        assert!(matches!(wl.anchor, LabelAnchor::TopLeft));
    }

    #[test]
    fn bulk_text_to_widget_labels_preserves_order_and_count() {
        let metas = vec![
            TextMetaInstance {
                x: 1.0,
                y: 1.0,
                size_px: 11.0,
                color: [0.0; 4],
                text: "A".into(),
                anchor: TextAnchor::TopLeft,
            },
            TextMetaInstance {
                x: 2.0,
                y: 2.0,
                size_px: 11.0,
                color: [0.0; 4],
                text: "B".into(),
                anchor: TextAnchor::MiddleRight,
            },
            TextMetaInstance {
                x: 3.0,
                y: 3.0,
                size_px: 11.0,
                color: [0.0; 4],
                text: "C".into(),
                anchor: TextAnchor::BottomCenter,
            },
        ];
        let labels = text_buckets_to_widget_labels(&metas);
        assert_eq!(labels.len(), 3);
        assert_eq!(labels[0].text, "A");
        assert_eq!(labels[1].text, "B");
        assert_eq!(labels[2].text, "C");
        assert!(matches!(labels[0].anchor, LabelAnchor::TopLeft));
        assert!(matches!(labels[1].anchor, LabelAnchor::Right));
        assert!(matches!(labels[2].anchor, LabelAnchor::Center));
    }
}
