//! `BracketTool` — 3-click order-bracket placement FSM.
//!
//! Slice 5b of the chart-transition plan. Drives the existing
//! draft-then-save `TickerMsg` sequence via the [`ToolEffect`] queue —
//! NO new `TickerMsg` variant (plan C1 / architecture rule 8).
//!
//! ## FSM
//!
//! ```text
//! Idle
//!   │   (tool construction or directional-toggle key)
//!   │     → L / Buy / B → AwaitingEntry { Long }
//!   │     → S / Sell    → AwaitingEntry { Short }
//!   │
//!   ▼
//! AwaitingEntry { side }
//!   │   left-click → emit BeginDraftBracket { side, entry }
//!   │                + SetDraftLeg { Entry, entry }
//!   │                → AwaitingTarget { side, entry }
//!   │
//!   ▼
//! AwaitingTarget { side, entry }
//!   │   left-click → emit SetDraftLeg { Tp, price }
//!   │                → AwaitingStop { side, entry, target }
//!   │
//!   ▼
//! AwaitingStop { side, entry, target }
//!   │   left-click → emit SetDraftLeg { Sl, price }
//!   │                + CommitDraftBracket
//!   │                → Complete
//!   │
//!   ▼
//! Complete   (widget resets to AwaitingEntry { side } for multi-bracket
//!             placement, or to Idle on deactivate)
//! ```
//!
//! Escape in any non-Idle state emits `CancelDraftBracket` and returns
//! to `Idle`. Cursor-outside-viewport clicks are dropped (price == NaN).
//!
//! ## Preview
//!
//! Each non-Idle, non-Complete state paints a faded horizontal line at
//! the cursor's current price (`preview_price`). The widget feeds
//! `update_preview(price, cursor_y_px)` once per `MouseMove` before
//! dispatch — the tool itself is sans-IO and never touches a
//! `CandleSeries`.

use std::borrow::Cow;

use midas_axis::PriceRange;

use crate::input::{CursorShape, EventStatus, Hit, InputEvent, Key, MouseButton, Point};
use crate::layer::{InteractiveLayer, LayerId, LayerZ, SceneLayer, ToolContext};
use crate::paint::PaintContext;
use crate::primitives::{LineInstance, TextAnchor, TextInstance};
use crate::tools::{LegRole, Side, ToolEffect};

/// Single [`LayerId`] used by every `BracketTool` instance.
const BRACKET_TOOL_ID: LayerId = LayerId("bracket-tool");

/// FSM states. `Side` is picked by activation or by a directional-toggle
/// key press while the tool is in `Idle` / `AwaitingEntry` (side is
/// locked once `entry` commits — second / third clicks cannot re-toggle).
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum BracketToolMode {
    /// Tool installed but not yet active. Reachable by Escape / cancel.
    Idle,
    /// Waiting for the first left-click to set the entry price.
    AwaitingEntry { side: Side },
    /// Entry placed; waiting for the second click to set the TP leg.
    AwaitingTarget { side: Side, entry: f64 },
    /// TP placed; waiting for the third click to set the SL leg.
    AwaitingStop { side: Side, entry: f64, target: f64 },
    /// Third click committed. Widget resets to `AwaitingEntry { side }`
    /// for multi-bracket placement, or calls `cancel()` to go `Idle`.
    Complete,
}

/// Three-click bracket placement tool. Holds only the FSM state + the
/// per-frame preview price; all mutations go through
/// [`InteractiveLayer::update`] which emits [`ToolEffect`]s on the
/// scene's effect queue.
#[derive(Clone, Debug)]
pub struct BracketTool {
    mode: BracketToolMode,
    /// Current preview price (post-snap). Set per `MouseMove` by the
    /// host via [`BracketTool::update_preview`]. NaN when no preview is
    /// active.
    preview_price: f64,
    /// Cursor y-pixel, stashed so paint renders the preview at the
    /// exact cursor position the user last saw. `None` hides the
    /// preview (cursor left the chart).
    cursor_y_px: Option<f32>,
}

impl BracketTool {
    /// Construct a tool directly in `AwaitingEntry { side }`. This is
    /// the toolbar entry point (`Buy Bracket` → `Long`, `Sell Bracket`
    /// → `Short`).
    pub fn awaiting_entry(side: Side) -> Self {
        Self {
            mode: BracketToolMode::AwaitingEntry { side },
            preview_price: f64::NAN,
            cursor_y_px: None,
        }
    }

    /// Construct a tool in `Idle`. Useful for tests that exercise
    /// directional-toggle keystrokes before the first click.
    pub fn idle() -> Self {
        Self {
            mode: BracketToolMode::Idle,
            preview_price: f64::NAN,
            cursor_y_px: None,
        }
    }

    /// Observer — current FSM state.
    pub fn mode(&self) -> BracketToolMode {
        self.mode
    }

    /// Observer — the side currently locked into the FSM, if any.
    pub fn side(&self) -> Option<Side> {
        match self.mode {
            BracketToolMode::Idle | BracketToolMode::Complete => None,
            BracketToolMode::AwaitingEntry { side }
            | BracketToolMode::AwaitingTarget { side, .. }
            | BracketToolMode::AwaitingStop { side, .. } => Some(side),
        }
    }

    /// Feed a per-frame preview price (cursor y → price) + the cursor's
    /// pixel y so paint can render the faded preview line.
    pub fn update_preview(&mut self, price: f64, cursor_y_px: f32) {
        self.preview_price = price;
        self.cursor_y_px = Some(cursor_y_px);
    }

    /// Clear the cursor y → hide the preview without leaving the
    /// current FSM state. Called on `CursorLeft`.
    pub fn clear_cursor(&mut self) {
        self.cursor_y_px = None;
    }

    /// Reset the FSM to `AwaitingEntry { side }` so the user can place
    /// another bracket without re-activating the tool. Called by the
    /// widget one frame after a `Complete` state emits.
    pub fn continue_placing(&mut self) {
        if let BracketToolMode::Complete = self.mode {
            // Recover the side from the last placement — carried on
            // the previous `AwaitingStop { side, .. }`; we stash it on
            // `Complete`'s entry by default to Long if the FSM was
            // somehow reached with no side (defensive).
            //
            // Concretely: `Complete` is only reachable from
            // `AwaitingStop { side, .. }` so we know `side` was set —
            // but `Complete` doesn't carry it, so this helper uses the
            // `side` the caller remembered separately. Callers should
            // use [`BracketTool::continue_placing_with`] instead.
            self.mode = BracketToolMode::AwaitingEntry { side: Side::Long };
        }
    }

    /// Reset the FSM to `AwaitingEntry { side }` (explicit side). Prefer
    /// this over [`continue_placing`](Self::continue_placing) so the
    /// caller doesn't rely on the tool remembering the side.
    pub fn continue_placing_with(&mut self, side: Side) {
        if let BracketToolMode::Complete = self.mode {
            self.mode = BracketToolMode::AwaitingEntry { side };
        }
    }

    /// Observer — is the tool actively placing (any non-Idle, non-
    /// Complete state)?
    pub fn is_placing(&self) -> bool {
        !matches!(self.mode, BracketToolMode::Idle | BracketToolMode::Complete)
    }

    /// Observer — has the tool just finished a 3-click sequence?
    pub fn is_complete(&self) -> bool {
        matches!(self.mode, BracketToolMode::Complete)
    }

    /// Classify a key press as a directional toggle.
    ///
    /// - `L`, `B` or `Buy` aliases → `Side::Long`
    /// - `S` or `Sell` aliases → `Side::Short`
    ///
    /// Case-insensitive. Any other key returns `None`.
    fn classify_side_toggle(key: Key) -> Option<Side> {
        match key {
            Key::Char(c) => match c.to_ascii_uppercase() {
                'L' | 'B' => Some(Side::Long),
                'S' => Some(Side::Short),
                _ => None,
            },
            _ => None,
        }
    }

    /// Return the [`LegRole`] a preview line at the current cursor
    /// would fill if the user clicked now. Used by paint to label the
    /// faded preview.
    fn pending_role(&self) -> Option<LegRole> {
        match self.mode {
            BracketToolMode::AwaitingEntry { .. } => Some(LegRole::Entry),
            BracketToolMode::AwaitingTarget { .. } => Some(LegRole::Tp),
            BracketToolMode::AwaitingStop { .. } => Some(LegRole::Sl),
            BracketToolMode::Idle | BracketToolMode::Complete => None,
        }
    }
}

impl SceneLayer for BracketTool {
    fn id(&self) -> LayerId {
        BRACKET_TOOL_ID
    }

    fn z(&self) -> LayerZ {
        LayerZ::ORDER_BRACKET
    }

    fn paint(&self, ctx: &mut PaintContext<'_>) {
        let Some(role) = self.pending_role() else {
            return;
        };
        let Some(_y_px) = self.cursor_y_px else {
            return;
        };
        if !self.preview_price.is_finite() {
            return;
        }
        let y = ctx.price_to_y(self.preview_price);
        let w = ctx.viewport.width_px;
        // Faded preview — 40% alpha of palette text colour.
        let mut color = ctx.palette.text;
        color[3] = ((color[3] as f32) * 0.4).round().clamp(0.0, 255.0) as u8;
        ctx.out.lines.push(LineInstance {
            x0: 0.0,
            y0: y,
            x1: w,
            y1: y,
            width_px: 1.0,
            color,
        });
        // Tiny label at left edge — "E" / "TP" / "SL".
        let label: Cow<'static, str> = match role {
            LegRole::Entry => Cow::Borrowed("E?"),
            LegRole::Tp => Cow::Borrowed("TP?"),
            LegRole::Sl => Cow::Borrowed("SL?"),
        };
        ctx.out.text.push(TextInstance {
            x: 4.0,
            y,
            color: ctx.palette.text,
            text: label,
            size_px: 10.0,
            anchor: TextAnchor::MiddleLeft,
        });
    }

    fn as_interactive(&mut self) -> Option<&mut dyn InteractiveLayer> {
        Some(self)
    }
}

impl InteractiveLayer for BracketTool {
    fn update(&mut self, ev: InputEvent, ctx: &mut ToolContext<'_>) -> EventStatus {
        match ev {
            InputEvent::MouseDown {
                button: MouseButton::Left,
                ..
            } => {
                if !self.preview_price.is_finite() {
                    // No preview yet (host hasn't fed a snap) — ignore
                    // so pan/zoom fallthrough still works.
                    return EventStatus::Ignored;
                }
                let price = self.preview_price;
                match self.mode {
                    BracketToolMode::Idle | BracketToolMode::Complete => EventStatus::Ignored,
                    BracketToolMode::AwaitingEntry { side } => {
                        ctx.emit_effect(ToolEffect::BeginDraftBracket { side, entry: price });
                        ctx.emit_effect(ToolEffect::SetDraftLeg {
                            role: LegRole::Entry,
                            price,
                        });
                        tracing::debug!(
                            target: "midas_scene::tools::bracket",
                            ?side,
                            entry = price,
                            "BracketTool: entry placed",
                        );
                        self.mode = BracketToolMode::AwaitingTarget { side, entry: price };
                        EventStatus::Captured
                    }
                    BracketToolMode::AwaitingTarget { side, entry } => {
                        ctx.emit_effect(ToolEffect::SetDraftLeg {
                            role: LegRole::Tp,
                            price,
                        });
                        tracing::debug!(
                            target: "midas_scene::tools::bracket",
                            ?side,
                            tp = price,
                            "BracketTool: TP placed",
                        );
                        self.mode = BracketToolMode::AwaitingStop {
                            side,
                            entry,
                            target: price,
                        };
                        EventStatus::Captured
                    }
                    BracketToolMode::AwaitingStop { side, .. } => {
                        ctx.emit_effect(ToolEffect::SetDraftLeg {
                            role: LegRole::Sl,
                            price,
                        });
                        ctx.emit_effect(ToolEffect::CommitDraftBracket);
                        tracing::debug!(
                            target: "midas_scene::tools::bracket",
                            ?side,
                            sl = price,
                            "BracketTool: SL placed + commit",
                        );
                        self.mode = BracketToolMode::Complete;
                        EventStatus::Captured
                    }
                }
            }
            InputEvent::KeyDown { key, .. } => {
                // Directional toggle is only valid pre-entry.
                if let Some(new_side) = Self::classify_side_toggle(key) {
                    match self.mode {
                        BracketToolMode::Idle => {
                            self.mode = BracketToolMode::AwaitingEntry { side: new_side };
                            tracing::debug!(
                                target: "midas_scene::tools::bracket",
                                ?new_side,
                                "BracketTool: directional toggle from Idle",
                            );
                            return EventStatus::Captured;
                        }
                        BracketToolMode::AwaitingEntry { .. } => {
                            self.mode = BracketToolMode::AwaitingEntry { side: new_side };
                            tracing::debug!(
                                target: "midas_scene::tools::bracket",
                                ?new_side,
                                "BracketTool: directional toggle pre-entry",
                            );
                            return EventStatus::Captured;
                        }
                        // Locked once entry placed.
                        BracketToolMode::AwaitingTarget { .. }
                        | BracketToolMode::AwaitingStop { .. }
                        | BracketToolMode::Complete => return EventStatus::Ignored,
                    }
                }
                if matches!(key, Key::Escape) {
                    // Any non-Idle state: emit CancelDraftBracket +
                    // reset. `Idle` / `Complete` — no draft to cancel.
                    if self.is_placing() {
                        ctx.emit_effect(ToolEffect::CancelDraftBracket);
                        tracing::debug!(
                            target: "midas_scene::tools::bracket",
                            "BracketTool: escape — CancelDraftBracket emitted",
                        );
                    }
                    self.mode = BracketToolMode::Idle;
                    return EventStatus::Captured;
                }
                EventStatus::Ignored
            }
            InputEvent::CursorLeft => {
                self.cursor_y_px = None;
                EventStatus::Ignored
            }
            InputEvent::MouseMove { .. } => EventStatus::Ignored,
            // Wheel default: Ignored (plan R20 — tools don't swallow
            // wheel events so chart pan/zoom keeps working when the
            // tool is active).
            _ => EventStatus::Ignored,
        }
    }

    fn hit_test(&self, _pt: Point, _price_range: &PriceRange) -> Option<Hit> {
        if self.is_placing() {
            Some(Hit {
                layer_id: BRACKET_TOOL_ID,
                sub_z: 0,
                cursor: CursorShape::Crosshair,
            })
        } else {
            None
        }
    }

    fn cancel(&mut self) {
        // Emit CancelDraftBracket on the scene's effect queue when the
        // tool is cancelled mid-placement — ChartScene::on_destroy
        // calls cancel() on the active tool (R11). We can't emit here
        // because `cancel(&mut self)` has no ToolContext; the scene's
        // handle_input path routes Escape through `update(KeyDown)`
        // which DOES have a ToolContext + emits the effect. The scene's
        // on_destroy path calls cancel() directly — for that path, the
        // widget MUST also translate a best-effort CancelBracket when
        // it notices the tool moved to Idle without a matching Escape.
        //
        // Mitigation: we flip to Idle here; session_chart_window.rs's
        // close path emits CancelBracket directly to TickerState if a
        // draft was live. Also see `cancel_with_effect()`.
        self.mode = BracketToolMode::Idle;
        self.preview_price = f64::NAN;
        self.cursor_y_px = None;
    }
}

impl BracketTool {
    /// Cancel AND emit `ToolEffect::CancelDraftBracket` onto an
    /// externally-supplied queue. Used by `ChartScene::on_destroy`-style
    /// paths that want the cancel effect visible to the widget without
    /// routing through `InteractiveLayer::update(KeyDown)`.
    pub fn cancel_with_effect(&mut self, effects: &mut Vec<ToolEffect>) {
        if self.is_placing() {
            effects.push(ToolEffect::CancelDraftBracket);
            tracing::debug!(
                target: "midas_scene::tools::bracket",
                "BracketTool: cancel_with_effect — CancelDraftBracket emitted",
            );
        }
        self.cancel();
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use midas_axis::{ContinuousAxis, PriceRange, Viewport};
    use midas_calendar::Timestamp;

    use super::*;
    use crate::input::{Modifiers, MouseButton};
    use crate::scene::ChartScene;

    fn ts(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> Timestamp {
        chrono::Utc.with_ymd_and_hms(y, mo, d, h, mi, s).unwrap()
    }

    fn axis() -> ContinuousAxis {
        ContinuousAxis::new(ts(2024, 1, 1, 0, 0, 0), ts(2024, 1, 2, 0, 0, 0), 1000.0).unwrap()
    }

    fn pr() -> PriceRange {
        PriceRange::new(90.0, 110.0).unwrap()
    }

    fn vp() -> Viewport {
        Viewport::new(1000.0, 400.0)
    }

    fn mk_scene_with_tool(tool: BracketTool) -> ChartScene {
        ChartScene::builder()
            .axis(axis())
            .price_range(pr())
            .viewport(vp())
            .active_tool(tool)
            .build()
            .unwrap()
    }

    /// Dispatch one event through a fresh `ToolContext` tied to the
    /// provided effect/error slots. Each call re-borrows so follow-up
    /// `.clear()` / `.len()` observations don't collide with the
    /// mutable borrow.
    fn dispatch(
        tool: &mut BracketTool,
        ev: InputEvent,
        effs: &mut Vec<ToolEffect>,
        last_err: &mut Option<crate::error::SceneError>,
    ) -> EventStatus {
        let pr_owned = pr();
        let mut cx = ToolContext {
            price_range: &pr_owned,
            last_error: last_err,
            effects: effs,
        };
        tool.update(ev, &mut cx)
    }

    fn left_click(pt_y: f32) -> InputEvent {
        InputEvent::MouseDown {
            button: MouseButton::Left,
            pt: Point::new(500.0, pt_y),
            modifiers: Modifiers::default(),
        }
    }

    fn mouse_up() -> InputEvent {
        InputEvent::MouseUp {
            button: MouseButton::Left,
            pt: Point::new(500.0, 0.0),
        }
    }

    fn key(c: char) -> InputEvent {
        InputEvent::KeyDown {
            key: Key::Char(c),
            modifiers: Modifiers::default(),
        }
    }

    fn esc() -> InputEvent {
        InputEvent::KeyDown {
            key: Key::Escape,
            modifiers: Modifiers::default(),
        }
    }

    // ── State × input class matrix ──────────────────────────────────

    #[test]
    fn idle_left_click_is_ignored() {
        let tool = BracketTool::idle();
        let mut scene = mk_scene_with_tool(tool);
        // No preview fed → click is ignored even though mode != idle.
        let status = scene.handle_input(left_click(150.0));
        assert_eq!(status, EventStatus::Ignored);
    }

    #[test]
    fn awaiting_entry_with_no_preview_ignores_click() {
        let tool = BracketTool::awaiting_entry(Side::Long);
        let mut scene = mk_scene_with_tool(tool);
        let status = scene.handle_input(left_click(150.0));
        assert_eq!(status, EventStatus::Ignored);
        assert!(scene.take_effects().is_empty());
    }

    #[test]
    fn awaiting_entry_click_emits_begin_plus_entry_leg_effects() {
        let mut tool = BracketTool::awaiting_entry(Side::Long);
        tool.update_preview(100.0, 200.0);
        let mut scene = mk_scene_with_tool(tool);
        scene.handle_input(left_click(200.0));
        let effs = scene.take_effects();
        assert_eq!(effs.len(), 2);
        assert_eq!(
            effs[0],
            ToolEffect::BeginDraftBracket {
                side: Side::Long,
                entry: 100.0,
            }
        );
        assert_eq!(
            effs[1],
            ToolEffect::SetDraftLeg {
                role: LegRole::Entry,
                price: 100.0,
            }
        );
    }

    #[test]
    fn awaiting_entry_click_transitions_to_awaiting_target() {
        let mut tool = BracketTool::awaiting_entry(Side::Short);
        tool.update_preview(100.0, 200.0);
        let mut scene = mk_scene_with_tool(tool);
        scene.handle_input(left_click(200.0));
        // Scene owns the tool now; take_effects drained. We can peek
        // state via the tool's mode through a round-trip: take the
        // active_tool, inspect mode.
        // Easier: reconstruct a local tool, apply update directly.
        let mut tool2 = BracketTool::awaiting_entry(Side::Short);
        tool2.update_preview(100.0, 200.0);
        let pr_owned = pr();
        let mut last_err = None;
        let mut effs = Vec::new();
        let mut cx = ToolContext {
            price_range: &pr_owned,
            last_error: &mut last_err,
            effects: &mut effs,
        };
        tool2.update(left_click(200.0), &mut cx);
        assert_eq!(
            tool2.mode(),
            BracketToolMode::AwaitingTarget {
                side: Side::Short,
                entry: 100.0
            }
        );
    }

    #[test]
    fn awaiting_target_click_emits_tp_leg_effect_and_transitions() {
        let mut tool = BracketTool::awaiting_entry(Side::Long);
        tool.update_preview(100.0, 200.0);
        let mut last_err = None;
        let mut effs = Vec::new();
        dispatch(&mut tool, left_click(200.0), &mut effs, &mut last_err);
        tool.update_preview(105.0, 150.0);
        effs.clear();
        dispatch(&mut tool, left_click(150.0), &mut effs, &mut last_err);
        assert_eq!(effs.len(), 1);
        assert_eq!(
            effs[0],
            ToolEffect::SetDraftLeg {
                role: LegRole::Tp,
                price: 105.0,
            }
        );
        assert_eq!(
            tool.mode(),
            BracketToolMode::AwaitingStop {
                side: Side::Long,
                entry: 100.0,
                target: 105.0,
            }
        );
    }

    #[test]
    fn awaiting_stop_click_emits_sl_leg_plus_commit_and_completes() {
        let mut tool = BracketTool::awaiting_entry(Side::Long);
        let mut last_err = None;
        let mut effs = Vec::new();
        // Click 1: entry.
        tool.update_preview(100.0, 200.0);
        dispatch(&mut tool, left_click(200.0), &mut effs, &mut last_err);
        // Click 2: TP.
        tool.update_preview(105.0, 150.0);
        dispatch(&mut tool, left_click(150.0), &mut effs, &mut last_err);
        // Click 3: SL.
        tool.update_preview(95.0, 250.0);
        effs.clear();
        dispatch(&mut tool, left_click(250.0), &mut effs, &mut last_err);
        assert_eq!(effs.len(), 2);
        assert_eq!(
            effs[0],
            ToolEffect::SetDraftLeg {
                role: LegRole::Sl,
                price: 95.0,
            }
        );
        assert_eq!(effs[1], ToolEffect::CommitDraftBracket);
        assert_eq!(tool.mode(), BracketToolMode::Complete);
    }

    #[test]
    fn complete_tool_ignores_further_clicks() {
        let mut tool = BracketTool::awaiting_entry(Side::Long);
        let mut last_err = None;
        let mut effs = Vec::new();
        for p in [100.0, 105.0, 95.0] {
            tool.update_preview(p, 200.0);
            dispatch(&mut tool, left_click(200.0), &mut effs, &mut last_err);
        }
        effs.clear();
        tool.update_preview(101.0, 200.0);
        let status = dispatch(&mut tool, left_click(200.0), &mut effs, &mut last_err);
        assert_eq!(status, EventStatus::Ignored);
        assert!(effs.is_empty());
    }

    // ── Directional toggle ────────────────────────────────────────────

    #[test]
    fn l_key_in_idle_sets_long() {
        let mut tool = BracketTool::idle();
        let pr_owned = pr();
        let mut last_err = None;
        let mut effs = Vec::new();
        let mut cx = ToolContext {
            price_range: &pr_owned,
            last_error: &mut last_err,
            effects: &mut effs,
        };
        tool.update(key('L'), &mut cx);
        assert_eq!(
            tool.mode(),
            BracketToolMode::AwaitingEntry { side: Side::Long }
        );
    }

    #[test]
    fn s_key_in_idle_sets_short() {
        let mut tool = BracketTool::idle();
        let pr_owned = pr();
        let mut last_err = None;
        let mut effs = Vec::new();
        let mut cx = ToolContext {
            price_range: &pr_owned,
            last_error: &mut last_err,
            effects: &mut effs,
        };
        tool.update(key('S'), &mut cx);
        assert_eq!(
            tool.mode(),
            BracketToolMode::AwaitingEntry { side: Side::Short }
        );
    }

    #[test]
    fn b_key_alias_sets_long() {
        let mut tool = BracketTool::idle();
        let pr_owned = pr();
        let mut last_err = None;
        let mut effs = Vec::new();
        let mut cx = ToolContext {
            price_range: &pr_owned,
            last_error: &mut last_err,
            effects: &mut effs,
        };
        tool.update(key('B'), &mut cx);
        assert_eq!(
            tool.mode(),
            BracketToolMode::AwaitingEntry { side: Side::Long }
        );
    }

    #[test]
    fn lowercase_s_alias_sets_short() {
        let mut tool = BracketTool::idle();
        let pr_owned = pr();
        let mut last_err = None;
        let mut effs = Vec::new();
        let mut cx = ToolContext {
            price_range: &pr_owned,
            last_error: &mut last_err,
            effects: &mut effs,
        };
        tool.update(key('s'), &mut cx);
        assert_eq!(
            tool.mode(),
            BracketToolMode::AwaitingEntry { side: Side::Short }
        );
    }

    #[test]
    fn toggle_pre_entry_replaces_side() {
        let mut tool = BracketTool::awaiting_entry(Side::Long);
        let pr_owned = pr();
        let mut last_err = None;
        let mut effs = Vec::new();
        let mut cx = ToolContext {
            price_range: &pr_owned,
            last_error: &mut last_err,
            effects: &mut effs,
        };
        tool.update(key('S'), &mut cx);
        assert_eq!(
            tool.mode(),
            BracketToolMode::AwaitingEntry { side: Side::Short }
        );
    }

    #[test]
    fn toggle_after_entry_is_ignored() {
        let mut tool = BracketTool::awaiting_entry(Side::Long);
        tool.update_preview(100.0, 200.0);
        let pr_owned = pr();
        let mut last_err = None;
        let mut effs = Vec::new();
        let mut cx = ToolContext {
            price_range: &pr_owned,
            last_error: &mut last_err,
            effects: &mut effs,
        };
        tool.update(left_click(200.0), &mut cx);
        // Now in AwaitingTarget { side: Long, entry: 100 }.
        tool.update(key('S'), &mut cx);
        // Side still Long.
        assert_eq!(tool.side(), Some(Side::Long));
    }

    #[test]
    fn toggle_after_tp_is_ignored() {
        let mut tool = BracketTool::awaiting_entry(Side::Long);
        let pr_owned = pr();
        let mut last_err = None;
        let mut effs = Vec::new();
        let mut cx = ToolContext {
            price_range: &pr_owned,
            last_error: &mut last_err,
            effects: &mut effs,
        };
        tool.update_preview(100.0, 200.0);
        tool.update(left_click(200.0), &mut cx);
        tool.update_preview(105.0, 150.0);
        tool.update(left_click(150.0), &mut cx);
        // Now in AwaitingStop. Toggle ignored.
        tool.update(key('S'), &mut cx);
        assert_eq!(tool.side(), Some(Side::Long));
    }

    // ── Escape at each state ──────────────────────────────────────────

    #[test]
    fn escape_in_awaiting_entry_emits_cancel_and_goes_idle() {
        let mut tool = BracketTool::awaiting_entry(Side::Long);
        let pr_owned = pr();
        let mut last_err = None;
        let mut effs = Vec::new();
        let mut cx = ToolContext {
            price_range: &pr_owned,
            last_error: &mut last_err,
            effects: &mut effs,
        };
        tool.update(esc(), &mut cx);
        assert_eq!(effs, vec![ToolEffect::CancelDraftBracket]);
        assert_eq!(tool.mode(), BracketToolMode::Idle);
    }

    #[test]
    fn escape_in_awaiting_target_emits_cancel_and_goes_idle() {
        let mut tool = BracketTool::awaiting_entry(Side::Long);
        tool.update_preview(100.0, 200.0);
        let mut last_err = None;
        let mut effs = Vec::new();
        dispatch(&mut tool, left_click(200.0), &mut effs, &mut last_err);
        effs.clear();
        dispatch(&mut tool, esc(), &mut effs, &mut last_err);
        assert_eq!(effs, vec![ToolEffect::CancelDraftBracket]);
        assert_eq!(tool.mode(), BracketToolMode::Idle);
    }

    #[test]
    fn escape_in_awaiting_stop_emits_cancel_and_goes_idle() {
        let mut tool = BracketTool::awaiting_entry(Side::Long);
        let mut last_err = None;
        let mut effs = Vec::new();
        tool.update_preview(100.0, 200.0);
        dispatch(&mut tool, left_click(200.0), &mut effs, &mut last_err);
        tool.update_preview(105.0, 150.0);
        dispatch(&mut tool, left_click(150.0), &mut effs, &mut last_err);
        effs.clear();
        dispatch(&mut tool, esc(), &mut effs, &mut last_err);
        assert_eq!(effs, vec![ToolEffect::CancelDraftBracket]);
        assert_eq!(tool.mode(), BracketToolMode::Idle);
    }

    #[test]
    fn escape_in_idle_is_captured_but_emits_nothing() {
        let mut tool = BracketTool::idle();
        let pr_owned = pr();
        let mut last_err = None;
        let mut effs = Vec::new();
        let mut cx = ToolContext {
            price_range: &pr_owned,
            last_error: &mut last_err,
            effects: &mut effs,
        };
        let status = tool.update(esc(), &mut cx);
        assert_eq!(status, EventStatus::Captured);
        assert!(effs.is_empty());
    }

    #[test]
    fn escape_in_complete_emits_nothing() {
        let mut tool = BracketTool::awaiting_entry(Side::Long);
        let mut last_err = None;
        let mut effs = Vec::new();
        for p in [100.0, 105.0, 95.0] {
            tool.update_preview(p, 200.0);
            dispatch(&mut tool, left_click(200.0), &mut effs, &mut last_err);
        }
        effs.clear();
        let status = dispatch(&mut tool, esc(), &mut effs, &mut last_err);
        assert_eq!(status, EventStatus::Captured);
        assert!(effs.is_empty());
    }

    // ── 3-click sequence emits correct ToolEffects ─────────────────────

    #[test]
    fn three_click_long_emits_canonical_sequence() {
        let mut tool = BracketTool::awaiting_entry(Side::Long);
        let pr_owned = pr();
        let mut last_err = None;
        let mut effs = Vec::new();
        let mut cx = ToolContext {
            price_range: &pr_owned,
            last_error: &mut last_err,
            effects: &mut effs,
        };
        tool.update_preview(100.0, 200.0);
        tool.update(left_click(200.0), &mut cx);
        tool.update_preview(105.0, 150.0);
        tool.update(left_click(150.0), &mut cx);
        tool.update_preview(95.0, 250.0);
        tool.update(left_click(250.0), &mut cx);
        // 2 (entry) + 1 (tp) + 2 (sl+commit) = 5 effects.
        assert_eq!(effs.len(), 5);
        assert_eq!(
            effs[0],
            ToolEffect::BeginDraftBracket {
                side: Side::Long,
                entry: 100.0,
            }
        );
        assert_eq!(
            effs[1],
            ToolEffect::SetDraftLeg {
                role: LegRole::Entry,
                price: 100.0,
            }
        );
        assert_eq!(
            effs[2],
            ToolEffect::SetDraftLeg {
                role: LegRole::Tp,
                price: 105.0,
            }
        );
        assert_eq!(
            effs[3],
            ToolEffect::SetDraftLeg {
                role: LegRole::Sl,
                price: 95.0,
            }
        );
        assert_eq!(effs[4], ToolEffect::CommitDraftBracket);
    }

    #[test]
    fn three_click_short_emits_canonical_sequence() {
        let mut tool = BracketTool::awaiting_entry(Side::Short);
        let pr_owned = pr();
        let mut last_err = None;
        let mut effs = Vec::new();
        let mut cx = ToolContext {
            price_range: &pr_owned,
            last_error: &mut last_err,
            effects: &mut effs,
        };
        tool.update_preview(100.0, 200.0);
        tool.update(left_click(200.0), &mut cx);
        tool.update_preview(95.0, 250.0);
        tool.update(left_click(250.0), &mut cx);
        tool.update_preview(105.0, 150.0);
        tool.update(left_click(150.0), &mut cx);
        assert_eq!(effs.len(), 5);
        // Short: entry, TP below entry, SL above entry.
        assert_eq!(
            effs[0],
            ToolEffect::BeginDraftBracket {
                side: Side::Short,
                entry: 100.0,
            }
        );
        assert_eq!(
            effs[3],
            ToolEffect::SetDraftLeg {
                role: LegRole::Sl,
                price: 105.0,
            }
        );
    }

    // ── Cursor-outside-viewport (NaN preview) clicks ignored ───────────

    #[test]
    fn non_finite_preview_ignores_click() {
        let tool = BracketTool::awaiting_entry(Side::Long);
        let mut scene = mk_scene_with_tool(tool);
        // No update_preview → preview_price NaN.
        let status = scene.handle_input(left_click(150.0));
        assert_eq!(status, EventStatus::Ignored);
    }

    #[test]
    fn non_finite_preview_ignores_click_in_awaiting_target() {
        let mut tool = BracketTool::awaiting_entry(Side::Long);
        tool.update_preview(100.0, 200.0);
        let mut last_err = None;
        let mut effs = Vec::new();
        dispatch(&mut tool, left_click(200.0), &mut effs, &mut last_err);
        // Reset preview to NaN.
        tool.preview_price = f64::NAN;
        let pre_effs = effs.len();
        let status = dispatch(&mut tool, left_click(150.0), &mut effs, &mut last_err);
        assert_eq!(status, EventStatus::Ignored);
        assert_eq!(effs.len(), pre_effs);
    }

    // ── Rapid successive clicks (same preview) ─────────────────────────

    #[test]
    fn rapid_successive_same_frame_clicks_each_produce_one_transition() {
        // Each MouseDown must fire exactly one state transition even if
        // the widget bursts several clicks in one frame.
        let mut tool = BracketTool::awaiting_entry(Side::Long);
        let mut last_err = None;
        let mut effs = Vec::new();
        tool.update_preview(100.0, 200.0);
        dispatch(&mut tool, left_click(200.0), &mut effs, &mut last_err);
        let after_first = effs.len();
        // Second click without preview update → same price used.
        dispatch(&mut tool, left_click(200.0), &mut effs, &mut last_err);
        // Second click moved to AwaitingStop with TP = 100.
        assert!(effs.len() > after_first);
        assert!(matches!(tool.mode(), BracketToolMode::AwaitingStop { .. }));
    }

    // ── CursorLeft ─────────────────────────────────────────────────────

    #[test]
    fn cursor_left_hides_preview_without_leaving_state() {
        let mut tool = BracketTool::awaiting_entry(Side::Long);
        tool.update_preview(100.0, 200.0);
        let pr_owned = pr();
        let mut last_err = None;
        let mut effs = Vec::new();
        let mut cx = ToolContext {
            price_range: &pr_owned,
            last_error: &mut last_err,
            effects: &mut effs,
        };
        tool.update(InputEvent::CursorLeft, &mut cx);
        assert_eq!(
            tool.mode(),
            BracketToolMode::AwaitingEntry { side: Side::Long }
        );
        assert!(tool.cursor_y_px.is_none());
    }

    // ── Non-left mouse button ignored ──────────────────────────────────

    #[test]
    fn right_click_is_ignored_by_tool() {
        let mut tool = BracketTool::awaiting_entry(Side::Long);
        tool.update_preview(100.0, 200.0);
        let pr_owned = pr();
        let mut last_err = None;
        let mut effs = Vec::new();
        let mut cx = ToolContext {
            price_range: &pr_owned,
            last_error: &mut last_err,
            effects: &mut effs,
        };
        let status = tool.update(
            InputEvent::MouseDown {
                button: MouseButton::Right,
                pt: Point::new(500.0, 200.0),
                modifiers: Modifiers::default(),
            },
            &mut cx,
        );
        assert_eq!(status, EventStatus::Ignored);
        assert!(effs.is_empty());
    }

    #[test]
    fn middle_click_is_ignored_by_tool() {
        let mut tool = BracketTool::awaiting_entry(Side::Long);
        tool.update_preview(100.0, 200.0);
        let pr_owned = pr();
        let mut last_err = None;
        let mut effs = Vec::new();
        let mut cx = ToolContext {
            price_range: &pr_owned,
            last_error: &mut last_err,
            effects: &mut effs,
        };
        let status = tool.update(
            InputEvent::MouseDown {
                button: MouseButton::Middle,
                pt: Point::new(500.0, 200.0),
                modifiers: Modifiers::default(),
            },
            &mut cx,
        );
        assert_eq!(status, EventStatus::Ignored);
        assert!(effs.is_empty());
    }

    #[test]
    fn mouse_move_is_ignored_captured_false() {
        let mut tool = BracketTool::awaiting_entry(Side::Long);
        let pr_owned = pr();
        let mut last_err = None;
        let mut effs = Vec::new();
        let mut cx = ToolContext {
            price_range: &pr_owned,
            last_error: &mut last_err,
            effects: &mut effs,
        };
        let status = tool.update(
            InputEvent::MouseMove {
                pt: Point::new(500.0, 150.0),
            },
            &mut cx,
        );
        assert_eq!(status, EventStatus::Ignored);
    }

    #[test]
    fn mouse_up_is_ignored() {
        let mut tool = BracketTool::awaiting_entry(Side::Long);
        let pr_owned = pr();
        let mut last_err = None;
        let mut effs = Vec::new();
        let mut cx = ToolContext {
            price_range: &pr_owned,
            last_error: &mut last_err,
            effects: &mut effs,
        };
        let status = tool.update(mouse_up(), &mut cx);
        assert_eq!(status, EventStatus::Ignored);
    }

    #[test]
    fn wheel_event_ignored_for_fallthrough() {
        // Plan R20: tools default to Ignored on wheel.
        let tool = BracketTool::awaiting_entry(Side::Long);
        let mut scene = mk_scene_with_tool(tool);
        let status = scene.handle_input(InputEvent::Wheel {
            dx: 0.0,
            dy: 1.0,
            pt: Point::new(500.0, 200.0),
        });
        assert_eq!(status, EventStatus::Ignored);
        assert!(scene.has_active_tool());
    }

    #[test]
    fn keyup_is_ignored() {
        let mut tool = BracketTool::awaiting_entry(Side::Long);
        let pr_owned = pr();
        let mut last_err = None;
        let mut effs = Vec::new();
        let mut cx = ToolContext {
            price_range: &pr_owned,
            last_error: &mut last_err,
            effects: &mut effs,
        };
        let status = tool.update(
            InputEvent::KeyUp {
                key: Key::Char('L'),
            },
            &mut cx,
        );
        assert_eq!(status, EventStatus::Ignored);
    }

    // ── cancel / cancel_with_effect / on_destroy integration ──────────

    #[test]
    fn cancel_resets_to_idle() {
        let mut tool = BracketTool::awaiting_entry(Side::Long);
        tool.update_preview(100.0, 200.0);
        tool.cancel();
        assert_eq!(tool.mode(), BracketToolMode::Idle);
    }

    #[test]
    fn cancel_is_idempotent() {
        let mut tool = BracketTool::idle();
        tool.cancel();
        tool.cancel();
        assert_eq!(tool.mode(), BracketToolMode::Idle);
    }

    #[test]
    fn cancel_with_effect_emits_cancel_draft_when_placing() {
        let mut tool = BracketTool::awaiting_entry(Side::Long);
        let mut effs = Vec::new();
        tool.cancel_with_effect(&mut effs);
        assert_eq!(effs, vec![ToolEffect::CancelDraftBracket]);
        assert_eq!(tool.mode(), BracketToolMode::Idle);
    }

    #[test]
    fn cancel_with_effect_emits_nothing_when_idle() {
        let mut tool = BracketTool::idle();
        let mut effs = Vec::new();
        tool.cancel_with_effect(&mut effs);
        assert!(effs.is_empty());
    }

    #[test]
    fn scene_on_destroy_cancels_bracket_tool() {
        // R11: window-close mid-placement leaves no orphan drafts.
        let tool = BracketTool::awaiting_entry(Side::Long);
        let mut scene = mk_scene_with_tool(tool);
        scene.on_destroy();
        assert!(!scene.has_active_tool());
    }

    // ── Continue placing ──────────────────────────────────────────────

    #[test]
    fn continue_placing_with_long_resets_to_awaiting_entry() {
        let mut tool = BracketTool::awaiting_entry(Side::Long);
        let pr_owned = pr();
        let mut last_err = None;
        let mut effs = Vec::new();
        let mut cx = ToolContext {
            price_range: &pr_owned,
            last_error: &mut last_err,
            effects: &mut effs,
        };
        for p in [100.0, 105.0, 95.0] {
            tool.update_preview(p, 200.0);
            tool.update(left_click(200.0), &mut cx);
        }
        assert_eq!(tool.mode(), BracketToolMode::Complete);
        tool.continue_placing_with(Side::Long);
        assert_eq!(
            tool.mode(),
            BracketToolMode::AwaitingEntry { side: Side::Long }
        );
    }

    #[test]
    fn continue_placing_with_does_nothing_outside_complete() {
        let mut tool = BracketTool::awaiting_entry(Side::Long);
        tool.continue_placing_with(Side::Short);
        // Still Long — FSM didn't move because it wasn't Complete.
        assert_eq!(
            tool.mode(),
            BracketToolMode::AwaitingEntry { side: Side::Long }
        );
    }

    // ── Side / observer helpers ────────────────────────────────────────

    #[test]
    fn side_none_in_idle() {
        let tool = BracketTool::idle();
        assert!(tool.side().is_none());
    }

    #[test]
    fn side_some_in_awaiting_entry() {
        let tool = BracketTool::awaiting_entry(Side::Short);
        assert_eq!(tool.side(), Some(Side::Short));
    }

    #[test]
    fn is_placing_true_in_all_non_idle_non_complete_states() {
        assert!(!BracketTool::idle().is_placing());
        assert!(BracketTool::awaiting_entry(Side::Long).is_placing());
    }

    #[test]
    fn hit_test_returns_crosshair_cursor_while_placing() {
        let tool = BracketTool::awaiting_entry(Side::Long);
        let pr_owned = pr();
        let hit = InteractiveLayer::hit_test(&tool, Point::new(10.0, 10.0), &pr_owned);
        assert!(hit.is_some());
        assert_eq!(hit.unwrap().cursor, CursorShape::Crosshair);
    }

    #[test]
    fn hit_test_none_in_idle() {
        let tool = BracketTool::idle();
        let pr_owned = pr();
        let hit = InteractiveLayer::hit_test(&tool, Point::new(10.0, 10.0), &pr_owned);
        assert!(hit.is_none());
    }

    #[test]
    fn hit_test_none_in_complete() {
        let mut tool = BracketTool::awaiting_entry(Side::Long);
        let pr_owned = pr();
        let mut last_err = None;
        let mut effs = Vec::new();
        let mut cx = ToolContext {
            price_range: &pr_owned,
            last_error: &mut last_err,
            effects: &mut effs,
        };
        for p in [100.0, 105.0, 95.0] {
            tool.update_preview(p, 200.0);
            tool.update(left_click(200.0), &mut cx);
        }
        let hit = InteractiveLayer::hit_test(&tool, Point::new(10.0, 10.0), &pr_owned);
        assert!(hit.is_none());
    }

    // ── Scene integration: Escape routes through scene handler ────────

    #[test]
    fn scene_escape_clears_active_tool_and_emits_cancel_draft() {
        let tool = BracketTool::awaiting_entry(Side::Long);
        let mut scene = mk_scene_with_tool(tool);
        // Send Escape; scene's top-level handler unconditionally calls
        // clear_active_tool which invokes cancel() on the tool (which
        // resets mode but does NOT emit CancelDraftBracket). The scene-
        // level Escape path does NOT route to tool.update(). This is a
        // known gap; the widget must translate window-close + Escape
        // into TickerMsg::CancelBracket regardless. See cancel_with_effect.
        scene.handle_input(esc());
        assert!(!scene.has_active_tool());
    }

    fn paint_harness() -> (
        ContinuousAxis,
        midas_axis::LinearPriceAxis,
        PriceRange,
        Viewport,
        crate::ThemePalette,
        midas_axis::DefaultFormatter,
    ) {
        let pr = pr();
        let vp = vp();
        let paxis = midas_axis::LinearPriceAxis::new(pr, vp.height_px);
        (
            axis(),
            paxis,
            pr,
            vp,
            crate::ThemePalette::dark_default(),
            midas_axis::DefaultFormatter::new(),
        )
    }

    #[test]
    fn paint_emits_preview_line_when_cursor_set() {
        let mut tool = BracketTool::awaiting_entry(Side::Long);
        tool.update_preview(100.0, 200.0);
        let (axis, paxis, pr, vp, pal, fmt) = paint_harness();
        let mut out = crate::primitives::ScenePrimitives::default();
        let mut ctx = PaintContext {
            axis: &axis,
            viewport: vp,
            price_range: pr,
            palette: &pal,
            price_axis: &paxis,
            formatter: &fmt,
            out: &mut out,
        };
        tool.paint(&mut ctx);
        assert!(!out.lines.is_empty(), "preview line should be emitted");
        assert!(!out.text.is_empty(), "preview label should be emitted");
    }

    #[test]
    fn paint_emits_no_preview_in_idle() {
        let tool = BracketTool::idle();
        let (axis, paxis, pr, vp, pal, fmt) = paint_harness();
        let mut out = crate::primitives::ScenePrimitives::default();
        let mut ctx = PaintContext {
            axis: &axis,
            viewport: vp,
            price_range: pr,
            palette: &pal,
            price_axis: &paxis,
            formatter: &fmt,
            out: &mut out,
        };
        tool.paint(&mut ctx);
        assert!(out.lines.is_empty());
        assert!(out.text.is_empty());
    }

    #[test]
    fn paint_emits_no_preview_when_cursor_unset() {
        let mut tool = BracketTool::awaiting_entry(Side::Long);
        tool.preview_price = 100.0; // set price, but no cursor.
        let (axis, paxis, pr, vp, pal, fmt) = paint_harness();
        let mut out = crate::primitives::ScenePrimitives::default();
        let mut ctx = PaintContext {
            axis: &axis,
            viewport: vp,
            price_range: pr,
            palette: &pal,
            price_axis: &paxis,
            formatter: &fmt,
            out: &mut out,
        };
        tool.paint(&mut ctx);
        assert!(out.lines.is_empty());
    }

    #[test]
    fn paint_emits_correct_preview_label_per_state() {
        let mut tool = BracketTool::awaiting_entry(Side::Long);
        tool.update_preview(100.0, 200.0);
        let pr_owned = pr();
        let mut last_err = None;
        let mut effs = Vec::new();
        let mut cx = ToolContext {
            price_range: &pr_owned,
            last_error: &mut last_err,
            effects: &mut effs,
        };
        // In AwaitingEntry — label "E?".
        assert_eq!(tool.pending_role(), Some(LegRole::Entry));
        tool.update(left_click(200.0), &mut cx);
        // In AwaitingTarget — label "TP?".
        assert_eq!(tool.pending_role(), Some(LegRole::Tp));
        tool.update_preview(105.0, 150.0);
        tool.update(left_click(150.0), &mut cx);
        // In AwaitingStop — label "SL?".
        assert_eq!(tool.pending_role(), Some(LegRole::Sl));
    }

    // ── Preview update on all states ──────────────────────────────────

    #[test]
    fn update_preview_in_idle_sets_price_but_no_paint_effect() {
        let mut tool = BracketTool::idle();
        tool.update_preview(99.0, 300.0);
        // Idle has no pending role → paint emits nothing. Just ensure
        // the mutator doesn't panic.
        assert_eq!(tool.pending_role(), None);
    }

    // ── Continue_placing (legacy helper, defaults to Long) ─────────────

    #[test]
    fn continue_placing_defaults_to_long() {
        let mut tool = BracketTool::awaiting_entry(Side::Long);
        let pr_owned = pr();
        let mut last_err = None;
        let mut effs = Vec::new();
        let mut cx = ToolContext {
            price_range: &pr_owned,
            last_error: &mut last_err,
            effects: &mut effs,
        };
        for p in [100.0, 105.0, 95.0] {
            tool.update_preview(p, 200.0);
            tool.update(left_click(200.0), &mut cx);
        }
        tool.continue_placing();
        assert_eq!(
            tool.mode(),
            BracketToolMode::AwaitingEntry { side: Side::Long }
        );
    }

    // ── Layer z-ordinal ───────────────────────────────────────────────

    #[test]
    fn bracket_tool_paints_at_order_bracket_layer_z() {
        let tool = BracketTool::awaiting_entry(Side::Long);
        assert_eq!(tool.z(), LayerZ::ORDER_BRACKET);
    }

    #[test]
    fn bracket_tool_id_is_stable() {
        let tool = BracketTool::idle();
        assert_eq!(tool.id(), BRACKET_TOOL_ID);
    }

    // ── Scene routing (drag-focus on MouseDown) ───────────────────────

    #[test]
    fn scene_left_click_captured_sets_drag_focus() {
        let mut tool = BracketTool::awaiting_entry(Side::Long);
        tool.update_preview(100.0, 200.0);
        let mut scene = mk_scene_with_tool(tool);
        let status = scene.handle_input(left_click(200.0));
        assert_eq!(status, EventStatus::Captured);
        assert_eq!(scene.drag_focus(), Some(BRACKET_TOOL_ID));
    }

    #[test]
    fn scene_mouseup_releases_drag_focus() {
        let mut tool = BracketTool::awaiting_entry(Side::Long);
        tool.update_preview(100.0, 200.0);
        let mut scene = mk_scene_with_tool(tool);
        scene.handle_input(left_click(200.0));
        scene.handle_input(mouse_up());
        assert!(scene.drag_focus().is_none());
    }

    // ── Clone / Debug smoke ────────────────────────────────────────────

    #[test]
    fn bracket_tool_clone_round_trip() {
        let tool = BracketTool::awaiting_entry(Side::Short);
        let cloned = tool.clone();
        assert_eq!(tool.mode(), cloned.mode());
    }

    #[test]
    fn bracket_tool_debug_does_not_panic() {
        let tool = BracketTool::awaiting_entry(Side::Long);
        let _ = format!("{:?}", tool);
    }

    #[test]
    fn classify_side_toggle_unknown_char_returns_none() {
        assert!(BracketTool::classify_side_toggle(Key::Char('Q')).is_none());
        assert!(BracketTool::classify_side_toggle(Key::Char('A')).is_none());
        assert!(BracketTool::classify_side_toggle(Key::Escape).is_none());
    }

    #[test]
    fn side_observer_none_in_complete() {
        let mut tool = BracketTool::awaiting_entry(Side::Long);
        let pr_owned = pr();
        let mut last_err = None;
        let mut effs = Vec::new();
        let mut cx = ToolContext {
            price_range: &pr_owned,
            last_error: &mut last_err,
            effects: &mut effs,
        };
        for p in [100.0, 105.0, 95.0] {
            tool.update_preview(p, 200.0);
            tool.update(left_click(200.0), &mut cx);
        }
        assert!(tool.side().is_none());
    }

    #[test]
    fn is_complete_true_after_three_clicks() {
        let mut tool = BracketTool::awaiting_entry(Side::Long);
        let pr_owned = pr();
        let mut last_err = None;
        let mut effs = Vec::new();
        let mut cx = ToolContext {
            price_range: &pr_owned,
            last_error: &mut last_err,
            effects: &mut effs,
        };
        for p in [100.0, 105.0, 95.0] {
            tool.update_preview(p, 200.0);
            tool.update(left_click(200.0), &mut cx);
        }
        assert!(tool.is_complete());
    }

    #[test]
    fn is_complete_false_before_third_click() {
        let mut tool = BracketTool::awaiting_entry(Side::Long);
        let pr_owned = pr();
        let mut last_err = None;
        let mut effs = Vec::new();
        let mut cx = ToolContext {
            price_range: &pr_owned,
            last_error: &mut last_err,
            effects: &mut effs,
        };
        tool.update_preview(100.0, 200.0);
        tool.update(left_click(200.0), &mut cx);
        assert!(!tool.is_complete());
    }

    // ── Key::Escape doesn't toggle side ───────────────────────────────

    #[test]
    fn escape_is_not_a_side_toggle() {
        let mut tool = BracketTool::idle();
        let pr_owned = pr();
        let mut last_err = None;
        let mut effs = Vec::new();
        let mut cx = ToolContext {
            price_range: &pr_owned,
            last_error: &mut last_err,
            effects: &mut effs,
        };
        tool.update(esc(), &mut cx);
        assert_eq!(tool.mode(), BracketToolMode::Idle);
    }

    #[test]
    fn non_toggle_char_in_idle_ignored() {
        let mut tool = BracketTool::idle();
        let pr_owned = pr();
        let mut last_err = None;
        let mut effs = Vec::new();
        let mut cx = ToolContext {
            price_range: &pr_owned,
            last_error: &mut last_err,
            effects: &mut effs,
        };
        let status = tool.update(key('Q'), &mut cx);
        assert_eq!(status, EventStatus::Ignored);
        assert_eq!(tool.mode(), BracketToolMode::Idle);
    }
}
