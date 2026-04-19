//! View-model for the top toolbar.
//!
//! Resolves the two provider-picker drop-downs (data + broker) up
//! front so the view doesn't reach into `MidasApp::providers` four
//! times.

/// Pre-resolved inputs for the toolbar's two provider drop-downs.
/// The other toolbar buttons are static and stay inline in the view.
#[derive(Debug, Clone)]
pub struct ToolbarVm {
    /// Available data-provider names (drives the data drop-down).
    pub data_provider_names: Vec<String>,
    /// Currently active data-provider display name.
    pub active_data_provider: String,
    /// Available broker names (drives the broker drop-down).
    pub broker_names: Vec<String>,
    /// Currently active broker display name.
    pub active_broker: String,
}
