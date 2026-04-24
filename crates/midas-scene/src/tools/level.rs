//! `LevelTool` — 1-click horizontal price-level placement.
//!
//! FSM: `Idle` → `Placing { snapped_price, preview_px }` → (left-click
//! commits, Escape returns to `Idle`, tool stays in `Placing` after
//! commit so the user can place multiple levels without reactivating).
//!
//! Per plan slice 4:
//!
//! - Activation is a host-driven affair (the widget's "Add Level"
//!   toolbar button constructs `LevelTool::placing()` and installs it
//!   as `ChartScene::set_active_tool`).
//! - Left-click emits `ToolEffect::CreateLevel { price, lock: false }`
//!   with the snapped price. The tool stays in `Placing` so multi-level
//!   workflows don't need repeated toolbar clicks.
//! - Escape pivots to `Idle` and signals the host to `clear_active_tool`
//!   (handled by the scene's standard escape-cancels-tool path).
//! - While in `Placing`, the tool emits a faded preview line at
//!   `snapped_price` during `paint`.
//!
//! Snap feeds in via [`LevelTool::update_snap`] called from the widget
//! before each frame — the tool itself doesn't hold a `CandleSeries`;
//! it receives the finished snap result so `update` stays sans-IO.

use std::borrow::Cow;

use midas_axis::PriceRange;

use crate::input::{CursorShape, EventStatus, Hit, InputEvent, Key, MouseButton, Point};
use crate::layer::{InteractiveLayer, LayerId, LayerZ, SceneLayer, ToolContext};
use crate::paint::PaintContext;
use crate::primitives::{LineInstance, TextAnchor, TextInstance};
use crate::tools::ToolEffect;

/// Single [`LayerId`] used by every `LevelTool` instance — the scene's
/// drag-focus routing is id-based, and multiple `LevelTool`s are not a
/// supported scenario (R2: one active tool at a time).
const LEVEL_TOOL_ID: LayerId = LayerId("level-tool");

/// Explicit FSM state. Two variants today; keeping the enum so slice 5b
/// can follow the same pattern without churn.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum LevelToolMode {
    /// Tool installed but not yet "placing". In practice the widget
    /// constructs the tool directly in `Placing` (single-click model),
    /// so `Idle` is reached via Escape + on_destroy only.
    Idle,
    /// Preview follows the cursor; next left-click commits.
    Placing {
        /// Current preview price (post-snap).
        price: f64,
        /// Cursor y-pixel, stashed so paint can render the preview at
        /// the exact cursor position the user last saw. When `None`,
        /// the preview is hidden (cursor left the chart).
        cursor_y_px: Option<f32>,
    },
}

/// Horizontal level placement tool.
///
/// Holds only the FSM state + the per-frame preview price. Snap math
/// runs externally (widget) and is fed in through
/// [`LevelTool::update_snap`] before dispatch — keeps `midas-scene`
/// free of `CandleSeries` access from inside the tool's `update` call.
#[derive(Clone, Debug)]
pub struct LevelTool {
    mode: LevelToolMode,
    /// Whether the next `CreateLevel` should commit a locked level.
    /// Default `false`; the toolbar can expose a "lock new levels"
    /// checkbox later.
    default_lock: bool,
}

impl LevelTool {
    /// Build a tool already in `Placing`. Preview stays hidden until
    /// the first `MouseMove` (or the host calls [`update_snap`]).
    pub fn placing() -> Self {
        Self {
            mode: LevelToolMode::Placing {
                price: f64::NAN,
                cursor_y_px: None,
            },
            default_lock: false,
        }
    }

    /// Build a tool in `Idle`. Useful for tests.
    pub fn idle() -> Self {
        Self {
            mode: LevelToolMode::Idle,
            default_lock: false,
        }
    }

    /// Observer — current mode. Used by tests + the dev-harness
    /// `DumpState` projection.
    pub fn mode(&self) -> LevelToolMode {
        self.mode
    }

    /// Set whether committed levels start locked. Default is `false`.
    pub fn with_default_lock(mut self, lock: bool) -> Self {
        self.default_lock = lock;
        self
    }

    /// Feed the tool a pre-computed snap price + cursor y. The widget
    /// calls this once per `MouseMove` event after running
    /// [`crate::tools::snap_to_ohlc`] with the visible candle window.
    pub fn update_snap(&mut self, snapped_price: f64, cursor_y_px: f32) {
        if matches!(self.mode, LevelToolMode::Placing { .. }) {
            self.mode = LevelToolMode::Placing {
                price: snapped_price,
                cursor_y_px: Some(cursor_y_px),
            };
        }
    }

    /// Observer — is the tool actively placing?
    pub fn is_placing(&self) -> bool {
        matches!(self.mode, LevelToolMode::Placing { .. })
    }
}

impl SceneLayer for LevelTool {
    fn id(&self) -> LayerId {
        LEVEL_TOOL_ID
    }

    fn z(&self) -> LayerZ {
        // Preview paints above levels so it sits on top of any
        // existing ones.
        LayerZ::LEVEL
    }

    fn paint(&self, ctx: &mut PaintContext<'_>) {
        let LevelToolMode::Placing { price, cursor_y_px } = self.mode else {
            return;
        };
        let Some(_y) = cursor_y_px else {
            return;
        };
        if !price.is_finite() {
            return;
        }
        // Prefer the axis projection over the raw cursor y so the
        // preview snaps to the OHLC row visually, not to the raw
        // pointer.
        let y = ctx.price_to_y(price);
        let w = ctx.viewport.width_px;
        // Preview colour: palette text with reduced alpha (~40%).
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
        // Tiny label at left edge.
        ctx.out.text.push(TextInstance {
            x: 4.0,
            y,
            color: ctx.palette.text,
            text: Cow::Borrowed("+L"),
            size_px: 10.0,
            anchor: TextAnchor::MiddleLeft,
        });
    }

    fn as_interactive(&mut self) -> Option<&mut dyn InteractiveLayer> {
        Some(self)
    }
}

impl InteractiveLayer for LevelTool {
    fn update(&mut self, ev: InputEvent, ctx: &mut ToolContext<'_>) -> EventStatus {
        match ev {
            InputEvent::MouseDown {
                button: MouseButton::Left,
                ..
            } => {
                // Commit the preview price if we're placing + have a
                // finite snap.
                let LevelToolMode::Placing { price, .. } = self.mode else {
                    return EventStatus::Ignored;
                };
                if !price.is_finite() {
                    // No snap landed yet; ignore the click so the
                    // widget's pan/zoom fallthrough can handle it.
                    return EventStatus::Ignored;
                }
                ctx.emit_effect(ToolEffect::CreateLevel {
                    price,
                    lock: self.default_lock,
                });
                tracing::debug!(
                    target: "midas_scene::tools::level",
                    price,
                    "LevelTool committed CreateLevel; staying in Placing",
                );
                // Stay in Placing — multi-level workflow.
                EventStatus::Captured
            }
            InputEvent::KeyDown {
                key: Key::Escape, ..
            } => {
                // Scene's top-level Escape handler cancels the tool
                // outright via `clear_active_tool`. We still reset our
                // own state defensively so a tool reused across
                // install/reinstall starts clean.
                self.mode = LevelToolMode::Idle;
                EventStatus::Captured
            }
            InputEvent::CursorLeft => {
                // Hide preview without leaving Placing.
                if let LevelToolMode::Placing { price, .. } = self.mode {
                    self.mode = LevelToolMode::Placing {
                        price,
                        cursor_y_px: None,
                    };
                }
                EventStatus::Ignored
            }
            // Wheel default: Ignored (plan R20).
            _ => EventStatus::Ignored,
        }
    }

    fn hit_test(&self, _pt: Point, _price_range: &PriceRange) -> Option<Hit> {
        // Tools don't claim hover; the crosshair cursor is the
        // appropriate shape and the scene's cursor-shape cascade picks
        // it up at the widget edge.
        if self.is_placing() {
            Some(Hit {
                layer_id: LEVEL_TOOL_ID,
                sub_z: 0,
                cursor: CursorShape::Crosshair,
            })
        } else {
            None
        }
    }

    fn cancel(&mut self) {
        self.mode = LevelToolMode::Idle;
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use midas_axis::{ContinuousAxis, PriceRange, Viewport};
    use midas_calendar::Timestamp;

    use super::*;
    use crate::error::SceneError;
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

    fn mk_scene_with_tool(tool: LevelTool) -> ChartScene {
        ChartScene::builder()
            .axis(axis())
            .price_range(pr())
            .viewport(vp())
            .active_tool(tool)
            .build()
            .unwrap()
    }

    #[test]
    fn idle_tool_is_not_placing() {
        let tool = LevelTool::idle();
        assert!(!tool.is_placing());
        assert_eq!(tool.mode(), LevelToolMode::Idle);
    }

    #[test]
    fn placing_tool_reports_placing_true() {
        let tool = LevelTool::placing();
        assert!(tool.is_placing());
    }

    #[test]
    fn update_snap_sets_price_and_cursor_y() {
        let mut tool = LevelTool::placing();
        tool.update_snap(100.5, 150.0);
        match tool.mode() {
            LevelToolMode::Placing { price, cursor_y_px } => {
                assert_eq!(price, 100.5);
                assert_eq!(cursor_y_px, Some(150.0));
            }
            _ => panic!("expected Placing"),
        }
    }

    #[test]
    fn update_snap_on_idle_tool_is_no_op() {
        let mut tool = LevelTool::idle();
        tool.update_snap(100.0, 200.0);
        assert_eq!(tool.mode(), LevelToolMode::Idle);
    }

    #[test]
    fn left_click_in_placing_emits_create_level_and_stays_placing() {
        let mut tool = LevelTool::placing();
        tool.update_snap(100.5, 150.0);
        let mut scene = mk_scene_with_tool(tool);

        scene.handle_input(InputEvent::MouseDown {
            button: MouseButton::Left,
            pt: Point::new(500.0, 150.0),
            modifiers: Modifiers::default(),
        });
        let effects = scene.take_effects();
        assert_eq!(effects.len(), 1);
        assert_eq!(
            effects[0],
            ToolEffect::CreateLevel {
                price: 100.5,
                lock: false,
            }
        );
        // Still placing after commit.
        assert!(scene.has_active_tool());
    }

    #[test]
    fn left_click_without_snap_is_ignored() {
        // No `update_snap` call → price stays NaN → click is Ignored
        // so pan/zoom fallthrough still works.
        let tool = LevelTool::placing();
        let mut scene = mk_scene_with_tool(tool);

        let status = scene.handle_input(InputEvent::MouseDown {
            button: MouseButton::Left,
            pt: Point::new(500.0, 150.0),
            modifiers: Modifiers::default(),
        });
        assert_eq!(status, EventStatus::Ignored);
        assert!(scene.take_effects().is_empty());
    }

    #[test]
    fn escape_cancels_tool_to_idle() {
        let mut tool = LevelTool::placing();
        tool.update_snap(100.5, 150.0);
        let mut scene = mk_scene_with_tool(tool);

        scene.handle_input(InputEvent::KeyDown {
            key: Key::Escape,
            modifiers: Modifiers::default(),
        });
        assert!(!scene.has_active_tool());
    }

    #[test]
    fn cursor_left_hides_preview_without_leaving_placing() {
        let mut tool = LevelTool::placing();
        tool.update_snap(100.5, 150.0);
        // Simulate CursorLeft without going through the scene (we
        // want the direct InteractiveLayer path here).
        let pr_owned = pr();
        let mut last_err: Option<SceneError> = None;
        let mut effects: Vec<ToolEffect> = Vec::new();
        let mut ctx = ToolContext {
            price_range: &pr_owned,
            last_error: &mut last_err,
            effects: &mut effects,
        };
        let status = tool.update(InputEvent::CursorLeft, &mut ctx);
        assert_eq!(status, EventStatus::Ignored);
        match tool.mode() {
            LevelToolMode::Placing { price, cursor_y_px } => {
                assert_eq!(price, 100.5);
                assert_eq!(cursor_y_px, None);
            }
            _ => panic!("expected Placing with cursor cleared"),
        }
    }

    #[test]
    fn multi_level_click_emits_one_effect_per_click() {
        let mut tool = LevelTool::placing();
        tool.update_snap(100.5, 150.0);
        let mut scene = mk_scene_with_tool(tool);

        // First click.
        scene.handle_input(InputEvent::MouseDown {
            button: MouseButton::Left,
            pt: Point::new(500.0, 150.0),
            modifiers: Modifiers::default(),
        });
        let a = scene.take_effects();
        assert_eq!(a.len(), 1);

        // Second click — tool still in Placing.
        scene.handle_input(InputEvent::MouseUp {
            button: MouseButton::Left,
            pt: Point::new(500.0, 150.0),
        });
        scene.handle_input(InputEvent::MouseDown {
            button: MouseButton::Left,
            pt: Point::new(500.0, 200.0),
            modifiers: Modifiers::default(),
        });
        let b = scene.take_effects();
        assert_eq!(b.len(), 1);
    }

    #[test]
    fn non_left_mousedown_is_ignored() {
        let mut tool = LevelTool::placing();
        tool.update_snap(100.5, 150.0);
        let pr_owned = pr();
        let mut last_err: Option<SceneError> = None;
        let mut effects: Vec<ToolEffect> = Vec::new();
        let mut ctx = ToolContext {
            price_range: &pr_owned,
            last_error: &mut last_err,
            effects: &mut effects,
        };
        let status = tool.update(
            InputEvent::MouseDown {
                button: MouseButton::Right,
                pt: Point::new(500.0, 150.0),
                modifiers: Modifiers::default(),
            },
            &mut ctx,
        );
        assert_eq!(status, EventStatus::Ignored);
        assert!(effects.is_empty());
    }

    #[test]
    fn wheel_is_ignored_for_fallthrough() {
        // Plan R20: tools default to Ignored on wheel.
        let tool = LevelTool::placing();
        let mut scene = mk_scene_with_tool(tool);
        let status = scene.handle_input(InputEvent::Wheel {
            dx: 0.0,
            dy: 1.0,
            pt: Point::new(500.0, 150.0),
        });
        assert_eq!(status, EventStatus::Ignored);
        assert!(scene.has_active_tool());
    }

    #[test]
    fn default_lock_propagates_to_create_level_effect() {
        let mut tool = LevelTool::placing().with_default_lock(true);
        tool.update_snap(99.0, 300.0);
        let mut scene = mk_scene_with_tool(tool);
        scene.handle_input(InputEvent::MouseDown {
            button: MouseButton::Left,
            pt: Point::new(200.0, 300.0),
            modifiers: Modifiers::default(),
        });
        let effects = scene.take_effects();
        assert_eq!(
            effects[0],
            ToolEffect::CreateLevel {
                price: 99.0,
                lock: true,
            }
        );
    }

    #[test]
    fn hit_test_returns_crosshair_cursor_while_placing() {
        let tool = LevelTool::placing();
        let pr = PriceRange::new(90.0, 110.0).unwrap();
        let hit = InteractiveLayer::hit_test(&tool, Point::new(10.0, 10.0), &pr);
        assert!(hit.is_some());
        assert_eq!(hit.unwrap().cursor, CursorShape::Crosshair);
    }

    #[test]
    fn hit_test_returns_none_when_idle() {
        let tool = LevelTool::idle();
        let pr = PriceRange::new(90.0, 110.0).unwrap();
        let hit = InteractiveLayer::hit_test(&tool, Point::new(10.0, 10.0), &pr);
        assert!(hit.is_none());
    }
}
