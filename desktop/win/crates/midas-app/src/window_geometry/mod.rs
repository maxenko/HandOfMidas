//! OS-window geometry controller (audit P1, slice 2).
//!
//! Owns the four `MidasApp` fields that used to track the main
//! window's geometry directly: `main_window`, `window_position`,
//! `window_size`, `monitor_size`. Together they're the persistence
//! contract for restoring the window to its last position/size on
//! the same monitor across launches.
//!
//! # Pattern
//!
//! Same shape as [`crate::toast::ToastController`] (Slice 0):
//! private state, `update(WindowGeometryMsg) -> Vec<Effect>`,
//! parent interprets effects. Round-trips through
//! [`midas_core::config::WindowConfig`] for persistence so the saved
//! TOML stays byte-identical to the pre-split layout.
//!
//! # Effects
//!
//! - [`Effect::MarkConfigDirty`] fires on every state change so the
//!   parent's debounced save picks it up.
//! - [`Effect::QueryMonitor`] fires after `MainWindowOpened` and
//!   after `Moved` so the parent spawns the iced runtime task that
//!   resolves to a [`WindowGeometryMsg::MonitorSizeResult`].
//!   Controllers can't spawn `iced::Task` directly — that's a
//!   parent-only concern by Halloy convention.

use iced::window;
use midas_core::config::WindowConfig;

/// Window-geometry sub-controller. Field-private; mutate only via
/// [`Self::update`].
#[derive(Debug, Clone)]
pub struct WindowGeometry {
    main_window: Option<window::Id>,
    position: Option<(i32, i32)>,
    size: (u32, u32),
    monitor_size: Option<(u32, u32)>,
}

/// Messages routed to the window-geometry controller.
#[derive(Clone, Debug)]
pub enum WindowGeometryMsg {
    /// The main app window was created with this `window::Id`.
    /// The controller stores the id so subsequent `Moved` events
    /// can re-query the monitor (the user may have dragged the
    /// window onto a different monitor).
    MainWindowOpened(window::Id),
    /// OS reported the main window moved to the given position.
    Moved(i32, i32),
    /// OS reported the main window was resized.
    Resized(u32, u32),
    /// Iced runtime resolved a monitor-size query. `None` means the
    /// query failed (e.g. window has no current monitor); we leave
    /// the previously known monitor size untouched in that case.
    MonitorSizeResult(Option<iced::Size>),
}

/// Effects emitted by the controller for the parent to interpret.
///
/// Two variants — kept tight on purpose so the parent boundary
/// stays narrow. New variants need a real cross-controller use case
/// AND a paragraph of justification at the call site.
#[derive(Debug, Clone)]
pub enum Effect {
    /// Mark the parent's config-dirty flag so the next debounced
    /// save catches up to the current geometry.
    MarkConfigDirty,
    /// Spawn an iced `window::monitor_size(id)` task; the result
    /// re-enters the controller as [`WindowGeometryMsg::MonitorSizeResult`].
    QueryMonitor(window::Id),
}

/// Compile-time guard against [`Effect`] drift.
const _: () = {
    fn _count(e: &Effect) -> u8 {
        match e {
            Effect::MarkConfigDirty => 1,
            Effect::QueryMonitor(_) => 2,
        }
    }
};

impl WindowGeometry {
    /// Fresh controller with the given initial size and no main
    /// window yet. The initial size is the parent's fallback when
    /// [`WindowConfig`] doesn't carry one (rare; only first launch
    /// or hand-edited config).
    #[allow(dead_code)] // tests construct via this; production goes through `from_config`.
    pub fn new(initial_size: (u32, u32)) -> Self {
        Self {
            main_window: None,
            position: None,
            size: initial_size,
            monitor_size: None,
        }
    }

    /// Hydrate from a persisted [`WindowConfig`]. The position and
    /// monitor-size fields are optional in the on-disk schema; the
    /// `main_window` id is intentionally not persisted (iced assigns
    /// fresh ids per launch).
    pub fn from_config(cfg: &WindowConfig, initial_size_fallback: (u32, u32)) -> Self {
        let size = if cfg.width > 0 && cfg.height > 0 {
            (cfg.width, cfg.height)
        } else {
            initial_size_fallback
        };
        let position = match (cfg.x, cfg.y) {
            (Some(x), Some(y)) => Some((x, y)),
            _ => None,
        };
        let monitor_size = match (cfg.monitor_width, cfg.monitor_height) {
            (Some(w), Some(h)) => Some((w, h)),
            _ => None,
        };
        Self {
            main_window: None,
            position,
            size,
            monitor_size,
        }
    }

    /// Project to a [`WindowConfig`] for persistence. The `maximized`
    /// field stays false here — it's not yet wired through iced 0.14
    /// (no event for max/restore is exposed); whoever lights up the
    /// maximize toggle should add a `Message::Window` variant for it
    /// rather than reading window state out-of-band.
    pub fn to_config(&self) -> WindowConfig {
        WindowConfig {
            width: self.size.0,
            height: self.size.1,
            maximized: false,
            x: self.position.map(|(x, _)| x),
            y: self.position.map(|(_, y)| y),
            monitor_width: self.monitor_size.map(|(w, _)| w),
            monitor_height: self.monitor_size.map(|(_, h)| h),
        }
    }

    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    /// Last-saved window position. Used by the `dev_harness` state
    /// dump; production reads go through `to_config` for persistence.
    #[allow(dead_code)]
    pub fn position(&self) -> Option<(i32, i32)> {
        self.position
    }

    /// Last-known monitor size for the window. Exposed for the dev
    /// harness state dump and future auto-fit-to-monitor logic.
    #[allow(dead_code)]
    pub fn monitor_size(&self) -> Option<(u32, u32)> {
        self.monitor_size
    }

    pub fn main_window(&self) -> Option<window::Id> {
        self.main_window
    }

    /// Apply a `WindowGeometryMsg` and return any effects the parent
    /// must interpret. Pure: no I/O, no time queries, no parent
    /// state access.
    pub fn update(&mut self, msg: WindowGeometryMsg) -> Vec<Effect> {
        match msg {
            WindowGeometryMsg::MainWindowOpened(id) => {
                self.main_window = Some(id);
                vec![Effect::QueryMonitor(id)]
            }
            WindowGeometryMsg::Moved(x, y) => {
                self.position = Some((x, y));
                let mut effects = vec![Effect::MarkConfigDirty];
                // Re-query monitor — the user may have dragged onto
                // a different one.
                if let Some(id) = self.main_window {
                    effects.push(Effect::QueryMonitor(id));
                }
                effects
            }
            WindowGeometryMsg::Resized(w, h) => {
                self.size = (w, h);
                vec![Effect::MarkConfigDirty]
            }
            WindowGeometryMsg::MonitorSizeResult(Some(size)) => {
                self.monitor_size = Some((size.width as u32, size.height as u32));
                vec![Effect::MarkConfigDirty]
            }
            WindowGeometryMsg::MonitorSizeResult(None) => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests;
