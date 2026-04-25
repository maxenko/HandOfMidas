//! Per-window state owned by `MidasApp::windows`.
//!
//! Slice A1 introduces the minimal shape: just the [`WorkspaceLayout`]
//! that used to live on `MidasApp::workspace`. The owning `WindowKey`
//! is the BTreeMap key on `MidasApp::windows`, so it's already
//! reachable from the iterator.
//!
//! Slice C extends this with per-window iced ids, geometry, and the
//! `is_main` / `opening` flags needed once arbitrary user-named
//! windows can be created and closed at runtime. Per the plan's
//! "no `#[allow(dead_code)]` scaffolding" rule, fields land in the
//! slice that first uses them.

use crate::layout::WorkspaceLayout;

/// State for a single application window.
pub struct WindowState {
    /// Pane-grid layout owned by this window. Replaces the singleton
    /// `MidasApp::workspace` field — slice A1.
    pub layout: WorkspaceLayout,
}

impl WindowState {
    /// Construct a `WindowState` from an already-built layout. Used
    /// by `MidasApp::new()` after the legacy single-workspace restore
    /// path produces a `WorkspaceLayout`.
    pub fn new(layout: WorkspaceLayout) -> Self {
        Self { layout }
    }
}
