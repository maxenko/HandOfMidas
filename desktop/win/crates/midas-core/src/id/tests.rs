use super::*;
use std::collections::HashSet;

#[test]
fn chart_id_equality() {
    assert_eq!(ChartId::new(1), ChartId::new(1));
    assert_ne!(ChartId::new(1), ChartId::new(2));
}

#[test]
fn pane_id_equality() {
    assert_eq!(PaneId::new(10), PaneId::new(10));
    assert_ne!(PaneId::new(10), PaneId::new(20));
}

#[test]
fn chart_id_hash() {
    let mut set = HashSet::new();
    set.insert(ChartId::new(1));
    set.insert(ChartId::new(2));
    set.insert(ChartId::new(1)); // duplicate
    assert_eq!(set.len(), 2);
}

#[test]
fn pane_id_hash() {
    let mut set = HashSet::new();
    set.insert(PaneId::new(100));
    set.insert(PaneId::new(200));
    set.insert(PaneId::new(100)); // duplicate
    assert_eq!(set.len(), 2);
}

#[test]
fn chart_id_display() {
    assert_eq!(ChartId::new(7).to_string(), "Chart(7)");
}

#[test]
fn pane_id_display() {
    assert_eq!(PaneId::new(42).to_string(), "Pane(42)");
}

#[test]
fn watchlist_id_equality() {
    assert_eq!(WatchlistId::new(1), WatchlistId::new(1));
    assert_ne!(WatchlistId::new(1), WatchlistId::new(2));
}

#[test]
fn watchlist_id_hash() {
    let mut set = HashSet::new();
    set.insert(WatchlistId::new(3));
    set.insert(WatchlistId::new(4));
    set.insert(WatchlistId::new(3)); // duplicate
    assert_eq!(set.len(), 2);
}

#[test]
fn watchlist_id_display() {
    assert_eq!(WatchlistId::new(5).to_string(), "Watchlist(5)");
}

#[test]
fn order_panel_id_equality() {
    assert_eq!(OrderPanelId::new(1), OrderPanelId::new(1));
    assert_ne!(OrderPanelId::new(1), OrderPanelId::new(2));
}

#[test]
fn order_panel_id_hash() {
    let mut set = HashSet::new();
    set.insert(OrderPanelId::new(3));
    set.insert(OrderPanelId::new(4));
    set.insert(OrderPanelId::new(3)); // duplicate
    assert_eq!(set.len(), 2);
}

#[test]
fn order_panel_id_display() {
    assert_eq!(OrderPanelId::new(5).to_string(), "Order(5)");
}

#[test]
fn ordering() {
    assert!(ChartId::new(1) < ChartId::new(2));
    assert!(PaneId::new(10) < PaneId::new(20));
    assert!(WatchlistId::new(1) < WatchlistId::new(2));
    assert!(OrderPanelId::new(1) < OrderPanelId::new(2));
}

#[test]
fn copy_semantics() {
    let a = ChartId::new(5);
    let b = a; // Copy
    assert_eq!(a, b); // `a` is still valid

    let w = WatchlistId::new(3);
    let w2 = w;
    assert_eq!(w, w2);

    let o = OrderPanelId::new(7);
    let o2 = o;
    assert_eq!(o, o2);
}

#[test]
fn serde_roundtrip() {
    let id = ChartId::new(42);
    let json = serde_json::to_string(&id).unwrap();
    let back: ChartId = serde_json::from_str(&json).unwrap();
    assert_eq!(id, back);

    let pid = PaneId::new(999);
    let json = serde_json::to_string(&pid).unwrap();
    let back: PaneId = serde_json::from_str(&json).unwrap();
    assert_eq!(pid, back);

    let wid = WatchlistId::new(11);
    let json = serde_json::to_string(&wid).unwrap();
    let back: WatchlistId = serde_json::from_str(&json).unwrap();
    assert_eq!(wid, back);

    let oid = OrderPanelId::new(99);
    let json = serde_json::to_string(&oid).unwrap();
    let back: OrderPanelId = serde_json::from_str(&json).unwrap();
    assert_eq!(oid, back);
}
