//! Slice A1 — load a real on-disk annotation fixture through the new
//! crate's `Annotation` type and assert each `AnnotationKind` variant
//! survives the deserialize migration path.
//!
//! The fixture lives at `desktop/win/tests/fixtures/annotations_v1_pre_decorator.json`
//! (one level above this crate). It is a pre-Slice-7 flat-shape v1
//! payload with two `Level` annotations and exercises the
//! `HorizontalLevel` V1→V2 deserialise path that moved with the type.

use midas_annotation_types::{Annotation, AnnotationId, AnnotationKind, LevelIcon};
use serde::Deserialize;
use std::path::PathBuf;

/// On-disk envelope used by `midas-app/src/annotation_persistence.rs`.
/// We re-declare it here (matching the on-disk shape) so the test does
/// not depend on the app crate.
#[derive(Debug, Deserialize)]
struct AnnotationFile {
    version: u32,
    symbol: String,
    annotations: Vec<Annotation>,
}

#[test]
fn load_v1_pre_decorator_fixture() {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    // crates/midas-annotation-types -> desktop/win
    path.pop();
    path.pop();
    path.push("tests");
    path.push("fixtures");
    path.push("annotations_v1_pre_decorator.json");

    assert!(
        path.exists(),
        "fixture missing — expected at {}",
        path.display()
    );

    let raw = std::fs::read_to_string(&path).expect("read fixture");
    let file: AnnotationFile = serde_json::from_str(&raw).expect("parse fixture through new crate");

    assert_eq!(file.version, 1);
    assert_eq!(file.symbol, "AAPL");
    assert_eq!(file.annotations.len(), 2, "fixture has 2 level annotations");

    // First annotation: Level with label "Support" + Star icon
    let first = &file.annotations[0];
    assert_eq!(first.id, AnnotationId(1));
    match &first.kind {
        AnnotationKind::Level(level) => {
            assert_eq!(level.id, 1);
            assert!((level.line.price - 189.42).abs() < f64::EPSILON);
            assert_eq!(level.line.stroke.color, [0.2, 0.6, 1.0, 0.9]);
            assert_eq!(level.line.stroke.width, 2.0);
            assert_eq!(level.label.as_deref(), Some("Support"));
            assert_eq!(level.icon, LevelIcon::Star);
        }
        other => panic!("expected AnnotationKind::Level, got {other:?}"),
    }

    // Second annotation: Level with no label, no icon
    let second = &file.annotations[1];
    assert_eq!(second.id, AnnotationId(2));
    match &second.kind {
        AnnotationKind::Level(level) => {
            assert_eq!(level.id, 2);
            assert!((level.line.price - 192.0).abs() < f64::EPSILON);
            assert_eq!(level.label, None);
            assert_eq!(level.icon, LevelIcon::None);
        }
        other => panic!("expected AnnotationKind::Level, got {other:?}"),
    }
}
