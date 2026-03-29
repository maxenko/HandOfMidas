# 06 — Persistence

## Separation from config.toml

Annotations are **user-created data**, not application preferences.
They must not be mixed with ephemeral config (window size, camera position, theme).

```
$LOCAL_APPDATA/HandOfMidas/
├── config.toml                          # app preferences (existing)
└── annotations/
    ├── AAPL_D1.json                     # per-symbol, per-timeframe
    ├── AAPL_M5.json
    ├── MSFT_D1.json
    └── ...
```

### Why Per-Symbol-Per-Timeframe

- A daily-chart bracket at 185.50 doesn't make sense on a 5-minute chart (different price context)
- Users draw different things at different timeframes
- Keeps file sizes small (< 50 KB each for hundreds of annotations)
- Easy to back up or share a single symbol's annotations

### Why JSON (Not TOML, Not SQLite)

| Format | Pro | Con |
|---|---|---|
| **JSON** | Human-readable, serde derive, trivial schema evolution | No indexing, full-file read/write |
| TOML | Matches config.toml style | Poor for arrays of complex nested structs |
| SQLite | Queryable, atomic writes, cross-chart queries | Heavier dependency, overkill for < 500 records |

**Start with JSON.** Migrate to SQLite if we need:
- Cross-chart queries ("all open brackets across all symbols")
- Concurrent write access (multi-window editing same symbol)
- Partial updates (modify one annotation without rewriting the file)

None of these are needed in v1.

## File Format

```json
{
  "version": 1,
  "symbol": "AAPL",
  "timeframe": "D1",
  "next_id": 47,
  "annotations": [
    {
      "id": 12,
      "kind": {
        "Level": {
          "price": 185.50,
          "color": [0.0, 0.8, 0.0, 1.0],
          "line_width": 1.0,
          "style": "Solid",
          "label": "Support",
          "extend": "FullWidth"
        }
      },
      "created_at": 1711584000000,
      "modified_at": 1711584000000,
      "visible": true,
      "locked": false,
      "tags": ["support"],
      "external_id": null
    },
    {
      "id": 23,
      "kind": {
        "Bracket": {
          "entry": {
            "price": 185.50,
            "timestamp": 1711584000000,
            "color": null,
            "style": "Solid",
            "line_width": 1.5,
            "label": "Entry"
          },
          "take_profit": {
            "price": 192.00,
            "timestamp": null,
            "color": null,
            "style": "Solid",
            "line_width": 1.0,
            "label": "TP +3.5%"
          },
          "stop_loss": {
            "price": 182.00,
            "timestamp": null,
            "color": null,
            "style": "Solid",
            "line_width": 1.0,
            "label": "SL -1.9%"
          },
          "side": "Long",
          "status": "Active",
          "quantity": 100.0
        }
      },
      "created_at": 1711584000000,
      "modified_at": 1711670400000,
      "visible": true,
      "locked": true,
      "tags": ["order"],
      "external_id": "018e4a6b-7c8e-7f00-b123-456789abcdef"
    }
  ]
}
```

## Rust Types for Persistence

```rust
/// Top-level file format for annotation persistence.
#[derive(Debug, Serialize, Deserialize)]
pub struct AnnotationFile {
    pub version: u32,
    pub symbol: String,
    pub timeframe: String,
    pub next_id: u64,
    pub annotations: Vec<Annotation>,
}

impl AnnotationFile {
    pub const CURRENT_VERSION: u32 = 1;
}
```

The `Annotation` and all sub-types derive `Serialize + Deserialize`.
The file version field enables forward-compatible schema evolution.

## Save Strategy

### When to Save

Annotations save on the same debounce as config.toml (2 seconds after last change):

```rust
// In midas-app:
fn maybe_save_annotations(&mut self) {
    if !self.annotations_dirty { return; }
    if self.last_annotation_save.elapsed() < Duration::from_secs(2) { return; }
    self.flush_annotations();
}
```

### What Triggers a Save

- `CreateBracket`, `CreateNote`, `PlaceMarker`, `CreateLevel`
- `DragLevel`, `DragBracketLeg`, `DragNote` (on mouse release, not during drag)
- `DeleteSelectedAnnotation`
- `UpdateAnnotationStatus` (order bridge status change)
- `ToggleAnnotationVisibility`, `ToggleAnnotationLock`
- `SetAnnotationTags`

During drag operations, only mark dirty on release. Saving on every mouse move
would thrash the file system.

### Atomic Writes

Write to a temp file, then rename:

```rust
fn save_annotations(path: &Path, file: &AnnotationFile) -> Result<()> {
    let json = serde_json::to_string_pretty(file)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &json)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}
```

This prevents corruption if the app crashes mid-write.

## Load Strategy

### On Chart Open

When a chart panel is created or its symbol/timeframe changes:

```rust
fn load_annotations(symbol: &str, timeframe: &str) -> AnnotationFile {
    let path = annotations_dir().join(format!("{}_{}.json", symbol, timeframe));
    match std::fs::read_to_string(&path) {
        Ok(json) => match serde_json::from_str(&json) {
            Ok(file) => file,
            Err(e) => {
                tracing::warn!("corrupt annotation file {}: {e}", path.display());
                AnnotationFile::empty(symbol, timeframe)
            }
        },
        Err(_) => AnnotationFile::empty(symbol, timeframe),
    }
}
```

### Schema Migration

When `version < CURRENT_VERSION`, run migration:

```rust
fn migrate(mut file: AnnotationFile) -> AnnotationFile {
    if file.version == 0 {
        // v0 → v1: add default tags, etc.
        for ann in &mut file.annotations {
            if ann.tags.is_empty() {
                ann.tags = vec![];
            }
        }
        file.version = 1;
    }
    // future migrations chain here
    file
}
```

## Migration from Existing Levels

Current `HorizontalLevel` data lives in `config.toml` under each chart:

```toml
[[charts]]
symbol = "AAPL"
timeframe = "D1"
[[charts.levels]]
price = 185.50
color = [0.0, 0.8, 0.0, 1.0]
line_width = 1.0
```

On first run after the annotation system ships:

1. Read existing levels from config.toml
2. Convert each to `Annotation { kind: AnnotationKind::Level(...) }`
3. Save to `AAPL_D1.json`
4. Remove `levels` array from config.toml
5. Set a `migrated_annotations: true` flag in config to avoid re-migration

This is a one-time, non-destructive operation.

## Backup & Export

Future: annotations can be exported as a zip of JSON files for backup/sharing.
The JSON format is designed to be human-readable and self-describing —
a user can open `AAPL_D1.json` in a text editor and understand it.
