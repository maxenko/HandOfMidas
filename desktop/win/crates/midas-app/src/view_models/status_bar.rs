//! View-model for the status bar at the bottom of the window.
//!
//! Pre-resolves every input the status bar reads off `MidasApp` —
//! active-chart label, pane count, broker/data-provider connection
//! state + colour, status message, current time, frame-overlay
//! indicator. The view function consumes the VM and stays
//! presentation-only.

use iced::Color;

/// Projected inputs for the status bar.
#[derive(Debug, Clone)]
pub struct StatusBarVm {
    /// Right-side "active chart" descriptor, e.g. `"AAPL | D1"` or
    /// `"No chart"` / `"---"` for the missing/empty cases.
    pub active_info: String,
    /// Pane count, rendered as `"{n} pane(s)"`.
    pub pane_count: usize,
    /// Static suffix appended when the F11 frame overlay is on; empty
    /// otherwise.
    pub overlay_indicator: &'static str,
    /// Free-form status text from `MidasApp::status_message`.
    pub status_message: String,
    /// Pre-formatted clock string from `MidasApp::current_time`.
    pub current_time: String,
    /// Data-provider connection block (left of the broker block).
    pub data_connection: ConnectionBlockVm,
    /// Broker-connection block (the middle section that flips colour
    /// based on Ready/Disconnected/connecting state).
    pub broker_connection: ConnectionBlockVm,
}

/// Dot colour + label for one of the status-bar connection blocks.
#[derive(Debug, Clone)]
pub struct ConnectionBlockVm {
    pub dot_color: Color,
    pub label: String,
}
