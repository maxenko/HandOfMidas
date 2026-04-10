//! Horizontal level annotation type and shared line rendering.
//!
//! A `HorizontalLevel` represents a user-drawn horizontal line at a specific
//! price. `segmented_line()` is the shared line renderer used by both levels
//! and order brackets.

use crate::instances::GridLineInstance;
use crate::levels::LevelIcon;
use serde::{Deserialize, Serialize};

use super::compute::{ComputeContext, LabelAnchor, WidgetLabel, WidgetOutput};
use super::hit_test::{CursorIcon, HitZone, HitZoneKind};
use super::AnnotationId;

/// A horizontal line at a specific price.
///
/// The most common annotation type. Represents support/resistance levels,
/// moving average values, or any price of interest.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HorizontalLevel {
    /// Price at which the horizontal line is drawn.
    pub price: f64,
    /// RGBA color of the line (linear space, NOT sRGB).
    pub color: [f32; 4],
    /// Line width in logical pixels. Typical: 1.0-3.0.
    pub line_width: f32,
    /// Line rendering style (solid, dashed, dotted).
    pub style: LineStyle,
    /// Optional text label displayed next to the price on the Y axis.
    pub label: Option<String>,
    /// How far the line extends horizontally.
    pub extend: LevelExtend,
    /// Icon displayed next to the label.
    pub icon: LevelIcon,
}

/// Line rendering style.
///
/// Dashed and dotted lines are rendered as multiple short `GridLineInstance`
/// segments. The GPU pipeline is unchanged -- it still draws axis-aligned
/// rectangles. The segmentation happens in the compute phase.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum LineStyle {
    /// Continuous line.
    #[default]
    Solid,
    /// Alternating dash/gap segments.
    Dashed {
        /// Length of each dash segment in logical pixels.
        dash_len: f32,
        /// Length of each gap between dashes in logical pixels.
        gap_len: f32,
    },
    /// Regularly spaced dots.
    Dotted {
        /// Spacing between dot centers in logical pixels.
        dot_spacing: f32,
    },
}

/// How far a level line extends horizontally across the chart.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum LevelExtend {
    /// Spans the entire visible chart width. Most common.
    #[default]
    FullWidth,
    /// Starts at a specific time, extends infinitely to the right.
    RightFrom {
        /// Epoch milliseconds at which the line starts.
        timestamp: i64,
    },
    /// Bounded segment between two timestamps.
    Between {
        /// Start timestamp (epoch ms).
        start: i64,
        /// End timestamp (epoch ms).
        end: i64,
    },
}

// ── Shared line renderer ────────────────────────────────────────────

/// Split a full-width horizontal line into segments based on `LineStyle`.
///
/// For `Solid`, returns a single `GridLineInstance`. For `Dashed`, returns
/// `ceil(width / (dash + gap))` short rects. For `Dotted`, returns small
/// squares at regular intervals.
///
/// Used by both `compute_level()` and `compute_bracket()`.
pub fn segmented_line(
    x0: f32,
    x1: f32,
    y: f32,
    height: f32,
    color: [f32; 4],
    style: &LineStyle,
) -> Vec<GridLineInstance> {
    match style {
        LineStyle::Solid => vec![GridLineInstance {
            rect: [x0, y, x1, y + height],
            color,
        }],
        LineStyle::Dashed { dash_len, gap_len } => {
            let total = x1 - x0;
            let step = dash_len + gap_len;
            if step <= 0.0 || total <= 0.0 {
                return vec![GridLineInstance {
                    rect: [x0, y, x1, y + height],
                    color,
                }];
            }
            let count = (total / step).ceil() as usize;
            let mut segments = Vec::with_capacity(count);
            let mut cx = x0;
            while cx < x1 {
                let end = (cx + dash_len).min(x1);
                segments.push(GridLineInstance {
                    rect: [cx, y, end, y + height],
                    color,
                });
                cx += step;
            }
            segments
        }
        LineStyle::Dotted { dot_spacing } => {
            let total = x1 - x0;
            if *dot_spacing <= 0.0 || total <= 0.0 {
                return vec![GridLineInstance {
                    rect: [x0, y, x1, y + height],
                    color,
                }];
            }
            let count = (total / dot_spacing).ceil() as usize;
            let mut dots = Vec::with_capacity(count);
            let dot_size = height; // square dots
            let mut cx = x0;
            while cx < x1 {
                dots.push(GridLineInstance {
                    rect: [cx, y, cx + dot_size, y + height],
                    color,
                });
                cx += dot_spacing;
            }
            dots
        }
    }
}

// ── Level compute ───────────────────────────────────────────────────

/// Compute render primitives for a horizontal level annotation.
///
/// Produces a segmented line, a hit zone for interaction, and a price
/// label. Locked levels render normally but their hit zones use
/// `CursorIcon::Crosshair` (no drag affordance).
pub fn compute_level(
    level: &HorizontalLevel,
    annotation_id: AnnotationId,
    ctx: &ComputeContext<'_>,
    alpha: f32,
    locked: bool,
) -> WidgetOutput {
    let mut output = WidgetOutput::default();
    let vp_width = ctx.viewport.width as f32;
    let y = ctx.camera.price_to_y(level.price);

    let mut color = level.color;
    color[3] *= alpha;

    // Selection glow: wider semi-transparent fill behind the line.
    let is_selected = ctx.selected_annotation == Some(annotation_id);
    if is_selected {
        let glow_thickness = level.line_width + ctx.theme.selection_thickness * 2.0;
        let glow_y = y - ctx.theme.selection_thickness;
        let mut glow_color = color;
        glow_color[3] = ctx.theme.selection_color[3] * alpha;
        output.fills.push(GridLineInstance {
            rect: [0.0, glow_y, vp_width, glow_y + glow_thickness],
            color: glow_color,
        });
    }

    // Hover highlight.
    let mut width = level.line_width;
    let is_hovered = ctx
        .hovered_annotation
        .map(|(aid, kind)| aid == annotation_id && kind == HitZoneKind::LevelLine)
        .unwrap_or(false);
    if is_hovered {
        width += 1.0;
    }

    output
        .lines
        .extend(segmented_line(0.0, vp_width, y, width, color, &level.style));

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

    // Hit zone (full width, ±6px).
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

    // Price label (right-aligned).
    let price_text = format!("{:.2}", level.price);
    output.labels.push(WidgetLabel {
        text: price_text,
        screen_x: vp_width - 10.0,
        screen_y: y,
        bg_color: [0.12, 0.12, 0.15, 0.85 * alpha],
        text_color: color,
        font_size: 11.0,
        anchor: LabelAnchor::Right,
    });

    // Icon + name label (left-aligned).
    let icon_label = match (&level.label, level.icon.as_char()) {
        (Some(lbl), Some(icon_ch)) if !lbl.is_empty() => Some(format!("{} {}", icon_ch, lbl)),
        (Some(lbl), None) if !lbl.is_empty() => Some(lbl.clone()),
        (None, Some(icon_ch)) | (Some(_), Some(icon_ch)) => Some(icon_ch.to_string()),
        _ => None,
    };
    if let Some(text) = icon_label {
        output.labels.push(WidgetLabel {
            text,
            screen_x: 8.0,
            screen_y: y,
            bg_color: [color[0] * 0.3, color[1] * 0.3, color[2] * 0.3, 0.75 * alpha],
            text_color: [color[0], color[1], color[2], color[3].max(0.9)],
            font_size: 14.0,
            anchor: LabelAnchor::Left,
        });
    }

    output
}
