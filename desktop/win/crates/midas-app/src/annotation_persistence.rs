//! Annotation persistence: per-symbol JSON files with atomic writes.
//!
//! Each symbol's annotations are saved as a separate JSON file in
//! `data/annotations/<SYMBOL>.json`. Writes are atomic (write to .tmp,
//! then rename) and debounced (500ms after last mutation).
//!
//! ## Chart-transition slice 8.5 status
//!
//! The `midas_annotation_types::Annotation` import is the shared persistent
//! annotation shape (plan D9 — `AnnotationStore` format unchanged).
//! Session-chart paths never call this module directly; all persistence
//! lookups route through `AnnotationStore`. The type migrates to its
//! new home in slice 9c's atomic deletion PR.

use crate::annotation_store::AnnotationStore;
use midas_annotation_types::Annotation;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// On-disk format for a single symbol's annotations.
///
/// Used for deserialization during v1 migration and tests.
/// Construction (serialization) is test-only since Slice 4.
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(not(test), allow(dead_code))]
pub struct AnnotationFile {
    /// Schema version for forward compatibility.
    pub version: u32,
    /// The symbol these annotations belong to.
    pub symbol: String,
    /// The annotations themselves.
    pub annotations: Vec<Annotation>,
}

impl AnnotationFile {
    /// Current schema version.
    #[cfg_attr(not(test), allow(dead_code))]
    pub const CURRENT_VERSION: u32 = 1;
}

/// Resolves the annotations directory, creating it if needed.
pub fn annotations_dir(data_dir: &Path) -> PathBuf {
    let dir = data_dir.join("annotations");
    if !dir.exists() {
        let _ = std::fs::create_dir_all(&dir);
    }
    dir
}

/// Path to the JSON file for a given symbol.
fn symbol_file_path(annotations_dir: &Path, symbol: &str) -> PathBuf {
    annotations_dir.join(format!("{}.json", symbol.to_uppercase()))
}

/// Save a single symbol's annotations to disk atomically.
///
/// Writes to a `.tmp` file first, then renames. This prevents
/// corruption if the process crashes mid-write.
///
/// **Deprecated in Slice 4**: bracket annotations are now persisted via
/// `TickerStatePersistHandle` (redb v2). This function is retained
/// only for tests.
#[cfg(test)]
pub fn save_symbol(
    annotations_dir: &Path,
    symbol: &str,
    annotations: &[Annotation],
) -> anyhow::Result<()> {
    let file_path = symbol_file_path(annotations_dir, symbol);
    let tmp_path = file_path.with_extension("json.tmp");

    let file = AnnotationFile {
        version: AnnotationFile::CURRENT_VERSION,
        symbol: symbol.to_uppercase(),
        annotations: annotations.to_vec(),
    };

    let json = serde_json::to_string_pretty(&file)?;
    std::fs::write(&tmp_path, &json)?;
    if let Err(e) = std::fs::rename(&tmp_path, &file_path) {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(e.into());
    }

    Ok(())
}

/// Load a single symbol's annotations from disk.
///
/// Uses forward-compatible deserialization: entries with unknown
/// `AnnotationKind` variants are silently skipped. This ensures
/// files written by a later version (with more annotation types)
/// can be loaded by earlier code without crashing.
pub fn load_symbol(annotations_dir: &Path, symbol: &str) -> anyhow::Result<Vec<Annotation>> {
    let file_path = symbol_file_path(annotations_dir, symbol);

    if !file_path.exists() {
        return Ok(Vec::new());
    }

    let content = match std::fs::read_to_string(&file_path) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(
                "Failed to read annotations for {}: {}. Starting empty.",
                symbol,
                e
            );
            return Ok(Vec::new());
        }
    };

    // Two-pass deserialization for forward compatibility.
    let raw: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(e) => {
            // Corrupt file — rename to .corrupt.bak and start empty.
            let backup = file_path.with_extension("json.corrupt.bak");
            let _ = std::fs::rename(&file_path, &backup);
            tracing::warn!(
                "Corrupt annotations file for {}: {}. Backed up to {:?}",
                symbol,
                e,
                backup
            );
            return Ok(Vec::new());
        }
    };

    let annotations_array = raw
        .get("annotations")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let mut annotations = Vec::with_capacity(annotations_array.len());
    for entry in annotations_array {
        match serde_json::from_value::<Annotation>(entry) {
            Ok(ann) => annotations.push(ann),
            Err(e) => {
                // Unknown variant — skip silently (forward compat).
                tracing::debug!("Skipping unknown annotation entry: {}", e);
            }
        }
    }

    Ok(annotations)
}

/// Load all annotation files from the annotations directory.
///
/// Returns a map of symbol -> Vec<Annotation>. Sets `next_id` on
/// the store to one past the highest ID found.
pub fn load_all(data_dir: &Path) -> anyhow::Result<HashMap<String, Vec<Annotation>>> {
    let dir = annotations_dir(data_dir);
    let mut result = HashMap::new();

    if !dir.exists() {
        return Ok(result);
    }

    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().is_none_or(|ext| ext != "json") {
            continue;
        }

        let symbol = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_uppercase();

        if symbol.is_empty() {
            continue;
        }

        match load_symbol(&dir, &symbol) {
            Ok(anns) if !anns.is_empty() => {
                result.insert(symbol, anns);
            }
            Ok(_) => {} // empty, skip
            Err(e) => {
                tracing::warn!("Failed to load annotations for {}: {}", symbol, e);
            }
        }
    }

    Ok(result)
}

/// Build an AnnotationStore from loaded files.
///
/// Normalizes every bracket annotation to match current entry_type rules
/// and hides unsaved Draft brackets (they don't survive restart — only
/// explicitly saved brackets are recalled).
pub fn store_from_files(files: HashMap<String, Vec<Annotation>>) -> AnnotationStore {
    let mut store = AnnotationStore::new();
    let mut max_id: u64 = 0;

    for (symbol, annotations) in &files {
        for ann in annotations {
            max_id = max_id.max(ann.id.0);
            let mut ann = ann.clone();

            if let midas_annotation_types::AnnotationKind::OrderBracket(ref mut b) = ann.kind {
                // Normalize bracket data to match entry_type rules.
                crate::order_panel::normalize_bracket(b);

                // Unsaved Draft brackets don't survive restart.
                if b.status == midas_annotation_types::order_bracket::BracketStatus::Draft
                    && !b.saved
                {
                    max_id = max_id.max(ann.id.0);
                    continue;
                }
                // Saved brackets load Active so they're immediately visible.
            }

            store.add_raw(symbol, ann);
        }
    }

    store.set_next_id(max_id + 1);
    store
}

#[cfg(test)]
mod tests {
    use super::*;
    use midas_annotation_types::price_line::{LineExtent, LineStroke, PriceLine};
    use midas_annotation_types::{
        AnnotationId, AnnotationKind, HorizontalLevel, LevelIcon, LineStyle, Presence,
    };
    use tempfile::TempDir;

    fn make_test_annotation(id: u64, price: f64) -> Annotation {
        Annotation {
            id: AnnotationId(id),
            kind: AnnotationKind::Level(HorizontalLevel {
                id,
                line: PriceLine {
                    price,
                    extent: LineExtent::default(),
                    stroke: LineStroke {
                        color: [1.0, 0.0, 0.0, 1.0],
                        width: 1.0,
                        style: LineStyle::default(),
                    },
                },
                label: Some("Test".into()),
                icon: LevelIcon::None,
            }),
            presence: Presence::Active,
            visible_timeframes: None,
            locked: false,
            created_at: 1000,
            modified_at: 1000,
        }
    }

    #[test]
    fn save_and_load_round_trip() {
        let tmp = TempDir::new().unwrap();
        let dir = annotations_dir(tmp.path());

        let annotations = vec![
            make_test_annotation(1, 185.50),
            make_test_annotation(2, 192.00),
        ];
        save_symbol(&dir, "AAPL", &annotations).unwrap();

        let loaded = load_symbol(&dir, "AAPL").unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].id, AnnotationId(1));
        assert_eq!(loaded[1].id, AnnotationId(2));
    }

    #[test]
    fn load_nonexistent_returns_empty() {
        let tmp = TempDir::new().unwrap();
        let dir = annotations_dir(tmp.path());
        let loaded = load_symbol(&dir, "MISSING").unwrap();
        assert!(loaded.is_empty());
    }

    #[test]
    fn corrupt_file_handled_gracefully() {
        let tmp = TempDir::new().unwrap();
        let dir = annotations_dir(tmp.path());
        let path = dir.join("BAD.json");
        std::fs::write(&path, "not valid json {{{{").unwrap();

        let loaded = load_symbol(&dir, "BAD").unwrap();
        assert!(loaded.is_empty());

        // Backup should exist.
        assert!(dir.join("BAD.json.corrupt.bak").exists());
    }

    #[test]
    fn unknown_annotation_kind_skipped() {
        let tmp = TempDir::new().unwrap();
        let dir = annotations_dir(tmp.path());

        // Write a file with a known annotation (v1 flat shape, exercising
        // the Slice 7 migration `Deserialize` path) and an unknown one.
        let json = r#"{
            "version": 1,
            "symbol": "AAPL",
            "annotations": [
                {
                    "id": 1,
                    "kind": {"Level": {"id": 1, "price": 185.0, "color": [1,0,0,1], "line_width": 1.0, "style": "Solid", "label": null, "extend": "FullWidth", "icon": "None"}},
                    "presence": "Active",
                    "visible_timeframes": null,
                    "locked": false,
                    "created_at": 0,
                    "modified_at": 0
                },
                {
                    "id": 2,
                    "kind": {"FutureWidget": {"some_field": 42}},
                    "presence": "Active",
                    "visible_timeframes": null,
                    "locked": false,
                    "created_at": 0,
                    "modified_at": 0
                }
            ]
        }"#;
        let path = dir.join("AAPL.json");
        std::fs::write(&path, json).unwrap();

        let loaded = load_symbol(&dir, "AAPL").unwrap();
        assert_eq!(loaded.len(), 1, "unknown variant should be skipped");
        assert_eq!(loaded[0].id, AnnotationId(1));
    }

    #[test]
    fn annotation_persistence_loads_v1_json_fixture() {
        // Slice 7 migration test: load a pre-refactor JSON fixture via
        // the manual `HorizontalLevel::deserialize` v1 fallback and
        // verify both levels round-trip to the new composed shape.
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/annotations_v1_pre_decorator.json");
        let text = std::fs::read_to_string(&path).expect("read v1 json fixture");
        let file: AnnotationFile = serde_json::from_str(&text).expect("parse v1 json fixture");
        assert_eq!(file.annotations.len(), 2);

        let first = &file.annotations[0];
        assert_eq!(first.id, AnnotationId(1));
        match &first.kind {
            AnnotationKind::Level(level) => {
                assert!((level.line.price - 189.42).abs() < f64::EPSILON);
                assert_eq!(level.line.stroke.color, [0.2, 0.6, 1.0, 0.9]);
                assert_eq!(level.line.stroke.width, 2.0);
                assert_eq!(level.label.as_deref(), Some("Support"));
                assert_eq!(level.icon, LevelIcon::Star);
            }
            _ => panic!("expected Level variant for annotation 1"),
        }

        let second = &file.annotations[1];
        match &second.kind {
            AnnotationKind::Level(level) => {
                assert!((level.line.price - 192.0).abs() < f64::EPSILON);
                assert_eq!(level.label, None);
                assert_eq!(level.icon, LevelIcon::None);
            }
            _ => panic!("expected Level variant for annotation 2"),
        }
    }

    #[test]
    fn save_all_and_load_all() {
        let tmp = TempDir::new().unwrap();

        // Save two symbols.
        let dir = annotations_dir(tmp.path());
        save_symbol(&dir, "AAPL", &[make_test_annotation(1, 185.0)]).unwrap();
        save_symbol(&dir, "MSFT", &[make_test_annotation(2, 400.0)]).unwrap();

        let all = load_all(tmp.path()).unwrap();
        assert_eq!(all.len(), 2);
        assert!(all.contains_key("AAPL"));
        assert!(all.contains_key("MSFT"));
    }
}
