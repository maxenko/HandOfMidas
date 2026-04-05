use super::*;

#[test]
fn single_layout_has_one_pane() {
    let (layout, chart_id) = WorkspaceLayout::single();
    assert_eq!(layout.pane_count(), 1);
    assert_eq!(layout.focused_chart_id(), Some(chart_id));
}

#[test]
fn split_creates_two_panes() {
    let (mut layout, _first_id) = WorkspaceLayout::single();
    let first_pane = layout.focus.unwrap();

    let result = layout.split(pane_grid::Axis::Vertical, first_pane);
    assert!(result.is_some());
    assert_eq!(layout.pane_count(), 2);
}

#[test]
fn close_removes_pane() {
    let (mut layout, _first_id) = WorkspaceLayout::single();
    let first_pane = layout.focus.unwrap();

    let (new_id, new_pane) = layout.split(pane_grid::Axis::Vertical, first_pane).unwrap();

    let closed = layout.close(new_pane);
    assert!(matches!(closed, Some(PanelContent::Chart(id)) if id == new_id));
    assert_eq!(layout.pane_count(), 1);
}

#[test]
fn cannot_close_last_pane() {
    let (mut layout, _first_id) = WorkspaceLayout::single();
    let first_pane = layout.focus.unwrap();

    let closed = layout.close(first_pane);
    assert!(closed.is_none());
    assert_eq!(layout.pane_count(), 1);
}

#[test]
fn preset_single_resets_to_one_pane() {
    let (mut layout, _) = WorkspaceLayout::single();
    let first_pane = layout.focus.unwrap();
    layout.split(pane_grid::Axis::Vertical, first_pane);
    assert_eq!(layout.pane_count(), 2);

    let ids = layout.apply_preset(&LayoutPresetKind::Single);
    assert_eq!(ids.len(), 1);
    assert_eq!(layout.pane_count(), 1);
}

#[test]
fn preset_split_h_creates_two_panes() {
    let (mut layout, _) = WorkspaceLayout::single();
    let ids = layout.apply_preset(&LayoutPresetKind::SplitH);
    assert_eq!(ids.len(), 2);
    assert_eq!(layout.pane_count(), 2);
}

#[test]
fn preset_split_v_creates_two_panes() {
    let (mut layout, _) = WorkspaceLayout::single();
    let ids = layout.apply_preset(&LayoutPresetKind::SplitV);
    assert_eq!(ids.len(), 2);
    assert_eq!(layout.pane_count(), 2);
}

#[test]
fn preset_grid_2x2_creates_four_panes() {
    let (mut layout, _) = WorkspaceLayout::single();
    let ids = layout.apply_preset(&LayoutPresetKind::Grid2x2);
    assert_eq!(ids.len(), 4);
    assert_eq!(layout.pane_count(), 4);
}

#[test]
fn focus_tracks_across_split() {
    let (mut layout, first_id) = WorkspaceLayout::single();
    assert_eq!(layout.focused_chart_id(), Some(first_id));

    let first_pane = layout.focus.unwrap();
    let (new_id, new_pane) = layout
        .split(pane_grid::Axis::Horizontal, first_pane)
        .unwrap();

    // Focus should still be on the original pane after split.
    assert_eq!(layout.focused_chart_id(), Some(first_id));

    // Switch focus to the new pane.
    layout.set_focus(new_pane);
    assert_eq!(layout.focused_chart_id(), Some(new_id));
}

#[test]
fn find_pane_by_chart_id() {
    let (layout, first_id) = WorkspaceLayout::single();
    let found = layout.find_pane(first_id);
    assert!(found.is_some());
    assert_eq!(found.unwrap(), layout.focus.unwrap());
}

#[test]
fn chart_ids_returns_all() {
    let (mut layout, _) = WorkspaceLayout::single();
    let ids = layout.apply_preset(&LayoutPresetKind::Grid2x2);
    let all = layout.chart_ids();
    assert_eq!(all.len(), 4);
    for id in &ids {
        assert!(all.contains(id));
    }
}

#[test]
fn next_chart_id_is_monotonic() {
    let (mut layout, _) = WorkspaceLayout::single();
    let a = layout.next_chart_id();
    let b = layout.next_chart_id();
    assert!(b.0 > a.0);
}

#[test]
fn watchlist_pane_in_layout() {
    let (mut layout, _first_id) = WorkspaceLayout::single();
    let first_pane = layout.focus.unwrap();

    // Split to get a new pane, then manually replace it with a watchlist.
    let (_chart_id, new_pane) =
        layout.split(pane_grid::Axis::Vertical, first_pane).unwrap();
    let wl_id = layout.next_watchlist_id();
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
    let (mut layout, _first_id) = WorkspaceLayout::single();
    let first_pane = layout.focus.unwrap();

    // Replace the only pane with a watchlist.
    let wl_id = layout.next_watchlist_id();
    if let Some(state) = layout.panes.get_mut(first_pane) {
        *state = PaneState::watchlist(wl_id);
        state.is_focused = true;
    }

    assert_eq!(layout.focused_chart_id(), None);
}

#[test]
fn split_on_watchlist_pane_creates_chart() {
    let (mut layout, _first_id) = WorkspaceLayout::single();
    let first_pane = layout.focus.unwrap();

    // Replace the pane with a watchlist.
    let wl_id = layout.next_watchlist_id();
    if let Some(state) = layout.panes.get_mut(first_pane) {
        *state = PaneState::watchlist(wl_id);
    }

    // Splitting should create a chart, not a watchlist.
    let result = layout.split(pane_grid::Axis::Vertical, first_pane);
    assert!(result.is_some());
    let (new_chart_id, new_pane) = result.unwrap();
    let new_state = layout.panes.get(new_pane).unwrap();
    assert!(matches!(new_state.content, PanelContent::Chart(id) if id == new_chart_id));
}

#[test]
fn apply_preset_produces_only_chart_panes() {
    let (mut layout, _) = WorkspaceLayout::single();
    let first_pane = layout.focus.unwrap();

    // Add a watchlist pane.
    let (_cid, new_pane) = layout.split(pane_grid::Axis::Vertical, first_pane).unwrap();
    let wl_id = layout.next_watchlist_id();
    if let Some(state) = layout.panes.get_mut(new_pane) {
        *state = PaneState::watchlist(wl_id);
    }

    // Apply a preset — should produce only chart panes.
    let ids = layout.apply_preset(&LayoutPresetKind::Grid2x2);
    assert_eq!(ids.len(), 4);
    // Every pane should be a chart.
    for state in layout.panes.panes.values() {
        assert!(matches!(state.content, PanelContent::Chart(_)));
    }
    // No watchlist panes should survive.
    assert!(layout.find_watchlist_pane(wl_id).is_none());
}
