//! Unified column-resize drag state + event envelope.
//!
//! Collapses four parallel drag lifecycles (Watchlist, Account Orders,
//! Account History, Account Recents) into one [`ColumnResizeEvent`] plumbed
//! through [`Message::ColumnResize`][crate::app::Message::ColumnResize] and
//! dispatched by `MidasApp::handle_column_resize`.
//!
//! Before this collapse each surface had its own `*ColumnResizeStart/ing/End`
//! Message triple and a dedicated `resizing_*_column` field on `MidasApp`.
//! The single [`ColumnResizeTarget`] enum replaces all four with a target-
//! routed envelope; the inline `match` inside the handler keeps grid-state
//! lookup per-target without introducing a trait.
//!
//! Emit-site shape is unified: both Watchlist and Account `Begin` events
//! pass `start_x = f32::NAN`. The handler stores NaN, and the first `Move`
//! event back-fills it (the original per-surface handlers all did this
//! lazily for the Account variants and at Begin-emit-time for Watchlist;
//! making the Watchlist shape match is a no-op since its view site also
//! passed `0.0` as the eager start_x, never the actual cursor position).

use midas_core::{AccountPanelId, WatchlistId};

/// Which on-screen grid a column-resize drag is targeting.
///
/// Each variant names the live grid-state map on `MidasApp`. Routing
/// inside `handle_column_resize` pattern-matches on this enum to find
/// the right `GridState` to mutate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColumnResizeTarget {
    /// Watchlist panel grid.
    Watchlist(WatchlistId),
    /// Account panel Orders tab.
    AccountOrders(AccountPanelId),
    /// Account panel Trade-History tab (runtime-only — not persisted).
    AccountHistory(AccountPanelId),
    /// Account panel Recent-Instruments tab (runtime-only — not persisted).
    AccountRecents(AccountPanelId),
}

/// In-flight column-resize drag state stored on `MidasApp`.
///
/// `start_x` is the cursor x captured on the *first* `Move` event (the
/// initial `Begin` stamps `f32::NAN` and the `Move` arm back-fills it).
/// `start_width` is the column's width at drag-start; new width is
/// `(start_width + (cursor_x - start_x)).max(min_width)`.
#[derive(Debug, Clone, Copy)]
pub struct ColumnResizeState {
    /// Which surface + grid the resize is targeting.
    pub target: ColumnResizeTarget,
    /// Column index into the target's column-id list.
    pub col_idx: usize,
    /// Cursor-x captured at first `Move`; `NaN` until then.
    pub start_x: f32,
    /// Column width at drag-start (pixels).
    pub start_width: f32,
}

/// Column-resize drag lifecycle events.
///
/// Round-trips:
/// - `Begin` — header ResizeHandle press. Captures target + column index.
/// - `Move` — cursor position while the overlay `mouse_area` is active.
/// - `End` — overlay `mouse_area` release; commits width + (when the
///   target persists) schedules a config flush.
#[derive(Debug, Clone, Copy)]
pub enum ColumnResizeEvent {
    /// User pressed a column-resize handle.
    Begin(ColumnResizeTarget, usize),
    /// Cursor-move while the resize overlay is live (x in logical pixels).
    Move(f32),
    /// Drag released.
    End,
}
