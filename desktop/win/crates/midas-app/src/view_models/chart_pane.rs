//! View-model for the chart pane's overlay layer (audit P1).
//!
//! Slice 5 (`MidasApp::chart_render_snapshot`) projected the GPU
//! shader's inputs. This slice projects what's left in
//! `view_pane_body` after the snapshot — the four overlay-related
//! `self.*` reads:
//!
//! - `self.market_cache` → G.ATR badge (via `gatr_render_from_cache`)
//! - `self.level_placing` → drawing-panel highlight
//! - `self.level_store` + `chart.editing_level_*` → level-editor popup
//! - `self.link_picker_open` → link-picker dimension (when targeting
//!   this chart)
//!
//! Together with the snapshot builder, these collapse the remaining
//! ~4 `self.*` reads in `view_pane_body` into one VM build call.

use midas_chart::GerchikAtrRender;

use crate::level_store::StoredLevel;
use crate::link::LinkDimension;

/// Overlay state for one chart pane. Each field maps to one of the
/// stacked iced layers built by `view_pane_body` after the GPU shader.
#[derive(Debug, Clone)]
pub struct ChartPaneOverlaysVm {
    /// G.ATR badge data (text + colour + percentage). `None` when no
    /// market snapshot exists for the symbol or when GATR isn't
    /// computed yet.
    pub gatr: Option<GerchikAtrRender>,
    /// Whether the level-placing toolbar highlight is active. Pre-
    /// resolved off `MidasApp::level_placing` so the view doesn't
    /// reach for it.
    pub level_placing: bool,
    /// `Some` while a level on this chart is open in the inline
    /// editor popup.
    pub editing_level: Option<EditingLevelVm>,
    /// `Some(dim)` when the link picker is open targeting this
    /// chart. The view paints the picker overlay; `None` skips it.
    pub link_picker_dim: Option<LinkDimension>,
}

/// Inputs the inline level-editor popup needs.
#[derive(Debug, Clone)]
pub struct EditingLevelVm {
    /// The level being edited (cloned out of the level store so the
    /// VM stays owning).
    pub level: StoredLevel,
    /// Screen-space anchor for the popup (set when the user clicked
    /// the level on the chart).
    pub screen_pos: (f32, f32),
    /// Current text in the editor's price input.
    pub price_input: String,
    pub viewport_width: u32,
    pub viewport_height: u32,
}
