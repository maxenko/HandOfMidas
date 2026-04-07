//! Hit-testing types for annotation interaction.
//!
//! Hit zones are precomputed during the widget compute phase and stored
//! in `WidgetOutput`. The interaction layer queries them to determine
//! which annotation the user clicked or is hovering over.

use super::AnnotationId;

/// A point in screen coordinates (logical pixels).
#[derive(Clone, Copy, Debug)]
pub struct Point {
    /// X coordinate in logical pixels.
    pub x: f32,
    /// Y coordinate in logical pixels.
    pub y: f32,
}

/// Result of a successful hit-test.
///
/// Tells the interaction layer which annotation was hit and which
/// specific sub-element was clicked, so it knows what drag behavior
/// to use.
#[derive(Clone, Debug)]
pub struct HitResult {
    /// Which annotation was hit.
    pub annotation_id: AnnotationId,
    /// Which part of the annotation was hit.
    pub zone: HitZoneKind,
    /// Screen distance from the hit point to the nearest edge of the
    /// hit zone. Used for priority (closer = higher priority when
    /// multiple annotations overlap).
    pub distance: f32,
}

/// Which part of an annotation was hit.
///
/// The interaction layer uses this to determine drag behavior:
/// - `LevelLine` -> vertical drag (price only)
/// - `BracketEntry` -> vertical drag (moves entire bracket)
/// - `BracketTP` / `BracketSL` -> vertical drag (moves single leg)
/// - `BracketStopTrigger` -> vertical drag (moves stop trigger price)
/// - `BracketZone` -> select only (no drag)
/// - `MarkerIcon` -> select only
/// - `NoteBody` -> 2D drag (price + time)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HitZoneKind {
    /// A level's horizontal line.
    LevelLine,
    /// A bracket's entry line.
    BracketEntry,
    /// A bracket's take-profit line.
    BracketTP,
    /// A bracket's stop-loss line.
    BracketSL,
    /// A bracket's stop trigger line (StopLimit entry type).
    BracketStopTrigger,
    /// A bracket's zone fill (between entry and TP or SL).
    BracketZone,
    /// A marker's icon area.
    MarkerIcon,
    /// A text note's bounding box.
    NoteBody,
    /// A volume profile's histogram area.
    VolumeProfileBar,
    /// [Submit] button on a Draft bracket entry line.
    BracketSubmit,
    /// [Save] button on a Draft bracket entry line.
    BracketSave,
    /// [SL] toggle button on a Draft bracket entry line.
    BracketToggleSL,
    /// [X] cancel button on a Draft bracket entry line.
    BracketCancel,
    /// [X] cancel-SL button on a Draft bracket SL line.
    BracketCancelSL,
}

/// An interactive area registered by a widget during compute.
///
/// Collected into `WidgetOutput::hit_zones` and used for hit-testing
/// without re-computing the widget's geometry.
#[derive(Clone, Debug)]
pub struct HitZone {
    /// Which annotation owns this hit zone.
    pub annotation_id: AnnotationId,
    /// Screen-space bounding rectangle: [left, top, right, bottom].
    pub rect: [f32; 4],
    /// What kind of element this hit zone represents.
    pub kind: HitZoneKind,
    /// Cursor icon to show when hovering this zone.
    pub cursor: CursorIcon,
}

/// Cursor icon for hovering over interactive zones.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CursorIcon {
    /// Default chart crosshair.
    #[default]
    Crosshair,
    /// Grab/move (open hand).
    Grab,
    /// Grabbing (closed hand) -- during active drag.
    Grabbing,
    /// Vertical resize (N-S arrows).
    ResizeNS,
    /// Horizontal resize (E-W arrows).
    ResizeEW,
    /// Clickable pointer (hand with finger).
    Pointer,
    /// Text input cursor.
    Text,
}

/// Screen-space bounding box in logical pixels.
#[derive(Clone, Copy, Debug)]
pub struct BoundingBox {
    /// Left edge X.
    pub left: f32,
    /// Top edge Y.
    pub top: f32,
    /// Right edge X.
    pub right: f32,
    /// Bottom edge Y.
    pub bottom: f32,
}

impl BoundingBox {
    /// Whether a point is inside this bounding box.
    pub fn contains(&self, point: Point) -> bool {
        point.x >= self.left
            && point.x <= self.right
            && point.y >= self.top
            && point.y <= self.bottom
    }

    /// Expand the bounding box by `margin` pixels in each direction.
    pub fn expand(&self, margin: f32) -> Self {
        Self {
            left: self.left - margin,
            top: self.top - margin,
            right: self.right + margin,
            bottom: self.bottom + margin,
        }
    }
}
