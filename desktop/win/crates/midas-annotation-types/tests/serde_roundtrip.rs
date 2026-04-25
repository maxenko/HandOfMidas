//! Slice A1 — round-trip serde tests for every `AnnotationKind`
//! variant. Catches any tag drift introduced by the type move out of
//! `midas-chart`.
//!
//! Each test builds an annotation, serialises it to JSON, deserialises
//! it back, and asserts the round-trip preserves both the variant tag
//! and the inner payload bit-for-bit.

use midas_annotation_types::{
    Annotation, AnnotationId, AnnotationKind, BracketLeg, BracketSide, BracketStatus, EntryType,
    HorizontalLevel, LegRole, LevelIcon, LineExtent, LineStroke, LineStyle, MarkerAnnotation,
    MarkerIcon, OrderBracket, Presence, PriceLine, TextNote,
};

fn level_annotation() -> Annotation {
    Annotation {
        id: AnnotationId(101),
        kind: AnnotationKind::Level(HorizontalLevel {
            id: 101,
            line: PriceLine {
                price: 175.5,
                extent: LineExtent::FullWidth,
                stroke: LineStroke {
                    color: [0.2, 0.6, 1.0, 0.9],
                    width: 1.5,
                    style: LineStyle::Solid,
                },
            },
            label: Some("Resistance".into()),
            icon: LevelIcon::ArrowUp,
        }),
        presence: Presence::Active,
        visible_timeframes: None,
        locked: false,
        created_at: 1_700_000_000_000,
        modified_at: 1_700_000_000_001,
    }
}

fn bracket_leg(price: f64, role: LegRole) -> BracketLeg {
    BracketLeg {
        line: PriceLine {
            price,
            extent: LineExtent::FullWidth,
            stroke: LineStroke {
                color: [0.20, 0.78, 0.35, 1.0],
                width: 1.0,
                style: LineStyle::Solid,
            },
        },
        role,
        projected_pnl: None,
        projected_pnl_pct: None,
    }
}

fn bracket_annotation() -> Annotation {
    let bracket = OrderBracket {
        entry: bracket_leg(180.0, LegRole::Entry),
        take_profit: Some(bracket_leg(190.0, LegRole::TakeProfit)),
        stop_loss: Some(bracket_leg(175.0, LegRole::StopLoss)),
        side: BracketSide::Long,
        status: BracketStatus::Draft,
        quantity: Some(100.0),
        saved: false,
        filled_qty: None,
        entry_type: EntryType::Limit,
        entry_stop_price: None,
        wrong_side_warning: false,
    };
    Annotation {
        id: AnnotationId(202),
        kind: AnnotationKind::OrderBracket(Box::new(bracket)),
        presence: Presence::Active,
        visible_timeframes: None,
        locked: false,
        created_at: 1_700_000_000_002,
        modified_at: 1_700_000_000_003,
    }
}

fn text_note_annotation() -> Annotation {
    Annotation {
        id: AnnotationId(303),
        kind: AnnotationKind::TextNote(TextNote {
            price: 185.0,
            timestamp: 1_700_000_000_000,
            text: "Support zone".into(),
            background_color: [0.15, 0.15, 0.20, 0.85],
            text_color: [0.9, 0.9, 0.9, 1.0],
            font_size: 12.0,
            max_width: Some(200.0),
        }),
        presence: Presence::Active,
        visible_timeframes: None,
        locked: false,
        created_at: 1_700_000_000_004,
        modified_at: 1_700_000_000_005,
    }
}

fn marker_annotation() -> Annotation {
    Annotation {
        id: AnnotationId(404),
        kind: AnnotationKind::Marker(MarkerAnnotation {
            price: 185.50,
            timestamp: 1_700_000_000_000,
            icon: MarkerIcon::TriangleUp,
            color: [0.0, 1.0, 0.0, 1.0],
            size: 10.0,
            tooltip: Some("Buy fill @ 185.50".into()),
        }),
        presence: Presence::Active,
        visible_timeframes: None,
        locked: false,
        created_at: 1_700_000_000_006,
        modified_at: 1_700_000_000_007,
    }
}

#[test]
fn roundtrip_level_variant() {
    let original = level_annotation();
    let json = serde_json::to_string(&original).expect("serialize Level");
    assert!(
        json.contains("\"Level\":"),
        "wire-format tag must stay PascalCase 'Level': {json}"
    );
    let decoded: Annotation = serde_json::from_str(&json).expect("deserialize Level");
    let json_back = serde_json::to_string(&decoded).expect("re-serialize");
    assert_eq!(json, json_back, "JSON must be byte-identical on round-trip");
    match decoded.kind {
        AnnotationKind::Level(level) => {
            assert_eq!(level.id, 101);
            assert!((level.line.price - 175.5).abs() < f64::EPSILON);
            assert_eq!(level.icon, LevelIcon::ArrowUp);
            assert_eq!(level.label.as_deref(), Some("Resistance"));
        }
        other => panic!("expected Level, got {other:?}"),
    }
}

#[test]
fn roundtrip_order_bracket_variant() {
    let original = bracket_annotation();
    let json = serde_json::to_string(&original).expect("serialize OrderBracket");
    assert!(
        json.contains("\"OrderBracket\":"),
        "wire-format tag must stay PascalCase 'OrderBracket': {json}"
    );
    let decoded: Annotation = serde_json::from_str(&json).expect("deserialize OrderBracket");
    let json_back = serde_json::to_string(&decoded).expect("re-serialize");
    assert_eq!(json, json_back, "JSON must be byte-identical on round-trip");
    match decoded.kind {
        AnnotationKind::OrderBracket(bracket) => {
            assert!((bracket.entry.line.price - 180.0).abs() < f64::EPSILON);
            assert_eq!(bracket.side, BracketSide::Long);
            assert_eq!(bracket.entry_type, EntryType::Limit);
            assert_eq!(bracket.status, BracketStatus::Draft);
            assert!(bracket.take_profit.is_some());
            assert!(bracket.stop_loss.is_some());
        }
        other => panic!("expected OrderBracket, got {other:?}"),
    }
}

#[test]
fn roundtrip_text_note_variant() {
    let original = text_note_annotation();
    let json = serde_json::to_string(&original).expect("serialize TextNote");
    assert!(
        json.contains("\"TextNote\":"),
        "wire-format tag must stay PascalCase 'TextNote': {json}"
    );
    let decoded: Annotation = serde_json::from_str(&json).expect("deserialize TextNote");
    let json_back = serde_json::to_string(&decoded).expect("re-serialize");
    assert_eq!(json, json_back, "JSON must be byte-identical on round-trip");
    match decoded.kind {
        AnnotationKind::TextNote(note) => {
            assert_eq!(note.text, "Support zone");
            assert!((note.price - 185.0).abs() < f64::EPSILON);
            assert_eq!(note.max_width, Some(200.0));
        }
        other => panic!("expected TextNote, got {other:?}"),
    }
}

#[test]
fn roundtrip_marker_variant() {
    let original = marker_annotation();
    let json = serde_json::to_string(&original).expect("serialize Marker");
    assert!(
        json.contains("\"Marker\":"),
        "wire-format tag must stay PascalCase 'Marker': {json}"
    );
    let decoded: Annotation = serde_json::from_str(&json).expect("deserialize Marker");
    let json_back = serde_json::to_string(&decoded).expect("re-serialize");
    assert_eq!(json, json_back, "JSON must be byte-identical on round-trip");
    match decoded.kind {
        AnnotationKind::Marker(marker) => {
            assert_eq!(marker.icon, MarkerIcon::TriangleUp);
            assert_eq!(marker.tooltip.as_deref(), Some("Buy fill @ 185.50"));
        }
        other => panic!("expected Marker, got {other:?}"),
    }
}

#[test]
fn presence_variants_roundtrip() {
    for presence in [Presence::Active, Presence::Ghost, Presence::Hidden] {
        let json = serde_json::to_string(&presence).expect("serialize Presence");
        let back: Presence = serde_json::from_str(&json).expect("deserialize Presence");
        assert_eq!(back, presence);
    }
}

#[test]
fn level_v1_legacy_shape_still_deserialises() {
    // The pre-Slice-7 flat shape must keep parsing — this is the
    // forward-compat path that ships with `HorizontalLevel`'s custom
    // `Deserialize` impl.
    let json = r#"{
        "id": 7,
        "price": 189.42,
        "color": [0.2, 0.6, 1.0, 0.9],
        "line_width": 2.0,
        "style": "Solid",
        "label": "Support",
        "icon": "Star",
        "extend": "FullWidth",
        "locked": false
    }"#;
    let level: HorizontalLevel = serde_json::from_str(json).expect("v1 -> v2 migration");
    assert_eq!(level.id, 7);
    assert!((level.line.price - 189.42).abs() < f64::EPSILON);
    assert_eq!(level.line.stroke.width, 2.0);
    assert_eq!(level.label.as_deref(), Some("Support"));
    assert_eq!(level.icon, LevelIcon::Star);
}
