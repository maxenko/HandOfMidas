//! Input events + hit-test primitives for [`InteractiveLayer`].
//!
//! Sans-IO vocabulary: every event a scene needs to react to arrives
//! as an [`InputEvent`]. The scene dispatches top-down through its
//! layers; each may return [`EventStatus::Captured`] to claim the
//! event (and establish drag-focus) or [`EventStatus::Ignored`] to
//! let it bubble. Per `plan/chart-transition/00-index.md` D3/D4, the
//! pattern mirrors Bevy's `bevy_mod_picking` — first hit wins;
//! drag-captured layer bypasses hit-test until `MouseUp`.

use crate::layer::LayerId;

/// Opaque pixel coordinate — top-left origin of the chart viewport.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

impl Point {
    #[inline]
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
}

/// Mouse button identifier. Mirrors iced's three-button model without
/// pulling in the iced dep — keeps `midas-scene` sans-IO.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

/// Keyboard modifier flags. Small bitfield — at most 4 modifiers.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub meta: bool,
}

/// Keyboard key identifier. `Named` covers the usual specials; `Char`
/// carries a single printable character for FSM transitions that key
/// on `L`/`S`/`E` etc. (e.g. bracket-tool directional toggle).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Escape,
    Enter,
    Delete,
    Backspace,
    Tab,
    ArrowUp,
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    Home,
    End,
    PageUp,
    PageDown,
}

/// Input event delivered to [`crate::scene::ChartScene::handle_input`].
///
/// Deliberately a flat enum (not a trait) so the dispatch code can
/// match without dynamic dispatch. Variants mirror the set of user
/// gestures a chart tool needs to react to — no widget-framework
/// baggage.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum InputEvent {
    MouseDown {
        button: MouseButton,
        pt: Point,
        modifiers: Modifiers,
    },
    MouseUp {
        button: MouseButton,
        pt: Point,
    },
    MouseMove {
        pt: Point,
    },
    Wheel {
        /// Horizontal wheel delta (logical units; positive = right).
        dx: f32,
        /// Vertical wheel delta (positive = up/away).
        dy: f32,
        pt: Point,
    },
    KeyDown {
        key: Key,
        modifiers: Modifiers,
    },
    KeyUp {
        key: Key,
    },
    /// Cursor left the chart surface. Tools use this to clear hover
    /// state without receiving a synthetic MouseMove.
    CursorLeft,
}

/// Hit-test result from [`InteractiveLayer::hit_test`].
///
/// `sub_z` is the intra-layer sub-ordinal (e.g. within a bracket:
/// entry < TP < SL < drag-handle). Let the scene break ties between
/// same-layer hits without the caller doing extra work.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Hit {
    pub layer_id: LayerId,
    pub sub_z: u16,
    pub cursor: CursorShape,
}

/// Cursor-shape hint the scene surfaces to the widget. Kept minimal —
/// maps one-to-one onto `iced::mouse::Interaction` at the widget edge.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum CursorShape {
    Default,
    Pointer,
    Crosshair,
    Grab,
    Grabbing,
    ResizeNorthSouth,
    ResizeEastWest,
}

/// Return value from [`InteractiveLayer::update`].
///
/// `Captured` means the layer claimed the event — scene stops
/// dispatching further. `Ignored` means the event keeps bubbling. Per
/// plan R20, tools default to `Ignored` on wheel events so chart
/// zoom/pan keeps working when a tool is active.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum EventStatus {
    Captured,
    Ignored,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_event_is_copy() {
        fn takes_copy<T: Copy>(_: T) {}
        takes_copy(InputEvent::MouseMove {
            pt: Point::new(0.0, 0.0),
        });
    }

    #[test]
    fn hit_is_copy() {
        fn takes_copy<T: Copy>(_: T) {}
        takes_copy(Hit {
            layer_id: LayerId("t"),
            sub_z: 0,
            cursor: CursorShape::Default,
        });
    }

    #[test]
    fn modifiers_default_all_false() {
        let m = Modifiers::default();
        assert!(!m.ctrl && !m.shift && !m.alt && !m.meta);
    }
}
