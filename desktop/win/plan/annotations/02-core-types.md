# 02 — Core Types

## AnnotationId

```rust
/// Monotonically increasing per-chart identifier.
/// Not globally unique — scoped to the AnnotationStore that owns it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AnnotationId(pub u64);
```

The store tracks a `next_id: u64` counter. IDs are never reused within a chart session.
Persistence roundtrips restore the counter to `max(existing_ids) + 1`.

## Anchor

Defines how an annotation attaches to chart coordinate space.

```rust
/// How an annotation is positioned in price/time space.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Anchor {
    /// Horizontal line spanning full chart width. Used for levels.
    PriceOnly { price: f64 },

    /// Vertical line spanning full chart height. Used for time markers.
    TimeOnly { timestamp: i64 },

    /// Single point pinned to a specific price and time. Used for notes, markers.
    Point { price: f64, timestamp: i64 },

    /// Horizontal line starting at a specific time, extending to the right.
    /// Used for bracket legs (entry placed at a time, extends indefinitely).
    Ray { price: f64, timestamp: i64 },

    /// Span between two price/time points. Used for ranges, rectangles (future).
    Span {
        price_start: f64, timestamp_start: i64,
        price_end: f64,   timestamp_end: i64,
    },
}
```

### Anchor Behavior During Zoom/Pan

| Anchor | Pan X | Pan Y | Zoom X | Zoom Y |
|---|---|---|---|---|
| PriceOnly | no effect | moves with price | no effect | scales with price |
| TimeOnly | moves with time | no effect | scales with time | no effect |
| Point | moves | moves | scales | scales |
| Ray | start moves, extends right | moves with price | start scales | scales with price |
| Span | both ends move | both ends move | scales | scales |

## Annotation

The top-level annotation struct. Every drawable chart object is an `Annotation`.

```rust
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Annotation {
    pub id: AnnotationId,
    pub kind: AnnotationKind,
    pub created_at: i64,           // epoch millis
    pub modified_at: i64,          // epoch millis
    pub visible: bool,
    pub locked: bool,              // prevent accidental drag/delete
    pub tags: Vec<String>,         // for filtering: "order", "analysis", "support"
    pub external_id: Option<String>, // opaque — app layer stores order UUIDs here
}
```

### Why `external_id` is `Option<String>` Not `Option<Uuid>`

The chart crate must not depend on `uuid`. The app layer serializes order IDs to strings.
This is a deliberate indirection to keep the sans-IO boundary clean.

## AnnotationKind

```rust
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum AnnotationKind {
    /// Horizontal line at a price. Replaces current HorizontalLevel.
    Level(LevelAnnotation),

    /// Entry + optional TP/SL bracket. Maps to broker orders.
    Bracket(OrderBracket),

    /// Text note anchored to a point on the chart.
    Note(TextNote),

    /// Icon or stamp at a point. Used for fills, signals, alerts.
    Marker(MarkerAnnotation),
}
```

Each variant carries its own data struct (defined in dedicated files).
The `Annotation` wrapper adds shared metadata (id, timestamps, visibility, tags).

## LevelAnnotation

Migration target for existing `HorizontalLevel`.

```rust
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LevelAnnotation {
    pub price: f64,
    pub color: [f32; 4],
    pub line_width: f32,
    pub style: LineStyle,
    pub label: Option<String>,     // "Support", "Resistance", "200 SMA"
    pub extend: LevelExtend,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub enum LineStyle {
    #[default]
    Solid,
    Dashed { dash_len: f32, gap_len: f32 },
    Dotted { dot_spacing: f32 },
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub enum LevelExtend {
    #[default]
    FullWidth,                     // spans entire visible chart
    RightFrom { timestamp: i64 }, // starts at time, extends right
    Between { start: i64, end: i64 }, // bounded segment
}
```

### Migration Path from HorizontalLevel

```rust
// Old (levels.rs):
HorizontalLevel { id: u64, price: f64, color: [f32; 4], line_width: f32 }

// New (annotations/):
Annotation {
    id: AnnotationId(old.id),
    kind: AnnotationKind::Level(LevelAnnotation {
        price: old.price,
        color: old.color,
        line_width: old.line_width,
        style: LineStyle::Solid,
        label: None,
        extend: LevelExtend::FullWidth,
    }),
    created_at: 0,
    modified_at: 0,
    visible: true,
    locked: false,
    tags: vec![],
    external_id: None,
}
```

Existing `ChartAction::CreateLevel`, `DragLevel`, `DeleteSelectedLevel` continue to work.
Internally they operate on `AnnotationStore` instead of `Vec<HorizontalLevel>`.

## Style Types

```rust
/// Visual style applied to any annotation.
/// Each AnnotationKind has its own defaults; this overrides them.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AnnotationStyle {
    pub color: [f32; 4],
    pub line_width: f32,
    pub line_style: LineStyle,
    pub font_size: Option<f32>,    // for notes/labels
    pub opacity: f32,              // multiplied with color alpha
}
```

## MarkerAnnotation

```rust
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MarkerAnnotation {
    pub price: f64,
    pub timestamp: i64,
    pub icon: MarkerIcon,
    pub color: [f32; 4],
    pub size: f32,                  // diameter in logical pixels
    pub label: Option<String>,      // tooltip text
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub enum MarkerIcon {
    Circle,         // generic marker
    FilledCircle,   // fill events
    Triangle,       // buy signal
    InvTriangle,    // sell signal
    Diamond,        // alert
    Cross,          // stop/cancel
    Flag,           // important event
    Star,           // bookmarked
}
```

## TextNote

```rust
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TextNote {
    pub price: f64,
    pub timestamp: i64,
    pub text: String,
    pub background_color: [f32; 4],
    pub text_color: [f32; 4],
    pub font_size: f32,
    pub max_width: Option<f32>,     // word wrap boundary
}
```

## Type Size Budget

All annotation types should be small enough to clone cheaply (< 256 bytes each).
The `tags: Vec<String>` and `text: String` are heap-allocated, but these are per-annotation
not per-frame — they're cloned only on persistence, never in the render loop.

The render path works with `AnnotationRender` (see 05-rendering.md), which is a compact
GPU-ready struct — no strings, no allocations.
