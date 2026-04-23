//! Per-chart mutable interaction state.
//!
//! Per R2-NM-7 the old `Camera2D` is deleted; projection state lives
//! on the axis + price range + viewport. What REMAINS as mutable,
//! per-chart state is the interaction bookkeeping — hover target,
//! drag session, crosshair pixel.
//!
//! This type is moved into `midas-scene` now so downstream slices
//! (S8 `SessionChart`, S9 `chart-input`) can depend on it without
//! pulling any iced or wgpu machinery. Paint is pure; event handlers
//! mutate this state and dirty the projection.
//!
//! ## Removed fields
//!
//! - `last_wheel_ts` — originally part of the `Camera2D` → `Chart`
//!   migration for wheel-velocity decay. App-harden L3 / arch-audit
//!   P3 noted that nothing reads it, and the field would need
//!   `Clock::now_monotonic()` populating rather than
//!   `Instant::now()` directly. Removed per YAGNI; future consumers
//!   should re-introduce it alongside a `Clock` injection.

/// What the pointer is currently over, if anything. Populated by
/// hit-test routines; consumed by decorator visibility rules and by
/// hover highlights in each layer.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum HoverTarget {
    /// The Nth candle in the active [`CandleSeries`](midas_bars::CandleSeries).
    Candle(usize),
    /// A leg of the named order bracket.
    Bracket { id: u64, leg: BracketLeg },
    /// A named price-line annotation.
    PriceLine(u64),
    /// A named price level annotation.
    Level(u64),
}

/// Which leg of a three-leg order bracket the pointer sits on.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum BracketLeg {
    Entry,
    TakeProfit,
    StopLoss,
}

/// An in-flight drag. `start_px` is captured at `mouse_down`;
/// `current_px` updates on `mouse_move`; releasing commits or
/// cancels the drag and the session is cleared.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct DragSession {
    pub target: HoverTarget,
    pub start_px: (f32, f32),
    pub current_px: (f32, f32),
}

/// Mutable per-chart interaction state. All fields are directly `pub`
/// so the input-layer slice (S9) can mutate in-place without a
/// proliferation of setters. The type is deliberately simple — this
/// is the minimum surface every chart needs; richer state (e.g.
/// bracket-placement 3-click state machine) lives in the specific
/// tool, not here.
#[derive(Clone, Debug, Default)]
pub struct InteractionState {
    pub hover: Option<HoverTarget>,
    pub drag: Option<DragSession>,
    pub crosshair_px: Option<(f32, f32)>,
}

impl InteractionState {
    /// Build a fresh interaction state — hover, drag, and crosshair
    /// all `None`.
    #[inline]
    pub fn new() -> Self {
        Self::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_all_none() {
        let s = InteractionState::new();
        assert!(s.hover.is_none());
        assert!(s.drag.is_none());
        assert!(s.crosshair_px.is_none());
    }

    #[test]
    fn hover_target_equality() {
        let a = HoverTarget::Bracket {
            id: 7,
            leg: BracketLeg::TakeProfit,
        };
        let b = HoverTarget::Bracket {
            id: 7,
            leg: BracketLeg::TakeProfit,
        };
        let c = HoverTarget::Bracket {
            id: 7,
            leg: BracketLeg::StopLoss,
        };
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn drag_session_is_copy() {
        fn takes_copy<T: Copy>(_: T) {}
        takes_copy(DragSession {
            target: HoverTarget::Candle(0),
            start_px: (0.0, 0.0),
            current_px: (0.0, 0.0),
        });
    }
}
