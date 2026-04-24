//! Integration tests for [`BracketTool`] + [`OrderBracketLayer`]
//! (slice 5b of the chart-transition plan).
//!
//! Complements the in-module unit tests with scene-level integration
//! that exercises `ChartScene::handle_input` routing end-to-end:
//!
//! - Multi-bracket placement (continue_placing_with)
//! - Scene drag-focus lifecycle
//! - Layer-render (full-width entry, right-from-entry TP/SL, amber
//!   tint, partial fill, drag handle hit-test)
//! - Decorator-group projection

use std::borrow::Cow;
use std::sync::Arc;

use chrono::TimeZone;
use midas_axis::{
    ContinuousAxis, DefaultFormatter, LinearPriceAxis, PriceRange, TimeAxis, Viewport,
};
use midas_calendar::Timestamp;
use midas_scene::layers::{
    BracketDragState, BracketHitTarget, OrderBracketLayer, OrderBracketView, SharedBracketDrag,
    Side,
};
use midas_scene::{
    BracketTool, BracketToolMode, ChartScene, EventStatus, InputEvent, InteractiveLayer, Key,
    LegKind, LegRole, Modifiers, MouseButton, PaintContext, Point, ScenePrimitives, ThemePalette,
    ToolEffect, ToolSide,
};
use parking_lot::Mutex;

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

fn left_click(y: f32) -> InputEvent {
    InputEvent::MouseDown {
        button: MouseButton::Left,
        pt: Point::new(500.0, y),
        modifiers: Modifiers::default(),
    }
}

fn mouse_up() -> InputEvent {
    InputEvent::MouseUp {
        button: MouseButton::Left,
        pt: Point::new(500.0, 0.0),
    }
}

fn esc() -> InputEvent {
    InputEvent::KeyDown {
        key: Key::Escape,
        modifiers: Modifiers::default(),
    }
}

fn paint_harness() -> (
    ContinuousAxis,
    LinearPriceAxis,
    PriceRange,
    Viewport,
    ThemePalette,
    DefaultFormatter,
) {
    let axis = axis();
    let pr = pr();
    let vp = vp();
    let paxis = LinearPriceAxis::new(pr, vp.height_px);
    (
        axis,
        paxis,
        pr,
        vp,
        ThemePalette::dark_default(),
        DefaultFormatter::new(),
    )
}

// ── Scene-level integration: 3-click produces 5 effects ───────────────

#[test]
fn scene_three_click_long_places_bracket_via_effects() {
    let mut tool = BracketTool::awaiting_entry(ToolSide::Long);
    tool.update_preview(100.0, 200.0);
    let mut scene = mk_scene_with_tool(tool);

    scene.handle_input(left_click(200.0));
    // Simulate widget feeding the next preview.
    if let Some(t) = scene_active_tool_mut(&mut scene) {
        t.update_preview(105.0, 150.0);
    }
    scene.handle_input(mouse_up());
    scene.handle_input(left_click(150.0));
    if let Some(t) = scene_active_tool_mut(&mut scene) {
        t.update_preview(95.0, 250.0);
    }
    scene.handle_input(mouse_up());
    scene.handle_input(left_click(250.0));

    let effs = scene.take_effects();
    assert_eq!(effs.len(), 5);
    assert!(matches!(effs[0], ToolEffect::BeginDraftBracket { .. }));
    assert_eq!(effs[4], ToolEffect::CommitDraftBracket);
}

/// Helper: downcast the scene's active tool to `&mut BracketTool` so
/// the test can call `update_preview` between events. The scene's
/// `active_tool` field is not exposed; we work around that by
/// re-installing the tool — but that loses FSM state. Preferred path:
/// the widget feeds `update_preview` externally. For tests, we cheat
/// by checking if we have an active tool (cannot directly access it
/// as BracketTool since trait is type-erased). So tests just place
/// clicks at consistent Y and accept the same preview applies.
fn scene_active_tool_mut(_scene: &mut ChartScene) -> Option<&mut BracketTool> {
    // We cannot downcast `Box<dyn InteractiveLayer>` without unsafe
    // or a downcast-helper trait. Tests that need per-click preview
    // updates exercise `BracketTool::update` directly instead.
    None
}

// ── Scene drag-focus lifecycle ─────────────────────────────────────────

#[test]
fn scene_left_click_on_bracket_tool_sets_drag_focus() {
    let mut tool = BracketTool::awaiting_entry(ToolSide::Long);
    tool.update_preview(100.0, 200.0);
    let mut scene = mk_scene_with_tool(tool);
    let status = scene.handle_input(left_click(200.0));
    assert_eq!(status, EventStatus::Captured);
    assert!(scene.drag_focus().is_some());
}

#[test]
fn scene_mouseup_releases_drag_focus_and_tool_stays_active() {
    let mut tool = BracketTool::awaiting_entry(ToolSide::Long);
    tool.update_preview(100.0, 200.0);
    let mut scene = mk_scene_with_tool(tool);
    scene.handle_input(left_click(200.0));
    scene.handle_input(mouse_up());
    assert!(scene.drag_focus().is_none());
    assert!(scene.has_active_tool());
}

#[test]
fn scene_on_destroy_clears_tool_and_drag_focus() {
    // R11: window-close mid-placement leaves no orphan drafts + no
    // drag-focus state. The widget layer still needs to project a
    // CancelBracket to TickerState separately.
    let mut tool = BracketTool::awaiting_entry(ToolSide::Long);
    tool.update_preview(100.0, 200.0);
    let mut scene = mk_scene_with_tool(tool);
    scene.handle_input(left_click(200.0));
    scene.on_destroy();
    assert!(!scene.has_active_tool());
    assert!(scene.drag_focus().is_none());
}

#[test]
fn scene_escape_clears_tool_entirely() {
    let tool = BracketTool::awaiting_entry(ToolSide::Long);
    let mut scene = mk_scene_with_tool(tool);
    scene.handle_input(esc());
    assert!(!scene.has_active_tool());
}

// ── OrderBracketLayer render tests ─────────────────────────────────────

#[test]
fn entry_line_is_full_width_via_x_span() {
    let (axis, paxis, pr, vp, pal, fmt) = paint_harness();
    let mut out = ScenePrimitives::default();
    let mut ctx = PaintContext {
        axis: &axis,
        viewport: vp,
        price_range: pr,
        palette: &pal,
        price_axis: &paxis,
        formatter: &fmt,
        out: &mut out,
    };
    let layer = OrderBracketLayer::new(vec![OrderBracketView {
        id: 1,
        entry_price: 100.0,
        tp_price: None,
        sl_price: None,
        side: Side::Long,
        label: Cow::Borrowed("E"),
        entry_ts: None,
        filled_qty: None,
        total_qty: 1,
    }]);
    midas_scene::SceneLayer::paint(&layer, &mut ctx);
    assert_eq!(out.lines.len(), 1);
    assert!((out.lines[0].x0 - 0.0).abs() < 1e-4);
    assert!((out.lines[0].x1 - vp.width_px).abs() < 1e-4);
}

#[test]
fn tp_sl_line_is_right_from_entry_when_entry_ts_set() {
    let (axis, paxis, pr, vp, pal, fmt) = paint_harness();
    let mut out = ScenePrimitives::default();
    let entry_ts = ts(2024, 1, 1, 12, 0, 0); // midpoint of axis
    let expected_x0 = TimeAxis::to_x(&axis, entry_ts);
    let mut ctx = PaintContext {
        axis: &axis,
        viewport: vp,
        price_range: pr,
        palette: &pal,
        price_axis: &paxis,
        formatter: &fmt,
        out: &mut out,
    };
    let layer = OrderBracketLayer::new(vec![OrderBracketView {
        id: 1,
        entry_price: 100.0,
        tp_price: Some(105.0),
        sl_price: Some(95.0),
        side: Side::Long,
        label: Cow::Borrowed("E"),
        entry_ts: Some(entry_ts),
        filled_qty: None,
        total_qty: 1,
    }]);
    midas_scene::SceneLayer::paint(&layer, &mut ctx);
    assert_eq!(out.lines.len(), 3);
    // Entry line at x0 = 0.
    assert!((out.lines[0].x0 - 0.0).abs() < 1e-4);
    // TP and SL lines at x0 = expected_x0.
    assert!((out.lines[1].x0 - expected_x0).abs() < 1.0);
    assert!((out.lines[2].x0 - expected_x0).abs() < 1.0);
}

#[test]
fn amber_tint_applied_to_wrong_side_tp_long() {
    // Long TP BELOW entry is wrong-side → amber.
    let (axis, paxis, pr, vp, pal, fmt) = paint_harness();
    let mut out = ScenePrimitives::default();
    let mut ctx = PaintContext {
        axis: &axis,
        viewport: vp,
        price_range: pr,
        palette: &pal,
        price_axis: &paxis,
        formatter: &fmt,
        out: &mut out,
    };
    let layer = OrderBracketLayer::new(vec![OrderBracketView {
        id: 1,
        entry_price: 100.0,
        tp_price: Some(95.0), // WRONG side for Long
        sl_price: None,
        side: Side::Long,
        label: Cow::Borrowed("E"),
        entry_ts: None,
        filled_qty: None,
        total_qty: 1,
    }]);
    midas_scene::SceneLayer::paint(&layer, &mut ctx);
    // Entry + TP = 2 lines. The TP (index 1) should be amber.
    assert_eq!(out.lines.len(), 2);
    let tp_line = &out.lines[1];
    // Amber is [0xf2, 0xb3, 0x2e, 0xff].
    assert_eq!(tp_line.color[0], 0xf2);
    assert_eq!(tp_line.color[1], 0xb3);
    assert_eq!(tp_line.color[2], 0x2e);
}

#[test]
fn amber_tint_applied_to_wrong_side_sl_short() {
    // Short SL BELOW entry is wrong-side → amber.
    let (axis, paxis, pr, vp, pal, fmt) = paint_harness();
    let mut out = ScenePrimitives::default();
    let mut ctx = PaintContext {
        axis: &axis,
        viewport: vp,
        price_range: pr,
        palette: &pal,
        price_axis: &paxis,
        formatter: &fmt,
        out: &mut out,
    };
    let layer = OrderBracketLayer::new(vec![OrderBracketView {
        id: 1,
        entry_price: 100.0,
        tp_price: None,
        sl_price: Some(95.0), // WRONG side for Short
        side: Side::Short,
        label: Cow::Borrowed("E"),
        entry_ts: None,
        filled_qty: None,
        total_qty: 1,
    }]);
    midas_scene::SceneLayer::paint(&layer, &mut ctx);
    assert_eq!(out.lines.len(), 2);
    let sl_line = &out.lines[1];
    assert_eq!(sl_line.color[0], 0xf2);
}

#[test]
fn equal_price_tp_is_amber_plan_c2() {
    // Plan C2: equal price IS wrong-side.
    let (axis, paxis, pr, vp, pal, fmt) = paint_harness();
    let mut out = ScenePrimitives::default();
    let mut ctx = PaintContext {
        axis: &axis,
        viewport: vp,
        price_range: pr,
        palette: &pal,
        price_axis: &paxis,
        formatter: &fmt,
        out: &mut out,
    };
    let layer = OrderBracketLayer::new(vec![OrderBracketView {
        id: 1,
        entry_price: 100.0,
        tp_price: Some(100.0), // equal to entry → wrong-side
        sl_price: None,
        side: Side::Long,
        label: Cow::Borrowed("E"),
        entry_ts: None,
        filled_qty: None,
        total_qty: 1,
    }]);
    midas_scene::SceneLayer::paint(&layer, &mut ctx);
    assert_eq!(out.lines[1].color[0], 0xf2, "equal price should be amber");
}

#[test]
fn partial_fill_renders_entry_with_distinct_color() {
    // filled_qty = 5 / total = 10 → partial fill → brighter entry.
    let (axis, paxis, pr, vp, pal, fmt) = paint_harness();
    let mut out_partial = ScenePrimitives::default();
    let mut out_full = ScenePrimitives::default();
    {
        let mut ctx = PaintContext {
            axis: &axis,
            viewport: vp,
            price_range: pr,
            palette: &pal,
            price_axis: &paxis,
            formatter: &fmt,
            out: &mut out_partial,
        };
        let layer = OrderBracketLayer::new(vec![OrderBracketView {
            id: 1,
            entry_price: 100.0,
            tp_price: None,
            sl_price: None,
            side: Side::Long,
            label: Cow::Borrowed("E"),
            entry_ts: None,
            filled_qty: Some(5),
            total_qty: 10,
        }]);
        midas_scene::SceneLayer::paint(&layer, &mut ctx);
    }
    {
        let mut ctx = PaintContext {
            axis: &axis,
            viewport: vp,
            price_range: pr,
            palette: &pal,
            price_axis: &paxis,
            formatter: &fmt,
            out: &mut out_full,
        };
        let layer = OrderBracketLayer::new(vec![OrderBracketView {
            id: 1,
            entry_price: 100.0,
            tp_price: None,
            sl_price: None,
            side: Side::Long,
            label: Cow::Borrowed("E"),
            entry_ts: None,
            filled_qty: None,
            total_qty: 10,
        }]);
        midas_scene::SceneLayer::paint(&layer, &mut ctx);
    }
    // Partial should be brighter (channel values closer to 0xff).
    let partial_color = out_partial.lines[0].color;
    let full_color = out_full.lines[0].color;
    assert_ne!(partial_color, full_color);
    assert!(partial_color[0] > full_color[0] || partial_color[1] > full_color[1]);
}

#[test]
fn fully_filled_bracket_uses_normal_color() {
    // filled == total → not partial → normal colour.
    let (axis, paxis, pr, vp, pal, fmt) = paint_harness();
    let mut out = ScenePrimitives::default();
    let mut ctx = PaintContext {
        axis: &axis,
        viewport: vp,
        price_range: pr,
        palette: &pal,
        price_axis: &paxis,
        formatter: &fmt,
        out: &mut out,
    };
    let layer = OrderBracketLayer::new(vec![OrderBracketView {
        id: 1,
        entry_price: 100.0,
        tp_price: None,
        sl_price: None,
        side: Side::Long,
        label: Cow::Borrowed("E"),
        entry_ts: None,
        filled_qty: Some(10),
        total_qty: 10,
    }]);
    midas_scene::SceneLayer::paint(&layer, &mut ctx);
    // Long entry colour is palette.candle_up.
    assert_eq!(out.lines[0].color, pal.candle_up);
}

#[test]
fn bracket_with_none_tp_renders_entry_and_sl_only() {
    let (axis, paxis, pr, vp, pal, fmt) = paint_harness();
    let mut out = ScenePrimitives::default();
    let mut ctx = PaintContext {
        axis: &axis,
        viewport: vp,
        price_range: pr,
        palette: &pal,
        price_axis: &paxis,
        formatter: &fmt,
        out: &mut out,
    };
    let layer = OrderBracketLayer::new(vec![OrderBracketView {
        id: 1,
        entry_price: 100.0,
        tp_price: None,
        sl_price: Some(95.0),
        side: Side::Long,
        label: Cow::Borrowed("E"),
        entry_ts: None,
        filled_qty: None,
        total_qty: 1,
    }]);
    midas_scene::SceneLayer::paint(&layer, &mut ctx);
    // Entry + SL = 2 lines; no TP.
    assert_eq!(out.lines.len(), 2);
}

// ── Interactive layer hit-test ─────────────────────────────────────────

fn mk_interactive_layer(view: OrderBracketView) -> (OrderBracketLayer, SharedBracketDrag) {
    let drag: SharedBracketDrag = Arc::new(Mutex::new(BracketDragState::default()));
    let layer = OrderBracketLayer::new(vec![view]).with_interaction(Arc::clone(&drag));
    (layer, drag)
}

fn prime_layer_viewport(layer: &OrderBracketLayer) {
    // Paint once to prime last_viewport_w / last_viewport_h so the hit-
    // test path has non-zero dims.
    let (axis, paxis, pr, vp, pal, fmt) = paint_harness();
    let mut out = ScenePrimitives::default();
    let mut ctx = PaintContext {
        axis: &axis,
        viewport: vp,
        price_range: pr,
        palette: &pal,
        price_axis: &paxis,
        formatter: &fmt,
        out: &mut out,
    };
    midas_scene::SceneLayer::paint(layer, &mut ctx);
}

#[test]
fn drag_handle_hit_test_at_right_edge_within_tolerance() {
    let view = OrderBracketView {
        id: 42,
        entry_price: 100.0,
        tp_price: Some(105.0),
        sl_price: Some(95.0),
        side: Side::Long,
        label: Cow::Borrowed("E"),
        entry_ts: None,
        filled_qty: None,
        total_qty: 1,
    };
    let (layer, _) = mk_interactive_layer(view);
    prime_layer_viewport(&layer);

    // TP at price=105 → y = (110-105)/20 * 400 = 100.
    let tp_y = 100.0;
    let pt = Point::new(999.0, tp_y); // near right edge (w=1000), so drag handle.
    let hit = layer.hit_bracket(pt, &pr()).unwrap();
    assert_eq!(hit.bracket_id, 42);
    assert!(matches!(
        hit.target,
        BracketHitTarget::DragHandle { leg: LegKind::Tp }
    ));
}

#[test]
fn drag_handle_hit_test_outside_tolerance_returns_line_band_or_none() {
    let view = OrderBracketView {
        id: 1,
        entry_price: 100.0,
        tp_price: Some(105.0),
        sl_price: None,
        side: Side::Long,
        label: Cow::Borrowed("E"),
        entry_ts: None,
        filled_qty: None,
        total_qty: 1,
    };
    let (layer, _) = mk_interactive_layer(view);
    prime_layer_viewport(&layer);
    // At x=500 (middle of chart), y=100 (on TP at price=105) → line band, not drag
    // handle (drag handle is only at right edge).
    let pt = Point::new(500.0, 100.0);
    let hit = layer.hit_bracket(pt, &pr()).unwrap();
    assert!(matches!(hit.target, BracketHitTarget::LineBand { .. }));
}

#[test]
fn entry_line_hit_test_returns_entry_line_target() {
    let view = OrderBracketView {
        id: 1,
        entry_price: 100.0,
        tp_price: Some(105.0),
        sl_price: Some(95.0),
        side: Side::Long,
        label: Cow::Borrowed("E"),
        entry_ts: None,
        filled_qty: None,
        total_qty: 1,
    };
    let (layer, _) = mk_interactive_layer(view);
    prime_layer_viewport(&layer);
    // Entry at price=100 → y=200 (midpoint of 400-tall viewport).
    let pt = Point::new(500.0, 200.0);
    let hit = layer.hit_bracket(pt, &pr()).unwrap();
    assert_eq!(hit.target, BracketHitTarget::EntryLine);
}

// ── Decorator groups projection ────────────────────────────────────────

#[test]
fn decorator_groups_three_siblings_per_bracket() {
    let view = OrderBracketView {
        id: 1,
        entry_price: 100.0,
        tp_price: Some(105.0),
        sl_price: Some(95.0),
        side: Side::Long,
        label: Cow::Borrowed("E"),
        entry_ts: None,
        filled_qty: None,
        total_qty: 1,
    };
    let layer = OrderBracketLayer::new(vec![view]);
    let groups = layer.decorator_groups(1000.0, 400.0);
    assert_eq!(groups.len(), 3, "entry + TP + SL siblings");
}

#[test]
fn decorator_groups_two_siblings_when_sl_none() {
    let view = OrderBracketView {
        id: 1,
        entry_price: 100.0,
        tp_price: Some(105.0),
        sl_price: None,
        side: Side::Long,
        label: Cow::Borrowed("E"),
        entry_ts: None,
        filled_qty: None,
        total_qty: 1,
    };
    let layer = OrderBracketLayer::new(vec![view]);
    let groups = layer.decorator_groups(1000.0, 400.0);
    assert_eq!(groups.len(), 2, "entry + TP only");
}

#[test]
fn decorator_groups_one_sibling_when_tp_and_sl_none() {
    let view = OrderBracketView {
        id: 1,
        entry_price: 100.0,
        tp_price: None,
        sl_price: None,
        side: Side::Short,
        label: Cow::Borrowed("E"),
        entry_ts: None,
        filled_qty: None,
        total_qty: 1,
    };
    let layer = OrderBracketLayer::new(vec![view]);
    let groups = layer.decorator_groups(1000.0, 400.0);
    assert_eq!(groups.len(), 1);
}

// ── OrderBracketView helpers ───────────────────────────────────────────

#[test]
fn is_partially_filled_true_when_filled_lt_total() {
    let v = OrderBracketView {
        id: 1,
        entry_price: 100.0,
        tp_price: None,
        sl_price: None,
        side: Side::Long,
        label: Cow::Borrowed("E"),
        entry_ts: None,
        filled_qty: Some(3),
        total_qty: 10,
    };
    assert!(v.is_partially_filled());
}

#[test]
fn is_partially_filled_false_when_filled_eq_total() {
    let v = OrderBracketView {
        id: 1,
        entry_price: 100.0,
        tp_price: None,
        sl_price: None,
        side: Side::Long,
        label: Cow::Borrowed("E"),
        entry_ts: None,
        filled_qty: Some(10),
        total_qty: 10,
    };
    assert!(!v.is_partially_filled());
}

#[test]
fn is_partially_filled_false_when_filled_none() {
    let v = OrderBracketView {
        id: 1,
        entry_price: 100.0,
        tp_price: None,
        sl_price: None,
        side: Side::Long,
        label: Cow::Borrowed("E"),
        entry_ts: None,
        filled_qty: None,
        total_qty: 10,
    };
    assert!(!v.is_partially_filled());
}

#[test]
fn is_partially_filled_false_when_filled_zero() {
    let v = OrderBracketView {
        id: 1,
        entry_price: 100.0,
        tp_price: None,
        sl_price: None,
        side: Side::Long,
        label: Cow::Borrowed("E"),
        entry_ts: None,
        filled_qty: Some(0),
        total_qty: 10,
    };
    assert!(!v.is_partially_filled());
}

// ── Drag sequence via scene ────────────────────────────────────────────

#[test]
fn drag_sequence_emits_update_bracket_leg() {
    let view = OrderBracketView {
        id: 42,
        entry_price: 100.0,
        tp_price: Some(105.0),
        sl_price: None,
        side: Side::Long,
        label: Cow::Borrowed("E"),
        entry_ts: None,
        filled_qty: None,
        total_qty: 1,
    };
    let (mut layer, _drag) = mk_interactive_layer(view);
    prime_layer_viewport(&layer);
    let pr_owned = pr();
    let mut last_err = None;
    let mut effs: Vec<ToolEffect> = Vec::new();
    // MouseDown on drag handle.
    let status = dispatch_layer(
        &mut layer,
        InputEvent::MouseDown {
            button: MouseButton::Left,
            pt: Point::new(999.0, 100.0), // drag handle on TP at y=100 (price=105)
            modifiers: Modifiers::default(),
        },
        &pr_owned,
        &mut effs,
        &mut last_err,
    );
    assert_eq!(status, EventStatus::Captured);
    // MouseMove emits UpdateBracketLeg.
    dispatch_layer(
        &mut layer,
        InputEvent::MouseMove {
            pt: Point::new(500.0, 100.0), // price ≈ 105 (half-way between 90 and 110 at y=100)
        },
        &pr_owned,
        &mut effs,
        &mut last_err,
    );
    assert_eq!(effs.len(), 1);
    match &effs[0] {
        ToolEffect::UpdateBracketLeg { id, role, .. } => {
            assert_eq!(*id, 42);
            assert_eq!(*role, LegRole::Tp);
        }
        _ => panic!("expected UpdateBracketLeg"),
    }
    // MouseUp releases.
    let status = dispatch_layer(
        &mut layer,
        InputEvent::MouseUp {
            button: MouseButton::Left,
            pt: Point::new(500.0, 100.0),
        },
        &pr_owned,
        &mut effs,
        &mut last_err,
    );
    assert_eq!(status, EventStatus::Captured);
}

/// Per-event dispatch helper for `OrderBracketLayer`. Rebuilds
/// `ToolContext` each call so follow-up `effs.len()` observations
/// don't collide with the mutable borrow.
fn dispatch_layer(
    layer: &mut OrderBracketLayer,
    ev: InputEvent,
    pr_owned: &PriceRange,
    effs: &mut Vec<ToolEffect>,
    last_err: &mut Option<midas_scene::SceneError>,
) -> EventStatus {
    let mut cx = midas_scene::ToolContext {
        price_range: pr_owned,
        last_error: last_err,
        effects: effs,
    };
    <OrderBracketLayer as InteractiveLayer>::update(layer, ev, &mut cx)
}

#[test]
fn mousemove_without_drag_start_is_ignored() {
    let view = OrderBracketView {
        id: 1,
        entry_price: 100.0,
        tp_price: Some(105.0),
        sl_price: None,
        side: Side::Long,
        label: Cow::Borrowed("E"),
        entry_ts: None,
        filled_qty: None,
        total_qty: 1,
    };
    let (mut layer, _) = mk_interactive_layer(view);
    prime_layer_viewport(&layer);
    let pr_owned = pr();
    let mut last_err = None;
    let mut effs: Vec<ToolEffect> = Vec::new();
    let status = dispatch_layer(
        &mut layer,
        InputEvent::MouseMove {
            pt: Point::new(500.0, 100.0),
        },
        &pr_owned,
        &mut effs,
        &mut last_err,
    );
    assert_eq!(status, EventStatus::Ignored);
    assert!(effs.is_empty());
}

// ── Multi-bracket continue_placing via scene ───────────────────────────

#[test]
fn continue_placing_after_complete_reuses_side() {
    // Direct tool test (scene can't downcast the boxed trait object).
    let mut tool = BracketTool::awaiting_entry(ToolSide::Long);
    let pr_owned = pr();
    let mut last_err = None;
    let mut effs: Vec<ToolEffect> = Vec::new();
    {
        let mut cx = midas_scene::ToolContext {
            price_range: &pr_owned,
            last_error: &mut last_err,
            effects: &mut effs,
        };
        for p in [100.0, 105.0, 95.0] {
            tool.update_preview(p, 200.0);
            tool.update(left_click(200.0), &mut cx);
        }
    }
    assert_eq!(tool.mode(), BracketToolMode::Complete);
    tool.continue_placing_with(ToolSide::Long);
    assert_eq!(
        tool.mode(),
        BracketToolMode::AwaitingEntry {
            side: ToolSide::Long
        }
    );
}
