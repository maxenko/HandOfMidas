use super::*;
use crate::app::panel_ids::PanelIdAllocator;

/// Build a fresh allocator for each test. Layout-level tests treat
/// the allocator as a black-box monotonic source.
fn alloc() -> PanelIdAllocator {
    PanelIdAllocator::default()
}

#[test]
fn single_layout_has_one_pane() {
    let mut a = alloc();
    let (layout, chart_id) = WorkspaceLayout::single(&mut a);
    assert_eq!(layout.pane_count(), 1);
    assert_eq!(layout.focused_chart_id(), Some(chart_id));
}

#[test]
fn split_creates_two_panes() {
    let mut a = alloc();
    let (mut layout, _first_id) = WorkspaceLayout::single(&mut a);
    let first_pane = layout.focus.unwrap();

    let next_id = a.next_chart();
    let result = layout.split(pane_grid::Axis::Vertical, first_pane, next_id);
    assert!(result.is_some());
    assert_eq!(layout.pane_count(), 2);
}

#[test]
fn close_removes_pane() {
    let mut a = alloc();
    let (mut layout, _first_id) = WorkspaceLayout::single(&mut a);
    let first_pane = layout.focus.unwrap();

    let (new_id, new_pane) = layout
        .split(pane_grid::Axis::Vertical, first_pane, a.next_chart())
        .unwrap();

    let closed = layout.close(new_pane);
    assert!(matches!(closed, Some(PanelContent::Chart(id)) if id == new_id));
    assert_eq!(layout.pane_count(), 1);
}

#[test]
fn cannot_close_last_pane() {
    let mut a = alloc();
    let (mut layout, _first_id) = WorkspaceLayout::single(&mut a);
    let first_pane = layout.focus.unwrap();

    let closed = layout.close(first_pane);
    assert!(closed.is_none());
    assert_eq!(layout.pane_count(), 1);
}

#[test]
fn preset_single_resets_to_one_pane() {
    let mut a = alloc();
    let (mut layout, _) = WorkspaceLayout::single(&mut a);
    let first_pane = layout.focus.unwrap();
    layout.split(pane_grid::Axis::Vertical, first_pane, a.next_chart());
    assert_eq!(layout.pane_count(), 2);

    let ids = layout.apply_preset(&LayoutPresetKind::Single, &mut a);
    assert_eq!(ids.len(), 1);
    assert_eq!(layout.pane_count(), 1);
}

#[test]
fn preset_split_h_creates_two_panes() {
    let mut a = alloc();
    let (mut layout, _) = WorkspaceLayout::single(&mut a);
    let ids = layout.apply_preset(&LayoutPresetKind::SplitH, &mut a);
    assert_eq!(ids.len(), 2);
    assert_eq!(layout.pane_count(), 2);
}

#[test]
fn preset_split_v_creates_two_panes() {
    let mut a = alloc();
    let (mut layout, _) = WorkspaceLayout::single(&mut a);
    let ids = layout.apply_preset(&LayoutPresetKind::SplitV, &mut a);
    assert_eq!(ids.len(), 2);
    assert_eq!(layout.pane_count(), 2);
}

#[test]
fn preset_grid_2x2_creates_four_panes() {
    let mut a = alloc();
    let (mut layout, _) = WorkspaceLayout::single(&mut a);
    let ids = layout.apply_preset(&LayoutPresetKind::Grid2x2, &mut a);
    assert_eq!(ids.len(), 4);
    assert_eq!(layout.pane_count(), 4);
}

#[test]
fn focus_tracks_across_split() {
    let mut a = alloc();
    let (mut layout, first_id) = WorkspaceLayout::single(&mut a);
    assert_eq!(layout.focused_chart_id(), Some(first_id));

    let first_pane = layout.focus.unwrap();
    let (new_id, new_pane) = layout
        .split(pane_grid::Axis::Horizontal, first_pane, a.next_chart())
        .unwrap();

    // Focus should still be on the original pane after split.
    assert_eq!(layout.focused_chart_id(), Some(first_id));

    // Switch focus to the new pane.
    layout.set_focus(new_pane);
    assert_eq!(layout.focused_chart_id(), Some(new_id));
}

#[test]
fn find_pane_by_chart_id() {
    let mut a = alloc();
    let (layout, first_id) = WorkspaceLayout::single(&mut a);
    let found = layout.find_pane(first_id);
    assert!(found.is_some());
    assert_eq!(found.unwrap(), layout.focus.unwrap());
}

#[test]
fn chart_ids_returns_all() {
    let mut a = alloc();
    let (mut layout, _) = WorkspaceLayout::single(&mut a);
    let ids = layout.apply_preset(&LayoutPresetKind::Grid2x2, &mut a);
    let all = layout.chart_ids();
    assert_eq!(all.len(), 4);
    for id in &ids {
        assert!(all.contains(id));
    }
}

#[test]
fn next_chart_id_is_monotonic() {
    let mut a = alloc();
    let id_a = a.next_chart();
    let id_b = a.next_chart();
    assert!(id_b.0 > id_a.0);
}

#[test]
fn watchlist_pane_in_layout() {
    let mut a = alloc();
    let (mut layout, _first_id) = WorkspaceLayout::single(&mut a);
    let first_pane = layout.focus.unwrap();

    // Split to get a new pane, then manually replace it with a watchlist.
    let (_chart_id, new_pane) = layout
        .split(pane_grid::Axis::Vertical, first_pane, a.next_chart())
        .unwrap();
    let wl_id = a.next_watchlist();
    if let Some(state) = layout.panes.get_mut(new_pane) {
        *state = PaneState::watchlist(wl_id);
    }

    // find_watchlist_pane should locate it.
    assert_eq!(layout.find_watchlist_pane(wl_id), Some(new_pane));

    // chart_ids should NOT include the watchlist pane.
    assert_eq!(layout.chart_ids().len(), 1);

    // Closing the watchlist pane returns Watchlist content.
    let closed = layout.close(new_pane);
    assert!(matches!(closed, Some(PanelContent::Watchlist(id)) if id == wl_id));
}

#[test]
fn focused_chart_id_returns_none_for_watchlist() {
    let mut a = alloc();
    let (mut layout, _first_id) = WorkspaceLayout::single(&mut a);
    let first_pane = layout.focus.unwrap();

    // Replace the only pane with a watchlist.
    let wl_id = a.next_watchlist();
    if let Some(state) = layout.panes.get_mut(first_pane) {
        *state = PaneState::watchlist(wl_id);
        state.is_focused = true;
    }

    assert_eq!(layout.focused_chart_id(), None);
}

#[test]
fn split_on_watchlist_pane_creates_chart() {
    let mut a = alloc();
    let (mut layout, _first_id) = WorkspaceLayout::single(&mut a);
    let first_pane = layout.focus.unwrap();

    // Replace the pane with a watchlist.
    let wl_id = a.next_watchlist();
    if let Some(state) = layout.panes.get_mut(first_pane) {
        *state = PaneState::watchlist(wl_id);
    }

    // Splitting should create a chart, not a watchlist.
    let next_id = a.next_chart();
    let result = layout.split(pane_grid::Axis::Vertical, first_pane, next_id);
    assert!(result.is_some());
    let (new_chart_id, new_pane) = result.unwrap();
    let new_state = layout.panes.get(new_pane).unwrap();
    assert!(matches!(new_state.content, PanelContent::Chart(id) if id == new_chart_id));
}

#[test]
fn apply_preset_produces_only_chart_panes() {
    let mut a = alloc();
    let (mut layout, _) = WorkspaceLayout::single(&mut a);
    let first_pane = layout.focus.unwrap();

    // Add a watchlist pane.
    let (_cid, new_pane) = layout
        .split(pane_grid::Axis::Vertical, first_pane, a.next_chart())
        .unwrap();
    let wl_id = a.next_watchlist();
    if let Some(state) = layout.panes.get_mut(new_pane) {
        *state = PaneState::watchlist(wl_id);
    }

    // Apply a preset — should produce only chart panes.
    let ids = layout.apply_preset(&LayoutPresetKind::Grid2x2, &mut a);
    assert_eq!(ids.len(), 4);
    // Every pane should be a chart.
    for state in layout.panes.panes.values() {
        assert!(matches!(state.content, PanelContent::Chart(_)));
    }
    // No watchlist panes should survive.
    assert!(layout.find_watchlist_pane(wl_id).is_none());
}

// Pre-A1 audit additions (Risk #11): lock in invariants the multi-window
// refactor's mechanical churn must not break.

#[test]
fn closing_focused_pane_moves_focus_to_sibling() {
    // The `if self.focus == Some(pane)` branch in `close()` reassigns
    // focus to the sibling so the layout never ends up with `focus = None`
    // while panes remain. A2's churn rewrites this around a deeper field
    // path; this test pins the post-condition.
    let mut a = alloc();
    let (mut layout, first_id) = WorkspaceLayout::single(&mut a);
    let first_pane = layout.focus.unwrap();
    let (new_id, new_pane) = layout
        .split(pane_grid::Axis::Vertical, first_pane, a.next_chart())
        .unwrap();

    // Move focus to the new pane, then close it.
    layout.set_focus(new_pane);
    assert_eq!(layout.focused_chart_id(), Some(new_id));

    let closed = layout.close(new_pane);
    assert!(matches!(closed, Some(PanelContent::Chart(id)) if id == new_id));
    // Focus migrated to the surviving sibling (which holds first_id).
    assert_eq!(layout.focused_chart_id(), Some(first_id));
    assert!(layout.focus.is_some());
}

#[test]
fn close_returns_order_panel_content() {
    // PaneClose handler removes from `order_panels` HashMap when
    // close() returns Order(_). Make sure the variant round-trips.
    let mut a = alloc();
    let (mut layout, _) = WorkspaceLayout::single(&mut a);
    let first_pane = layout.focus.unwrap();

    let (_cid, new_pane) = layout
        .split(pane_grid::Axis::Vertical, first_pane, a.next_chart())
        .unwrap();
    let op_id = a.next_order_panel();
    if let Some(state) = layout.panes.get_mut(new_pane) {
        *state = PaneState::order(op_id);
    }

    assert_eq!(layout.find_order_pane(op_id), Some(new_pane));
    let closed = layout.close(new_pane);
    assert!(matches!(closed, Some(PanelContent::Order(id)) if id == op_id));
}

#[test]
fn close_returns_account_panel_content() {
    let mut a = alloc();
    let (mut layout, _) = WorkspaceLayout::single(&mut a);
    let first_pane = layout.focus.unwrap();

    let (_cid, new_pane) = layout
        .split(pane_grid::Axis::Vertical, first_pane, a.next_chart())
        .unwrap();
    let ap_id = a.next_account_panel();
    if let Some(state) = layout.panes.get_mut(new_pane) {
        *state = PaneState::account(ap_id);
    }

    let closed = layout.close(new_pane);
    assert!(matches!(closed, Some(PanelContent::Account(id)) if id == ap_id));
}

#[test]
fn split_close_split_keeps_chart_id_monotonic() {
    // Risk: a refactor that resets `next_chart_id` on close (e.g. trying
    // to "compact" IDs) would silently let a closed-then-resplit ID
    // collide with an annotation-store key. Pin monotonicity across the
    // mutation cycle.
    let mut a = alloc();
    let (mut layout, first_id) = WorkspaceLayout::single(&mut a);
    let first_pane = layout.focus.unwrap();

    let (id_a, pane_a) = layout
        .split(pane_grid::Axis::Vertical, first_pane, a.next_chart())
        .unwrap();
    let (id_b, _pane_b) = layout
        .split(pane_grid::Axis::Horizontal, pane_a, a.next_chart())
        .unwrap();
    assert!(id_a.0 > first_id.0);
    assert!(id_b.0 > id_a.0);

    layout.close(pane_a);

    // Re-split — new id MUST exceed every previously-seen id, including
    // the one we just closed.
    let (id_c, _pane_c) = layout
        .split(pane_grid::Axis::Vertical, first_pane, a.next_chart())
        .unwrap();
    assert!(id_c.0 > id_b.0);
    assert!(id_c.0 > id_a.0);
}

#[test]
fn pane_count_tracks_split_close_sequence() {
    // Belt-and-braces: a long mutation sequence walks the layout through
    // the same shapes the handler dispatches end-to-end. Catches any
    // off-by-one a `windows[main].layout` rewrite might introduce.
    let mut a = alloc();
    let (mut layout, _) = WorkspaceLayout::single(&mut a);
    let first_pane = layout.focus.unwrap();
    assert_eq!(layout.pane_count(), 1);

    let (_id_a, pane_a) = layout
        .split(pane_grid::Axis::Vertical, first_pane, a.next_chart())
        .unwrap();
    assert_eq!(layout.pane_count(), 2);

    let (_id_b, pane_b) = layout
        .split(pane_grid::Axis::Horizontal, pane_a, a.next_chart())
        .unwrap();
    assert_eq!(layout.pane_count(), 3);

    layout.close(pane_b);
    assert_eq!(layout.pane_count(), 2);

    layout.close(pane_a);
    assert_eq!(layout.pane_count(), 1);

    // Last pane refuses to close.
    assert!(layout.close(first_pane).is_none());
    assert_eq!(layout.pane_count(), 1);
}
