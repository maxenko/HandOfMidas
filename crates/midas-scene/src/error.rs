//! Error type surfaced through [`crate::interaction::InteractionState`]
//! and the [`crate::scene::ChartScene::last_error`] slot.
//!
//! Tools and layers don't thread `Result` through
//! [`crate::input::EventStatus`] — the return channel is strictly
//! `Captured | Ignored`. When an interactive-layer operation fails
//! (ticker rejected a bracket commit, persistence write failed, etc.)
//! the layer calls `ctx.emit_error(SceneError)` on the tool-context
//! (slice 4+ owns the context type; for slice 1 the scene parks the
//! error on a last-error field and a follow-up slice surfaces it as
//! a toast).

use crate::layer::LayerId;

/// Fault that occurred inside a scene interaction. Kept small and
/// cheaply `Clone`-able so it can sit on `ChartScene.last_error`
/// across frames until the widget drains it.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SceneError {
    /// A tool's commit step was rejected by the ticker-state machine
    /// (rule 8: all bracket mutations flow through `TickerMsg`; this
    /// variant carries whatever the apply side returned).
    #[error("ticker rejected commit: {0}")]
    TickerRejected(String),

    /// Persistence (AnnotationStore, ChartViewStore, fixture write)
    /// failed. Detail string is the lower-level IO / DB error.
    #[error("persistence failed: {0}")]
    PersistenceFailed(String),

    /// A projection math invariant broke. Should be unreachable given
    /// slice-2a's `PriceAxis::to_y` contract; kept as a defensive
    /// variant so a tracing::error! doesn't panic the render thread.
    #[error("axis range invalid: {0}")]
    AxisRange(String),

    /// A tool referenced an annotation that no longer exists (e.g. a
    /// drag-in-flight pointing at a level deleted concurrently).
    #[error("annotation not found")]
    AnnotationNotFound,

    /// A layer's `paint()` panicked; slice 1's `catch_unwind`
    /// substituted a fallback quad. The scene logs + surfaces this so
    /// dev-harness tests can assert the recovery path fired.
    #[error("layer `{layer}` panicked during paint; fallback quad emitted")]
    PanicFallback { layer: LayerId },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scene_error_is_clone_send_sync() {
        fn assert_clone_send_sync<T: Clone + Send + Sync>() {}
        assert_clone_send_sync::<SceneError>();
    }

    #[test]
    fn scene_error_panic_fallback_formats_layer_id() {
        let e = SceneError::PanicFallback {
            layer: LayerId("candles"),
        };
        assert!(e.to_string().contains("candles"));
    }

    #[test]
    fn scene_error_variants_are_equal_when_shapes_match() {
        let a = SceneError::AnnotationNotFound;
        let b = SceneError::AnnotationNotFound;
        assert_eq!(a, b);
    }
}
