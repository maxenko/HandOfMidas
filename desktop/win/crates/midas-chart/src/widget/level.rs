//! Line rendering primitives shared by levels and bracket legs.
//!
//! Slice 7 of the decorator-system plan collapsed the renderer-side
//! `HorizontalLevel` and `LevelExtend` types that used to live here.
//! `LineStyle` and `segmented_line()` stayed — both are still used by
//! `compute_price_line_geometry` (below) and by
//! `widget::order_bracket::compute_bracket()`.
//!
//! The level compute entry point (`compute_level`) is a thin wrapper:
//! 1. `compute_price_line_geometry()` emits the line geometry, selection
//!    glow, drag ghost and the line-level `HitZoneKind::LevelLine` hit
//!    zone.
//! 2. For each `DecoratorGroup` returned by
//!    `HorizontalLevel::to_decorators(locked)`, a
//!    `compute_decorator_group()` call merges the decorator's primitives
//!    into the same `WidgetOutput`.

use crate::instances::GridLineInstance;
use crate::levels::HorizontalLevel;
use crate::widget::decorator::compute_decorator_group;
use crate::widget::price_line::PriceLine;
use serde::{Deserialize, Serialize};
use smallvec::{smallvec, SmallVec};

use super::compute::{ComputeContext, WidgetOutput};
use super::hit_test::{CursorIcon, HitZone, HitZoneKind};
use super::AnnotationId;

/// Line rendering style.
///
/// `Pattern` holds an SVG-style `stroke-dasharray`: alternating on/off run
/// lengths in logical pixels, walked cyclically starting with an "on" run.
/// An empty pattern is equivalent to `Solid`. Dashed and dotted lines are
/// rendered as multiple short `GridLineInstance` segments; the GPU pipeline
/// still draws axis-aligned rectangles.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum LineStyle {
    /// Continuous line.
    #[default]
    Solid,
    /// SVG-style dash pattern. Alternating on/off run lengths in logical
    /// pixels, walked cyclically. An empty pattern is equivalent to `Solid`.
    Pattern(SmallVec<[f32; 6]>),
}

impl LineStyle {
    /// 1-on / 3-off dotted rhythm.
    pub fn dotted() -> Self {
        Self::Pattern(smallvec![1.0, 3.0])
    }
    /// 1-on / 6-off sparse dotted rhythm.
    pub fn sparse_dotted() -> Self {
        Self::Pattern(smallvec![1.0, 6.0])
    }
    /// 6-on / 3-off dashed rhythm.
    pub fn dashed() -> Self {
        Self::Pattern(smallvec![6.0, 3.0])
    }
    /// 10-on / 4-off long-dash rhythm.
    pub fn dashed_long() -> Self {
        Self::Pattern(smallvec![10.0, 4.0])
    }
    /// 6-on / 3-off / 1-on / 3-off dash-dot rhythm.
    pub fn dash_dot() -> Self {
        Self::Pattern(smallvec![6.0, 3.0, 1.0, 3.0])
    }
    /// 6-on / 3-off / 1-on / 3-off / 1-on / 3-off dash-dot-dot rhythm.
    pub fn dash_dot_dot() -> Self {
        Self::Pattern(smallvec![6.0, 3.0, 1.0, 3.0, 1.0, 3.0])
    }

    /// True when this style draws as a single continuous segment.
    pub fn is_solid(&self) -> bool {
        match self {
            Self::Solid => true,
            Self::Pattern(p) => p.is_empty(),
        }
    }
}

// ── Shared line renderer ────────────────────────────────────────────

/// Split a horizontal line into segments based on `LineStyle`.
///
/// For `Solid` (or `Pattern` with an empty list), returns a single
/// `GridLineInstance`. For `Pattern`, walks the alternating on/off run
/// lengths cyclically starting with an "on" run, emitting one instance per
/// on-run clipped to `[x0, x1]`.
///
/// Used by both `compute_price_line_geometry()` and `compute_bracket()`.
pub fn segmented_line(
    x0: f32,
    x1: f32,
    y: f32,
    height: f32,
    color: [f32; 4],
    style: &LineStyle,
) -> Vec<GridLineInstance> {
    let solid_fallback = || {
        vec![GridLineInstance {
            rect: [x0, y, x1, y + height],
            color,
        }]
    };

    if x1 <= x0 {
        return Vec::new();
    }

    let pattern: &[f32] = match style {
        LineStyle::Solid => return solid_fallback(),
        LineStyle::Pattern(p) if p.is_empty() => return solid_fallback(),
        LineStyle::Pattern(p) => p,
    };

    // Sum of pattern runs. If the pattern is degenerate (all zero or
    // negative), fall back to a solid segment to avoid an infinite loop.
    let cycle: f32 = pattern.iter().copied().filter(|r| *r > 0.0).sum();
    if cycle <= 0.0 {
        return solid_fallback();
    }

    let total = x1 - x0;
    let expected = (total / cycle).ceil() as usize * (pattern.len() / 2 + 1);
    let mut segments = Vec::with_capacity(expected.max(1));

    let mut cursor = x0;
    let mut idx = 0usize;
    let mut is_on = true;
    while cursor < x1 {
        let run = pattern[idx % pattern.len()];
        if run > 0.0 {
            let end = (cursor + run).min(x1);
            if is_on && end > cursor {
                segments.push(GridLineInstance {
                    rect: [cursor, y, end, y + height],
                    color,
                });
            }
            cursor = end;
        }
        idx += 1;
        is_on = !is_on;
    }

    segments
}

// ── PriceLine geometry ──────────────────────────────────────────────

/// Emit the line primitives for a `PriceLine`: the segmented stroke,
/// selection glow, drag ghost, and the `HitZoneKind::LevelLine` hit
/// zone that drives drag-to-edit interactions.
///
/// This is the line-only half of `compute_level()`; decorator groups are
/// dispatched separately. Extracting it lets future annotation types
/// that share a `PriceLine` (e.g. the bracket legs in Slice 8a) reuse
/// the exact same line emission without duplicating the glow/ghost/hit
/// bookkeeping.
pub fn compute_price_line_geometry(
    line: &PriceLine,
    annotation_id: AnnotationId,
    ctx: &ComputeContext<'_>,
    alpha: f32,
    locked: bool,
) -> WidgetOutput {
    let mut output = WidgetOutput::default();
    let vp_width = ctx.viewport.width as f32;
    let y = ctx.camera.price_to_y(line.price);

    let mut color = line.stroke.color;
    color[3] *= alpha;

    // Selection glow: wider semi-transparent fill behind the line.
    let is_selected = ctx.selected_annotation == Some(annotation_id);
    if is_selected {
        let glow_thickness = line.stroke.width + ctx.theme.selection_thickness * 2.0;
        let glow_y = y - ctx.theme.selection_thickness;
        let mut glow_color = color;
        glow_color[3] = ctx.theme.selection_color[3] * alpha;
        output.fills.push(GridLineInstance {
            rect: [0.0, glow_y, vp_width, glow_y + glow_thickness],
            color: glow_color,
        });
    }

    // Hover highlight: thicker stroke when the cursor is over this line.
    let mut width = line.stroke.width;
    let is_hovered = ctx
        .hovered_annotation
        .map(|(aid, kind)| aid == annotation_id && kind == HitZoneKind::LevelLine)
        .unwrap_or(false);
    if is_hovered {
        width += 1.0;
    }

    output.lines.extend(segmented_line(
        0.0,
        vp_width,
        y,
        width,
        color,
        &line.stroke.style,
    ));

    // Drag ghost: faint line at the original price during drag.
    if let Some((ghost_id, ghost_price)) = ctx.drag_ghost {
        if ghost_id == annotation_id {
            let ghost_y = ctx.camera.price_to_y(ghost_price);
            let mut ghost_color = color;
            ghost_color[3] = color[3] * 0.2;
            output.fills.push(GridLineInstance {
                rect: [0.0, ghost_y, vp_width, ghost_y + 1.0],
                color: ghost_color,
            });
        }
    }

    // Hit zone (full width, ±6px). Locked lines use Crosshair cursor
    // (no drag affordance); unlocked use ResizeNS.
    let cursor = if locked {
        CursorIcon::Crosshair
    } else {
        CursorIcon::ResizeNS
    };
    output.hit_zones.push(HitZone {
        annotation_id,
        rect: [0.0, y - 6.0, vp_width, y + 6.0],
        kind: HitZoneKind::LevelLine,
        cursor,
    });

    output
}

// ── Level compute ───────────────────────────────────────────────────

/// Compute render primitives for a horizontal level annotation.
///
/// Slice 7 end state: a thin wrapper that composes
/// `compute_price_line_geometry()` (line geometry + line-level hit zone)
/// with a `compute_decorator_group()` call per group returned by
/// `HorizontalLevel::to_decorators(locked)`.
///
/// `locked` is sourced from the wrapper (`Annotation.locked` for the
/// widget path, or `StoredLevel.locked` for the legacy LevelStore path)
/// and forwarded to both the hit-zone cursor selector and
/// `to_decorators()` so the lock badge is emitted consistently.
pub fn compute_level(
    level: &HorizontalLevel,
    annotation_id: AnnotationId,
    ctx: &ComputeContext<'_>,
    alpha: f32,
    locked: bool,
) -> WidgetOutput {
    let mut output = compute_price_line_geometry(&level.line, annotation_id, ctx, alpha, locked);
    for group in level.to_decorators(locked) {
        output.merge(compute_decorator_group(
            &group,
            &level.line,
            annotation_id,
            ctx,
            alpha,
        ));
    }
    output
}
