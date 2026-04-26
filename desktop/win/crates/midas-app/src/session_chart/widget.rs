//! [`SessionChart`] — the iced-compatible widget scaffold.
//!
//! ## Phase C scope (S10–S14)
//!
//! Widens the S8 vertical-slice MVP from "crypto-only, ContinuousAxis,
//! M1 only" to a **calendar-agnostic**, **multi-period**, **EhPolicy-
//! aware** widget that the session-aware-charts plan calls for.
//!
//! Highlights:
//!
//! - [`AxisKind`] enumerates the three supported axis shapes:
//!   - `Continuous` — crypto / anything whose calendar reports
//!     [`TimeAxisPolicy::Continuous`][midas_calendar::TimeAxisPolicy].
//!   - `Compressed` — equities / futures / FX
//!     ([`TimeAxisPolicy::CompressedSessionBoundaries`]).
//!   - `SessionIndex` — analytical / extreme-zoom (reserved; the Phase
//!     C widget never constructs this, but the enum carves the slot so
//!     Phase F's auto-switch lands without a breaking change).
//! - [`EhPolicy`] — per the ideal design:
//!   - `ShowAll` — pre+RTH+post candles + bands + separators + full
//!     chrome.
//!   - `HideExtended` — ethicksmarket bars filtered OUT via
//!     [`midas_stream::Filtered<_, EhFilter>`] at the stream seam;
//!     the scene chrome stays identical (band/separator layers paint
//!     only *visible* sessions, and nothing in Extended windows is
//!     visible by construction).
//!   - `ShowBarsOnly` — bars render (tinted by session kind from the
//!     candle's `SessionKind`) but bands + separators don't emit.
//! - Period picker — the widget carries a [`BarPeriod`][midas_bars::BarPeriod]
//!   field. Re-subscription lives on the host (the handler in `app.rs`
//!   owns the driver lifetime); the widget exposes
//!   [`SessionChart::set_period`] for the host to reflect the new
//!   period after a re-subscribe.
//!
//! ## State model
//!
//! The widget is a **value** (not a pin-pointer-stable iced widget
//! node) — all large fields are `Arc`-shared or `Copy`. `series` is
//! an [`Arc<RwLock<CandleSeries>>`] shared with the driver — the pump
//! is the sole writer, the paint path is one of several concurrent
//! readers. Paint takes a short-lived `read()` guard inside
//! [`paint_buckets`](SessionChart::paint_buckets) and drops it before
//! returning; the guard never escapes the paint scope and never
//! crosses an `.await`.
//!
//! ## Interaction
//!
//! Event translation — mouse move updating `interaction.crosshair_px`,
//! wheel → zoom, drag → pan — is an input-layer concern. The widget
//! exposes direct mutators
//! ([`set_crosshair`](SessionChart::set_crosshair),
//! [`clear_crosshair`](SessionChart::clear_crosshair),
//! [`set_viewport`](SessionChart::set_viewport),
//! [`set_axis`](SessionChart::set_axis),
//! [`set_price_range`](SessionChart::set_price_range),
//! [`cycle_eh_policy`](SessionChart::cycle_eh_policy)) so host code +
//! integration tests can drive the state manually.
//!
//! ## Open TODOs deferred to later slices
//!
//! - Keyboard shortcuts (arrow-keys pan, +/- zoom, "E" cycles
//!   [`EhPolicy`], "X" close window). The iced window handler in
//!   `app.rs` wires the close action; the remaining bindings are UX
//!   polish scheduled post-Phase-C.
//! - On-screen toggle chip ("EH") — lives in the host window's view,
//!   not the widget value itself. The widget exposes
//!   [`SessionChart::eh_policy`] so the view can read it.
//! - Period cycling UX — the widget supports any [`BarPeriod`] the
//!   calendar accepts; the drop-down / cycling-button affordance is
//!   again a host-view concern.
//! - G-4 disk persistence of `(eh_policy, period, axis_kind)`.
//!   Deferred.

use std::sync::Arc;

use midas_axis::{AxisError, CompressedAxis, ContinuousAxis, PriceRange, Viewport};
use midas_bars::{BarPeriod, CandleSeries};
use midas_calendar::{ExchangeCalendar, Timestamp};
use midas_scene::layers::{LevelDragState, LevelView, SharedLevelDrag};
use midas_scene::tools::{
    BracketTool, ContextMenuAction, LegRole as SceneLegRole, LevelTool, Side as SceneBracketSide,
    ToolEffect,
};
use midas_scene::{
    InteractionState, ScenePrimitives, SharedCandleSeries, ThemePalette, VolumeProfileConfig,
};
use parking_lot::{Mutex, RwLock};

use super::axis_box::AxisBox;
use super::driver::SessionChartDriver;
use super::policy::EhPolicy;
use super::primitives_bridge::{translate, RenderBuckets};
use super::scene_builder::{build_scene, SceneConfig, SceneLayers};

pub use super::axis_box::AxisKind;

/// `tracing` target shared by every log emitted from this module.
const TRACE_TARGET: &str = "midas_app::session_chart::widget";

/// Error surface for [`SessionChart`] construction. App-harden M5:
/// previously the widget's axis construction `.expect`-panicked the UI
/// thread on degenerate inputs. Callers now handle the `Err` branch
/// and fall back gracefully (typically by closing the offending
/// window).
#[derive(Debug, thiserror::Error)]
pub enum SessionChartError {
    /// Axis construction rejected the inputs (degenerate time window,
    /// bad viewport width, etc.). Wraps the underlying
    /// [`AxisError`] for diagnostics.
    #[error("session chart axis construction failed: {0}")]
    Axis(#[from] AxisError),
}

/// Slice 4 chart-transition: persistent holder for the level-placement
/// tool + shared drag state. Lives on [`SessionChart`] so the tool FSM
/// and drag session survive the per-frame scene rebuild.
#[derive(Debug, Default)]
pub struct LevelToolHost {
    /// Active level-placement tool, if the user activated the toolbar
    /// "Add Level" button. `None` while the tool is off.
    pub tool: Option<LevelTool>,
    /// Shared drag-state Arc — cloned into every `LevelLayer` the
    /// widget builds per frame so drag session survives scene rebuilds.
    pub drag: SharedLevelDrag,
    /// Cached level views painted this frame. Built by the host from
    /// `AnnotationStore::levels_for` and handed to the scene via the
    /// layer pipeline; kept here so `project_effects` can resolve
    /// `UpdateLevel { id, price }` back to the owning symbol.
    pub levels: Vec<LevelView>,
}

impl LevelToolHost {
    pub fn new() -> Self {
        Self {
            tool: None,
            drag: Arc::new(Mutex::new(LevelDragState::default())),
            levels: Vec::new(),
        }
    }

    /// Is the placement tool currently active (user clicked "Add Level")?
    pub fn is_active(&self) -> bool {
        self.tool.as_ref().is_some_and(|t| t.is_placing())
    }

    /// Activate the tool. Caller is the widget's toolbar handler.
    pub fn activate(&mut self) {
        self.tool = Some(LevelTool::placing());
    }

    /// Deactivate the tool. Called on Escape at the widget edge and on
    /// window close.
    pub fn deactivate(&mut self) {
        self.tool = None;
    }

    /// Replace the level-view list — typically called once per frame
    /// from the level list the `AnnotationStore` exposes for the
    /// current symbol.
    pub fn set_levels(&mut self, levels: Vec<LevelView>) {
        self.levels = levels;
    }
}

/// Slice 5b chart-transition: persistent holder for the bracket-tool
/// FSM. Lives on [`SessionChart`] so the FSM survives per-frame scene
/// rebuilds.
#[derive(Debug, Default)]
pub struct BracketToolHost {
    /// Active bracket-placement tool, if the user activated
    /// "Buy Bracket" / "Sell Bracket". `None` while off.
    pub tool: Option<BracketTool>,
    /// The side the user chose when activating the tool. Kept separate
    /// from the tool's own `side()` observer because the FSM drops the
    /// side in `Complete` — this field powers the "reset to
    /// AwaitingEntry { side }" multi-bracket workflow.
    last_activated_side: Option<SceneBracketSide>,
}

impl BracketToolHost {
    pub fn new() -> Self {
        Self {
            tool: None,
            last_activated_side: None,
        }
    }

    /// True iff the bracket tool is active + placing (non-Idle, non-
    /// Complete).
    pub fn is_active(&self) -> bool {
        self.tool.as_ref().is_some_and(|t| t.is_placing())
    }

    /// Activate the tool for a Long (Buy) bracket.
    pub fn activate_buy(&mut self) {
        self.tool = Some(BracketTool::awaiting_entry(SceneBracketSide::Long));
        self.last_activated_side = Some(SceneBracketSide::Long);
    }

    /// Activate the tool for a Short (Sell) bracket.
    pub fn activate_sell(&mut self) {
        self.tool = Some(BracketTool::awaiting_entry(SceneBracketSide::Short));
        self.last_activated_side = Some(SceneBracketSide::Short);
    }

    /// Deactivate the tool. Called on Escape / window close / tool swap.
    /// R11: mid-placement deactivation must also translate to a
    /// `CancelBracket` on `TickerState` — the widget handles that via
    /// [`SessionChart::deactivate_bracket_tool`].
    pub fn deactivate(&mut self) {
        self.tool = None;
        self.last_activated_side = None;
    }
}

/// A single translated effect ready for the app's `update()` to route.
///
/// Widget-level helper returned by [`SessionChart::drain_level_effects`]
/// so the caller (dev-harness scripts, unit tests, the app binary's
/// message translator) never has to match on the raw [`ToolEffect`]
/// variant. Slice 5b (chart-transition plan) adds bracket variants
/// that translate to the existing draft-then-save `TickerMsg` sequence
/// (plan C1 / architecture rule 8 — no new `TickerMsg` variant).
#[derive(Clone, Debug, PartialEq)]
pub enum ProjectedEffect {
    /// Commit a new level at `price`. Host translates to
    /// `Message::CreateLevel(symbol, price, lock)`.
    CreateLevel { price: f64, lock: bool },
    /// Update an existing level's price. Host translates to
    /// `Message::UpdateLevel(symbol, id, price)`.
    UpdateLevel { id: u64, price: f64 },
    /// Delete a level.
    DeleteLevel { id: u64 },
    /// Open a context menu.
    OpenContextMenu {
        x: f32,
        y: f32,
        annotation_id: u64,
        action: ContextMenuAction,
    },
    /// Start a draft bracket. Translates to
    /// `TickerMsg::EnsureDraftBracket { side, entry_type: Limit }`
    /// + `TickerMsg::SetLegPrice { role: Entry, price: entry }`.
    ///
    /// The host maps the scene-native [`Side`][midas_scene::tools::Side]
    /// onto its own domain-side type at the dispatch layer. Carrying
    /// the scene-native shape here keeps the session-chart path free
    /// of the legacy `midas-chart` widget vocabulary (chart-transition
    /// slice 8.5 grep gate).
    BeginDraftBracket { side: SceneBracketSide, entry: f64 },
    /// Set a TP / SL leg on the draft bracket. Translates to
    /// `TickerMsg::SetLegPrice { role, price }` plus
    /// `SetTpEnabled(true)` / `SetSlEnabled(true)` as appropriate.
    SetDraftLeg { role: SceneLegRole, price: f64 },
    /// Finalise the draft bracket. Translates to
    /// `TickerMsg::SaveBracket`.
    CommitDraftBracket,
    /// Discard the draft bracket. Translates to
    /// `TickerMsg::CancelBracket`.
    CancelDraftBracket,
    /// Drag-move on a live bracket's TP / SL leg. Translates to
    /// `TickerMsg::SetLegPrice { role, price }` (the live bracket's
    /// leg id is implicit — the app's ticker state owns the live
    /// bracket for the symbol).
    UpdateLiveBracketLeg {
        bracket_id: u64,
        role: SceneLegRole,
        price: f64,
    },
    /// Tool-layer error (panic fallback, persistence fault, etc.).
    Error(String),
}

impl ProjectedEffect {
    /// Translate one raw [`ToolEffect`] into the projected form. Context
    /// menu variants expand to one `ProjectedEffect` per item so the
    /// host can wire each action independently.
    pub fn from_tool_effect(raw: ToolEffect) -> Vec<Self> {
        match raw {
            ToolEffect::CreateLevel { price, lock } => {
                vec![ProjectedEffect::CreateLevel { price, lock }]
            }
            ToolEffect::UpdateLevel { id, price } => {
                vec![ProjectedEffect::UpdateLevel { id, price }]
            }
            ToolEffect::DeleteLevel { id } => vec![ProjectedEffect::DeleteLevel { id }],
            ToolEffect::OpenContextMenu { pt, items } => items
                .into_iter()
                .map(|item| {
                    let annotation_id = match item.action {
                        ContextMenuAction::Edit { id } => id,
                        ContextMenuAction::ToggleLock { id } => id,
                        ContextMenuAction::Delete { id } => id,
                    };
                    ProjectedEffect::OpenContextMenu {
                        x: pt.x,
                        y: pt.y,
                        annotation_id,
                        action: item.action,
                    }
                })
                .collect(),
            ToolEffect::ReportError(err) => vec![ProjectedEffect::Error(err.to_string())],
            ToolEffect::BeginDraftBracket { side, entry } => {
                vec![ProjectedEffect::BeginDraftBracket { side, entry }]
            }
            ToolEffect::SetDraftLeg { role, price } => {
                vec![ProjectedEffect::SetDraftLeg { role, price }]
            }
            ToolEffect::CommitDraftBracket => vec![ProjectedEffect::CommitDraftBracket],
            ToolEffect::CancelDraftBracket => vec![ProjectedEffect::CancelDraftBracket],
            ToolEffect::UpdateBracketLeg { id, role, price } => {
                vec![ProjectedEffect::UpdateLiveBracketLeg {
                    bracket_id: id,
                    role,
                    price,
                }]
            }
        }
    }
}

/// Calendar-agnostic scaffold widget. Holds per-frame inputs for the
/// scene pipeline plus the shared series/driver handles.
pub struct SessionChart {
    /// Shared candle series written by the driver, read here through a
    /// short-lived read-guard during paint. Using the same
    /// `Arc<RwLock<_>>` the driver writes to — no deep-copy per frame
    /// (arch-audit F1 / R1 fix).
    series: SharedCandleSeries,
    /// Reference to the pump driver so dropping the widget cascades
    /// shutdown (driver's `Drop` aborts the pump task).
    #[allow(dead_code)]
    driver: Arc<SessionChartDriver>,
    /// The current time axis — sum over [`ContinuousAxis`] and
    /// [`CompressedAxis`].
    axis: AxisBox,
    /// Vertical price range shown in the viewport.
    price_range: PriceRange,
    /// Chart rectangle in logical pixels.
    viewport: Viewport,
    /// Colour palette.
    palette: ThemePalette,
    /// Calendar for session classification.
    calendar: &'static dyn ExchangeCalendar,
    /// Current bar period. Mutated via [`SessionChart::set_period`] by
    /// the host after a re-subscribe.
    period: BarPeriod,
    /// Per-chart interaction (hover / drag / crosshair / last-wheel).
    interaction: InteractionState,
    /// Extended-hours policy. Defaults to [`EhPolicy::ShowAll`].
    eh_policy: EhPolicy,
    /// Time range to warm the session-band + separator caches with.
    /// Typically derived from the axis; exposed so callers can pin a
    /// wider range for pre-load.
    time_window: (Timestamp, Timestamp),
    /// Reusable primitives buffer.
    primitives: ScenePrimitives,
    /// Last observed [`CandleSeries::version`] at paint entry. Used by
    /// [`paint_buckets`](Self::paint_buckets) to short-circuit recompute
    /// of session-band / separator buffers when the series has not
    /// advanced — tracking R1's "avoid rebuilding sessions-buffer if
    /// version hasn't changed" follow-on.
    version_at_paint: u64,
    /// Slice 4 of chart-transition: persistent level-tool + drag state
    /// + cached level views. Survives the per-frame scene rebuild.
    level_host: LevelToolHost,
    /// Slice 5b of chart-transition: persistent bracket-tool FSM.
    /// Survives per-frame scene rebuilds alongside the level host.
    bracket_host: BracketToolHost,
    /// Slice 4: queue of projected tool effects. The widget drives the
    /// scene through `handle_input` (slice 1), then drains the effect
    /// queue into this vec. Host code calls
    /// [`drain_level_effects`](Self::drain_level_effects) per frame.
    pending_effects: Vec<ProjectedEffect>,
    /// Slice 3 of the VP-anchored plan: per-chart Volume Profile.
    /// `show_volume_profile` enables the layer; `volume_profile_config`
    /// carries the (already-bridged, kill-switch-resolved) anchor +
    /// width fraction + max profiles. The host
    /// (`crate::app::MidasApp`) is responsible for resolving the
    /// `experimental.disable_anchored_vp` kill-switch and the
    /// `midas_core ↔ midas_scene` enum bridge BEFORE handing the
    /// config to [`set_volume_profile`](Self::set_volume_profile).
    show_volume_profile: bool,
    volume_profile_config: VolumeProfileConfig,
}

impl SessionChart {
    /// Construct the widget. Calendar decides the axis shape —
    /// continuous for crypto, compressed for XNYS. Caller-supplied
    /// time window and viewport width configure the initial axis
    /// range.
    ///
    /// Returns [`SessionChartError::Axis`] on degenerate inputs
    /// (zero/negative viewport width, inverted time range, etc.) so
    /// the host can close the pending window rather than crash on a
    /// silent panic. App-harden M5.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        driver: Arc<SessionChartDriver>,
        calendar: &'static dyn ExchangeCalendar,
        period: BarPeriod,
        price_range: PriceRange,
        viewport: Viewport,
        palette: ThemePalette,
        time_window: (Timestamp, Timestamp),
    ) -> Result<Self, SessionChartError> {
        let series = driver.series();
        let axis = AxisBox::try_for_calendar(calendar, time_window, viewport.width_px)?;
        Ok(Self {
            series,
            driver,
            axis,
            price_range,
            viewport,
            palette,
            calendar,
            period,
            interaction: InteractionState::new(),
            eh_policy: EhPolicy::default(),
            time_window,
            primitives: ScenePrimitives::default(),
            // Bootstrap sentinel — mismatched against any real
            // `series.version()` so the first paint always runs the
            // full band/separator compute path.
            version_at_paint: u64::MAX,
            level_host: LevelToolHost::new(),
            bracket_host: BracketToolHost::new(),
            pending_effects: Vec::new(),
            show_volume_profile: false,
            volume_profile_config: VolumeProfileConfig::default(),
        })
    }

    // ── Mutators ────────────────────────────────────────────────────

    /// Replace the viewport (window resize). Rebuilds the axis at the
    /// new width so pixel-space queries stay correct.
    pub fn set_viewport(&mut self, viewport: Viewport) {
        self.viewport = viewport;
        self.rebuild_axis();
    }

    /// Replace the price range (auto-scale / zoom Y).
    pub fn set_price_range(&mut self, pr: PriceRange) {
        self.price_range = pr;
    }

    /// Replace the session-band / separator warm-up window AND the
    /// axis range. Called by the host on pan/zoom.
    pub fn set_time_window(&mut self, window: (Timestamp, Timestamp)) {
        self.time_window = window;
        self.rebuild_axis();
    }

    /// Replace the bar period. The host is responsible for re-
    /// subscribing the driver through a fresh
    /// [`crate::session_chart::SessionChartDriver::spawn`] before
    /// calling this mutator — the widget just reflects the new state.
    pub fn set_period(&mut self, period: BarPeriod) {
        self.period = period;
    }

    /// Replace the [`EhPolicy`]. Re-subscription through the stream
    /// filter is the host's concern (see
    /// `session_chart/mod.rs` module docs); the widget just reflects
    /// the new state.
    pub fn set_eh_policy(&mut self, policy: EhPolicy) {
        self.eh_policy = policy;
    }

    /// Cycle through the three [`EhPolicy`] variants. Returns the new
    /// policy so the host can use the return value for re-subscribe.
    pub fn cycle_eh_policy(&mut self) -> EhPolicy {
        let next = self.eh_policy.next();
        self.eh_policy = next;
        next
    }

    /// Update the crosshair position. Called from `CursorMoved` events.
    pub fn set_crosshair(&mut self, px: (f32, f32)) {
        self.interaction.crosshair_px = Some(px);
    }

    /// Clear the crosshair (pointer left the widget).
    pub fn clear_crosshair(&mut self) {
        self.interaction.crosshair_px = None;
    }

    /// Replace the axis directly. Rare — the host typically hands pan/
    /// zoom through [`set_time_window`] so the axis kind stays
    /// calendar-consistent. Provided for tests that want fine-grained
    /// control.
    pub fn set_axis(&mut self, axis: ContinuousAxis) {
        self.axis = AxisBox::Continuous(axis);
    }

    // ── Slice 2b: pan / zoom / keyboard helpers ─────────────────────

    /// Pan the visible time window horizontally by `dx_px`. Positive
    /// shifts right, negative shifts left. Rebuilds the axis at the
    /// new window so pixel-space queries stay correct.
    pub fn pan_x(&mut self, dx_px: f32) {
        let new = midas_scene::interaction::pan_time_window(
            self.time_window,
            dx_px,
            self.viewport.width_px,
        );
        tracing::debug!(
            target: "midas_app::session_chart::pan_x",
            dx_px,
            "pan x",
        );
        self.set_time_window(new);
    }

    /// Zoom the x-axis about a pixel anchor. `factor < 1.0` zooms in
    /// (narrows the visible span), `> 1.0` zooms out. Uses the
    /// period's wall-clock duration as the min-10-candles floor.
    pub fn zoom_x_at(&mut self, anchor_x_px: f32, factor: f32) {
        let candle_width_ns = bar_period_ns(self.period);
        let new = midas_scene::interaction::zoom_time_window_at(
            self.time_window,
            anchor_x_px,
            self.viewport.width_px,
            factor,
            candle_width_ns,
        );
        tracing::debug!(
            target: "midas_app::session_chart::zoom_x_at",
            anchor_x_px,
            factor,
            "zoom x",
        );
        self.set_time_window(new);
    }

    /// Zoom the y-axis (price) about a pixel anchor. No-op on
    /// degenerate inputs (factor ≤ 0, non-finite anchor).
    pub fn zoom_y_at(&mut self, anchor_y_px: f32, factor: f32) {
        if let Some(new_range) = midas_scene::interaction::zoom_price_range_at(
            self.price_range,
            anchor_y_px,
            self.viewport.height_px,
            factor,
        ) {
            tracing::debug!(
                target: "midas_app::session_chart::zoom_y_at",
                anchor_y_px,
                factor,
                "zoom y",
            );
            self.price_range = new_range;
        }
    }

    /// Jump the x-window to the FIRST bar in the series. No-op on an
    /// empty series. Keeps the current span.
    pub fn jump_home(&mut self) {
        let guard = self.series.read();
        let Some(first) = guard.at(0) else { return };
        let first_ts = first.ts_open();
        let span = self.time_window.1 - self.time_window.0;
        drop(guard);
        tracing::debug!(
            target: "midas_app::session_chart::jump_home",
            first_ts = %first_ts,
            "jump to first bar",
        );
        self.set_time_window((first_ts, first_ts + span));
    }

    /// Jump the x-window so the LAST bar sits at the right edge.
    /// No-op on an empty series. Keeps the current span.
    pub fn jump_end(&mut self) {
        let guard = self.series.read();
        let n = guard.len();
        if n == 0 {
            return;
        }
        let last = guard.at(n - 1).expect("bounds checked");
        let last_ts = last.ts_open();
        let span = self.time_window.1 - self.time_window.0;
        drop(guard);
        tracing::debug!(
            target: "midas_app::session_chart::jump_end",
            last_ts = %last_ts,
            "jump to last bar",
        );
        self.set_time_window((last_ts - span, last_ts));
    }

    /// Auto-scale the price range to fit the visible candles.
    /// Returns `true` iff the range changed. Used by the "first-ever
    /// data arrived, no saved viewport" path and by an explicit
    /// user-triggered auto-scale shortcut.
    pub fn auto_scale_price_to_visible(&mut self) -> bool {
        let guard = self.series.read();
        let n = guard.len();
        if n == 0 {
            return false;
        }
        // Visible range: the first → last bar (could be narrowed to a
        // binary-search of the current time_window — left as a
        // follow-up slice when a windowed view is needed).
        let visible = 0..n;
        let Some(new_range) = midas_scene::interaction::auto_scale_price(&guard, visible) else {
            return false;
        };
        drop(guard);
        if new_range != self.price_range {
            tracing::debug!(
                target: "midas_app::session_chart::auto_scale_price",
                low = new_range.low(),
                high = new_range.high(),
                "auto-scale price",
            );
            self.price_range = new_range;
            true
        } else {
            false
        }
    }

    /// Call once per frame — if the series has just gone from empty
    /// to non-empty AND no external caller has pinned a price range,
    /// auto-scale to fit. Returns `true` if auto-scale ran.
    ///
    /// "First-ever transition" is signalled by `version_at_paint ==
    /// u64::MAX` (the bootstrap sentinel) matched against a
    /// non-zero live series version.
    pub fn auto_scale_on_first_data(&mut self) -> bool {
        let v = { self.series.read().version() };
        let is_first_data = self.version_at_paint == u64::MAX && v > 0;
        if !is_first_data {
            return false;
        }
        self.auto_scale_price_to_visible()
    }
}

/// Approximate wall-clock nanoseconds per bar for the given period.
/// Used by zoom clamping to enforce the min-10-candles floor on an
/// unknown calendar (compressed gaps are ignored — the clamp is
/// intentionally loose).
fn bar_period_ns(period: BarPeriod) -> i64 {
    use chrono::Duration;
    match period {
        p if p == BarPeriod::m1() => Duration::minutes(1).num_nanoseconds().unwrap_or(0),
        p if p == BarPeriod::m5() => Duration::minutes(5).num_nanoseconds().unwrap_or(0),
        p if p == BarPeriod::d1_rth() => Duration::days(1).num_nanoseconds().unwrap_or(0),
        p if p == BarPeriod::w1() => Duration::days(7).num_nanoseconds().unwrap_or(0),
        // Fallback for any BarPeriod we didn't special-case — a
        // minute's worth of ns is close enough for a zoom floor.
        _ => Duration::minutes(1).num_nanoseconds().unwrap_or(0),
    }
}

impl SessionChart {
    /// Replace the axis with a compressed one directly.
    pub fn set_compressed_axis(&mut self, axis: CompressedAxis) {
        self.axis = AxisBox::Compressed(Box::new(axis));
    }

    /// Rebuild the time axis in-place. If the new inputs are
    /// degenerate (e.g. a Viewport with zero width during a resize
    /// animation), the existing axis is preserved and a warning is
    /// logged — callers don't need the axis to vanish mid-interaction.
    fn rebuild_axis(&mut self) {
        match AxisBox::try_for_calendar(self.calendar, self.time_window, self.viewport.width_px) {
            Ok(ax) => self.axis = ax,
            Err(e) => {
                tracing::warn!(
                    width = self.viewport.width_px,
                    from = %self.time_window.0,
                    to = %self.time_window.1,
                    error = %e,
                    "session_chart: axis rebuild rejected; keeping previous axis",
                );
            }
        }
    }

    // ── Observers ───────────────────────────────────────────────────

    /// Current axis kind (as picked by the calendar).
    pub fn axis_kind(&self) -> AxisKind {
        self.axis.kind()
    }

    /// Current EhPolicy.
    pub fn eh_policy(&self) -> EhPolicy {
        self.eh_policy
    }

    /// Current period.
    pub fn period(&self) -> BarPeriod {
        self.period
    }

    /// Current calendar.
    pub fn calendar(&self) -> &'static dyn ExchangeCalendar {
        self.calendar
    }

    /// Borrow the interaction state.
    pub fn interaction(&self) -> &InteractionState {
        &self.interaction
    }

    /// Current viewport.
    pub fn viewport(&self) -> Viewport {
        self.viewport
    }

    /// Current price range.
    pub fn price_range(&self) -> PriceRange {
        self.price_range
    }

    /// Reference to the shared candle series. Tests use this to read
    /// len / version after driving the pipeline.
    pub fn series(&self) -> SharedCandleSeries {
        Arc::clone(&self.series)
    }

    /// Current time-axis window (`(start, end)` UTC). Exposed for slice
    /// 8b of the chart-transition plan so the devloop `DumpState`
    /// projection can surface the visible axis range without rebuilding
    /// a scene.
    pub fn time_window(&self) -> (Timestamp, Timestamp) {
        self.time_window
    }

    /// String projection of the "layer id that currently captures
    /// events", used by the slice-8b `DumpState` projection. Mirrors
    /// the intent of `ChartScene::drag_focus()` but reads from the
    /// widget's `InteractionState.drag` because the scene is rebuilt
    /// per frame and doesn't hold stateful drag focus.
    ///
    /// Maps:
    /// - active level tool drag → `"levels"`
    /// - bracket leg drag → `"order_bracket"` (reserved, slice 5b)
    /// - price-line drag → `"price_line"`
    /// - candle / no drag → `None`
    pub fn drag_focus_label(&self) -> Option<&'static str> {
        self.interaction.drag.as_ref().map(|d| match d.target {
            midas_scene::HoverTarget::Level(_) => "levels",
            midas_scene::HoverTarget::Bracket { .. } => "order_bracket",
            midas_scene::HoverTarget::PriceLine(_) => "price_line",
            midas_scene::HoverTarget::Candle(_) => "candles",
        })
    }

    /// Which interactive tool is currently active on the widget. Slice
    /// 4 wires only the level tool; slice 5b adds the bracket tool.
    /// Exposed for slice 8b's `DumpState` projection — returns the
    /// string form as the devloop proto knows it ("level" / "bracket").
    pub fn active_tool_label(&self) -> Option<&'static str> {
        if self.level_host.is_active() {
            Some("level")
        } else {
            None
        }
    }

    /// End-to-end: snapshot the current series version, build a scene
    /// (sharing the same `Arc<RwLock<CandleSeries>>` with the driver —
    /// no deep-copy per frame, arch-audit F1 / R1), paint primitives,
    /// translate into [`RenderBuckets`]. This is what the eventual
    /// shader `Program::draw` will call to hand GPU-layout data to
    /// `wgpu`.
    ///
    /// The session-band / separator buffers and the holiday-marker
    /// year range are recomputed only when the series version has
    /// advanced since the last paint — the common "hover redraw on
    /// unchanged series" path stays allocation-free.
    ///
    /// # Panics
    ///
    /// Panics if `build_scene` errors. With the canonical construction
    /// path all three required builder inputs (axis/price_range/
    /// viewport) are set, so an error here indicates a programming
    /// mistake.
    pub fn paint_buckets(&mut self) -> RenderBuckets {
        // Version check — skip band/separator recompute when unchanged.
        // The read-guard is short-lived and released before `build_scene`.
        let current_version = { self.series.read().version() };
        let series_changed = current_version != self.version_at_paint;
        self.version_at_paint = current_version;
        // Select the matching axis variant and dispatch to
        // `build_scene` with the right monomorphisation.
        let is_xnys = self.axis.kind() == AxisKind::Compressed;
        let layers = self.layers_for_policy(is_xnys);
        // VP layer wants the whole visible series range. The widget
        // doesn't expose a narrower selection today; if S6 lands a
        // "show only the most-recent N candles" option, plumb it here.
        let vp_range = 0..self.series.read().len();
        let scene = match &self.axis {
            AxisBox::Continuous(axis) => build_scene(SceneConfig {
                series: Arc::clone(&self.series),
                axis: axis.clone(),
                price_range: self.price_range,
                viewport: self.viewport,
                palette: self.palette,
                calendar: self.calendar,
                interaction: &self.interaction,
                layers,
                time_window: self.time_window,
                series_changed,
                volume_profile_config: self.volume_profile_config.clone(),
                volume_profile_range: vp_range.clone(),
            }),
            AxisBox::Compressed(axis) => build_scene(SceneConfig {
                series: Arc::clone(&self.series),
                axis: (**axis).clone(),
                price_range: self.price_range,
                viewport: self.viewport,
                palette: self.palette,
                calendar: self.calendar,
                interaction: &self.interaction,
                layers,
                time_window: self.time_window,
                series_changed,
                volume_profile_config: self.volume_profile_config.clone(),
                volume_profile_range: vp_range,
            }),
        }
        .expect("build_scene with canonical inputs");
        scene.paint(&mut self.primitives);
        translate(&self.primitives)
    }

    /// Convenience wrapper around [`SceneLayers::from_eh_policy`]
    /// parameterised by whether this chart sits on XNYS. Exposed for
    /// unit tests that want to assert the layer config separately
    /// from the paint step.
    pub fn layers_for_policy(&self, is_xnys: bool) -> SceneLayers {
        let mut layers = SceneLayers::from_eh_policy(self.eh_policy, is_xnys);
        layers.volume_profile = self.show_volume_profile;
        layers
    }

    /// Slice 3 (VP-anchored): toggle the Volume Profile layer on/off
    /// and replace its config in one call. Host responsibility:
    /// resolve `experimental.disable_anchored_vp` and the
    /// `midas_core::VolumeProfileAnchor → midas_scene::VolumeProfileAnchor`
    /// bridge BEFORE calling.
    pub fn set_volume_profile(&mut self, show: bool, config: VolumeProfileConfig) {
        self.show_volume_profile = show;
        self.volume_profile_config = config;
    }

    /// Read-only view of the current VP config — exposed for tests
    /// that want to assert the host's resolution outcome.
    pub fn volume_profile_config(&self) -> &VolumeProfileConfig {
        &self.volume_profile_config
    }

    /// Read-only view of the show-VP flag.
    pub fn show_volume_profile(&self) -> bool {
        self.show_volume_profile
    }

    // ── Slice 4 chart-transition: level tool + effect drain ─────────

    /// Toggle the level-placement tool on. Toolbar "Add Level" button
    /// dispatches here.
    pub fn activate_level_tool(&mut self) {
        tracing::debug!(target: TRACE_TARGET, "activate level tool");
        self.level_host.activate();
    }

    /// Turn the level-placement tool off. Called on Escape / window
    /// close / tool swap.
    pub fn deactivate_level_tool(&mut self) {
        tracing::debug!(target: TRACE_TARGET, "deactivate level tool");
        self.level_host.deactivate();
    }

    /// True iff the level tool is currently active + in `Placing`.
    pub fn is_level_tool_active(&self) -> bool {
        self.level_host.is_active()
    }

    /// Update the cached level views (typically called from the host
    /// when `AnnotationStore::generation` bumps for the current
    /// symbol).
    pub fn set_level_views(&mut self, levels: Vec<LevelView>) {
        self.level_host.set_levels(levels);
    }

    /// Feed a tool-snap result in before dispatching a `MouseMove`
    /// that should update the preview. The caller runs
    /// [`midas_scene::tools::snap_to_ohlc`] with the visible candles.
    pub fn update_level_snap(&mut self, snapped_price: f64, cursor_y_px: f32) {
        if let Some(tool) = self.level_host.tool.as_mut() {
            tool.update_snap(snapped_price, cursor_y_px);
        }
    }

    /// Dispatch an [`InputEvent`] through a transient scene containing
    /// the active level tool + a `LevelLayer` populated from the
    /// cached level views. The scene's `take_effects` output is
    /// projected into [`ProjectedEffect`]s and queued on
    /// `pending_effects`.
    ///
    /// Construction cost: O(number of level views) per call. Acceptable
    /// because input events are rare compared to paint frames.
    pub fn handle_level_input(&mut self, ev: midas_scene::InputEvent) -> midas_scene::EventStatus {
        use midas_scene::layers::LevelLayer;
        use midas_scene::ChartScene;

        // Build a one-shot scene for input dispatch. We can't share the
        // paint-time scene because paint builds it inside `paint_buckets`
        // and throws it away; input can happen between paints.
        let mut builder = ChartScene::builder()
            .axis_boxed(self.axis.as_time_axis_boxed())
            .price_range(self.price_range)
            .viewport(self.viewport)
            .palette(self.palette);

        // Install the LevelLayer so its interactive handlers fire.
        let layer = LevelLayer::new(self.level_host.levels.clone())
            .with_interaction(Arc::clone(&self.level_host.drag));
        builder = builder.layer(layer);

        // Install the tool if active.
        if let Some(tool) = self.level_host.tool.as_ref() {
            builder = builder.active_tool(tool.clone());
        }

        let mut scene = builder
            .build()
            .expect("transient input scene with canonical inputs");
        // Prime the LevelLayer's viewport cache so hit-testing works.
        let mut scratch = midas_scene::primitives::ScenePrimitives::default();
        scene.paint(&mut scratch);

        let status = scene.handle_input(ev);

        // Drain scene effects → project → queue.
        for raw in scene.take_effects() {
            for projected in ProjectedEffect::from_tool_effect(raw) {
                self.pending_effects.push(projected);
            }
        }
        // Surface any recovered-panic / tool-layer error.
        if let Some(err) = scene.take_last_error() {
            self.pending_effects
                .push(ProjectedEffect::Error(err.to_string()));
        }

        status
    }

    /// Drain projected effects. The host translates each into an app
    /// `Message` per the slice 4 plan:
    ///
    /// - `CreateLevel` → `Message::CreateLevel(symbol, price, lock)`
    /// - `UpdateLevel` → `Message::UpdateLevel(symbol, id, price)`
    /// - `DeleteLevel` → `Message::DeleteLevel(symbol, id)`
    /// - `OpenContextMenu` → routed by `action`
    /// - `BracketDeferred` → slice 5b wires this up
    /// - `Error` → toast
    pub fn drain_level_effects(&mut self) -> Vec<ProjectedEffect> {
        std::mem::take(&mut self.pending_effects)
    }

    /// Observer — current tool (for the dev-harness `DumpState`
    /// projection).
    pub fn level_tool_mode(&self) -> Option<midas_scene::tools::LevelToolMode> {
        self.level_host.tool.as_ref().map(|t| t.mode())
    }

    /// Borrow the shared drag state so tests can inspect it.
    pub fn level_drag_state(&self) -> SharedLevelDrag {
        Arc::clone(&self.level_host.drag)
    }

    // ── Slice 5b chart-transition: bracket-tool plumbing ────────────

    /// Activate the bracket tool for a Buy (Long) bracket. Toolbar
    /// "Buy Bracket" button dispatches here.
    pub fn activate_buy_bracket_tool(&mut self) {
        tracing::debug!(
            target: TRACE_TARGET,
            "activate buy bracket tool",
        );
        self.bracket_host.activate_buy();
    }

    /// Activate the bracket tool for a Sell (Short) bracket.
    pub fn activate_sell_bracket_tool(&mut self) {
        tracing::debug!(
            target: TRACE_TARGET,
            "activate sell bracket tool",
        );
        self.bracket_host.activate_sell();
    }

    /// Deactivate the bracket tool. Called on Escape / window close /
    /// tool swap. If the tool is mid-placement, also emits a
    /// `CancelDraftBracket` projected effect so the host can translate
    /// to `TickerMsg::CancelBracket` — R11: zero orphan drafts.
    pub fn deactivate_bracket_tool(&mut self) {
        if let Some(tool) = self.bracket_host.tool.as_mut() {
            // Translate mid-placement cancel into a projected effect
            // on the drain queue so the host emits TickerMsg::CancelBracket.
            let mut effs = Vec::new();
            tool.cancel_with_effect(&mut effs);
            for raw in effs {
                for projected in ProjectedEffect::from_tool_effect(raw) {
                    self.pending_effects.push(projected);
                }
            }
        }
        self.bracket_host.deactivate();
    }

    /// True iff the bracket tool is currently active + placing.
    pub fn is_bracket_tool_active(&self) -> bool {
        self.bracket_host.is_active()
    }

    /// Observer — current bracket-tool mode (dev-harness `DumpState`).
    pub fn bracket_tool_mode(&self) -> Option<midas_scene::tools::BracketToolMode> {
        self.bracket_host.tool.as_ref().map(|t| t.mode())
    }

    /// Feed a preview price / cursor-y before dispatching a `MouseMove`
    /// that should update the bracket preview line.
    pub fn update_bracket_preview(&mut self, price: f64, cursor_y_px: f32) {
        if let Some(tool) = self.bracket_host.tool.as_mut() {
            tool.update_preview(price, cursor_y_px);
        }
    }

    /// Dispatch an [`InputEvent`] through the active bracket tool.
    /// Mirrors [`handle_level_input`](Self::handle_level_input);
    /// effects land on `pending_effects` via the same projection path.
    ///
    /// Unlike the level path — which constructs a transient scene so
    /// it can install a `LevelLayer` — the bracket tool is a pure FSM
    /// with no additional layers to run against; we dispatch the event
    /// straight into `BracketTool::update` and project the resulting
    /// effects.
    pub fn handle_bracket_input(
        &mut self,
        ev: midas_scene::InputEvent,
    ) -> midas_scene::EventStatus {
        let Some(tool) = self.bracket_host.tool.as_mut() else {
            return midas_scene::EventStatus::Ignored;
        };

        let mut effs: Vec<ToolEffect> = Vec::new();
        let mut last_err: Option<midas_scene::SceneError> = None;
        let status = {
            let mut cx = midas_scene::ToolContext {
                price_range: &self.price_range,
                last_error: &mut last_err,
                effects: &mut effs,
            };
            midas_scene::InteractiveLayer::update(tool, ev, &mut cx)
        };

        // Drain + project.
        for raw in effs {
            for projected in ProjectedEffect::from_tool_effect(raw) {
                self.pending_effects.push(projected);
            }
        }
        if let Some(err) = last_err {
            self.pending_effects
                .push(ProjectedEffect::Error(err.to_string()));
        }

        // If the tool moved to Complete, reset to AwaitingEntry with
        // the same side so the user can place another bracket in one
        // activation. FSM's `side()` returns `None` in `Complete`, so
        // consult `last_activated_side`.
        if tool.is_complete() {
            let side = self
                .bracket_host
                .last_activated_side
                .unwrap_or(SceneBracketSide::Long);
            if let Some(tool) = self.bracket_host.tool.as_mut() {
                tool.continue_placing_with(side);
            }
        }

        status
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use chrono::TimeZone;
    use midas_bars::{BarPeriod, Candle, Completeness, Ohlcv, Symbol};
    use midas_calendar::{crypto_spot, xnys};
    use midas_stream::{BarStream, BarStreamMeta, StreamError, TimeRange};
    use tokio::sync::mpsc;

    fn utc(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> Timestamp {
        chrono::Utc.with_ymd_and_hms(y, mo, d, h, mi, 0).unwrap()
    }

    fn mk_crypto(ts: Timestamp, price: f64) -> Candle {
        let cal = crypto_spot();
        let sym = Symbol::new("BTC-USD", cal.id());
        let session = cal.classify(ts);
        let window = cal.bar_window(ts, BarPeriod::m1()).unwrap();
        let ohlcv = Ohlcv::new(price, price + 10.0, price - 10.0, price + 5.0, 1, 1, None).unwrap();
        Candle::new(
            sym,
            cal,
            BarPeriod::m1(),
            session,
            window,
            ohlcv,
            Completeness::Completed,
        )
        .unwrap()
    }

    struct MockStream {
        rx: mpsc::Receiver<Candle>,
        meta: BarStreamMeta,
    }

    impl MockStream {
        fn crypto() -> (mpsc::Sender<Candle>, Self) {
            let cal = crypto_spot();
            let sym = Symbol::new("BTC-USD", cal.id());
            let (tx, rx) = mpsc::channel(32);
            let meta = BarStreamMeta::new(sym, cal, BarPeriod::m1());
            (tx, Self { rx, meta })
        }

        fn xnys() -> (mpsc::Sender<Candle>, Self) {
            let cal = xnys();
            let sym = Symbol::new("AAPL", cal.id());
            let (tx, rx) = mpsc::channel(32);
            let meta = BarStreamMeta::new(sym, cal, BarPeriod::m1());
            (tx, Self { rx, meta })
        }
    }

    #[async_trait]
    impl BarStream for MockStream {
        fn meta(&self) -> &BarStreamMeta {
            &self.meta
        }
        async fn next(&mut self) -> Option<Candle> {
            self.rx.recv().await
        }
        async fn snapshot(&mut self, _range: TimeRange) -> Result<Vec<Candle>, StreamError> {
            Err(StreamError::NotSeekable)
        }
    }

    fn fresh_crypto_series() -> Arc<RwLock<CandleSeries>> {
        let cal = crypto_spot();
        let sym = Symbol::new("BTC-USD", cal.id());
        Arc::new(RwLock::new(CandleSeries::new(
            cal.id(),
            BarPeriod::m1(),
            sym,
        )))
    }

    fn fresh_xnys_series() -> Arc<RwLock<CandleSeries>> {
        let cal = xnys();
        let sym = Symbol::new("AAPL", cal.id());
        Arc::new(RwLock::new(CandleSeries::new(
            cal.id(),
            BarPeriod::m1(),
            sym,
        )))
    }

    fn crypto_widget(driver: Arc<SessionChartDriver>) -> SessionChart {
        let cal = crypto_spot();
        let start = utc(2024, 3, 1, 0, 0);
        let end = utc(2024, 3, 2, 0, 0);
        let pr = PriceRange::new(49_900.0, 50_200.0).unwrap();
        let vp = Viewport::new(1000.0, 400.0);
        SessionChart::new(
            driver,
            cal,
            BarPeriod::m1(),
            pr,
            vp,
            ThemePalette::dark_default(),
            (start, end),
        )
        .expect("canonical crypto widget inputs must succeed")
    }

    fn xnys_widget(driver: Arc<SessionChartDriver>) -> SessionChart {
        let cal = xnys();
        // Two weekdays so CompressedAxis has sessions to work with.
        let start = utc(2024, 1, 17, 0, 0);
        let end = utc(2024, 1, 19, 0, 0);
        let pr = PriceRange::new(180.0, 200.0).unwrap();
        let vp = Viewport::new(1000.0, 400.0);
        SessionChart::new(
            driver,
            cal,
            BarPeriod::m1(),
            pr,
            vp,
            ThemePalette::dark_default(),
            (start, end),
        )
        .expect("canonical xnys widget inputs must succeed")
    }

    // ── Part A: axis kind picked from calendar ──────────────────────

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn crypto_widget_has_continuous_axis() {
        let (tx, stream) = MockStream::crypto();
        drop(tx);
        let driver = Arc::new(SessionChartDriver::spawn(fresh_crypto_series(), stream));
        let w = crypto_widget(driver);
        assert_eq!(w.axis_kind(), AxisKind::Continuous);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn xnys_widget_has_compressed_axis() {
        let (tx, stream) = MockStream::xnys();
        drop(tx);
        let driver = Arc::new(SessionChartDriver::spawn(fresh_xnys_series(), stream));
        let w = xnys_widget(driver);
        assert_eq!(w.axis_kind(), AxisKind::Compressed);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn axis_kind_for_calendar_matches_policy() {
        // Static helper — no widget needed.
        assert_eq!(AxisKind::for_calendar(crypto_spot()), AxisKind::Continuous);
        assert_eq!(AxisKind::for_calendar(xnys()), AxisKind::Compressed);
    }

    // ── Part D: EhPolicy cycling + layer derivation ─────────────────

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn default_eh_policy_is_show_all() {
        let (tx, stream) = MockStream::crypto();
        drop(tx);
        let driver = Arc::new(SessionChartDriver::spawn(fresh_crypto_series(), stream));
        let w = crypto_widget(driver);
        assert_eq!(w.eh_policy(), EhPolicy::ShowAll);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cycle_eh_policy_round_trips_after_three_calls() {
        let (tx, stream) = MockStream::crypto();
        drop(tx);
        let driver = Arc::new(SessionChartDriver::spawn(fresh_crypto_series(), stream));
        let mut w = crypto_widget(driver);
        assert_eq!(w.eh_policy(), EhPolicy::ShowAll);
        assert_eq!(w.cycle_eh_policy(), EhPolicy::HideExtended);
        assert_eq!(w.cycle_eh_policy(), EhPolicy::ShowBarsOnly);
        assert_eq!(w.cycle_eh_policy(), EhPolicy::ShowAll);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn xnys_show_all_layers_include_holidays_bands_and_separators() {
        let (tx, stream) = MockStream::xnys();
        drop(tx);
        let driver = Arc::new(SessionChartDriver::spawn(fresh_xnys_series(), stream));
        let w = xnys_widget(driver);
        let layers = w.layers_for_policy(true);
        assert!(layers.session_bands);
        assert!(layers.session_separators);
        assert!(layers.holidays);
        assert!(layers.candles);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn show_bars_only_drops_bands_and_separators() {
        let (tx, stream) = MockStream::xnys();
        drop(tx);
        let driver = Arc::new(SessionChartDriver::spawn(fresh_xnys_series(), stream));
        let mut w = xnys_widget(driver);
        w.set_eh_policy(EhPolicy::ShowBarsOnly);
        let layers = w.layers_for_policy(true);
        assert!(!layers.session_bands);
        assert!(!layers.session_separators);
        // Holidays survive — they are per-calendar not per-EhPolicy.
        assert!(layers.holidays);
        assert!(layers.candles);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn crypto_never_enables_holiday_layer() {
        let (tx, stream) = MockStream::crypto();
        drop(tx);
        let driver = Arc::new(SessionChartDriver::spawn(fresh_crypto_series(), stream));
        let mut w = crypto_widget(driver);
        for _ in 0..3 {
            let layers = w.layers_for_policy(false);
            assert!(!layers.holidays);
            w.cycle_eh_policy();
        }
    }

    // ── Part B: period picker reflection ────────────────────────────

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn set_period_updates_widget_field() {
        let (tx, stream) = MockStream::xnys();
        drop(tx);
        let driver = Arc::new(SessionChartDriver::spawn(fresh_xnys_series(), stream));
        let mut w = xnys_widget(driver);
        assert_eq!(w.period(), BarPeriod::m1());
        w.set_period(BarPeriod::d1_rth());
        assert_eq!(w.period(), BarPeriod::d1_rth());
        w.set_period(BarPeriod::w1());
        assert_eq!(w.period(), BarPeriod::w1());
    }

    // ── Paint smoke tests ───────────────────────────────────────────

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn crypto_paint_buckets_on_empty_series_yields_nonzero_grid() {
        let (tx, stream) = MockStream::crypto();
        drop(tx);
        let driver = Arc::new(SessionChartDriver::spawn(fresh_crypto_series(), stream));
        let mut widget = crypto_widget(driver);
        let b = widget.paint_buckets();
        assert_eq!(b.candles.len(), 0);
        assert!(!b.lines.is_empty());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn crypto_paint_buckets_after_candles_emits_candle_bucket() {
        let series = fresh_crypto_series();
        let (tx, stream) = MockStream::crypto();
        let driver = Arc::new(SessionChartDriver::spawn(Arc::clone(&series), stream));

        let start = utc(2024, 3, 1, 0, 0);
        for i in 0..5 {
            tx.send(mk_crypto(start + chrono::Duration::minutes(i), 50_000.0))
                .await
                .unwrap();
        }
        drop(tx);
        let mut rx = driver.version_receiver();
        while *rx.borrow_and_update() < 5 {
            if rx.changed().await.is_err() {
                break;
            }
        }

        let mut widget = crypto_widget(Arc::clone(&driver));
        let b = widget.paint_buckets();
        assert_eq!(b.candles.len(), 5);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn xnys_paint_buckets_on_empty_series_yields_nonzero_grid() {
        let (tx, stream) = MockStream::xnys();
        drop(tx);
        let driver = Arc::new(SessionChartDriver::spawn(fresh_xnys_series(), stream));
        let mut widget = xnys_widget(driver);
        let b = widget.paint_buckets();
        assert_eq!(b.candles.len(), 0);
        // Grid + bands should emit.
        assert!(!b.lines.is_empty());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn crosshair_toggle_round_trips_through_paint() {
        let (tx, stream) = MockStream::crypto();
        drop(tx);
        let driver = Arc::new(SessionChartDriver::spawn(fresh_crypto_series(), stream));
        let mut widget = crypto_widget(driver);

        let before = widget.paint_buckets();
        let before_lines = before.lines.len();

        widget.set_crosshair((400.0, 200.0));
        let after = widget.paint_buckets();
        assert!(after.lines.len() >= before_lines + 2);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn set_viewport_rebuilds_axis_at_new_width() {
        let (tx, stream) = MockStream::crypto();
        drop(tx);
        let driver = Arc::new(SessionChartDriver::spawn(fresh_crypto_series(), stream));
        let mut widget = crypto_widget(driver);
        widget.set_viewport(Viewport::new(1600.0, 900.0));
        assert!((widget.viewport().width_px - 1600.0).abs() < 1e-3);
        assert!((widget.axis.width_px() - 1600.0).abs() < 1e-3);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn xnys_set_viewport_preserves_compressed_axis_kind() {
        let (tx, stream) = MockStream::xnys();
        drop(tx);
        let driver = Arc::new(SessionChartDriver::spawn(fresh_xnys_series(), stream));
        let mut widget = xnys_widget(driver);
        assert_eq!(widget.axis_kind(), AxisKind::Compressed);
        widget.set_viewport(Viewport::new(2400.0, 1200.0));
        assert_eq!(widget.axis_kind(), AxisKind::Compressed);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn eh_policy_short_label_exposed() {
        assert_eq!(EhPolicy::ShowAll.short_label(), "EH");
        assert_eq!(EhPolicy::HideExtended.short_label(), "RTH");
        assert_eq!(EhPolicy::ShowBarsOnly.short_label(), "EH·bars");
    }

    /// Regression: app-harden M5. `SessionChart::new` now returns a
    /// `Result` on degenerate inputs (inverted time range, zero width)
    /// instead of panicking via `.expect`. Host code can close the
    /// half-opened window gracefully.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn new_returns_err_on_inverted_time_range() {
        let (tx, stream) = MockStream::crypto();
        drop(tx);
        let driver = Arc::new(SessionChartDriver::spawn(fresh_crypto_series(), stream));
        let cal = crypto_spot();
        let start = utc(2024, 3, 2, 0, 0);
        let end = utc(2024, 3, 1, 0, 0); // end < start → invalid.
        let pr = PriceRange::new(100.0, 200.0).unwrap();
        let vp = Viewport::new(1000.0, 400.0);
        let result = SessionChart::new(
            driver,
            cal,
            BarPeriod::m1(),
            pr,
            vp,
            ThemePalette::dark_default(),
            (start, end),
        );
        assert!(
            matches!(result, Err(SessionChartError::Axis(_))),
            "inverted time range must surface as Err, not panic"
        );
    }

    /// Regression: bug-hunt H5. The pre-R1 `shared_series_snapshot`
    /// deep-copied every row and accidentally hard-coded
    /// `trade_count=0, wap=None`, silently losing both values. The
    /// post-R1 widget shares the same `Arc<RwLock<CandleSeries>>` as
    /// the driver — no deep copy — so `trade_count` and `wap` arrive
    /// intact. This test pushes a candle with explicit values and
    /// reads them back from the shared series behind a read-guard.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn shared_series_preserves_trade_count_and_wap() {
        let series = fresh_crypto_series();
        let (tx, stream) = MockStream::crypto();
        let driver = Arc::new(SessionChartDriver::spawn(Arc::clone(&series), stream));

        let cal = crypto_spot();
        let sym = Symbol::new("BTC-USD", cal.id());
        let ts = utc(2024, 3, 1, 0, 0);
        let session = cal.classify(ts);
        let window = cal.bar_window(ts, BarPeriod::m1()).unwrap();
        let ohlcv = Ohlcv::new(
            50_000.0,
            50_100.0,
            49_900.0,
            50_050.0,
            12_345,
            77, // trade_count
            Some(50_025.5),
        )
        .unwrap();
        let candle = Candle::new(
            sym,
            cal,
            BarPeriod::m1(),
            session,
            window,
            ohlcv,
            Completeness::Completed,
        )
        .unwrap();
        tx.send(candle).await.unwrap();
        drop(tx);

        let mut rx = driver.version_receiver();
        while *rx.borrow_and_update() < 1 {
            if rx.changed().await.is_err() {
                break;
            }
        }

        let widget = crypto_widget(Arc::clone(&driver));
        let shared = widget.series();
        let guard = shared.read();
        assert_eq!(guard.len(), 1);
        let row = guard.at(0).unwrap();
        assert_eq!(row.trade_count(), 77);
        assert_eq!(row.wap(), Some(50_025.5));
    }

    /// Smoke benchmark for R1: painting a 10k-row series 100 times must
    /// complete in well under an eye-balling budget. The old
    /// `shared_series_snapshot` path deep-copied every row through the
    /// calendar on every paint — an easy O(n*paint_count) pathological
    /// case. The post-R1 widget shares the driver's
    /// `Arc<RwLock<CandleSeries>>` directly, so paint cost is O(n) per
    /// call, not O(n*k) for k-paints.
    ///
    /// The bound is intentionally loose (5 seconds for 100 paints over
    /// 10k rows) so CI flakiness doesn't derail green builds. The
    /// purpose is to catch regressions that reintroduce a per-frame
    /// deep copy — those push run times into the tens of seconds.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn paint_buckets_10k_rows_100_paints_no_quadratic_blowup() {
        let series = fresh_crypto_series();
        let (tx, stream) = MockStream::crypto();
        let driver = Arc::new(SessionChartDriver::spawn(Arc::clone(&series), stream));

        // Pre-populate the series with 10_000 rows by pushing directly —
        // bypasses the pump task so the driver isn't racing our paint
        // loop below.
        {
            let cal = crypto_spot();
            let sym = Symbol::new("BTC-USD", cal.id());
            let start = utc(2024, 3, 1, 0, 0);
            let mut guard = series.write();
            for i in 0..10_000i64 {
                let ts = start + chrono::Duration::minutes(i);
                let session = cal.classify(ts);
                let window = cal.bar_window(ts, BarPeriod::m1()).unwrap();
                let price = 50_000.0 + (i as f64) * 0.01;
                let ohlcv = Ohlcv::new(price, price + 0.5, price - 0.5, price + 0.25, 100, 1, None)
                    .unwrap();
                let candle = Candle::new(
                    sym,
                    cal,
                    BarPeriod::m1(),
                    session,
                    window,
                    ohlcv,
                    Completeness::Completed,
                )
                .unwrap();
                guard.push(candle);
            }
        }
        drop(tx);

        let mut widget = crypto_widget(Arc::clone(&driver));
        let started = std::time::Instant::now();
        for _ in 0..100 {
            let _ = widget.paint_buckets();
        }
        let elapsed = started.elapsed();
        assert!(
            elapsed < std::time::Duration::from_secs(5),
            "100 paints over 10k rows took {elapsed:?} — regression suspected (R1)"
        );
    }

    // ── Slice 2b: pan / zoom / keyboard / auto-scale ────────────────

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn pan_x_shifts_time_window_forward() {
        let (tx, stream) = MockStream::crypto();
        drop(tx);
        let driver = Arc::new(SessionChartDriver::spawn(fresh_crypto_series(), stream));
        let mut w = crypto_widget(driver);
        let (s0, e0) = w.time_window;
        w.pan_x(100.0);
        let (s1, e1) = w.time_window;
        assert!(s1 > s0);
        assert!(e1 > e0);
        assert_eq!(e1 - s1, e0 - s0, "span preserved");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn zoom_x_in_narrows_span_and_preserves_anchor() {
        let (tx, stream) = MockStream::crypto();
        drop(tx);
        let driver = Arc::new(SessionChartDriver::spawn(fresh_crypto_series(), stream));
        let mut w = crypto_widget(driver);
        let (s0, e0) = w.time_window;
        let span0 = e0 - s0;
        let anchor_px = w.viewport().width_px * 0.5;
        w.zoom_x_at(anchor_px, 0.5);
        let (s1, e1) = w.time_window;
        let span1 = e1 - s1;
        assert!(span1 < span0, "span narrowed on zoom-in");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn zoom_y_in_narrows_price_span() {
        let (tx, stream) = MockStream::crypto();
        drop(tx);
        let driver = Arc::new(SessionChartDriver::spawn(fresh_crypto_series(), stream));
        let mut w = crypto_widget(driver);
        let before = w.price_range();
        w.zoom_y_at(w.viewport().height_px * 0.5, 0.5);
        let after = w.price_range();
        assert!(after.span() < before.span());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn jump_home_positions_first_bar_at_left() {
        let series = fresh_crypto_series();
        let (tx, stream) = MockStream::crypto();
        let driver = Arc::new(SessionChartDriver::spawn(Arc::clone(&series), stream));
        // Push one candle so the series is non-empty.
        let ts = utc(2024, 3, 5, 0, 0);
        tx.send(mk_crypto(ts, 50_000.0)).await.unwrap();
        drop(tx);
        let mut rx = driver.version_receiver();
        while *rx.borrow_and_update() < 1 {
            if rx.changed().await.is_err() {
                break;
            }
        }
        let mut w = crypto_widget(Arc::clone(&driver));
        w.jump_home();
        assert_eq!(w.time_window.0, ts);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn jump_home_on_empty_series_is_noop() {
        let (tx, stream) = MockStream::crypto();
        drop(tx);
        let driver = Arc::new(SessionChartDriver::spawn(fresh_crypto_series(), stream));
        let mut w = crypto_widget(driver);
        let (s0, e0) = w.time_window;
        w.jump_home();
        assert_eq!(w.time_window.0, s0);
        assert_eq!(w.time_window.1, e0);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn jump_end_positions_last_bar_at_right() {
        let series = fresh_crypto_series();
        let (tx, stream) = MockStream::crypto();
        let driver = Arc::new(SessionChartDriver::spawn(Arc::clone(&series), stream));
        let ts = utc(2024, 3, 5, 0, 0);
        tx.send(mk_crypto(ts, 50_000.0)).await.unwrap();
        drop(tx);
        let mut rx = driver.version_receiver();
        while *rx.borrow_and_update() < 1 {
            if rx.changed().await.is_err() {
                break;
            }
        }
        let mut w = crypto_widget(Arc::clone(&driver));
        w.jump_end();
        assert_eq!(w.time_window.1, ts);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn auto_scale_on_first_data_sets_price_range() {
        let series = fresh_crypto_series();
        let (tx, stream) = MockStream::crypto();
        let driver = Arc::new(SessionChartDriver::spawn(Arc::clone(&series), stream));
        // Push one candle with known O/H/L/C.
        let ts = utc(2024, 3, 5, 0, 0);
        let cal = crypto_spot();
        let sym = Symbol::new("BTC-USD", cal.id());
        let session = cal.classify(ts);
        let window = cal.bar_window(ts, BarPeriod::m1()).unwrap();
        let ohlcv = Ohlcv::new(50_000.0, 50_100.0, 49_900.0, 50_050.0, 1, 1, None).unwrap();
        let candle = Candle::new(
            sym,
            cal,
            BarPeriod::m1(),
            session,
            window,
            ohlcv,
            Completeness::Completed,
        )
        .unwrap();
        tx.send(candle).await.unwrap();
        drop(tx);
        let mut rx = driver.version_receiver();
        while *rx.borrow_and_update() < 1 {
            if rx.changed().await.is_err() {
                break;
            }
        }
        let mut w = crypto_widget(Arc::clone(&driver));
        let fired = w.auto_scale_on_first_data();
        assert!(fired, "auto-scale must run on first-data transition");
        let r = w.price_range();
        // Fits high=50_100, low=49_900 with 5% pad.
        assert!(r.low() < 49_900.0);
        assert!(r.high() > 50_100.0);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn auto_scale_on_first_data_no_op_when_empty() {
        let (tx, stream) = MockStream::crypto();
        drop(tx);
        let driver = Arc::new(SessionChartDriver::spawn(fresh_crypto_series(), stream));
        let mut w = crypto_widget(driver);
        assert!(!w.auto_scale_on_first_data());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn zoom_x_min_floor_at_10_candles() {
        let (tx, stream) = MockStream::crypto();
        drop(tx);
        let driver = Arc::new(SessionChartDriver::spawn(fresh_crypto_series(), stream));
        let mut w = crypto_widget(driver);
        // Extreme zoom-in factor.
        w.zoom_x_at(500.0, 0.00001);
        let span = w.time_window.1 - w.time_window.0;
        // 10 × 1-minute min floor = 10 minutes.
        assert!(span >= chrono::Duration::minutes(10));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn zoom_y_no_op_on_nan_anchor() {
        let (tx, stream) = MockStream::crypto();
        drop(tx);
        let driver = Arc::new(SessionChartDriver::spawn(fresh_crypto_series(), stream));
        let mut w = crypto_widget(driver);
        let before = w.price_range();
        w.zoom_y_at(f32::NAN, 0.5);
        assert_eq!(before, w.price_range());
    }
}
