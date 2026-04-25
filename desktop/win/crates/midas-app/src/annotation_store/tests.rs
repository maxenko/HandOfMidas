use super::*;
use midas_annotation_types::price_line::{LineExtent, LineStroke, PriceLine};
use midas_annotation_types::{HorizontalLevel, LevelIcon, LineStyle};
use midas_core::config::LevelConfig;

fn make_level(price: f64) -> AnnotationKind {
    AnnotationKind::Level(HorizontalLevel {
        id: 0,
        line: PriceLine {
            price,
            extent: LineExtent::default(),
            stroke: LineStroke {
                color: [0.85, 0.85, 0.85, 0.8],
                width: 1.0,
                style: LineStyle::default(),
            },
        },
        label: None,
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
            level.line.price = 190.0;
        }
    });
    assert!(found);

    let ann = &store.get("AAPL")[0];
    match &ann.kind {
        AnnotationKind::Level(level) => {
            assert!((level.line.price - 190.0).abs() < f64::EPSILON);
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
        AnnotationKind::Level(level) => level.line.price > 188.0,
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

// ── Level helpers (folded from the retired LevelStore, P2b) ─────────

fn make_stored_level(id: u64, price: f64) -> StoredLevel {
    StoredLevel {
        level: HorizontalLevel {
            id,
            line: PriceLine {
                price,
                extent: LineExtent::default(),
                stroke: LineStroke {
                    color: [0.85, 0.85, 0.85, 0.8],
                    width: 1.0,
                    style: LineStyle::Solid,
                },
            },
            label: None,
            icon: LevelIcon::None,
        },
        locked: false,
    }
}

#[test]
fn alloc_level_id_is_monotonic() {
    let mut store = AnnotationStore::new();
    let a = store.alloc_level_id();
    let b = store.alloc_level_id();
    let c = store.alloc_level_id();
    assert_eq!(b, a + 1);
    assert_eq!(c, b + 1);
}

#[test]
fn add_level_and_query() {
    let mut store = AnnotationStore::new();
    let id = store.alloc_level_id();
    store.add_level("AAPL", make_stored_level(id, 185.50));

    let levels = store.levels_for("AAPL");
    assert_eq!(levels.len(), 1);
    assert!((levels[0].line.price - 185.50).abs() < f64::EPSILON);
    assert!(store.levels_for("MSFT").is_empty());
}

#[test]
fn add_level_bumps_generation() {
    let mut store = AnnotationStore::new();
    assert_eq!(store.generation("AAPL"), 0);

    let id1 = store.alloc_level_id();
    store.add_level("AAPL", make_stored_level(id1, 100.0));
    assert_eq!(store.generation("AAPL"), 1);

    let id2 = store.alloc_level_id();
    store.add_level("AAPL", make_stored_level(id2, 200.0));
    assert_eq!(store.generation("AAPL"), 2);

    // Other tickers unaffected.
    assert_eq!(store.generation("MSFT"), 0);
}

#[test]
fn remove_level_returns_true_when_found() {
    let mut store = AnnotationStore::new();
    let id1 = store.alloc_level_id();
    store.add_level("AAPL", make_stored_level(id1, 100.0));
    let id2 = store.alloc_level_id();
    store.add_level("AAPL", make_stored_level(id2, 200.0));
    let gen_before = store.generation("AAPL");

    assert!(store.remove_level("AAPL", id1));
    assert_eq!(store.levels_for("AAPL").len(), 1);
    assert!(store.generation("AAPL") > gen_before);
}

#[test]
fn remove_nonexistent_level_returns_false() {
    let mut store = AnnotationStore::new();
    let id = store.alloc_level_id();
    store.add_level("AAPL", make_stored_level(id, 100.0));
    let gen_before = store.generation("AAPL");

    assert!(!store.remove_level("AAPL", 999));
    assert!(!store.remove_level("MSFT", id));
    assert_eq!(store.generation("AAPL"), gen_before);
}

#[test]
fn clear_levels_empties_only_levels() {
    use midas_annotation_types::order_bracket::{
        BracketLeg, BracketSide, BracketStatus, EntryType, LegRole, OrderBracket,
    };
    let mut store = AnnotationStore::new();
    let lid = store.alloc_level_id();
    store.add_level("AAPL", make_stored_level(lid, 100.0));

    // Add a non-level annotation (an OrderBracket); it must survive
    // `clear_levels` which only strips `AnnotationKind::Level`.
    let bracket = OrderBracket {
        entry: BracketLeg {
            line: PriceLine {
                price: 185.0,
                extent: LineExtent::default(),
                stroke: LineStroke {
                    color: [1.0; 4],
                    width: 1.0,
                    style: LineStyle::Solid,
                },
            },
            role: LegRole::Entry,
            projected_pnl: None,
            projected_pnl_pct: None,
        },
        take_profit: None,
        stop_loss: None,
        side: BracketSide::Long,
        status: BracketStatus::Draft,
        quantity: None,
        saved: false,
        filled_qty: None,
        entry_type: EntryType::Market,
        entry_stop_price: None,
        wrong_side_warning: false,
    };
    let bracket_id = store.add("AAPL", AnnotationKind::OrderBracket(Box::new(bracket)));
    let gen_before = store.generation("AAPL");

    store.clear_levels("AAPL");
    assert!(store.levels_for("AAPL").is_empty());
    // Non-level annotation preserved.
    assert_eq!(store.get("AAPL").len(), 1);
    assert_eq!(store.get("AAPL")[0].id, bracket_id);
    assert!(store.generation("AAPL") > gen_before);
}

#[test]
fn clear_empty_levels_is_noop() {
    let mut store = AnnotationStore::new();
    store.clear_levels("AAPL");
    assert_eq!(store.generation("AAPL"), 0);
}

#[test]
fn find_level_across_tickers() {
    let mut store = AnnotationStore::new();
    let id1 = store.alloc_level_id();
    store.add_level("AAPL", make_stored_level(id1, 100.0));
    let id2 = store.alloc_level_id();
    store.add_level("MSFT", make_stored_level(id2, 400.0));

    let (ticker, level) = store.find_level(id2).unwrap();
    assert_eq!(ticker, "MSFT");
    assert!((level.line.price - 400.0).abs() < f64::EPSILON);

    assert!(store.find_level(999).is_none());
}

#[test]
fn update_level_mutates_geometry_and_lock() {
    let mut store = AnnotationStore::new();
    let id = store.alloc_level_id();
    store.add_level("AAPL", make_stored_level(id, 100.0));

    let found = store.update_level("AAPL", id, |level, locked| {
        level.line.price = 150.0;
        *locked = true;
    });
    assert!(found);

    let levels = store.levels_for("AAPL");
    assert!((levels[0].line.price - 150.0).abs() < f64::EPSILON);
    assert!(levels[0].locked);
}

#[test]
fn update_level_wrong_ticker_returns_false() {
    let mut store = AnnotationStore::new();
    let id = store.alloc_level_id();
    store.add_level("AAPL", make_stored_level(id, 100.0));
    assert!(!store.update_level("MSFT", id, |_, _| {}));
}

#[test]
fn level_config_round_trip() {
    let mut store = AnnotationStore::new();
    let id1 = store.alloc_level_id();
    let mut entry1 = make_stored_level(id1, 185.50);
    entry1.level.label = Some("Support".into());
    entry1.level.icon = LevelIcon::ArrowUp;
    entry1.locked = true;
    store.add_level("AAPL", entry1);

    let id2 = store.alloc_level_id();
    store.add_level("MSFT", make_stored_level(id2, 420.0));

    let config = store.to_level_configs();
    assert_eq!(config.len(), 2);
    assert_eq!(config["AAPL"].len(), 1);
    assert!((config["AAPL"][0].price - 185.50).abs() < f64::EPSILON);
    assert_eq!(config["AAPL"][0].label.as_deref(), Some("Support"));
    assert_eq!(config["AAPL"][0].icon, "arrow_up");
    assert!(config["AAPL"][0].locked);

    let mut restored = AnnotationStore::new();
    restored.import_level_configs(&config);
    assert_eq!(restored.levels_for("AAPL").len(), 1);
    assert!((restored.levels_for("AAPL")[0].line.price - 185.50).abs() < f64::EPSILON);
    assert_eq!(
        restored.levels_for("AAPL")[0].label.as_deref(),
        Some("Support")
    );
    assert_eq!(restored.levels_for("AAPL")[0].icon, LevelIcon::ArrowUp);
    assert!(restored.levels_for("AAPL")[0].locked);
    assert_eq!(restored.levels_for("MSFT").len(), 1);
}

#[test]
fn empty_symbols_not_serialized() {
    let mut store = AnnotationStore::new();
    let id = store.alloc_level_id();
    store.add_level("AAPL", make_stored_level(id, 100.0));
    store.clear_levels("AAPL");
    // Empty ticker should not appear in config output.
    assert!(store.to_level_configs().is_empty());
}

#[test]
fn level_ids_unique_across_tickers_after_import() {
    let mut cfgs: HashMap<String, Vec<LevelConfig>> = HashMap::new();
    cfgs.insert(
        "AAPL".into(),
        vec![LevelConfig {
            price: 100.0,
            color: [1.0, 0.0, 0.0, 1.0],
            line_width: 1.0,
            label: None,
            icon: "none".into(),
            locked: false,
        }],
    );
    cfgs.insert(
        "MSFT".into(),
        vec![LevelConfig {
            price: 400.0,
            color: [0.0, 1.0, 0.0, 1.0],
            line_width: 1.0,
            label: None,
            icon: "none".into(),
            locked: false,
        }],
    );
    let mut store = AnnotationStore::new();
    store.import_level_configs(&cfgs);
    let aapl_id = store.levels_for("AAPL")[0].id;
    let msft_id = store.levels_for("MSFT")[0].id;
    assert_ne!(aapl_id, msft_id);
}

#[test]
fn annotation_store_loads_v1_config_toml_fixture() {
    // Pre-Slice-7 TOML fixture. The `LevelConfig` on-disk shape is
    // unchanged; `AnnotationStore::import_level_configs` reads the
    // same flat shape the retired `LevelStore` did.
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/config_v1_pre_decorator.toml");
    let text = std::fs::read_to_string(&path).expect("read v1 toml fixture");
    #[derive(serde::Deserialize)]
    struct LevelsDoc {
        levels: HashMap<String, Vec<LevelConfig>>,
    }
    let doc: LevelsDoc = toml::from_str(&text).expect("parse v1 toml fixture");
    let mut store = AnnotationStore::new();
    store.import_level_configs(&doc.levels);
    let aapl = store.levels_for("AAPL");
    assert!(!aapl.is_empty(), "fixture should contain AAPL levels");
    assert!(
        (aapl[0].line.price - 189.42).abs() < f64::EPSILON,
        "fixture price should be 189.42, got {}",
        aapl[0].line.price
    );
    assert_eq!(aapl[0].label.as_deref(), Some("Support"));
    assert_eq!(aapl[0].icon, LevelIcon::Star);
    assert!(aapl[0].locked);
}
