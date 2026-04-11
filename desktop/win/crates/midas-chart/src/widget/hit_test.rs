//! Hit-testing types for annotation interaction.
//!
//! Hit zones are precomputed during the widget compute phase and stored
//! in `WidgetOutput`. The interaction layer queries them to determine
//! which annotation the user clicked or is hovering over.

use super::decorator::DecoratorAction;
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

/// Fixed-capacity breadcrumb into a nested decorator layout.
///
/// Wraps a `[u8; 4]` + `u8` length pair. Construction zeroes the unused
/// tail of the inner array so the derived `PartialEq`/`Hash` are sound
/// regardless of how the caller supplied the path — without this
/// invariant, two paths with equal logical length but differing garbage
/// bytes past the length byte would compare unequal, silently breaking
/// click dedup and hover-state lookup. Four bytes covers the deepest
/// realistic nesting (`group → stack_item → child_item → segment`).
///
/// This type is `Copy` so that [`HitZoneKind::Decorator`] preserves the
/// `Copy` derive on `HitZoneKind` itself — the entire hover/hit-test
/// pipeline depends on `HitZoneKind` being trivially copyable.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ItemPath {
    bytes: [u8; 4],
    len: u8,
}

impl ItemPath {
    /// Construct an `ItemPath` from a slice of byte indices.
    ///
    /// Debug-panics if `path.len() > 4`; release clamps to 4 to keep
    /// the invariant sound even if an upstream bug slips through.
    pub fn new(path: &[u8]) -> Self {
        debug_assert!(path.len() <= 4, "ItemPath max depth is 4");
        let mut bytes = [0u8; 4];
        let len = path.len().min(4);
        bytes[..len].copy_from_slice(&path[..len]);
        Self {
            bytes,
            len: len as u8,
        }
    }

    /// View the valid prefix of the path as a byte slice.
    pub fn as_slice(&self) -> &[u8] {
        &self.bytes[..self.len as usize]
    }

    /// Number of valid bytes in the path (0..=4).
    pub fn len(&self) -> usize {
        self.len as usize
    }

    /// Whether the path has zero valid bytes.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
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
/// - `Decorator` -> click routed to `DecoratorAction` via the path
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
    /// Click on a decorator item or segment. `item_path` is a fixed-
    /// capacity breadcrumb (max depth 4) wrapped in [`ItemPath`] to
    /// guarantee its unused tail is zeroed — see [`ItemPath`] for the
    /// rationale.
    Decorator {
        /// Stable group id unique within the parent annotation.
        group_id: u16,
        /// Breadcrumb into the nested decorator layout.
        item_path: ItemPath,
        /// Action emitted when the hit zone is clicked.
        action: DecoratorAction,
    },
}

// Compile-time proof that `HitZoneKind` is still `Copy`. Dropping this
// derive cascades through the whole hover/hit-test surface (see
// `plan/decorator-system/03-data-model.md#copy-must-be-preserved`).
const _: () = {
    const fn assert_copy<T: Copy>() {}
    assert_copy::<HitZoneKind>();
    assert_copy::<ItemPath>();
};

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
