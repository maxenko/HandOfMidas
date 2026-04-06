//! Widget system: unified annotation architecture for chart overlays.
//!
//! All visual chart elements (levels, order brackets, indicators, notes,
//! markers) share a common compute→scene→GPU pipeline. This module defines
//! the core type system and per-variant data models.

pub mod bracket_tool;
pub mod compute;
pub mod hit_test;
pub mod level;
pub mod marker;
pub mod order_bracket;
pub mod text_note;
pub mod theme;

// ── Re-exports ─────────────────────────────────────────────────────

pub use self::bracket_tool::{BracketTool, BracketToolMode, BracketToolResult};
pub use self::compute::{ComputeContext, LabelAnchor, Viewport, WidgetLabel, WidgetOutput};
pub use self::hit_test::{BoundingBox, CursorIcon, HitResult, HitZone, HitZoneKind, Point};
pub use self::level::{HorizontalLevel, LevelExtend, LineStyle};
pub use self::marker::{MarkerAnnotation, MarkerIcon};
pub use self::order_bracket::{BracketLeg, BracketSide, BracketStatus, EntryType, OrderBracket};
pub use self::text_note::TextNote;
pub use self::theme::Theme;

use midas_core::Timeframe;
use serde::{Deserialize, Serialize};
use std::fmt;

// ── AnnotationId ───────────────────────────────────────────────────

/// Monotonically increasing identifier for annotations.
///
/// Scoped to the `AnnotationStore` that owns it. Within a store, IDs
/// are never reused. IDs start at 1 so that `AnnotationId(0)` can
/// serve as a sentinel value.
///
/// Size: 8 bytes. Cheap to copy, hash, and compare.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AnnotationId(pub u64);

impl AnnotationId {
    /// The null/sentinel value. No valid annotation has this ID.
    pub const NONE: Self = Self(0);

    /// Whether this is a valid (non-sentinel) ID.
    pub fn is_valid(self) -> bool {
        self.0 != 0
    }
}

impl fmt::Display for AnnotationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ann#{}", self.0)
    }
}

// ── Presence ───────────────────────────────────────────────────────

/// Three-tier visibility state for annotations.
///
/// Adapted from Bevy's visibility system. Determines whether an
/// annotation is rendered, interactive, or completely dormant.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Presence {
    /// Fully rendered, interactive, hit-testable.
    #[default]
    Active,
    /// Rendered at reduced opacity. NOT interactive or hit-testable.
    Ghost,
    /// Not rendered at all. Zero GPU cost. Still in storage.
    Hidden,
}

impl Presence {
    /// Alpha multiplier for this presence state.
    pub fn alpha(&self) -> f32 {
        match self {
            Presence::Active => 1.0,
            Presence::Ghost => 0.4,
            Presence::Hidden => 0.0,
        }
    }

    /// Whether this annotation should respond to mouse events.
    pub fn is_interactive(&self) -> bool {
        matches!(self, Presence::Active)
    }

    /// Whether this annotation should be rendered.
    pub fn is_visible(&self) -> bool {
        !matches!(self, Presence::Hidden)
    }

    /// Whether this annotation should be included in hit-testing.
    pub fn is_hit_testable(&self) -> bool {
        self.is_interactive()
    }

    /// Transition to the next visibility state in the cycle:
    /// Active -> Ghost -> Hidden -> Active.
    pub fn cycle(&self) -> Self {
        match self {
            Presence::Active => Presence::Ghost,
            Presence::Ghost => Presence::Hidden,
            Presence::Hidden => Presence::Active,
        }
    }
}

// ── AnnotationKind ─────────────────────────────────────────────────

/// The specific widget type of an annotation.
///
/// This is a closed enum -- all variants are known at compile time.
/// Adding a new variant requires modifying this enum and every `match`
/// arm that dispatches on it. The compiler enforces exhaustiveness.
///
/// Only the `Level` variant is implemented in Phase 1A. Additional
/// variants (`OrderBracket`, `TextNote`, `Marker`) will be added in
/// their respective implementation phases.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum AnnotationKind {
    /// Horizontal line at a price. The most common annotation type.
    Level(HorizontalLevel),
    /// Entry + optional TP/SL bracket for order visualization.
    OrderBracket(Box<OrderBracket>),
    /// Text note anchored to a price/time point.
    TextNote(TextNote),
    /// Icon or stamp at a specific price/time.
    Marker(MarkerAnnotation),
}

// ── Annotation ─────────────────────────────────────────────────────

/// A chart annotation with metadata.
///
/// Every drawable element on a chart is an `Annotation`. The `kind`
/// field determines the specific widget type; the wrapper provides
/// shared metadata (ID, presence, timestamps, lock state).
///
/// Annotations are owned by `AnnotationStore` (in the app layer).
/// Charts receive `&[Annotation]` slices through `ChartInput`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Annotation {
    /// Unique identifier within the owning store.
    pub id: AnnotationId,
    /// The specific widget type and its data.
    pub kind: AnnotationKind,
    /// Visibility and interactivity state.
    pub presence: Presence,
    /// Optional timeframe filter. If `Some`, only rendered on charts
    /// with a matching timeframe. If `None`, rendered on all timeframes.
    pub visible_timeframes: Option<Vec<Timeframe>>,
    /// Whether this annotation is locked against drag/delete.
    pub locked: bool,
    /// Creation timestamp (epoch milliseconds). Set once, never changes.
    pub created_at: i64,
    /// Last modification timestamp (epoch milliseconds).
    pub modified_at: i64,
}

impl Annotation {
    /// Whether this annotation should be rendered on a chart with
    /// the given timeframe.
    pub fn should_render_on(&self, tf: Timeframe) -> bool {
        if !self.presence.is_visible() {
            return false;
        }
        match &self.visible_timeframes {
            None => true,
            Some(tfs) => tfs.contains(&tf),
        }
    }

    /// Whether this annotation should be included in hit-testing
    /// on a chart with the given timeframe.
    pub fn is_interactive_on(&self, tf: Timeframe) -> bool {
        self.should_render_on(tf) && self.presence.is_interactive()
    }

    /// Whether this annotation can be dragged (moved).
    pub fn is_draggable_on(&self, tf: Timeframe) -> bool {
        self.is_interactive_on(tf) && !self.locked
    }
}

#[cfg(test)]
mod tests;
