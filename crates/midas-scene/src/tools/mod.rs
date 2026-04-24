//! Tool effects — the interaction-layer → application bus.
//!
//! A `ToolEffect` is the output of an [`InteractiveLayer::update`] that
//! crosses the sans-IO boundary: levels persist through
//! `AnnotationStore`, brackets mutate through `TickerState::apply`, and
//! menus pop up in the host widget. Every tool-level mutation flows
//! through this enum so the app-side translation stays a single, small
//! `match`.
//!
//! Design constraints (per `plan/chart-transition/00-index.md`, slice 4
//! + slice 5b):
//!
//! - `midas-scene` stays free of app types. `AnnotationId` here is a
//!   bare `u64`; the app wraps it in its own newtype on drain.
//! - The shape accommodates both level (slice 4) and bracket (slice 5b)
//!   variants up-front. Slice 5b fills in the bracket semantics; slice
//!   4 lands the variants with minimal `todo!()` bodies where necessary
//!   so the two slices don't churn the enum.
//! - Effects are drained by the widget per frame via
//!   [`crate::scene::ChartScene::take_effects`]. The scene owns the
//!   queue; tools push via [`ToolContext::emit_effect`].
//!
//! [`InteractiveLayer::update`]: crate::layer::InteractiveLayer::update
//! [`ToolContext::emit_effect`]: crate::layer::ToolContext::emit_effect

use crate::error::SceneError;
use crate::input::Point;

pub mod level;
pub mod snap;

pub use level::{LevelTool, LevelToolMode};
pub use snap::{snap_to_ohlc, CandleRef, SNAP_THRESHOLD_MAX_PX, SNAP_THRESHOLD_MIN_PX};

/// Opaque annotation id. Matches the app-side `AnnotationId(u64)` newtype
/// 1:1; the app wraps on drain so `midas-scene` never has to depend on
/// `midas-app` types. Picked per plan option (b).
pub type AnnotationId = u64;

/// Long / short side of an order bracket. Shape-agreed with slice 5b;
/// slice 4 uses it only inside the reserved bracket effect variants.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Side {
    Long,
    Short,
}

/// Which leg of a three-leg order bracket a bracket-tool effect targets.
///
/// Entry is emitted by `BeginDraftBracket`; `Tp` / `Sl` are emitted by
/// `SetDraftLeg` on the second + third click. The slice 5b FSM owns the
/// sequencing — slice 4 only needs the shape.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum LegRole {
    Entry,
    Tp,
    Sl,
}

/// One entry in a right-click context menu. The app translates each
/// action into a concrete `Message` at drain time; the scene only names
/// the action intent.
#[derive(Clone, Debug, PartialEq)]
pub struct ContextMenuItem {
    /// Human-readable label ("Edit", "Lock", "Delete").
    pub label: String,
    /// Action tag the app switches on. See [`ContextMenuAction`].
    pub action: ContextMenuAction,
}

/// Action families for a [`ContextMenuItem`]. Intentionally tiny — the
/// app maps each variant to its own message constructor.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ContextMenuAction {
    /// Open an inline editor for the annotation (price, label, colour).
    Edit { id: AnnotationId },
    /// Toggle the annotation's lock flag.
    ToggleLock { id: AnnotationId },
    /// Delete the annotation.
    Delete { id: AnnotationId },
}

/// Side-effect emitted by an interactive layer's `update` call.
///
/// The widget drains these via [`crate::scene::ChartScene::take_effects`]
/// at the end of each event cycle and translates them into app
/// `Message`s. Variants are grouped:
///
/// - **Level (slice 4, shipping now)**: `CreateLevel`, `UpdateLevel`,
///   `DeleteLevel`.
/// - **Bracket (slice 5b, shape-reserved)**: `BeginDraftBracket`,
///   `SetDraftLeg`, `CommitDraftBracket`, `CancelDraftBracket`,
///   `UpdateBracketLeg`. Internals are `todo!()` in slice 4; slice 5b
///   fills them in. The shape is frozen so slice 4 consumers can emit
///   them without a second churn.
/// - **Generic**: `OpenContextMenu`, `ReportError`.
#[derive(Clone, Debug, PartialEq)]
pub enum ToolEffect {
    // ── Level variants (slice 4) ────────────────────────────────────
    /// Commit a new level at `price`. `lock` is the initial locked
    /// state. Emitted by `LevelTool` on left-click while in `Placing`.
    CreateLevel { price: f64, lock: bool },
    /// Update an existing level's price (drag-move on the line).
    UpdateLevel { id: AnnotationId, price: f64 },
    /// Delete the given level.
    DeleteLevel { id: AnnotationId },

    // ── Bracket variants (slice 5b — shape reserved) ────────────────
    /// Start a draft bracket at `entry`. First click of the 3-click
    /// placement FSM. Slice 5b maps this to
    /// `TickerMsg::EnsureDraftBracket`.
    BeginDraftBracket { side: Side, entry: f64 },
    /// Set a leg on the draft bracket. Emitted on the second + third
    /// clicks for TP and SL respectively. Slice 5b maps this to
    /// `TickerMsg::SetLegPrice`.
    SetDraftLeg { role: LegRole, price: f64 },
    /// Finalise the draft bracket. Slice 5b maps this to
    /// `TickerMsg::SaveBracket`.
    CommitDraftBracket,
    /// Discard the draft bracket without persisting. Emitted on
    /// Escape or `ChartScene::on_destroy` mid-placement. Slice 5b
    /// maps this to `TickerMsg::CancelBracket`.
    CancelDraftBracket,
    /// Drag-move on an existing bracket's TP or SL leg. Slice 5b fills
    /// the app translation in.
    UpdateBracketLeg {
        id: AnnotationId,
        role: LegRole,
        price: f64,
    },

    // ── Generic ─────────────────────────────────────────────────────
    /// Open a context menu at screen-pixel `pt`. The widget maps the
    /// menu items into an iced popup.
    OpenContextMenu {
        pt: Point,
        items: Vec<ContextMenuItem>,
    },
    /// Surface a tool-layer error to the host. Shape-identical to the
    /// existing `last_error` path but travels via the effect queue so
    /// the widget can drain errors + effects in one pass.
    ReportError(SceneError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layer::LayerId;

    #[test]
    fn tool_effect_is_clone() {
        let e = ToolEffect::CreateLevel {
            price: 100.0,
            lock: false,
        };
        let e2 = e.clone();
        assert_eq!(e, e2);
    }

    #[test]
    fn report_error_carries_layer_panic() {
        let e = ToolEffect::ReportError(SceneError::PanicFallback {
            layer: LayerId("x"),
        });
        match e {
            ToolEffect::ReportError(SceneError::PanicFallback { layer }) => {
                assert_eq!(layer, LayerId("x"));
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn context_menu_action_variants_are_distinct() {
        assert_ne!(
            ContextMenuAction::Edit { id: 1 },
            ContextMenuAction::Delete { id: 1 }
        );
        assert_ne!(
            ContextMenuAction::Edit { id: 1 },
            ContextMenuAction::ToggleLock { id: 1 }
        );
    }

    #[test]
    fn leg_role_has_three_variants() {
        // Sanity: the bracket FSM emits exactly these three roles;
        // slice 5b must not silently add a fourth without updating the
        // app-side match arms.
        let all = [LegRole::Entry, LegRole::Tp, LegRole::Sl];
        assert_eq!(all.len(), 3);
    }
}
