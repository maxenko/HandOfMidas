//! Widget compute interface: context, output, and labels.
//!
//! The compute phase transforms data-space annotations into screen-space
//! render primitives. Each widget kind has a dedicated compute function
//! dispatched via `match` on `AnnotationKind`.

use crate::camera::Camera2D;
use crate::instances::GridLineInstance;
use midas_core::CandleData;

use super::hit_test::{HitZone, HitZoneKind};
use super::theme::Theme;
use super::AnnotationId;

/// Context passed to every widget compute function.
///
/// Contains everything needed to transform data-space annotation
/// coordinates into screen-space render primitives. Borrowed for
/// the duration of the compute pass -- never stored.
pub struct ComputeContext<'a> {
    /// Camera defining the visible time/price window.
    pub camera: &'a Camera2D,
    /// Candle data source for indicators and volume profile computation.
    pub data: &'a dyn CandleData,
    /// Viewport dimensions in logical pixels.
    pub viewport: Viewport,
    /// Current theme colors for default annotation styling.
    pub theme: &'a Theme,
    /// OHLC snap function: given a screen Y coordinate, returns the
    /// nearest OHLC snap target as `(snapped_screen_y, candle_index)`.
    /// Returns `None` if no snap target is within threshold distance.
    pub snap_fn: &'a dyn Fn(f32) -> Option<(f32, usize)>,
    /// Estimated candle duration in milliseconds.
    pub candle_duration_ms: f64,
    /// Whether gaps are collapsed (index-based X positioning).
    pub collapse_gaps: bool,
    /// Separator Y position between price area and volume area.
    pub separator_y: f32,
    /// DPI scale factor for physical pixel calculations.
    pub dpi_scale: f32,
    /// Hovered annotation element for highlight styling.
    pub hovered_annotation: Option<(AnnotationId, HitZoneKind)>,
    /// Currently selected annotation for selection glow.
    pub selected_annotation: Option<AnnotationId>,
    /// Drag ghost: annotation being dragged and its original price.
    /// The compute function emits a faint ghost line at this price.
    pub drag_ghost: Option<(AnnotationId, f64)>,
}

/// Viewport dimensions.
#[derive(Clone, Copy, Debug)]
pub struct Viewport {
    /// Width in logical pixels.
    pub width: u32,
    /// Height in logical pixels.
    pub height: u32,
}

/// Render primitives produced by a single widget's compute function.
///
/// Contains all the GPU-ready geometry and metadata needed to render
/// the widget. Organized by rendering layer for correct draw order.
///
/// | Field | GPU Layer | Draw Order |
/// |---|---|---|
/// | `fills` | Layer 6 | Behind annotation lines |
/// | `lines` | Layer 7 | On top of fills |
/// | `markers` | Layer 8 | On top of lines |
/// | `labels` | Layer 10 | iced overlay (above all GPU) |
/// | `hit_zones` | N/A | Not rendered, used for interaction |
#[derive(Clone, Debug, Default)]
pub struct WidgetOutput {
    /// Background fills rendered at Layer 6 (behind annotation lines).
    pub fills: Vec<GridLineInstance>,
    /// Lines and borders rendered at Layer 7.
    pub lines: Vec<GridLineInstance>,
    /// Markers and point elements rendered at Layer 8.
    pub markers: Vec<GridLineInstance>,
    /// Text labels rendered by the iced overlay at Layer 10.
    pub labels: Vec<WidgetLabel>,
    /// Interactive hit zones for mouse event handling (not rendered).
    pub hit_zones: Vec<HitZone>,
}

impl WidgetOutput {
    /// Create an empty output with no render primitives.
    pub fn empty() -> Self {
        Self::default()
    }

    /// Apply an alpha multiplier to all render primitives.
    ///
    /// Used to implement `Presence::Ghost` (0.4 alpha).
    pub fn apply_alpha(&mut self, alpha: f32) {
        for instance in self
            .fills
            .iter_mut()
            .chain(self.lines.iter_mut())
            .chain(self.markers.iter_mut())
        {
            instance.color[3] *= alpha;
        }
        for label in &mut self.labels {
            label.bg_color[3] *= alpha;
            label.text_color[3] *= alpha;
        }
    }

    /// Merge another `WidgetOutput` into this one.
    pub fn merge(&mut self, other: WidgetOutput) {
        self.fills.extend(other.fills);
        self.lines.extend(other.lines);
        self.markers.extend(other.markers);
        self.labels.extend(other.labels);
        self.hit_zones.extend(other.hit_zones);
    }

    /// Total number of GPU instances across all layers.
    pub fn instance_count(&self) -> usize {
        self.fills.len() + self.lines.len() + self.markers.len()
    }
}

/// A text label positioned in screen space, rendered by the iced overlay.
#[derive(Clone, Debug)]
pub struct WidgetLabel {
    /// Text content to display.
    pub text: String,
    /// Screen-space X position in logical pixels.
    pub screen_x: f32,
    /// Screen-space Y position in logical pixels.
    pub screen_y: f32,
    /// Background color (RGBA). Transparent for no background.
    pub bg_color: [f32; 4],
    /// Text color (RGBA).
    pub text_color: [f32; 4],
    /// Font size in logical pixels. Default: 11.0.
    pub font_size: f32,
    /// Anchor point for positioning.
    pub anchor: LabelAnchor,
}

/// Anchor point for label positioning.
#[derive(Clone, Copy, Debug, Default)]
pub enum LabelAnchor {
    /// Position is top-left corner of the label.
    #[default]
    TopLeft,
    /// Position is horizontal center, vertical center.
    Center,
    /// Position is left edge, vertically centered.
    Left,
    /// Position is right edge, vertically centered.
    Right,
}
