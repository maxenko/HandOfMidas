//! Slice 1 of chart-transition: `InteractiveLayer` machinery tests.
//!
//! Covers:
//! - Top-down dispatch order (active-tool → layers hi-z → low-z).
//! - Drag-focus capture on `MouseDown`; bypass until `MouseUp`.
//! - Escape-cancels-tool + clears drag-focus.
//! - `on_destroy` cancels tool + clears drag-focus.
//! - Panic in one layer's `paint` does not kill other layers; scene
//!   emits a fallback quad + `SceneError::PanicFallback`.
//! - Default `as_interactive` returns None (passive layer).
//! - Send + Sync bounds on `Box<dyn InteractiveLayer>`.
//! - Wheel events default to `Ignored` when the tool doesn't claim
//!   them (plan R20).

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use chrono::TimeZone;
use midas_axis::{ContinuousAxis, PriceRange, Viewport};
use midas_calendar::Timestamp;
use midas_scene::{
    error::SceneError,
    input::{CursorShape, EventStatus, Hit, InputEvent, Key, Modifiers, MouseButton, Point},
    layer::{InteractiveLayer, LayerId, LayerZ, SceneLayer, ToolContext},
    paint::PaintContext,
    scene::ChartScene,
};

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

// ── Test fixtures ─────────────────────────────────────────────────────

/// Minimal passive layer — opts out of `as_interactive` by default.
struct PassiveLayer {
    id: &'static str,
    z: LayerZ,
}
impl SceneLayer for PassiveLayer {
    fn id(&self) -> LayerId {
        LayerId(self.id)
    }
    fn z(&self) -> LayerZ {
        self.z
    }
    fn paint(&self, _ctx: &mut PaintContext<'_>) {}
}

/// Interactive layer that captures every `MouseDown` of the configured
/// button + records event count.
struct CapturingLayer {
    id: &'static str,
    z: LayerZ,
    button: MouseButton,
    mouse_down_count: Arc<AtomicU32>,
    mouse_up_count: Arc<AtomicU32>,
    cancel_count: Arc<AtomicU32>,
}

impl SceneLayer for CapturingLayer {
    fn id(&self) -> LayerId {
        LayerId(self.id)
    }
    fn z(&self) -> LayerZ {
        self.z
    }
    fn paint(&self, _ctx: &mut PaintContext<'_>) {}
    fn as_interactive(&mut self) -> Option<&mut dyn InteractiveLayer> {
        Some(self)
    }
}

impl InteractiveLayer for CapturingLayer {
    fn update(&mut self, ev: InputEvent, _ctx: &mut ToolContext<'_>) -> EventStatus {
        match ev {
            InputEvent::MouseDown { button, .. } if button == self.button => {
                self.mouse_down_count.fetch_add(1, Ordering::SeqCst);
                EventStatus::Captured
            }
            InputEvent::MouseUp { .. } => {
                self.mouse_up_count.fetch_add(1, Ordering::SeqCst);
                EventStatus::Captured
            }
            InputEvent::MouseMove { .. } => EventStatus::Captured,
            _ => EventStatus::Ignored,
        }
    }
    fn hit_test(&self, _pt: Point, _pr: &PriceRange) -> Option<Hit> {
        Some(Hit {
            layer_id: LayerId(self.id),
            sub_z: 0,
            cursor: CursorShape::Pointer,
        })
    }
    fn cancel(&mut self) {
        self.cancel_count.fetch_add(1, Ordering::SeqCst);
    }
}

/// Panicking layer for panic-recovery tests.
struct PanickingLayer {
    id: &'static str,
    z: LayerZ,
}
impl SceneLayer for PanickingLayer {
    fn id(&self) -> LayerId {
        LayerId(self.id)
    }
    fn z(&self) -> LayerZ {
        self.z
    }
    fn paint(&self, _ctx: &mut PaintContext<'_>) {
        panic!("boom");
    }
}

/// Minimal tool that cancels when told.
struct FakeTool {
    id: &'static str,
    cancel_count: Arc<AtomicU32>,
    captures_wheel: bool,
}
impl SceneLayer for FakeTool {
    fn id(&self) -> LayerId {
        LayerId(self.id)
    }
    fn z(&self) -> LayerZ {
        LayerZ::CROSSHAIR
    }
    fn paint(&self, _ctx: &mut PaintContext<'_>) {}
    fn as_interactive(&mut self) -> Option<&mut dyn InteractiveLayer> {
        Some(self)
    }
}
impl InteractiveLayer for FakeTool {
    fn update(&mut self, ev: InputEvent, _ctx: &mut ToolContext<'_>) -> EventStatus {
        match ev {
            InputEvent::Wheel { .. } if self.captures_wheel => EventStatus::Captured,
            InputEvent::Wheel { .. } => EventStatus::Ignored,
            InputEvent::MouseDown { .. } => EventStatus::Captured,
            _ => EventStatus::Ignored,
        }
    }
    fn hit_test(&self, _pt: Point, _pr: &PriceRange) -> Option<Hit> {
        None
    }
    fn cancel(&mut self) {
        self.cancel_count.fetch_add(1, Ordering::SeqCst);
    }
}

fn empty_scene() -> ChartScene {
    ChartScene::builder()
        .axis(axis())
        .price_range(pr())
        .viewport(vp())
        .build()
        .unwrap()
}

// ── Tests ─────────────────────────────────────────────────────────────

#[test]
fn passive_layer_returns_none_from_as_interactive() {
    let mut layer = PassiveLayer {
        id: "p",
        z: LayerZ::GRID,
    };
    assert!(layer.as_interactive().is_none());
}

#[test]
fn capturing_layer_returns_some_from_as_interactive() {
    let counts = Arc::new(AtomicU32::new(0));
    let mut layer = CapturingLayer {
        id: "c",
        z: LayerZ::CANDLE,
        button: MouseButton::Left,
        mouse_down_count: counts.clone(),
        mouse_up_count: counts.clone(),
        cancel_count: counts.clone(),
    };
    assert!(layer.as_interactive().is_some());
}

#[test]
fn empty_scene_dispatches_ignored() {
    let mut scene = empty_scene();
    let ev = InputEvent::MouseMove {
        pt: Point::new(10.0, 10.0),
    };
    assert_eq!(scene.handle_input(ev), EventStatus::Ignored);
}

#[test]
fn active_tool_captures_mousedown_and_sets_drag_focus() {
    let cancel = Arc::new(AtomicU32::new(0));
    let tool = FakeTool {
        id: "tool",
        cancel_count: cancel.clone(),
        captures_wheel: false,
    };
    let mut scene = ChartScene::builder()
        .axis(axis())
        .price_range(pr())
        .viewport(vp())
        .active_tool(tool)
        .build()
        .unwrap();

    assert!(scene.has_active_tool());
    let ev = InputEvent::MouseDown {
        button: MouseButton::Left,
        pt: Point::new(10.0, 10.0),
        modifiers: Modifiers::default(),
    };
    assert_eq!(scene.handle_input(ev), EventStatus::Captured);
    assert_eq!(scene.drag_focus(), Some(LayerId("tool")));
}

#[test]
fn mouseup_clears_drag_focus() {
    let cancel = Arc::new(AtomicU32::new(0));
    let tool = FakeTool {
        id: "tool",
        cancel_count: cancel,
        captures_wheel: false,
    };
    let mut scene = ChartScene::builder()
        .axis(axis())
        .price_range(pr())
        .viewport(vp())
        .active_tool(tool)
        .build()
        .unwrap();
    scene.handle_input(InputEvent::MouseDown {
        button: MouseButton::Left,
        pt: Point::new(10.0, 10.0),
        modifiers: Modifiers::default(),
    });
    assert_eq!(scene.drag_focus(), Some(LayerId("tool")));
    scene.handle_input(InputEvent::MouseUp {
        button: MouseButton::Left,
        pt: Point::new(20.0, 10.0),
    });
    assert!(scene.drag_focus().is_none());
}

#[test]
fn escape_cancels_active_tool() {
    let cancel = Arc::new(AtomicU32::new(0));
    let tool = FakeTool {
        id: "tool",
        cancel_count: cancel.clone(),
        captures_wheel: false,
    };
    let mut scene = ChartScene::builder()
        .axis(axis())
        .price_range(pr())
        .viewport(vp())
        .active_tool(tool)
        .build()
        .unwrap();
    let ev = InputEvent::KeyDown {
        key: Key::Escape,
        modifiers: Modifiers::default(),
    };
    assert_eq!(scene.handle_input(ev), EventStatus::Captured);
    assert!(!scene.has_active_tool());
    assert_eq!(cancel.load(Ordering::SeqCst), 1);
}

#[test]
fn escape_without_tool_is_ignored() {
    let mut scene = empty_scene();
    let ev = InputEvent::KeyDown {
        key: Key::Escape,
        modifiers: Modifiers::default(),
    };
    assert_eq!(scene.handle_input(ev), EventStatus::Ignored);
}

#[test]
fn on_destroy_cancels_tool_and_clears_drag_focus() {
    let cancel = Arc::new(AtomicU32::new(0));
    let tool = FakeTool {
        id: "tool",
        cancel_count: cancel.clone(),
        captures_wheel: false,
    };
    let mut scene = ChartScene::builder()
        .axis(axis())
        .price_range(pr())
        .viewport(vp())
        .active_tool(tool)
        .build()
        .unwrap();
    scene.handle_input(InputEvent::MouseDown {
        button: MouseButton::Left,
        pt: Point::new(1.0, 1.0),
        modifiers: Modifiers::default(),
    });
    assert!(scene.drag_focus().is_some());
    scene.on_destroy();
    assert!(!scene.has_active_tool());
    assert!(scene.drag_focus().is_none());
    assert_eq!(cancel.load(Ordering::SeqCst), 1);
}

#[test]
fn set_active_tool_replaces_existing_and_cancels_old() {
    let c1 = Arc::new(AtomicU32::new(0));
    let c2 = Arc::new(AtomicU32::new(0));
    let mut scene = empty_scene();
    scene.set_active_tool(Box::new(FakeTool {
        id: "t1",
        cancel_count: c1.clone(),
        captures_wheel: false,
    }));
    scene.set_active_tool(Box::new(FakeTool {
        id: "t2",
        cancel_count: c2.clone(),
        captures_wheel: false,
    }));
    assert_eq!(c1.load(Ordering::SeqCst), 1, "old tool should be cancelled");
    assert_eq!(c2.load(Ordering::SeqCst), 0, "new tool still live");
    assert!(scene.has_active_tool());
}

#[test]
fn clear_active_tool_cancels_and_drops() {
    let c = Arc::new(AtomicU32::new(0));
    let mut scene = empty_scene();
    scene.set_active_tool(Box::new(FakeTool {
        id: "t",
        cancel_count: c.clone(),
        captures_wheel: false,
    }));
    scene.clear_active_tool();
    assert!(!scene.has_active_tool());
    assert_eq!(c.load(Ordering::SeqCst), 1);
}

#[test]
fn clear_active_tool_is_idempotent() {
    let mut scene = empty_scene();
    scene.clear_active_tool();
    scene.clear_active_tool();
    assert!(!scene.has_active_tool());
}

#[test]
fn layer_captures_mousedown_bubbles_through_tool() {
    // Tool ignores MouseDown; a layer's capturing impl wins.
    struct PassThroughTool;
    impl SceneLayer for PassThroughTool {
        fn id(&self) -> LayerId {
            LayerId("t")
        }
        fn z(&self) -> LayerZ {
            LayerZ::CROSSHAIR
        }
        fn paint(&self, _: &mut PaintContext<'_>) {}
        fn as_interactive(&mut self) -> Option<&mut dyn InteractiveLayer> {
            Some(self)
        }
    }
    impl InteractiveLayer for PassThroughTool {
        fn update(&mut self, _ev: InputEvent, _ctx: &mut ToolContext<'_>) -> EventStatus {
            EventStatus::Ignored
        }
        fn hit_test(&self, _pt: Point, _pr: &PriceRange) -> Option<Hit> {
            None
        }
        fn cancel(&mut self) {}
    }

    let counts = Arc::new(AtomicU32::new(0));
    let layer = CapturingLayer {
        id: "L",
        z: LayerZ::CANDLE,
        button: MouseButton::Left,
        mouse_down_count: counts.clone(),
        mouse_up_count: Arc::new(AtomicU32::new(0)),
        cancel_count: Arc::new(AtomicU32::new(0)),
    };
    let mut scene = ChartScene::builder()
        .axis(axis())
        .price_range(pr())
        .viewport(vp())
        .layer(layer)
        .active_tool(PassThroughTool)
        .build()
        .unwrap();

    let ev = InputEvent::MouseDown {
        button: MouseButton::Left,
        pt: Point::new(10.0, 10.0),
        modifiers: Modifiers::default(),
    };
    assert_eq!(scene.handle_input(ev), EventStatus::Captured);
    assert_eq!(counts.load(Ordering::SeqCst), 1);
    assert_eq!(scene.drag_focus(), Some(LayerId("L")));
}

#[test]
fn top_z_layer_wins_hit_test() {
    // Two capturing layers; the higher-z one should capture first
    // (top-down iteration). We use the same button; both would capture.
    let a_count = Arc::new(AtomicU32::new(0));
    let b_count = Arc::new(AtomicU32::new(0));
    let low = CapturingLayer {
        id: "low",
        z: LayerZ::GRID,
        button: MouseButton::Left,
        mouse_down_count: a_count.clone(),
        mouse_up_count: Arc::new(AtomicU32::new(0)),
        cancel_count: Arc::new(AtomicU32::new(0)),
    };
    let high = CapturingLayer {
        id: "high",
        z: LayerZ::CROSSHAIR,
        button: MouseButton::Left,
        mouse_down_count: b_count.clone(),
        mouse_up_count: Arc::new(AtomicU32::new(0)),
        cancel_count: Arc::new(AtomicU32::new(0)),
    };
    let mut scene = ChartScene::builder()
        .axis(axis())
        .price_range(pr())
        .viewport(vp())
        .layer(low)
        .layer(high)
        .build()
        .unwrap();

    let ev = InputEvent::MouseDown {
        button: MouseButton::Left,
        pt: Point::new(10.0, 10.0),
        modifiers: Modifiers::default(),
    };
    assert_eq!(scene.handle_input(ev), EventStatus::Captured);
    assert_eq!(b_count.load(Ordering::SeqCst), 1);
    assert_eq!(a_count.load(Ordering::SeqCst), 0);
    assert_eq!(scene.drag_focus(), Some(LayerId("high")));
}

#[test]
fn drag_focus_routes_events_to_captured_layer_bypassing_hit_test() {
    // Low-z layer captures; drag-focus routes subsequent MouseMove
    // events to it even though a high-z layer sits above.
    struct Greedy {
        id: &'static str,
        z: LayerZ,
        moves: Arc<AtomicU32>,
    }
    impl SceneLayer for Greedy {
        fn id(&self) -> LayerId {
            LayerId(self.id)
        }
        fn z(&self) -> LayerZ {
            self.z
        }
        fn paint(&self, _: &mut PaintContext<'_>) {}
        fn as_interactive(&mut self) -> Option<&mut dyn InteractiveLayer> {
            Some(self)
        }
    }
    impl InteractiveLayer for Greedy {
        fn update(&mut self, ev: InputEvent, _: &mut ToolContext<'_>) -> EventStatus {
            match ev {
                InputEvent::MouseMove { .. } => {
                    self.moves.fetch_add(1, Ordering::SeqCst);
                    EventStatus::Captured
                }
                InputEvent::MouseDown { .. } | InputEvent::MouseUp { .. } => EventStatus::Captured,
                _ => EventStatus::Ignored,
            }
        }
        fn hit_test(&self, _: Point, _: &PriceRange) -> Option<Hit> {
            None
        }
        fn cancel(&mut self) {}
    }

    let low_moves = Arc::new(AtomicU32::new(0));
    let mut scene = ChartScene::builder()
        .axis(axis())
        .price_range(pr())
        .viewport(vp())
        .layer(Greedy {
            id: "low",
            z: LayerZ::GRID,
            moves: low_moves.clone(),
        })
        .build()
        .unwrap();

    // Mouse-down captures low.
    scene.handle_input(InputEvent::MouseDown {
        button: MouseButton::Left,
        pt: Point::new(1.0, 1.0),
        modifiers: Modifiers::default(),
    });
    // Subsequent moves go straight to low via drag-focus.
    scene.handle_input(InputEvent::MouseMove {
        pt: Point::new(2.0, 2.0),
    });
    scene.handle_input(InputEvent::MouseMove {
        pt: Point::new(3.0, 3.0),
    });
    assert_eq!(low_moves.load(Ordering::SeqCst), 2);
}

#[test]
fn panic_in_paint_is_recovered_and_emits_fallback_quad() {
    let mut scene = ChartScene::builder()
        .axis(axis())
        .price_range(pr())
        .viewport(vp())
        .layer(PanickingLayer {
            id: "boom",
            z: LayerZ::CANDLE,
        })
        .build()
        .unwrap();

    let mut out = midas_scene::primitives::ScenePrimitives::default();
    scene.paint_mut(&mut out);

    // Fallback quad landed despite the panic.
    assert_eq!(out.quads.len(), 1);
    assert_eq!(out.quads[0].color, [0xff, 0x00, 0x00, 0x55]);
    // Scene recorded the error.
    let err = scene.take_last_error();
    match err {
        Some(SceneError::PanicFallback { layer }) => {
            assert_eq!(layer, LayerId("boom"));
        }
        other => panic!("expected PanicFallback, got {other:?}"),
    }
}

#[test]
fn panic_in_one_layer_does_not_prevent_others_from_painting() {
    struct GoodLayer;
    impl SceneLayer for GoodLayer {
        fn id(&self) -> LayerId {
            LayerId("good")
        }
        fn z(&self) -> LayerZ {
            LayerZ::HOLIDAY_MARKER
        }
        fn paint(&self, ctx: &mut PaintContext<'_>) {
            ctx.out.quads.push(midas_scene::primitives::QuadInstance {
                x: 1.0,
                y: 1.0,
                w: 10.0,
                h: 10.0,
                color: [0xaa, 0xbb, 0xcc, 0xff],
            });
        }
    }

    let mut scene = ChartScene::builder()
        .axis(axis())
        .price_range(pr())
        .viewport(vp())
        .layer(PanickingLayer {
            id: "boom",
            z: LayerZ::CANDLE,
        })
        .layer(GoodLayer)
        .build()
        .unwrap();

    let mut out = midas_scene::primitives::ScenePrimitives::default();
    scene.paint_mut(&mut out);
    // 1 fallback quad + 1 real quad from GoodLayer.
    assert_eq!(out.quads.len(), 2);
}

#[test]
fn take_last_error_is_one_shot() {
    let mut scene = ChartScene::builder()
        .axis(axis())
        .price_range(pr())
        .viewport(vp())
        .layer(PanickingLayer {
            id: "boom",
            z: LayerZ::CANDLE,
        })
        .build()
        .unwrap();
    let mut out = midas_scene::primitives::ScenePrimitives::default();
    scene.paint_mut(&mut out);
    assert!(scene.take_last_error().is_some());
    assert!(scene.take_last_error().is_none());
}

#[test]
fn tool_context_emits_error_to_scene_last_error() {
    struct FailingTool;
    impl SceneLayer for FailingTool {
        fn id(&self) -> LayerId {
            LayerId("fail")
        }
        fn z(&self) -> LayerZ {
            LayerZ::CROSSHAIR
        }
        fn paint(&self, _: &mut PaintContext<'_>) {}
        fn as_interactive(&mut self) -> Option<&mut dyn InteractiveLayer> {
            Some(self)
        }
    }
    impl InteractiveLayer for FailingTool {
        fn update(&mut self, _ev: InputEvent, ctx: &mut ToolContext<'_>) -> EventStatus {
            ctx.emit_error(SceneError::AnnotationNotFound);
            EventStatus::Captured
        }
        fn hit_test(&self, _: Point, _: &PriceRange) -> Option<Hit> {
            None
        }
        fn cancel(&mut self) {}
    }

    let mut scene = ChartScene::builder()
        .axis(axis())
        .price_range(pr())
        .viewport(vp())
        .active_tool(FailingTool)
        .build()
        .unwrap();
    scene.handle_input(InputEvent::MouseDown {
        button: MouseButton::Left,
        pt: Point::new(0.0, 0.0),
        modifiers: Modifiers::default(),
    });
    assert_eq!(
        scene.take_last_error(),
        Some(SceneError::AnnotationNotFound)
    );
}

#[test]
fn wheel_default_tool_ignores_allows_chart_zoom_fallthrough() {
    let cancel = Arc::new(AtomicU32::new(0));
    let tool = FakeTool {
        id: "tool",
        cancel_count: cancel,
        captures_wheel: false,
    };
    let mut scene = ChartScene::builder()
        .axis(axis())
        .price_range(pr())
        .viewport(vp())
        .active_tool(tool)
        .build()
        .unwrap();
    let ev = InputEvent::Wheel {
        dx: 0.0,
        dy: 1.0,
        pt: Point::new(10.0, 10.0),
    };
    // Plan R20: tools default to Ignored on wheel; scene returns
    // Ignored so caller pans/zooms.
    assert_eq!(scene.handle_input(ev), EventStatus::Ignored);
    assert!(scene.has_active_tool());
}

#[test]
fn wheel_opt_in_tool_captures() {
    let cancel = Arc::new(AtomicU32::new(0));
    let tool = FakeTool {
        id: "tool",
        cancel_count: cancel,
        captures_wheel: true,
    };
    let mut scene = ChartScene::builder()
        .axis(axis())
        .price_range(pr())
        .viewport(vp())
        .active_tool(tool)
        .build()
        .unwrap();
    let ev = InputEvent::Wheel {
        dx: 0.0,
        dy: 1.0,
        pt: Point::new(10.0, 10.0),
    };
    assert_eq!(scene.handle_input(ev), EventStatus::Captured);
}

#[test]
fn interactive_layer_box_is_send_sync() {
    fn assert_send_sync<T: Send + Sync + ?Sized>() {}
    assert_send_sync::<Box<dyn InteractiveLayer>>();
}

#[test]
fn scene_error_is_send_sync_clone() {
    fn assert_bounds<T: Send + Sync + Clone>() {}
    assert_bounds::<SceneError>();
}

#[test]
fn hit_struct_is_copy() {
    fn takes_copy<T: Copy>(_: T) {}
    takes_copy(Hit {
        layer_id: LayerId("t"),
        sub_z: 0,
        cursor: CursorShape::Crosshair,
    });
}

#[test]
fn drag_focus_releases_on_mouseup_even_if_layer_ignores() {
    // Defensive test: once drag-focus is set, MouseUp clears it
    // even if the target returned Ignored.
    struct Captures;
    impl SceneLayer for Captures {
        fn id(&self) -> LayerId {
            LayerId("c")
        }
        fn z(&self) -> LayerZ {
            LayerZ::CANDLE
        }
        fn paint(&self, _: &mut PaintContext<'_>) {}
        fn as_interactive(&mut self) -> Option<&mut dyn InteractiveLayer> {
            Some(self)
        }
    }
    impl InteractiveLayer for Captures {
        fn update(&mut self, ev: InputEvent, _: &mut ToolContext<'_>) -> EventStatus {
            match ev {
                InputEvent::MouseDown { .. } => EventStatus::Captured,
                InputEvent::MouseUp { .. } => EventStatus::Ignored, // defensive: don't capture
                _ => EventStatus::Ignored,
            }
        }
        fn hit_test(&self, _: Point, _: &PriceRange) -> Option<Hit> {
            None
        }
        fn cancel(&mut self) {}
    }

    let mut scene = ChartScene::builder()
        .axis(axis())
        .price_range(pr())
        .viewport(vp())
        .layer(Captures)
        .build()
        .unwrap();
    scene.handle_input(InputEvent::MouseDown {
        button: MouseButton::Left,
        pt: Point::new(1.0, 1.0),
        modifiers: Modifiers::default(),
    });
    assert_eq!(scene.drag_focus(), Some(LayerId("c")));
    scene.handle_input(InputEvent::MouseUp {
        button: MouseButton::Left,
        pt: Point::new(2.0, 2.0),
    });
    assert!(scene.drag_focus().is_none());
}

#[test]
fn passive_layer_mixed_with_interactive_routes_correctly() {
    let a_count = Arc::new(AtomicU32::new(0));
    let mut scene = ChartScene::builder()
        .axis(axis())
        .price_range(pr())
        .viewport(vp())
        .layer(PassiveLayer {
            id: "passive",
            z: LayerZ::CROSSHAIR,
        })
        .layer(CapturingLayer {
            id: "active",
            z: LayerZ::CANDLE,
            button: MouseButton::Left,
            mouse_down_count: a_count.clone(),
            mouse_up_count: Arc::new(AtomicU32::new(0)),
            cancel_count: Arc::new(AtomicU32::new(0)),
        })
        .build()
        .unwrap();
    let ev = InputEvent::MouseDown {
        button: MouseButton::Left,
        pt: Point::new(1.0, 1.0),
        modifiers: Modifiers::default(),
    };
    assert_eq!(scene.handle_input(ev), EventStatus::Captured);
    // Passive layer sat above capturing-z but returned None from
    // as_interactive, so scene skipped it and routed to the
    // capturing layer beneath.
    assert_eq!(a_count.load(Ordering::SeqCst), 1);
    assert_eq!(scene.drag_focus(), Some(LayerId("active")));
}

#[test]
fn key_down_non_escape_bubbles_through_without_tool() {
    let mut scene = empty_scene();
    let ev = InputEvent::KeyDown {
        key: Key::ArrowLeft,
        modifiers: Modifiers::default(),
    };
    assert_eq!(scene.handle_input(ev), EventStatus::Ignored);
}
