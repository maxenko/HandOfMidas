//! Sans-IO action vocabulary for decorator clicks.
//!
//! Clicks on decorator items produce a `DecoratorAction` inside a
//! `ChartAction::DecoratorClick` variant (see `interaction/mod.rs`). The app
//! layer matches on the action and maps each variant to a broker command or
//! UI message. `DecoratorAction` is `Copy` so it can live inside
//! `HitZoneKind::Decorator` without breaking that enum's `Copy` derive.

use serde::{Deserialize, Serialize};

/// Fixed vocabulary of actions a decorator item can emit when clicked.
///
/// `Custom(u32)` is the escape hatch for app-defined actions that don't yet
/// justify a named variant. The namespace is owned by whichever annotation
/// kind emitted the click; collisions are the app layer's problem.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DecoratorAction {
    /// Delete the parent annotation.
    CloseAnnotation,
    /// Attach a new take-profit leg to the parent bracket.
    CreateTakeProfit,
    /// Attach a new stop-loss leg to the parent bracket.
    CreateStopLoss,
    /// Detach the stop-loss leg from the parent bracket.
    RemoveStopLoss,
    /// Cycle `Limit` / `Stop` / `Market` on a draft bracket entry.
    CycleEntryType,
    /// Open an inline editor for the quantity field.
    EditQuantity,
    /// Open an inline editor for the price field.
    EditPrice,
    /// Flip the `Annotation.locked` flag.
    ToggleLocked,
    /// Transmit a draft bracket to the broker.
    Submit,
    /// Persist a draft bracket or level.
    Save,
    /// Toggle the `pinned` state on the parent symbol's
    /// `TickerOrderIntent`. Wired in Slice 4 to drive the
    /// `PinToggle` bracket decorator. A pinned intent is exempt from
    /// the GATR snap rule — see `plan/ticker-order-state/README.md`
    /// section D4.
    TogglePin,
    /// App-defined action keyed by an opaque `u32`.
    Custom(u32),
}
