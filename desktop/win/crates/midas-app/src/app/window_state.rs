//! Per-window state owned by `MidasApp::windows`.
//!
//! Slice A1 introduced the minimal shape: just the [`WorkspaceLayout`].
//! Slice C extends with per-window iced ids, geometry, and the
//! `is_main` / `opening` flags needed once arbitrary user-named
//! windows can be created and closed at runtime.

use std::time::Instant;

use iced::window;

use midas_core::config::WindowGeometryConfig;
use midas_core::WindowKey;

use crate::layout::WorkspaceLayout;

/// Tracking entry for an in-flight `window::open` task.
///
/// Populated by [`super::Message::CreateWindow`] and drained either by
/// [`super::Message::WindowAttached`] (success) or
/// [`super::Message::WindowAttachWatchdog`] after 5 s without an
/// attach (failure → [`super::Message::WindowAttachFailed`]).
pub struct WindowAttachAttempt {
    /// User-visible name the window is being opened under.
    pub key: WindowKey,
    /// Wall-clock instant the open task was spawned.
    pub started_at: Instant,
}

/// State for a single application window.
pub struct WindowState {
    /// User-visible name; mirrors the `BTreeMap` key on
    /// `MidasApp::windows`. Cached here so handlers that already hold
    /// a `&WindowState` don't have to thread the key through their
    /// signature.
    pub key: WindowKey,
    /// Whether this is the main window. Exactly one entry has this
    /// flag set after the load-time validation pass; closing the main
    /// window quits the app, closing any other window just disposes
    /// its panels.
    pub is_main: bool,
    /// iced runtime id for the OS window. `None` until
    /// `window::open`'s task resolves into [`super::Message::WindowAttached`].
    pub iced_id: Option<window::Id>,
    /// Pane-grid layout owned by this window. Replaces the singleton
    /// `MidasApp::workspace` field (slice A1).
    pub layout: WorkspaceLayout,
    /// Persisted size + position + monitor for restore. Updated by
    /// the OS focus / move / resize event subscription.
    pub geometry: WindowGeometryConfig,
    /// `true` between `CreateWindow` and `WindowAttached`, used to
    /// suppress geometry events that would otherwise fire on the
    /// initial OS placement of a brand-new window. Cleared on attach.
    pub opening: bool,
}

impl WindowState {
    /// Hydrate a non-main window from its persisted [`WindowGeometryConfig`]
    /// and an empty placeholder layout. Used by slice C's startup path
    /// when iterating `config.windows` and by the `CreateWindow`
    /// handler. The `iced_id` stays `None` until the daemon task
    /// resolves.
    pub fn opening(key: WindowKey, is_main: bool, geometry: WindowGeometryConfig) -> Self {
        Self {
            key,
            is_main,
            iced_id: None,
            layout: WorkspaceLayout::placeholder(),
            geometry,
            opening: true,
        }
    }
}
