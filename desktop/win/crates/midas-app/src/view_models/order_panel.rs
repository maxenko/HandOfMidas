//! View-model for the dockable order panel pane.
//!
//! Two small VMs — one each for the title bar and body — round out
//! the audit P1 view-models migration for the order panel.

use midas_core::LinkMode;

use crate::order_panel::OrderPanelState;

/// Projection of the order panel's TitleBar inputs.
#[derive(Debug, Clone)]
pub struct OrderPanelTitleBarVm {
    /// Pre-formatted title text — `"Order"` when no symbol is bound,
    /// `"Order: AAPL"` otherwise. Falls back to `"Order"` when the
    /// panel is missing.
    pub title_text: String,
    /// Symbol-link mode for the [S] button colour. Defaults to
    /// `Unlinked` for a missing panel.
    pub symbol_link: LinkMode,
}

/// Projection of the order panel's body inputs.
///
/// **Borrowing VM** — the form fields render directly off the
/// existing `OrderPanelState` struct (its String fields back the
/// iced `text_input`s), so cloning the state would force every cell
/// to clone or build text from scratch. The `_for` style mirror of
/// the chart-pane builders would be overkill for this single
/// caller.
#[derive(Debug, Clone, Copy)]
pub struct OrderPanelBodyVm<'a> {
    /// Borrowed form state (entry type, side, qty, TP/SL fields,
    /// errors, etc.). Lifetime tracks the source `MidasApp` borrow.
    pub state: &'a OrderPanelState,
    /// Last traded price for the bound symbol, looked up from
    /// `MarketDataCache`. `None` means the data hasn't arrived yet —
    /// the body shows "Waiting for market data..." in that case.
    pub last_price: Option<f64>,
    /// Coarse step size for mouse-wheel price adjustments. Pre-
    /// computed off `last_price` (or 100.0 fallback) so the view
    /// doesn't redo the calculation per scroll handler.
    pub coarse_step: f64,
}
