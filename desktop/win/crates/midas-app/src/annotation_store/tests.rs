use super::*;
use midas_chart::levels::LevelIcon;
use midas_chart::widget::{level::LineStyle, level::LevelExtend, HorizontalLevel};

fn make_level(price: f64) -> AnnotationKind {
    AnnotationKind::Level(HorizontalLevel {
        price,
        color: [0.85, 0.85, 0.85, 0.8],
        line_width: 1.0,
        style: LineStyle::default(),
        label: None,
        extend: LevelExtend::default(),
        icon: LevelIcon::None,
    })
}

#[test]
fn new_store_is_empty() {
    let store = AnnotationStore::new();
    assert!(store.get("AAPL").is_empty());
    assert_eq!(store.generation("AAPL"), 0);
    assert_eq!(store.global_generation(), 0);
}

#[test]
fn add_returns_unique_ids() {
    let mut store = AnnotationStore::new();
    let id1 = store.add("AAPL", make_level(185.0));
    let id2 = store.add("AAPL", make_level(190.0));
    let id3 = store.add("MSFT", make_level(400.0));

    assert_ne!(id1, id2);
    assert_ne!(id2, id3);
    assert!(id1.is_valid());
}

#[test]
fn add_and_get() {
    let mut store = AnnotationStore::new();
    store.add("AAPL", make_level(185.0));
    store.add("AAPL", make_level(190.0));

    let anns = store.get("AAPL");
    assert_eq!(anns.len(), 2);
    assert!(store.get("MSFT").is_empty());
}

#[test]
fn add_bumps_generation() {
    let mut store = AnnotationStore::new();
    assert_eq!(store.generation("AAPL"), 0);

    store.add("AAPL", make_level(185.0));
    assert_eq!(store.generation("AAPL"), 1);
    assert_eq!(store.global_generation(), 1);

    store.add("AAPL", make_level(190.0));
    assert_eq!(store.generation("AAPL"), 2);
    assert_eq!(store.global_generation(), 2);

    // Other symbols unaffected.
    assert_eq!(store.generation("MSFT"), 0);
}

#[test]
fn remove_returns_true_when_found() {
    let mut store = AnnotationStore::new();
    let id = store.add("AAPL", make_level(185.0));
    let gen_before = store.generation("AAPL");

    assert!(store.remove("AAPL", id));
    assert!(store.get("AAPL").is_empty());
    assert!(store.generation("AAPL") > gen_before);
}

#[test]
fn remove_returns_false_when_not_found() {
    let mut store = AnnotationStore::new();
    store.add("AAPL", make_level(185.0));
    let gen_before = store.generation("AAPL");

    assert!(!store.remove("AAPL", AnnotationId(999)));
    assert!(!store.remove("MSFT", AnnotationId(1)));
    assert_eq!(store.generation("AAPL"), gen_before);
}

#[test]
fn update_via_closure() {
    let mut store = AnnotationStore::new();
    let id = store.add("AAPL", make_level(185.0));

    let found = store.update("AAPL", id, |ann| {
        if let AnnotationKind::Level(ref mut level) = ann.kind {
            level.price = 190.0;
        }
    });
    assert!(found);

    let ann = &store.get("AAPL")[0];
    match &ann.kind {
        AnnotationKind::Level(level) => {
            assert!((level.price - 190.0).abs() < f64::EPSILON);
        }
        _ => panic!("expected Level variant"),
    }
}

#[test]
fn update_nonexistent_returns_false() {
    let mut store = AnnotationStore::new();
    store.add("AAPL", make_level(185.0));

    assert!(!store.update("AAPL", AnnotationId(999), |_| {}));
    assert!(!store.update("MSFT", AnnotationId(1), |_| {}));
}

#[test]
fn clear_removes_all() {
    let mut store = AnnotationStore::new();
    store.add("AAPL", make_level(185.0));
    store.add("AAPL", make_level(190.0));
    let gen_before = store.generation("AAPL");

    store.clear("AAPL");
    assert!(store.get("AAPL").is_empty());
    assert!(store.generation("AAPL") > gen_before);
}

#[test]
fn clear_empty_is_noop() {
    let mut store = AnnotationStore::new();
    store.clear("AAPL");
    assert_eq!(store.generation("AAPL"), 0);
    assert_eq!(store.global_generation(), 0);
}

#[test]
fn find_across_symbols() {
    let mut store = AnnotationStore::new();
    let id1 = store.add("AAPL", make_level(185.0));
    let id2 = store.add("MSFT", make_level(400.0));

    let (sym, ann) = store.find(id2).unwrap();
    assert_eq!(sym, "MSFT");
    assert_eq!(ann.id, id2);

    let (sym, ann) = store.find(id1).unwrap();
    assert_eq!(sym, "AAPL");
    assert_eq!(ann.id, id1);

    assert!(store.find(AnnotationId(999)).is_none());
}

#[test]
fn get_visible_filters_by_timeframe() {
    let mut store = AnnotationStore::new();
    let id_all = store.add("AAPL", make_level(185.0));
    let id_m5 = store.add("AAPL", make_level(190.0));

    // Restrict second annotation to M5 only.
    store.update("AAPL", id_m5, |ann| {
        ann.visible_timeframes = Some(vec![Timeframe::M5]);
    });

    let m5_visible = store.get_visible("AAPL", Timeframe::M5);
    assert_eq!(m5_visible.len(), 2);

    let d1_visible = store.get_visible("AAPL", Timeframe::D1);
    assert_eq!(d1_visible.len(), 1);
    assert_eq!(d1_visible[0].id, id_all);
}

#[test]
fn get_visible_excludes_hidden() {
    let mut store = AnnotationStore::new();
    let id = store.add("AAPL", make_level(185.0));

    store.update("AAPL", id, |ann| {
        ann.presence = Presence::Hidden;
    });

    let visible = store.get_visible("AAPL", Timeframe::D1);
    assert!(visible.is_empty());
}

#[test]
fn retain_removes_matching() {
    let mut store = AnnotationStore::new();
    store.add("AAPL", make_level(185.0));
    store.add("AAPL", make_level(190.0));
    store.add("AAPL", make_level(195.0));

    let removed = store.retain("AAPL", |ann| match &ann.kind {
        AnnotationKind::Level(level) => level.price > 188.0,
        _ => true,
    });
    assert_eq!(removed, 1);
    assert_eq!(store.get("AAPL").len(), 2);
}

#[test]
fn symbol_key_normalizes_to_uppercase() {
    let mut store = AnnotationStore::new();
    store.add("aapl", make_level(185.0));
    // Both cases find the same entry because get() normalizes.
    assert_eq!(store.get("AAPL").len(), 1);
    assert_eq!(store.get("aapl").len(), 1);
    assert_eq!(store.get("Aapl").len(), 1);
}

#[test]
fn symbols_iterator() {
    let mut store = AnnotationStore::new();
    store.add("AAPL", make_level(185.0));
    store.add("MSFT", make_level(400.0));

    let mut syms: Vec<&str> = store.symbols().collect();
    syms.sort();
    assert_eq!(syms, vec!["AAPL", "MSFT"]);
}
