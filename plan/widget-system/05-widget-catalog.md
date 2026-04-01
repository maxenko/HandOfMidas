# 05 -- Widget Catalog

Complete specification for every concrete widget type in the Hand of Midas
widget system. Each section covers data model, render output, compute function,
interaction behavior, visual design, and migration notes where applicable.

**Audience**: Senior developers implementing the widget system.

**Coordinate system convention**: All pixel values are logical pixels
(pre-DPI-scaling). The projection matrix in the GPU pipeline handles the
mapping to physical pixels and NDC. Price axis is inverted: higher prices
map to lower Y values (top of screen).

---

## Table of Contents

1. [Architecture Overview](#1-architecture-overview)
2. [Horizontal Level (Migration)](#2-horizontal-level)
3. [Order Bracket](#3-order-bracket)
4. [G.ATR Indicator (Migration)](#4-gatr-indicator)
5. [Volume Profile](#5-volume-profile)
6. [Moving Average](#6-moving-average)
7. [Velocity/Momentum Indicator](#7-velocitymomentum-indicator)
8. [Text Note](#8-text-note)
9. [Marker Annotation](#9-marker-annotation)
10. [Trendline](#10-trendline)
11. [Catalog Summary Table](#11-catalog-summary-table)
12. [Cross-Cutting Concerns](#12-cross-cutting-concerns)

---

## 1. Architecture Overview

### 1.1 Widget Categories

The system distinguishes two fundamentally different categories of widgets.
This distinction drives storage, lifecycle, and interaction behavior.

```rust
/// Top-level classification for all chart widgets.
pub enum WidgetCategory {
    /// User-placed objects stored in AnnotationStore.
    /// CRUD lifecycle, persisted per-symbol in JSON.
    /// Examples: levels, brackets, notes, markers.
    Annotation,

    /// Data-computed overlays configured per-chart.
    /// Computed fresh each frame from CandleData + config.
    /// Not stored in AnnotationStore. Controlled by chart settings.
    /// Examples: G.ATR, volume profile, moving averages.
    Indicator,
}
```

### 1.2 AnnotationKind Enum

Every annotation widget corresponds to a variant of `AnnotationKind`:

The canonical definition is in `01-core-architecture.md` Section 2.2:

```rust
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum AnnotationKind {
    Level(HorizontalLevel),
    OrderBracket(OrderBracket),
    TextNote(TextNote),
    Marker(MarkerAnnotation),
    // Future: Trendline, Region, etc.
}
```

Future variants extend this enum. The `Annotation` wrapper (defined in
`01-core-architecture.md` Section 2.4) adds shared metadata: `id`,
`presence` (Active/Ghost/Hidden), `visible_timeframes`, `locked`,
`created_at`, `modified_at`.

### 1.3 WidgetOutput

Every widget's compute function produces GPU-ready data through a common
output structure:

The canonical definition is in `01-core-architecture.md` Section 4:

```rust
/// Render output produced by any widget's compute function.
/// Uses three separate buffers that map to GPU draw layers.
pub struct WidgetOutput {
    /// Background fills rendered at Layer 4 (behind candles).
    pub fills: Vec<GridLineInstance>,
    /// Lines and borders rendered at Layer 7 (on top of fills).
    pub lines: Vec<GridLineInstance>,
    /// Markers and point elements rendered at Layer 8.
    pub markers: Vec<GridLineInstance>,
    /// Text labels rendered via iced overlay (above all GPU layers).
    pub labels: Vec<WidgetLabel>,
    /// Hit-test zones for mouse interaction (not rendered).
    pub hit_zones: Vec<HitZone>,
}

/// A text label positioned in screen space.
pub struct WidgetLabel {
    pub text: String,
    pub screen_x: f32,
    pub screen_y: f32,
    pub bg_color: [f32; 4],
    pub text_color: [f32; 4],
    pub font_size: f32,
    pub anchor: LabelAnchor, // TopLeft, Center, etc.
}

/// A region that responds to mouse events.
/// See canonical definition in `01-core-architecture.md` Section 2.6.
pub struct HitZone {
    pub annotation_id: AnnotationId, // Which annotation owns this zone
    pub rect: [f32; 4],              // [left, top, right, bottom] in screen px
    pub kind: HitZoneKind,           // What part was hit (level line, bracket leg, etc.)
    pub cursor: CursorIcon,          // What cursor to show on hover
}
```

### 1.4 Render Layer Assignment

Widgets produce `GridLineInstance` data that is sorted into sub-layers by
the renderer. The layer assignment is determined by the widget type and the
semantic role of each instance:

```
Layer 4: Annotation fills    -- bracket zone fills, note backgrounds (behind candles)
Layer 7: Annotation lines    -- level lines, bracket legs
Layer 8: Annotation markers  -- point markers, icons
```

All three layers use the same `GridPipeline` shader. They are separate
pipelines only for z-ordering purposes.

---

## 2. Horizontal Level

**Category**: Annotation | **Priority**: v1 (migration) | **Status**: Existing
implementation in `levels.rs`, `level_tool.rs`, `instances.rs`

### 2.1 Data Model

Current implementation (`levels.rs`):

```rust
// Existing -- to be migrated
pub struct HorizontalLevel {
    pub id: u64,
    pub price: f64,
    pub color: [f32; 4],
    pub line_width: f32,
    pub label: Option<String>,
    pub icon: LevelIcon,
    pub locked: bool,
}
```

Target annotation model (`annotations/types.rs`):

```rust
/// A horizontal line at a fixed price, spanning the chart width.
/// Migration target for the existing HorizontalLevel struct.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HorizontalLevel {
    /// Price at which the line is drawn.
    pub price: f64,

    /// RGBA color in linear space.
    pub color: [f32; 4],

    /// Line thickness in logical pixels. Default: 1.0.
    pub line_width: f32,

    /// Line rendering style.
    pub style: LineStyle,

    /// Optional text label shown on the chart near the line.
    /// Examples: "Support", "Resistance", "200 SMA", "Entry zone".
    pub label: Option<String>,

    /// How far the line extends horizontally.
    pub extend: LevelExtend,

    /// Icon shown on the level badge. Migrated from existing HorizontalLevel.
    pub icon: LevelIcon,
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
    /// Spans the entire visible chart width. Most common.
    #[default]
    FullWidth,
    /// Starts at a timestamp and extends infinitely to the right.
    RightFrom { timestamp: i64 },
    /// Bounded segment between two timestamps.
    Between { start: i64, end: i64 },
}
```

### 2.2 Render Output

A single level produces the following `WidgetOutput`:

```
+--------------------------------------------------------------+
|                                                              |
|  ======== "Resistance" =====  185.50  =====================  |  <- Level line
|                                              +---------+     |
|                                              | 185.50  |     |  <- Y-axis badge
|                                              +---------+     |
+--------------------------------------------------------------+
```

Components:
- **1 GridLineInstance** (or N segments for dashed/dotted): the horizontal
  line spanning the viewport width (or partial width for `RightFrom`/`Between`).
- **1 WidgetLabel**: price badge on the Y-axis (right edge of chart).
  Background color matches the level color. Text shows formatted price.
- **1 optional WidgetLabel**: user label text positioned to the left of the
  line, offset slightly above.
- **1 HitZone**: horizontal strip spanning the full line length, +/-4px
  vertically for grab tolerance.

#### Compute Function

```rust
/// Compute render data for a single horizontal level.
///
/// Returns None if the level is entirely outside the visible price range.
pub fn compute_level(
    level: &HorizontalLevel,
    annotation: &Annotation,     // shared metadata (id, visible, locked, etc.)
    camera: &Camera2D,
    viewport_width: u32,
    is_selected: bool,
    is_dragging: bool,
) -> Option<WidgetOutput> {
    // 1. Convert price to screen Y
    let y = camera.price_to_y(level.price);

    // 2. Early-out if off-screen (with margin for label)
    if y < -20.0 || y > camera.viewport_height as f32 + 20.0 {
        return None;
    }

    // 3. Compute X extents based on LevelExtend
    let (x_start, x_end) = match &level.extend {
        LevelExtend::FullWidth => (0.0, viewport_width as f32),
        LevelExtend::RightFrom { timestamp } => {
            let x = camera.time_to_x(*timestamp as f64);
            (x.max(0.0), viewport_width as f32)
        }
        LevelExtend::Between { start, end } => {
            let xs = camera.time_to_x(*start as f64);
            let xe = camera.time_to_x(*end as f64);
            (xs.max(0.0), xe.min(viewport_width as f32))
        }
    };

    let mut output = WidgetOutput::default();

    // 4. Generate line instances based on style
    let half_w = level.line_width * 0.5;
    match &level.style {
        LineStyle::Solid => {
            output.lines.push(GridLineInstance {
                rect: [x_start, y - half_w, x_end, y + half_w],
                color: level.color,
            });
        }
        LineStyle::Dashed { dash_len, gap_len } => {
            let mut x = x_start;
            while x < x_end {
                let seg_end = (x + dash_len).min(x_end);
                output.lines.push(GridLineInstance {
                    rect: [x, y - half_w, seg_end, y + half_w],
                    color: level.color,
                });
                x += dash_len + gap_len;
            }
        }
        LineStyle::Dotted { dot_spacing } => {
            let dot_size = level.line_width.max(2.0);
            let mut x = x_start;
            while x < x_end {
                output.lines.push(GridLineInstance {
                    rect: [x, y - dot_size * 0.5, x + dot_size, y + dot_size * 0.5],
                    color: level.color,
                });
                x += dot_spacing;
            }
        }
    }

    // 5. Selection highlight (thicker glow behind the main line)
    if is_selected {
        let glow_hw = half_w + 2.0;
        let mut glow_color = level.color;
        glow_color[0] = (glow_color[0] + 0.3).min(1.0);
        glow_color[1] = (glow_color[1] + 0.3).min(1.0);
        glow_color[2] = (glow_color[2] + 0.3).min(1.0);
        glow_color[3] = 0.4;
        // Insert glow BEFORE the main line so it renders behind
        output.lines.insert(0, GridLineInstance {
            rect: [x_start, y - glow_hw, x_end, y + glow_hw],
            color: glow_color,
        });
    }

    // 6. Price badge on Y-axis
    output.labels.push(WidgetLabel {
        text: format_price(level.price),
        screen_x: viewport_width as f32 - 2.0, // right-aligned
        screen_y: y,
        bg_color: [level.color[0], level.color[1], level.color[2], 0.8],
        text_color: [1.0, 1.0, 1.0, 1.0],
        font_size: 11.0,
        anchor: LabelAnchor::Right,
    });

    // 7. User label (if present)
    if let Some(ref label_text) = level.label {
        output.labels.push(WidgetLabel {
            text: label_text.clone(),
            screen_x: x_start + 8.0,
            screen_y: y - half_w - 12.0,
            bg_color: [0.0, 0.0, 0.0, 0.0], // transparent bg
            text_color: level.color,
            font_size: 10.0,
            anchor: LabelAnchor::Left,
        });
    }

    // 8. Hit zone (full line length, +/-4px vertical tolerance)
    if !annotation.locked {
        output.hit_zones.push(HitZone {
            rect: [x_start, y - 4.0, x_end, y + 4.0],
            kind: HitZoneKind::LevelLine,
            cursor: CursorIcon::ResizeNS,
        });
    }

    Some(output)
}
```

### 2.3 Interaction

The `LevelTool` state machine manages placement and dragging. It remains
largely unchanged from the existing implementation.

**State machine**:

```
Idle
  |  H key or toolbar button
  v
Placing
  |  cursor moves -> preview line follows mouse Y (OHLC snap unless Alt held)
  |  left click -> CreateLevel { price }
  v
Idle
```

```
Idle
  |  left-press on level hit zone
  v
Dragging { level_id, grab_offset }
  |  cursor moves -> DragLevel { id, new_price } (OHLC snap)
  |  left release -> commit new price
  v
Idle
```

**OHLC snap**: The existing `LevelTool::snap_to_ohlc()` function searches
candles within +/-1 of the candle nearest to cursor X, and snaps to the
nearest O/H/L/C value within an adaptive pixel threshold
(15-40px based on candle density). Alt key disables snap.

**Keyboard shortcuts**:
| Key | Action |
|---|---|
| `H` | Activate level placement mode |
| `Delete` | Delete selected level |
| `Escape` | Cancel placement / deselect |
| Arrow keys | Nudge selected level by price step |

**Right-click context menu** (implemented in midas-app):
```
+----------------------------+
| Edit Label...              |
| Change Color...            |
| Change Style...            |
| -------------------------- |
| Lock / Unlock              |
| Hide                       |
| Delete                     |
+----------------------------+
```

### 2.4 Migration Plan

The migration replaces `Vec<HorizontalLevel>` with `AnnotationStore`
containing `AnnotationKind::Level(HorizontalLevel)` variants.

**Step 1**: Create `annotations/types.rs` with `HorizontalLevel` and shared types.

**Step 2**: Create `annotations/store.rs` with `AnnotationStore` wrapping
`Vec<Annotation>` and providing CRUD operations with monotonic ID generation.

**Step 3**: Add a conversion function:

```rust
impl From<HorizontalLevel> for Annotation {
    fn from(old: HorizontalLevel) -> Self {
        Annotation {
            id: AnnotationId(old.id),
            kind: AnnotationKind::Level(HorizontalLevel {
                price: old.price,
                color: old.color,
                line_width: old.line_width,
                style: LineStyle::Solid,
                label: old.label,
                extend: LevelExtend::FullWidth,
                icon: old.icon,
            }),
            presence: Presence::Active,
            created_at: 0,
            modified_at: 0,
            locked: old.locked,
            visible_timeframes: None,
        }
    }
}
```

**Step 4**: Replace `ChartState.levels: Vec<HorizontalLevel>` with
`ChartState.annotations: AnnotationStore`. Update `compute_levels()` to read
from the store, filtering for `AnnotationKind::Level`.

**Step 5**: Update `interaction.rs` to route level hit-testing through
`AnnotationStore` instead of the direct `Vec`.

**Step 6**: Update persistence. First run migrates levels from `config.toml`
into per-symbol JSON files.

**Verification**: All existing tests pass with zero behavior change. The
`LevelTool` state machine is unchanged. Level create/drag/delete works
identically.

### 2.5 Visual Reference

```
                          Solid level (green):
+-----------------------------------------------------------------+
|                                                                 |
|  ===============================================================|
|                                              +---------+        |
|                                              | 185.50  |        |
|                                              +---------+        |
|                                                                 |

                          Dashed level (red, with label):
+-----------------------------------------------------------------+
|  "Resistance"                                                   |
|  ---- ---- ---- ---- ---- ---- ---- ---- ---- ---- ---- ----   |
|                                              +---------+        |
|                                              | 192.30  |        |
|                                              +---------+        |

                          Selected level (glow effect):
+-----------------------------------------------------------------+
|  ==========[GLOW]=============================================  |
|  =================== "Support" ================================ |
|  ==========[GLOW]=============================================  |
|                                              +---------+        |
|                                              | 175.00  |        |
+-----------------------------------------------------------------+
```

---

## 3. Order Bracket

**Category**: Annotation | **Priority**: v1 | **Status**: New implementation

### 3.1 Data Model

```rust
/// A trade idea: entry + optional TP + optional SL.
/// The chart crate treats this as pure geometry. The app layer
/// bridges to midas-broker for order submission.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OrderBracket {
    /// Entry price line. Always present.
    pub entry: BracketLeg,

    /// Take-profit target. None if not yet set.
    pub take_profit: Option<BracketLeg>,

    /// Stop-loss level. None if not yet set.
    pub stop_loss: Option<BracketLeg>,

    /// Trade direction. Determines which side TP/SL go on
    /// and the default color scheme.
    pub side: BracketSide,

    /// Visual status. Controls line style and opacity.
    /// The chart crate uses this for rendering only.
    pub status: BracketStatus,

    /// Display quantity (informational label, not order routing).
    /// Shown next to the entry label as "100 shares" or "2 contracts".
    pub quantity: Option<f64>,
}

/// A single price leg of a bracket (entry, TP, or SL).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BracketLeg {
    /// Price level for this leg.
    pub price: f64,

    /// Optional time anchor. None = full-width ray from left edge.
    /// Some(ts) = ray starting at timestamp, extending right.
    pub timestamp: Option<i64>,

    /// Override color. If None, derived from BracketSide + leg role.
    pub color: Option<[f32; 4]>,

    /// Line rendering style for this leg.
    pub style: LineStyle,

    /// Line thickness in logical pixels.
    pub line_width: f32,

    /// Label shown next to the price badge.
    /// Examples: "Entry 185.50", "TP +2.5%", "SL -1.2%".
    pub label: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BracketSide {
    /// Buy: entry below TP, above SL.
    Long,
    /// Sell: entry above TP, below SL.
    Short,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum BracketStatus {
    /// Being drawn on chart, not yet actionable.
    #[default]
    Draft,
    /// Submitted to broker, awaiting entry fill.
    Pending,
    /// Entry partially filled.
    PartialFill,
    /// Entry filled, TP/SL orders live at broker.
    Active,
    /// TP or SL triggered, position closed.
    Closed,
    /// User or broker cancelled.
    Cancelled,
}
```

### 3.2 Default Color Scheme

All colors are RGBA in linear space.

```rust
mod bracket_colors {
    /// Long entry: confident green.
    pub const LONG_ENTRY:    [f32; 4] = [0.15, 0.65, 0.35, 0.90];
    /// Long TP: lighter green.
    pub const LONG_TP:       [f32; 4] = [0.15, 0.65, 0.35, 0.50];
    /// Long SL: danger red.
    pub const LONG_SL:       [f32; 4] = [0.65, 0.15, 0.15, 0.50];
    /// Long TP zone fill: very subtle green tint.
    pub const LONG_TP_FILL:  [f32; 4] = [0.15, 0.65, 0.35, 0.06];
    /// Long SL zone fill: very subtle red tint.
    pub const LONG_SL_FILL:  [f32; 4] = [0.65, 0.15, 0.15, 0.06];

    /// Short entry: confident red.
    pub const SHORT_ENTRY:   [f32; 4] = [0.65, 0.15, 0.15, 0.90];
    /// Short TP: lighter red (profit is downward for shorts).
    pub const SHORT_TP:      [f32; 4] = [0.15, 0.65, 0.35, 0.50];
    /// Short SL: danger green (loss is upward for shorts).
    pub const SHORT_SL:      [f32; 4] = [0.65, 0.15, 0.15, 0.50];
    /// Short fill colors mirror Long fills.
    pub const SHORT_TP_FILL: [f32; 4] = [0.15, 0.65, 0.35, 0.06];
    pub const SHORT_SL_FILL: [f32; 4] = [0.65, 0.15, 0.15, 0.06];
}
```

### 3.3 Render Output

A complete bracket (entry + TP + SL) produces the following visual:

```
+-----------------------------------------------------------------+
|                                                                 |
|                                              +---------+        |
|  - - - - - - - - - - - - - - - - - - - - -  | 192.00  | TP     |
|  ///////////////////////////////////////////  | +3.5%   |        |
|  ////// TP zone (green tint, alpha=0.06) ///  +---------+        |
|  ///////////////////////////////////////////                     |
|  =============================================== +---------+    |
|                                              | 185.50  | Entry  |
|                                              |  R:R 1.9 |        |
|  =============================================== +---------+    |
|  /////////// SL zone (red tint, alpha=0.06) //                  |
|  ///////////////////////////////////////////                     |
|  - - - - - - - - - - - - - - - - - - - - -  +---------+        |
|                                              | 182.00  | SL     |
|                                              | -1.9%   |        |
|                                              +---------+        |
+-----------------------------------------------------------------+
```

#### Instance Breakdown

```rust
/// Compute render data for an order bracket.
pub fn compute_bracket(
    bracket: &OrderBracket,
    annotation: &Annotation,
    camera: &Camera2D,
    viewport_width: u32,
    is_selected: bool,
) -> WidgetOutput {
    let mut output = WidgetOutput::default();

    let vw = viewport_width as f32;

    // --- Determine visual modifiers from status ---
    let (line_style_override, alpha_mult) = match bracket.status {
        BracketStatus::Draft      => (Some(LineStyle::Dashed { dash_len: 8.0, gap_len: 4.0 }), 1.0),
        BracketStatus::Pending    => (Some(LineStyle::Dotted { dot_spacing: 6.0 }), 1.0),
        BracketStatus::PartialFill => (None, 1.0),  // solid, full opacity
        BracketStatus::Active     => (None, 1.0),    // solid, full opacity
        BracketStatus::Closed     => (None, 0.3),    // solid, dimmed
        BracketStatus::Cancelled  => (None, 0.3),    // solid, dimmed
    };

    // --- Entry line ---
    let entry_y = camera.price_to_y(bracket.entry.price);
    let entry_color = resolve_leg_color(
        &bracket.entry, bracket.side, LegRole::Entry, alpha_mult
    );
    emit_leg_line(&mut output, &bracket.entry, entry_y, 0.0, vw,
                  entry_color, &line_style_override);

    // --- TP line + zone fill ---
    if let Some(ref tp) = bracket.take_profit {
        let tp_y = camera.price_to_y(tp.price);
        let tp_color = resolve_leg_color(tp, bracket.side, LegRole::TakeProfit, alpha_mult);
        emit_leg_line(&mut output, tp, tp_y, 0.0, vw, tp_color, &line_style_override);

        // Zone fill between entry and TP
        let fill_color = match bracket.side {
            BracketSide::Long  => apply_alpha(bracket_colors::LONG_TP_FILL, alpha_mult),
            BracketSide::Short => apply_alpha(bracket_colors::SHORT_TP_FILL, alpha_mult),
        };
        let (fill_top, fill_bottom) = if entry_y < tp_y {
            (entry_y, tp_y)
        } else {
            (tp_y, entry_y)
        };
        output.fills.push(GridLineInstance {
            rect: [0.0, fill_top, vw, fill_bottom],
            color: fill_color,
        });
    }

    // --- SL line + zone fill ---
    if let Some(ref sl) = bracket.stop_loss {
        let sl_y = camera.price_to_y(sl.price);
        let sl_color = resolve_leg_color(sl, bracket.side, LegRole::StopLoss, alpha_mult);
        emit_leg_line(&mut output, sl, sl_y, 0.0, vw, sl_color, &line_style_override);

        // Zone fill between entry and SL
        let fill_color = match bracket.side {
            BracketSide::Long  => apply_alpha(bracket_colors::LONG_SL_FILL, alpha_mult),
            BracketSide::Short => apply_alpha(bracket_colors::SHORT_SL_FILL, alpha_mult),
        };
        let (fill_top, fill_bottom) = if entry_y < sl_y {
            (entry_y, sl_y)
        } else {
            (sl_y, entry_y)
        };
        output.fills.push(GridLineInstance {
            rect: [0.0, fill_top, vw, fill_bottom],
            color: fill_color,
        });
    }

    // --- Price labels ---
    emit_bracket_labels(&mut output, bracket, camera, vw, alpha_mult);

    // --- R:R ratio ---
    if let Some(rr) = compute_risk_reward(bracket) {
        output.labels.push(WidgetLabel {
            text: format!("R:R {:.1}:1", rr),
            screen_x: vw - 80.0,
            screen_y: entry_y - 14.0,
            bg_color: [0.0, 0.0, 0.0, 0.0],
            text_color: [0.7, 0.7, 0.7, alpha_mult],
            font_size: 10.0,
            anchor: LabelAnchor::Right,
        });
    }

    // --- Hit zones (only for Draft/Active, not Closed/Cancelled) ---
    if bracket.status != BracketStatus::Closed
        && bracket.status != BracketStatus::Cancelled
        && !annotation.locked
    {
        emit_bracket_hit_zones(&mut output, bracket, annotation.id, camera, vw);
    }

    output
}
```

#### Helper: Risk/Reward Computation

```rust
/// Compute risk:reward ratio for a bracket with both TP and SL set.
fn compute_risk_reward(bracket: &OrderBracket) -> Option<f64> {
    let tp = bracket.take_profit.as_ref()?;
    let sl = bracket.stop_loss.as_ref()?;
    let risk = (bracket.entry.price - sl.price).abs();
    let reward = (tp.price - bracket.entry.price).abs();
    if risk < f64::EPSILON {
        return None;
    }
    Some(reward / risk)
}
```

#### Helper: Leg Color Resolution

```rust
enum LegRole { Entry, TakeProfit, StopLoss }

/// Resolve the display color for a bracket leg.
/// Uses the leg's override color if set, otherwise derives from side + role.
fn resolve_leg_color(
    leg: &BracketLeg,
    side: BracketSide,
    role: LegRole,
    alpha_mult: f32,
) -> [f32; 4] {
    let base = match leg.color {
        Some(c) => c,
        None => match (side, role) {
            (BracketSide::Long,  LegRole::Entry)      => bracket_colors::LONG_ENTRY,
            (BracketSide::Long,  LegRole::TakeProfit)  => bracket_colors::LONG_TP,
            (BracketSide::Long,  LegRole::StopLoss)    => bracket_colors::LONG_SL,
            (BracketSide::Short, LegRole::Entry)       => bracket_colors::SHORT_ENTRY,
            (BracketSide::Short, LegRole::TakeProfit)  => bracket_colors::SHORT_TP,
            (BracketSide::Short, LegRole::StopLoss)    => bracket_colors::SHORT_SL,
        },
    };
    apply_alpha(base, alpha_mult)
}

fn apply_alpha(mut color: [f32; 4], mult: f32) -> [f32; 4] {
    color[3] *= mult;
    color
}
```

### 3.4 Visual States

Each `BracketStatus` maps to a distinct visual treatment:

```
Draft:
  Lines: dashed (8px dash, 4px gap)
  Colors: full opacity
  Interaction: full (drag any leg, delete, submit)
  Purpose: user is building a trade idea

Pending:
  Lines: dotted (6px spacing)
  Colors: full opacity
  Interaction: limited (drag sends modify to broker, confirmation required)
  Badge: "PENDING" next to entry label
  Purpose: orders submitted, waiting for fill

PartialFill:
  Lines: solid
  Colors: full opacity, entry line pulses (alternating alpha 0.7-1.0)
  Interaction: same as Pending
  Badge: "PARTIAL" with filled quantity
  Purpose: entry partially filled

Active:
  Lines: solid
  Colors: full opacity
  Interaction: drag sends modify to broker with confirmation
  Badge: "LIVE" badge in distinct color (amber)
  Purpose: position is open, TP/SL orders are live

Closed:
  Lines: solid
  Colors: all alphas multiplied by 0.3 (dimmed)
  Interaction: none (select only, no drag)
  Auto-behavior: hidden after 30 seconds (configurable)
  Purpose: historical trade result

Cancelled:
  Lines: solid
  Colors: all alphas multiplied by 0.3 (dimmed)
  Interaction: none (select only, no drag)
  Purpose: aborted trade
```

### 3.5 Bracket Drawing Interaction

The bracket tool uses a multi-click state machine:

```rust
pub enum BracketDrawPhase {
    /// Cursor shown, waiting for entry click. Preview line follows mouse.
    WaitingEntry,

    /// Entry placed. TP preview line follows mouse.
    /// Constraint: TP must be on the correct side of entry.
    WaitingTP {
        entry_price: f64,
        entry_time: i64,
    },

    /// Entry + TP placed. SL preview line follows mouse.
    /// Constraint: SL must be on the opposite side of entry from TP.
    WaitingSL {
        entry_price: f64,
        entry_time: i64,
        tp_price: f64,
    },
}
```

**State transition diagram**:

```
                Idle
                 |
                 | B key (Long) or Shift+B (Short)
                 v
          WaitingEntry
           |         |
    Click  |         | Escape -> Idle
           v
          WaitingTP
           |         |
    Click  |         | Escape -> Idle
           |         | Enter  -> create bracket (entry only, no TP/SL)
           v
          WaitingSL
           |         |
    Click  |         | Escape -> Idle
           |         | Enter  -> create bracket (entry + TP, no SL)
           v
      CreateBracket -> Idle
```

**Preview rendering during drawing**: While in any `Waiting*` phase, the
compute pipeline renders:
- Solid lines at already-committed prices
- A ghost dashed line following cursor Y for the leg being placed
- Zone fill preview between committed entry and cursor position

**Constraint enforcement during drawing**:

```rust
fn constrain_price(
    candidate: f64,
    side: BracketSide,
    role: LegRole,
    entry_price: f64,
) -> f64 {
    match (side, role) {
        (BracketSide::Long, LegRole::TakeProfit) => candidate.max(entry_price + 0.01),
        (BracketSide::Long, LegRole::StopLoss)   => candidate.min(entry_price - 0.01),
        (BracketSide::Short, LegRole::TakeProfit) => candidate.min(entry_price - 0.01),
        (BracketSide::Short, LegRole::StopLoss)   => candidate.max(entry_price + 0.01),
        (_, LegRole::Entry) => candidate,
    }
}
```

### 3.6 Bracket Leg Dragging

When the user grabs a bracket leg:

```rust
// In handle_mouse_move during DraggingBracketLeg:
let raw_price = camera.y_to_price(mouse_y) + grab_offset;

// Apply OHLC snap (same snap logic as level tool)
let snapped = level_tool.snap_to_ohlc(raw_price, mouse_x, camera, data, collapsed);

// Enforce side constraints -- if user drags entry past TP, swap them
match leg {
    BracketLegKind::Entry => {
        // Entry is unconstrained; TP/SL swap if crossed
        if let Some(ref tp) = bracket.take_profit {
            if (side == Long && snapped > tp.price)
                || (side == Short && snapped < tp.price)
            {
                // Swap entry and TP
                std::mem::swap(&mut bracket.entry, bracket.take_profit.as_mut().unwrap());
                bracket.entry.price = snapped;
            }
        }
        // Similar for SL
    }
    BracketLegKind::TakeProfit => {
        bracket.take_profit.as_mut().unwrap().price = constrain_price(
            snapped, side, LegRole::TakeProfit, bracket.entry.price
        );
    }
    BracketLegKind::StopLoss => {
        bracket.stop_loss.as_mut().unwrap().price = constrain_price(
            snapped, side, LegRole::StopLoss, bracket.entry.price
        );
    }
}
```

### 3.7 Hit Zones

Each bracket produces multiple hit zones with different drag behaviors:

```rust
fn emit_bracket_hit_zones(
    output: &mut WidgetOutput,
    bracket: &OrderBracket,
    id: AnnotationId,
    camera: &Camera2D,
    viewport_width: f32,
) {
    let entry_y = camera.price_to_y(bracket.entry.price);

    // Entry line: drag moves entire bracket (all legs shift by same delta)
    output.hit_zones.push(HitZone {
        rect: [0.0, entry_y - 6.0, viewport_width, entry_y + 6.0],
        kind: HitZoneKind::BracketEntry,
        cursor: CursorIcon::ResizeNS,
    });

    // TP line: drag moves only TP
    if let Some(ref tp) = bracket.take_profit {
        let tp_y = camera.price_to_y(tp.price);
        output.hit_zones.push(HitZone {
            rect: [0.0, tp_y - 6.0, viewport_width, tp_y + 6.0],
            kind: HitZoneKind::BracketTP,
            cursor: CursorIcon::ResizeNS,
        });
    }

    // SL line: drag moves only SL
    if let Some(ref sl) = bracket.stop_loss {
        let sl_y = camera.price_to_y(sl.price);
        output.hit_zones.push(HitZone {
            rect: [0.0, sl_y - 6.0, viewport_width, sl_y + 6.0],
            kind: HitZoneKind::BracketSL,
            cursor: CursorIcon::ResizeNS,
        });
    }

    // Zone fills: click selects the bracket (no drag)
    // Between entry and TP
    if let Some(ref tp) = bracket.take_profit {
        let tp_y = camera.price_to_y(tp.price);
        let (top, bottom) = ordered(entry_y, tp_y);
        output.hit_zones.push(HitZone {
            rect: [0.0, top, viewport_width, bottom],
            kind: HitZoneKind::BracketZone,
            cursor: CursorIcon::Pointer,
        });
    }
    // Between entry and SL (same pattern)
}
```

### 3.8 Complex Scenarios

**Multiple take profit levels**: The v1 model supports a single TP. For
partial-exit strategies (e.g., sell 50% at TP1, 50% at TP2), extend the
model in v2:

```rust
// Future v2 extension:
pub struct OrderBracket {
    pub entry: BracketLeg,
    pub stop_loss: Option<BracketLeg>,
    pub take_profits: Vec<BracketLegWithQuantity>,  // ordered by distance from entry
    // ...
}

pub struct BracketLegWithQuantity {
    pub leg: BracketLeg,
    pub quantity_pct: f32,  // percentage of total position (e.g., 0.5 = 50%)
}
```

**Broker-sourced brackets**: When orders come from the broker (not drawn by
user), they appear with a distinct visual style:

```rust
// Broker-sourced brackets:
// - Entry line: solid, same color scheme
// - Badge: "LIVE" in amber [0.9, 0.7, 0.1, 0.8] background
// - Line pattern: slightly different dash pattern to distinguish from user-drawn
// - Interaction: drag sends modify to broker (with confirmation dialog)
```

**Trailing stop**: Future extension. The SL leg would have a
`trail_distance: Option<f64>` field. When active, the chart computes the
current trailing stop price from the highest/lowest price since entry fill.

---

## 4. G.ATR Indicator

**Category**: Indicator | **Priority**: v1 (migration) | **Status**: Existing
implementation in `gerchik_atr.rs`

### 4.1 Data Model

```rust
/// Configuration for the Gerchik ATR indicator.
/// Stored per-chart in chart settings, not in AnnotationStore.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GerchikAtrConfig {
    /// ATR period (number of daily bars for smoothing). Default: 14.
    pub period: usize,

    /// Percentage threshold: below = green, at or above = red. Default: 75.0.
    pub threshold_pct: f32,

    /// Screen position for the badge. Default: TopRight.
    pub position: BadgePosition,

    /// Whether the indicator is enabled. Default: true for intraday charts.
    pub enabled: bool,
}

impl Default for GerchikAtrConfig {
    fn default() -> Self {
        Self {
            period: 14,
            threshold_pct: 75.0,
            position: BadgePosition::TopRight,
            enabled: true,
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub enum BadgePosition {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}
```

### 4.2 Render Output

The G.ATR is a **presence widget**: a text badge overlaid on the chart. It
produces no `GridLineInstance` data -- only a single `WidgetLabel`.

```
+-----------------------------------------------------------------+
|                                                                 |
|                                              +----------+       |
|                                              | G.ATR 67%|       |
|                                              +----------+       |
|                                              (green bg,         |
|                                               watermark alpha)  |
|                                                                 |
|       [ chart content ]                                         |
|                                                                 |
+-----------------------------------------------------------------+
```

**Render data** (existing `GerchikAtrRender`):

```rust
pub struct GerchikAtrRender {
    /// ATR percentage consumed (0.0+, can exceed 100).
    pub pct: f32,
    /// Display text (e.g. "G.ATR 67%").
    pub text: String,
    /// RGBA color: green [0.2, 0.8, 0.3, 0.18] or red [0.9, 0.25, 0.2, 0.18].
    pub color: [f32; 4],
}
```

In the widget system, this maps to a single `WidgetLabel`:

```rust
fn gatr_to_widget_label(
    render: &GerchikAtrRender,
    config: &GerchikAtrConfig,
    viewport_width: u32,
    viewport_height: u32,
) -> WidgetLabel {
    let (x, y) = match config.position {
        BadgePosition::TopRight => (viewport_width as f32 - 10.0, 24.0),
        BadgePosition::TopLeft => (10.0, 24.0),
        BadgePosition::BottomRight => (viewport_width as f32 - 10.0, viewport_height as f32 - 24.0),
        BadgePosition::BottomLeft => (10.0, viewport_height as f32 - 24.0),
    };

    WidgetLabel {
        text: render.text.clone(),
        screen_x: x,
        screen_y: y,
        bg_color: render.color, // watermark-style, low alpha
        text_color: render.color, // same color but rendered as text
        font_size: 24.0,         // large watermark text
        anchor: match config.position {
            BadgePosition::TopRight | BadgePosition::BottomRight => LabelAnchor::Right,
            BadgePosition::TopLeft | BadgePosition::BottomLeft => LabelAnchor::Left,
        },
    }
}
```

### 4.3 Compute Function

The existing `compute_gerchik_atr()` function is preserved unchanged. It:

1. Rejects non-intraday charts (candle duration >= 1 day)
2. Aggregates intraday candles into synthetic daily bars by UTC calendar day
3. Computes ATR using Wilder's smoothing method
4. Calculates the percentage of current session range vs ATR
5. Produces `GerchikAtrRender` with formatted text and color

```rust
pub fn compute_gerchik_atr(
    data: &dyn CandleData,
    candle_duration_ms: f64,
) -> Option<GerchikAtrRender>
```

### 4.4 Indicator vs Annotation Distinction

The G.ATR is the canonical example of how indicators differ from annotations:

| Property | Annotation (Level) | Indicator (G.ATR) |
|---|---|---|
| Created by | User action | Automatic from data |
| Stored in | AnnotationStore | Not stored (computed on-the-fly) |
| Persisted in | per-symbol JSON | chart config (enabled/threshold/position) |
| Has AnnotationId | Yes | No |
| Has hit zones | Yes (drag/select) | No (non-interactive) |
| Recomputed | On camera change | On camera change + data change |
| Deletable | Yes (user can delete) | No (toggle on/off in settings) |
| Per-instance data | Yes (each level is unique) | No (one config, one output) |

### 4.5 Migration Plan

The existing `gerchik_atr.rs` file remains largely intact. The migration:

1. Move `GerchikAtrConfig` to `annotations/indicators.rs` (or a new
   `indicators/` module alongside `annotations/`)
2. Keep `compute_gerchik_atr()` and `GerchikAtrRender` in `gerchik_atr.rs`
3. Add `gatr_to_widget_label()` adapter function
4. Add `gatr_config: GerchikAtrConfig` to `ChartSettings` (per-chart config)
5. Wire the output into `ChartScene.indicator_labels: Vec<WidgetLabel>`

The G.ATR compute function remains a pure function: `CandleData` in,
`GerchikAtrRender` out. No architectural change needed.

---

## 5. Volume Profile

**Category**: Indicator | **Priority**: v1 | **Status**: Existing
implementation in `volume_profile.rs`

### 5.1 Data Model

```rust
/// Configuration for the Volume Profile indicator.
/// Stored per-chart in chart settings.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VolumeProfileConfig {
    /// What time range to compute the profile over.
    pub period: VolumeProfilePeriod,

    /// Number of price bins. More bins = finer resolution. Default: 50.
    pub num_bins: usize,

    /// Which side of the chart to render the histogram. Default: Left.
    pub side: VolumeProfileSide,

    /// Whether to highlight the 70% value area. Default: true.
    pub show_value_area: bool,

    /// Whether to show the Point of Control line. Default: true.
    pub show_poc: bool,

    /// Color scheme.
    pub colors: VolumeProfileColors,

    /// Maximum width as a fraction of chart width. Default: 0.25 (25%).
    pub width_pct: f32,

    /// Whether the indicator is enabled. Default: true.
    pub enabled: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum VolumeProfilePeriod {
    /// Currently visible candles only. Recomputes on pan/zoom.
    VisibleRange,

    /// Current trading session. Resets at session boundaries.
    Session,

    /// Fixed time range. Does not change with pan/zoom.
    Custom { start_ms: i64, end_ms: i64 },
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub enum VolumeProfileSide {
    /// Render histogram from the left edge.
    #[default]
    Left,
    /// Render histogram from the right edge.
    Right,
    /// Render from both edges (mirrored).
    Both,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VolumeProfileColors {
    /// Buy volume bar color.
    pub buy: [f32; 4],
    /// Sell volume bar color.
    pub sell: [f32; 4],
    /// Point of Control marker color.
    pub poc: [f32; 4],
    /// Value Area boundary color (if shown).
    pub value_area: [f32; 4],
}

impl Default for VolumeProfileColors {
    fn default() -> Self {
        Self {
            buy:  [0.10, 0.55, 0.55, 0.30], // teal, semi-transparent
            sell: [0.65, 0.20, 0.20, 0.30],  // red, semi-transparent
            poc:  [0.70, 0.58, 0.08, 0.45],  // muted gold
            value_area: [0.50, 0.50, 0.50, 0.15], // gray highlight
        }
    }
}

impl Default for VolumeProfileConfig {
    fn default() -> Self {
        Self {
            period: VolumeProfilePeriod::VisibleRange,
            num_bins: 50,
            side: VolumeProfileSide::Left,
            show_value_area: true,
            show_poc: true,
            colors: VolumeProfileColors::default(),
            width_pct: 0.25,
            enabled: true,
        }
    }
}
```

### 5.2 Render Output

The volume profile renders as a horizontal histogram of bars, one per price
bin. Each bar's width is proportional to the volume in that bin relative to
the maximum (POC) bin.

```
+-----------------------------------------------------------------+
|  |                                                              |
| =|====                                                          |
| =|========                                                      |
| =|==============  <- POC (longest bar, gold dot at end)         |
| =|==========                                                    |
| =|======                    [ candles here ]                    |
| =|====                                                          |
| =|==                                                            |
|  |                                                              |
+-----------------------------------------------------------------+
   ^
   histogram bars (left edge, width proportional to volume)
   teal = buy volume, red = sell volume (stacked)
```

Components:
- **N GridLineInstances per bin** (typically 2: buy portion + sell portion):
  horizontal rectangles from chart edge, height spanning the bin's price
  range, width proportional to volume.
- **POC indicator**: small gold dot (stacked scanline circle, ~4px radius)
  at the right edge of the longest bar.
- **Optional Value Area**: a subtle background highlight covering the bins
  that contain 70% of total volume.
- **Hover labels** (future): tooltip showing exact volume on hover.

### 5.3 Compute Function

The existing compute pipeline has two stages:

**Stage 1**: `compute_volume_profile()` -- bins candle volume into price bins.

```rust
/// Compute volume profile bins from visible candle data.
/// Returns None if the visible range is empty or price range is zero.
pub fn compute_volume_profile(
    data: &dyn CandleData,
    vis_start: usize,
    vis_end: usize,
    price_low: f32,
    price_high: f32,
    num_bins: usize,
) -> Option<VolumeProfile>
```

Algorithm:
1. Divide the price range `[price_low, price_high]` into `num_bins`
   equal-width bins.
2. For each visible candle, uniformly distribute its volume across the bins
   that overlap with its `[low, high]` range. Bull candles contribute to
   `buy_volume`, bear candles to `sell_volume`.
3. Track the POC bin (highest total volume).
4. Return `VolumeProfile { bins, poc_index, total_volume }`.

**Stage 2**: `profile_to_instances()` -- converts bins to GPU-ready rects.

```rust
/// Convert a VolumeProfile into GPU-ready GridLineInstance rectangles.
pub fn profile_to_instances(
    profile: &VolumeProfile,
    camera: &Camera2D,
    viewport_width: u32,
) -> Vec<GridLineInstance>
```

Algorithm:
1. Find the maximum bin volume for normalization.
2. For each non-empty bin:
   - Compute Y range from `camera.price_to_y(bin.price_high)` to
     `camera.price_to_y(bin.price_low)`.
   - Compute bar width as `(bin.total / max_vol) * max_bar_px` where
     `max_bar_px = viewport_width * width_pct`.
   - Split into buy (left) and sell (right of buy) portions.
   - Emit `GridLineInstance` for each non-trivial portion.
3. Emit POC dot as stacked scanline rects (circle approximation).

### 5.4 Value Area Computation

The Value Area represents the price range containing 70% of total volume,
centered on the POC. This is a standard Market Profile concept.

```rust
/// Compute the Value Area (70% of volume centered on POC).
/// Returns (low_bin_index, high_bin_index) inclusive.
fn compute_value_area(profile: &VolumeProfile, pct: f32) -> Option<(usize, usize)> {
    if profile.bins.is_empty() || profile.total_volume == 0 {
        return None;
    }

    let target_volume = (profile.total_volume as f64 * pct as f64 / 100.0) as u64;
    let mut accumulated = profile.bins[profile.poc_index].total();
    let mut low = profile.poc_index;
    let mut high = profile.poc_index;

    // Expand outward from POC, always adding the larger adjacent bin
    while accumulated < target_volume {
        let can_go_lower = low > 0;
        let can_go_higher = high < profile.bins.len() - 1;

        match (can_go_lower, can_go_higher) {
            (true, true) => {
                let lower_vol = profile.bins[low - 1].total();
                let upper_vol = profile.bins[high + 1].total();
                if lower_vol >= upper_vol {
                    low -= 1;
                    accumulated += lower_vol;
                } else {
                    high += 1;
                    accumulated += upper_vol;
                }
            }
            (true, false) => {
                low -= 1;
                accumulated += profile.bins[low].total();
            }
            (false, true) => {
                high += 1;
                accumulated += profile.bins[high].total();
            }
            (false, false) => break,
        }
    }

    Some((low, high))
}
```

### 5.5 Performance and Caching

Volume profile computation scales with `O(visible_candles * bins_touched_per_candle)`.
For 5000 visible candles and 50 bins, this is ~250K operations -- fast enough
for every frame.

However, the instance generation (`profile_to_instances`) is called every
frame even when the profile hasn't changed. Caching strategy:

```rust
/// Cached volume profile to avoid recomputation on every frame.
pub struct VolumeProfileCache {
    /// The computed profile data.
    profile: Option<VolumeProfile>,
    /// The GPU instances generated from the profile.
    instances: Vec<GridLineInstance>,
    /// Camera state when last computed (for invalidation).
    last_price_low: f32,
    last_price_high: f32,
    last_vis_start: usize,
    last_vis_end: usize,
}

impl VolumeProfileCache {
    /// Returns true if the cache is stale and needs recomputation.
    pub fn needs_recompute(
        &self,
        price_low: f32,
        price_high: f32,
        vis_start: usize,
        vis_end: usize,
    ) -> bool {
        self.profile.is_none()
            || (self.last_price_low - price_low).abs() > 0.001
            || (self.last_price_high - price_high).abs() > 0.001
            || self.last_vis_start != vis_start
            || self.last_vis_end != vis_end
    }
}
```

The cache invalidates when the visible range changes (pan/zoom). During
continuous zoom, every frame recomputes, but this is acceptable given the
low per-frame cost.

### 5.6 Visual Reference

```
                          Left-aligned, buy/sell split:
+-----------------------------------------------------------------+
|  B|                                                             |
|  BB|                                                            |
|  BBB|S                                                          |
|  BBBBB|SS    <- bin with mixed buy (B) and sell (S) volume      |
|  BBBBBBB|SSSS  <- POC bin (longest), gold dot at end -->  *     |
|  BBBBB|SS                                                       |
|  BBB|S                                                          |
|  BB|                                                            |
|  B|                                                             |
+-----------------------------------------------------------------+

                          With Value Area highlight:
+-----------------------------------------------------------------+
|  B|                                                             |
| [BB|            ]  <- value area boundary (subtle gray bg)      |
| [BBB|S          ]                                               |
| [BBBBB|SS      ]  <- 70% of volume falls within these bins     |
| [BBBBBBB|SSSS  ]  <- POC                                       |
| [BBBBB|SS      ]                                               |
| [BBB|S          ]                                               |
| [BB|            ]  <- value area boundary                       |
|  B|                                                             |
+-----------------------------------------------------------------+
```

---

## 6. Moving Average

**Category**: Indicator | **Priority**: v2 | **Status**: Not yet implemented

### 6.1 Data Model

```rust
/// Configuration for a Moving Average overlay.
/// A chart can have multiple MA overlays with different configs.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MovingAverageConfig {
    /// MA period (number of candles). Default: 20.
    pub period: usize,

    /// Moving average algorithm.
    pub ma_type: MaType,

    /// Which price component to use. Default: Close.
    pub source: PriceSource,

    /// Line color.
    pub color: [f32; 4],

    /// Line width in logical pixels. Default: 1.0.
    pub line_width: f32,

    /// Whether this MA is visible. Default: true.
    pub enabled: bool,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub enum MaType {
    /// Simple Moving Average: arithmetic mean of last N values.
    #[default]
    SMA,
    /// Exponential Moving Average: weighted toward recent values.
    EMA,
    /// Weighted Moving Average: linearly weighted.
    WMA,
    /// Volume-Weighted Average Price.
    VWAP,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub enum PriceSource {
    Open,
    High,
    Low,
    #[default]
    Close,
    /// (High + Low) / 2
    HL2,
    /// (High + Low + Close) / 3
    HLC3,
    /// (Open + High + Low + Close) / 4
    OHLC4,
}
```

### 6.2 Compute Function

```rust
/// Compute moving average values for visible candles.
/// Returns one MA value per visible candle (NaN for insufficient lookback).
pub fn compute_moving_average(
    data: &dyn CandleData,
    vis_start: usize,
    vis_end: usize,
    config: &MovingAverageConfig,
) -> Vec<f32> {
    let n = config.period;
    let mut values = Vec::with_capacity(vis_end - vis_start);

    // Need `period` candles of lookback before vis_start
    let lookback_start = vis_start.saturating_sub(n);

    match config.ma_type {
        MaType::SMA => {
            // Rolling sum for O(1) per-candle amortized
            let mut sum: f64 = 0.0;
            for i in lookback_start..vis_end {
                sum += get_price_source(data, i, config.source) as f64;
                if i >= lookback_start + n {
                    sum -= get_price_source(data, i - n, config.source) as f64;
                }
                if i >= vis_start {
                    if i >= lookback_start + n - 1 {
                        values.push((sum / n as f64) as f32);
                    } else {
                        values.push(f32::NAN); // insufficient data
                    }
                }
            }
        }
        MaType::EMA => {
            let multiplier = 2.0 / (n as f64 + 1.0);
            // Seed EMA with SMA of first `n` values
            // ...implementation follows standard EMA algorithm
            todo!()
        }
        MaType::WMA | MaType::VWAP => {
            todo!()
        }
    }

    values
}

fn get_price_source(data: &dyn CandleData, idx: usize, source: PriceSource) -> f32 {
    match source {
        PriceSource::Open => data.open(idx),
        PriceSource::High => data.high(idx),
        PriceSource::Low => data.low(idx),
        PriceSource::Close => data.close(idx),
        PriceSource::HL2 => (data.high(idx) + data.low(idx)) / 2.0,
        PriceSource::HLC3 => (data.high(idx) + data.low(idx) + data.close(idx)) / 3.0,
        PriceSource::OHLC4 => {
            (data.open(idx) + data.high(idx) + data.low(idx) + data.close(idx)) / 4.0
        }
    }
}
```

### 6.3 Render Output (v1 Workaround)

The ideal rendering is a smooth diagonal line connecting MA values at each
candle position. This requires a `LineInstance` pipeline (diagonal line
segments) that does not yet exist.

**v1 workaround**: Render as horizontal segments connecting adjacent candle
X positions. Each segment is a `GridLineInstance` rectangle spanning from
`candle[i].x` to `candle[i+1].x` at the average of the two MA values.

```rust
/// Convert MA values to GridLineInstances using horizontal-step approximation.
/// This is a temporary workaround until the LineInstance pipeline is available.
fn ma_to_step_instances(
    ma_values: &[f32],
    candle_xs: &[f32],   // X positions of each candle center
    camera: &Camera2D,
    config: &MovingAverageConfig,
) -> Vec<GridLineInstance> {
    let mut instances = Vec::with_capacity(ma_values.len());
    let half_w = config.line_width * 0.5;

    for i in 0..ma_values.len().saturating_sub(1) {
        if ma_values[i].is_nan() || ma_values[i + 1].is_nan() {
            continue;
        }

        // Average Y position between this candle and next
        let y = camera.price_to_y(
            ((ma_values[i] + ma_values[i + 1]) / 2.0) as f64
        );

        instances.push(GridLineInstance {
            rect: [candle_xs[i], y - half_w, candle_xs[i + 1], y + half_w],
            color: config.color,
        });
    }

    instances
}
```

**Visual quality**: The step approximation looks acceptable when zoomed in
(many pixels per candle) but becomes noticeably jagged at low zoom levels.
This is acceptable for v1 because moving averages are a secondary feature.

### 6.4 Why Deferred to v2

The `GridLineInstance` pipeline only supports axis-aligned rectangles. Proper
diagonal lines connecting MA values require a new `LineInstance` pipeline
with:

```rust
/// Future: GPU instance for a line segment between two arbitrary points.
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct LineInstance {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
    pub width: f32,
    pub _pad: f32,
    pub color: [f32; 4],
}
// Size: 40 bytes per instance
```

This requires a new WGSL shader that expands line segments into quads in the
vertex shader. The implementation is straightforward but represents a new
render pipeline, which is out of scope for v1.

### 6.5 Visual Reference

```
                          v1 workaround (horizontal steps):
+-----------------------------------------------------------------+
|                                                                 |
|        ____                                                     |
|  _____/    \____                                                |
| /               \                                               |
|/                 \____     ____                                  |
|                       \___/    \                                 |
|                                \___                             |
|                                                                 |
+-----------------------------------------------------------------+
   Each flat segment is one GridLineInstance. Steps visible at transitions.

                          v2 proper (diagonal lines):
+-----------------------------------------------------------------+
|                                                                 |
|        /\                                                       |
|  _____/  \____                                                  |
| /             \                                                 |
|/               \____     /\                                     |
|                     \___/  \                                    |
|                             \___                                |
|                                                                 |
+-----------------------------------------------------------------+
   Smooth line, each diagonal segment is one LineInstance.
```

---

## 7. Velocity/Momentum Indicator

**Category**: Indicator | **Priority**: v2 | **Status**: Not yet implemented

### 7.1 Concept

A per-candle colored background overlay showing momentum or velocity. Renders
as colored bars behind the candles (in the fills layer, below candle wicks).
Useful for at-a-glance momentum assessment.

### 7.2 Data Model

```rust
/// Configuration for the Velocity/Momentum indicator.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VelocityConfig {
    /// Number of candles to look back for momentum calculation. Default: 10.
    pub lookback: usize,

    /// Momentum calculation method.
    pub method: VelocityMethod,

    /// Color gradient from bearish (negative momentum) to bullish (positive).
    pub color_map: VelocityColorMap,

    /// How to render the momentum values.
    pub render_style: VelocityStyle,

    /// Opacity multiplier for the overlay. Default: 0.15 (subtle background).
    pub opacity: f32,

    /// Whether the indicator is enabled. Default: false (opt-in).
    pub enabled: bool,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub enum VelocityMethod {
    /// Rate of Change: (close - close[lookback]) / close[lookback]
    #[default]
    ROC,
    /// Simple momentum: close - close[lookback]
    Momentum,
    /// RSI-based: 14-period RSI mapped to -1..+1
    RSI,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VelocityColorMap {
    /// Color for maximum bearish momentum (e.g., bright red).
    pub bearish: [f32; 4],
    /// Color for neutral momentum (e.g., gray).
    pub neutral: [f32; 4],
    /// Color for maximum bullish momentum (e.g., bright green).
    pub bullish: [f32; 4],
}

impl Default for VelocityColorMap {
    fn default() -> Self {
        Self {
            bearish: [0.8, 0.15, 0.15, 1.0],
            neutral: [0.3, 0.3, 0.3, 1.0],
            bullish: [0.15, 0.7, 0.3, 1.0],
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub enum VelocityStyle {
    /// Full-height background bars behind each candle.
    #[default]
    Bars,
    /// Continuous heatmap (bins cover full price range per candle).
    Heatmap,
}
```

### 7.3 Compute Function

```rust
/// Compute per-candle momentum values for the visible range.
/// Returns a value in [-1.0, +1.0] for each visible candle.
/// NaN for candles with insufficient lookback.
pub fn compute_velocity(
    data: &dyn CandleData,
    vis_start: usize,
    vis_end: usize,
    config: &VelocityConfig,
) -> Vec<f32> {
    let mut values = Vec::with_capacity(vis_end - vis_start);
    let n = config.lookback;

    for i in vis_start..vis_end {
        if i < n {
            values.push(f32::NAN);
            continue;
        }

        let current = data.close(i) as f64;
        let past = data.close(i - n) as f64;

        let raw = match config.method {
            VelocityMethod::ROC => {
                if past.abs() < f64::EPSILON { 0.0 }
                else { (current - past) / past }
            }
            VelocityMethod::Momentum => {
                current - past
            }
            VelocityMethod::RSI => {
                todo!() // RSI normalization to [-1, +1]
            }
        };

        // Normalize to [-1, +1] using sigmoid-like clamping
        // The exact normalization factor depends on the typical range
        // for the asset class; use a running percentile in production.
        let normalized = (raw * 10.0).tanh() as f32; // rough sigmoid
        values.push(normalized);
    }

    values
}
```

### 7.4 Render Output

Each candle gets one `GridLineInstance` as a full-height background bar.
The color is interpolated between bearish/neutral/bullish based on the
momentum value.

```rust
fn velocity_to_instances(
    values: &[f32],
    candle_xs: &[f32],
    candle_widths: &[f32],
    config: &VelocityConfig,
    viewport_height: u32,
) -> Vec<GridLineInstance> {
    let mut instances = Vec::with_capacity(values.len());

    for (i, &val) in values.iter().enumerate() {
        if val.is_nan() { continue; }

        let color = interpolate_velocity_color(&config.color_map, val, config.opacity);
        let half_w = candle_widths[i] * 0.5;

        instances.push(GridLineInstance {
            rect: [
                candle_xs[i] - half_w,
                0.0,
                candle_xs[i] + half_w,
                viewport_height as f32,
            ],
            color,
        });
    }

    instances
}

/// Interpolate between bearish/neutral/bullish colors based on momentum value.
/// val in [-1.0, +1.0], opacity is a global multiplier.
fn interpolate_velocity_color(
    map: &VelocityColorMap,
    val: f32,
    opacity: f32,
) -> [f32; 4] {
    let (from, to, t) = if val < 0.0 {
        (&map.bearish, &map.neutral, val + 1.0) // -1..0 maps to 0..1
    } else {
        (&map.neutral, &map.bullish, val)        // 0..+1 maps to 0..1
    };

    [
        from[0] + (to[0] - from[0]) * t,
        from[1] + (to[1] - from[1]) * t,
        from[2] + (to[2] - from[2]) * t,
        opacity,
    ]
}
```

### 7.5 Layer Placement

Velocity bars render in **Layer 4 (annotation fills)** -- behind candles
and bodies. This ensures they serve as subtle background context without
obscuring price data.

### 7.6 Visual Reference

```
+-----------------------------------------------------------------+
| R R R R r r . . . g g G G G g . . r r R R R R r . . g g G G   |
| R R R R r r . . . g g G G G g . . r r R R R R r . . g g G G   |
| R R R R r r . . . g g G G G g . . r r R R R R r . . g g G G   |
| R R R R r r . . . g g G G G g . . r r R R R R r . . g g G G   |
| R R R R r r . . . g g G G G g . . r r R R R R r . . g g G G   |
+-----------------------------------------------------------------+
  ^--- bearish ---^  ^- bullish -^  ^- bearish -^   ^- bullish

  R = strong red, r = faded red, . = neutral gray
  G = strong green, g = faded green
  Candles render ON TOP of these colored backgrounds.
  At opacity 0.15, these are very subtle background tints.
```

---

## 8. Text Note

**Category**: Annotation | **Priority**: v2 | **Status**: Not yet implemented

### 8.1 Data Model

```rust
/// A text annotation anchored to a price/time point on the chart.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TextNote {
    /// Anchor price. The note is vertically positioned at this price.
    pub price: f64,

    /// Anchor timestamp (epoch ms). The note is horizontally positioned here.
    pub timestamp: i64,

    /// The note content. Can be multi-line.
    pub text: String,

    /// Background color for the note box.
    pub background_color: [f32; 4],

    /// Text color.
    pub text_color: [f32; 4],

    /// Font size in logical pixels. Default: 12.0.
    pub font_size: f32,

    /// Maximum width for word wrapping. None = no wrapping.
    pub max_width: Option<f32>,

    /// Whether to draw a border around the note box.
    pub show_border: bool,

    /// Whether to draw a connector line from the anchor point to the note.
    pub show_connector: bool,
}
```

### 8.2 Render Output

Text notes are rendered primarily via the iced overlay layer (not GPU text).
The GPU layer provides only a background rectangle and optional connector line.

```
+-----------------------------------------------------------------+
|                                                                 |
|           X                     <- anchor point                 |
|           |                     <- connector line               |
|     +------------+                                              |
|     | Note text  |              <- iced overlay text            |
|     | goes here  |                                              |
|     +------------+              <- GPU background rect          |
|                                                                 |
+-----------------------------------------------------------------+
```

Components:
- **1 GridLineInstance**: background rectangle (Layer 4: fills).
  Semi-transparent background behind the text.
- **1 optional GridLineInstance**: connector line from anchor point to note
  box (1px wide vertical or angled line, approximated as a thin rect).
- **1 WidgetLabel**: the text content, positioned at the note's screen
  coordinates. Rendered by iced as an overlay widget.
- **1 HitZone**: the bounding box of the note (for selection and drag).

```rust
pub fn compute_note(
    note: &TextNote,
    annotation: &Annotation,
    camera: &Camera2D,
) -> Option<WidgetOutput> {
    let anchor_x = camera.time_to_x(note.timestamp as f64);
    let anchor_y = camera.price_to_y(note.price);

    // Estimate text bounding box (approximate; actual size from iced layout)
    let text_width = note.max_width.unwrap_or(200.0);
    let line_count = note.text.lines().count().max(1);
    let text_height = line_count as f32 * (note.font_size + 4.0);

    // Position note box slightly offset from anchor
    let box_x = anchor_x + 12.0;
    let box_y = anchor_y - text_height * 0.5;

    let mut output = WidgetOutput::default();

    // Background rect (Layer 4: fills)
    output.fills.push(GridLineInstance {
        rect: [box_x - 4.0, box_y - 4.0, box_x + text_width + 4.0, box_y + text_height + 4.0],
        color: note.background_color,
    });

    // Border (if enabled) -- 4 thin rects forming a border (Layer 7: lines)
    if note.show_border {
        let border_color = note.text_color;
        let l = box_x - 4.0;
        let t = box_y - 4.0;
        let r = box_x + text_width + 4.0;
        let b = box_y + text_height + 4.0;

        // Top, bottom, left, right border lines
        for rect in [
            [l, t, r, t + 1.0],         // top
            [l, b - 1.0, r, b],         // bottom
            [l, t, l + 1.0, b],         // left
            [r - 1.0, t, r, b],         // right
        ] {
            output.lines.push(GridLineInstance { rect, color: border_color });
        }
    }

    // Connector line (Layer 7: lines)
    if note.show_connector {
        output.lines.push(GridLineInstance {
            rect: [anchor_x, anchor_y, anchor_x + 1.0, box_y],
            color: [note.text_color[0], note.text_color[1], note.text_color[2], 0.4],
        });
    }

    // Text label (iced overlay)
    output.labels.push(WidgetLabel {
        text: note.text.clone(),
        screen_x: box_x,
        screen_y: box_y,
        bg_color: [0.0; 4], // transparent (bg already rendered as GridLineInstance)
        text_color: note.text_color,
        font_size: note.font_size,
        anchor: LabelAnchor::TopLeft,
    });

    // Hit zone (drag to reposition)
    if !annotation.locked {
        output.hit_zones.push(HitZone {
            rect: [box_x - 4.0, box_y - 4.0, box_x + text_width + 4.0, box_y + text_height + 4.0],
            kind: HitZoneKind::NoteBody,
            cursor: CursorIcon::Move,
        });
    }

    Some(output)
}
```

### 8.3 Interaction

**Creation**: Double-click on chart area opens a text input overlay at the
click position. On confirm (Enter or click away), a `TextNote` annotation is
created at the clicked price/time.

**Editing**: Double-click on an existing note opens the text input overlay
pre-filled with the current text.

**Dragging**: Single-click and drag moves the note to a new price/time anchor.
Uses the grab-offset pattern to prevent the note from jumping to the cursor.

```
Idle
  |  double-click on empty space
  v
EditingNote { price, timestamp, text: "" }
  |  user types text, presses Enter
  v
CreateNote { price, timestamp, text }
  v
Idle
```

### 8.4 Visual Reference

```
                          Default note (dark bg, white text):
+-----------------------------------------------------------------+
|                                                                 |
|        *                        <- anchor point                 |
|        |                        <- connector line               |
|  +------------------+                                           |
|  | Breakout above   |                                           |
|  | resistance zone  |                                           |
|  +------------------+                                           |
|                                                                 |

                          Note without connector, with border:
+-----------------------------------------------------------------+
|                                                                 |
|        +------------------+                                     |
|        | Watch for        |                                     |
|        | earnings         |                                     |
|        | announcement     |                                     |
|        +------------------+                                     |
|                                                                 |
+-----------------------------------------------------------------+
```

---

## 9. Marker Annotation

**Category**: Annotation | **Priority**: v2 | **Status**: Not yet implemented

### 9.1 Data Model

```rust
/// A small icon or stamp at a specific price/time point.
/// Used for trade fills, signals, alerts, bookmarks.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MarkerAnnotation {
    /// Exact price for the marker position.
    pub price: f64,

    /// Exact timestamp (epoch ms) for the marker position.
    pub timestamp: i64,

    /// Shape of the marker icon.
    pub icon: MarkerIcon,

    /// Color of the marker.
    pub color: [f32; 4],

    /// Diameter in logical pixels. Default: 8.0.
    pub size: f32,

    /// Optional tooltip text shown on hover.
    pub label: Option<String>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub enum MarkerIcon {
    /// Filled circle. Used for generic markers and fill events.
    Circle,
    /// Up-pointing triangle. Used for buy signals.
    Triangle,
    /// Down-pointing triangle. Used for sell signals.
    InvTriangle,
    /// Diamond shape. Used for alerts.
    Diamond,
    /// Plus/cross shape. Used for stop/cancel events.
    Cross,
    /// Flag shape. Used for important events.
    Flag,
    /// Star shape. Used for bookmarks.
    Star,
}
```

### 9.2 Render Output

Markers are small shapes rendered in Layer 8 (annotation markers). Until the
`MarkerInstance` pipeline is available, all shapes are approximated using
`GridLineInstance` rectangles.

```
+-----------------------------------------------------------------+
|                                                                 |
|              ^       <- Triangle (buy signal, green)            |
|              *       <- Circle (fill event, green)              |
|              v       <- InvTriangle (sell signal, red)          |
|              +       <- Cross (stop, red)                       |
|              <>      <- Diamond (alert, amber)                  |
|                                                                 |
+-----------------------------------------------------------------+
```

#### v1 Shape Approximations

```rust
/// Generate GridLineInstances approximating a marker shape.
/// All shapes use the stacked-scanline technique from the VP POC dot.
fn marker_to_instances(
    icon: MarkerIcon,
    center_x: f32,
    center_y: f32,
    size: f32,
    color: [f32; 4],
) -> Vec<GridLineInstance> {
    let r = size * 0.5;
    match icon {
        MarkerIcon::Circle => {
            // Stacked horizontal scanlines approximating a circle
            let slices = (r * 2.0).ceil() as i32;
            let mut instances = Vec::with_capacity(slices as usize);
            for s in 0..slices {
                let y = center_y - r + s as f32;
                let dist = ((s as f32 + 0.5) - r).abs();
                let half_w = (r * r - dist * dist).max(0.0).sqrt();
                if half_w < 0.5 { continue; }
                instances.push(GridLineInstance {
                    rect: [center_x - half_w, y, center_x + half_w, y + 1.0],
                    color,
                });
            }
            instances
        }

        MarkerIcon::Triangle => {
            // Up-pointing triangle: width narrows from base to tip
            let slices = (r * 2.0).ceil() as i32;
            let mut instances = Vec::with_capacity(slices as usize);
            for s in 0..slices {
                let t = s as f32 / slices as f32; // 0 = top (tip), 1 = bottom (base)
                let half_w = t * r;
                if half_w < 0.5 { continue; }
                let y = center_y - r + s as f32;
                instances.push(GridLineInstance {
                    rect: [center_x - half_w, y, center_x + half_w, y + 1.0],
                    color,
                });
            }
            instances
        }

        MarkerIcon::InvTriangle => {
            // Down-pointing triangle: mirror of Triangle
            let slices = (r * 2.0).ceil() as i32;
            let mut instances = Vec::with_capacity(slices as usize);
            for s in 0..slices {
                let t = 1.0 - s as f32 / slices as f32; // 0 = bottom, 1 = top (tip at bottom)
                let half_w = t * r;
                if half_w < 0.5 { continue; }
                let y = center_y - r + s as f32;
                instances.push(GridLineInstance {
                    rect: [center_x - half_w, y, center_x + half_w, y + 1.0],
                    color,
                });
            }
            instances
        }

        MarkerIcon::Diamond => {
            // Diamond: width peaks at center
            let slices = (r * 2.0).ceil() as i32;
            let mut instances = Vec::with_capacity(slices as usize);
            for s in 0..slices {
                let dist_from_center = ((s as f32 + 0.5) / slices as f32 - 0.5).abs();
                let half_w = (0.5 - dist_from_center) * 2.0 * r;
                if half_w < 0.5 { continue; }
                let y = center_y - r + s as f32;
                instances.push(GridLineInstance {
                    rect: [center_x - half_w, y, center_x + half_w, y + 1.0],
                    color,
                });
            }
            instances
        }

        MarkerIcon::Cross => {
            // Plus sign: two perpendicular rectangles
            let arm_width = (r * 0.3).max(1.0);
            vec![
                // Horizontal arm
                GridLineInstance {
                    rect: [center_x - r, center_y - arm_width * 0.5,
                           center_x + r, center_y + arm_width * 0.5],
                    color,
                },
                // Vertical arm
                GridLineInstance {
                    rect: [center_x - arm_width * 0.5, center_y - r,
                           center_x + arm_width * 0.5, center_y + r],
                    color,
                },
            ]
        }

        MarkerIcon::Flag | MarkerIcon::Star => {
            // Simplified: small filled square as placeholder
            // Real shapes need the MarkerInstance pipeline (v3)
            vec![GridLineInstance {
                rect: [center_x - r, center_y - r, center_x + r, center_y + r],
                color,
            }]
        }
    }
}
```

### 9.3 Compute Function

```rust
pub fn compute_marker(
    marker: &MarkerAnnotation,
    annotation: &Annotation,
    camera: &Camera2D,
) -> Option<WidgetOutput> {
    let x = camera.time_to_x(marker.timestamp as f64);
    let y = camera.price_to_y(marker.price);

    // Early-out if off-screen
    let margin = marker.size;
    if x < -margin || x > camera.viewport_width as f32 + margin
        || y < -margin || y > camera.viewport_height as f32 + margin
    {
        return None;
    }

    let mut output = WidgetOutput::default();

    // Shape instances (Layer 8: markers)
    output.markers = marker_to_instances(
        marker.icon, x, y, marker.size, marker.color,
    );

    // Tooltip label (shown on hover, handled by iced overlay)
    if let Some(ref label) = marker.label {
        output.labels.push(WidgetLabel {
            text: label.clone(),
            screen_x: x + marker.size * 0.5 + 4.0,
            screen_y: y - 8.0,
            bg_color: [0.1, 0.1, 0.1, 0.85],
            text_color: [0.9, 0.9, 0.9, 1.0],
            font_size: 10.0,
            anchor: LabelAnchor::Left,
        });
    }

    // Hit zone (circular grab area)
    let grab_radius = (marker.size * 0.5 + 4.0).max(8.0);
    if !annotation.locked {
        output.hit_zones.push(HitZone {
            rect: [x - grab_radius, y - grab_radius, x + grab_radius, y + grab_radius],
            kind: HitZoneKind::MarkerIcon,
            cursor: CursorIcon::Pointer,
        });
    }

    Some(output)
}
```

### 9.4 Interaction

**Placement**: Activate marker tool (M key or toolbar), select icon type,
click on chart to place.

**Selection**: Click on an existing marker to select it. Shows a selection
ring (slightly larger circle behind the marker in the selection color).

**Dragging**: Grab and drag to reposition to a new price/time point.

**Tooltip**: Hover over a marker for 300ms to show its label as a tooltip.
If no label is set, show the formatted price and time.

### 9.5 Trade Fill Markers

The most common use of markers is displaying trade fill events. The app layer
converts broker fill events into locked marker annotations:

```rust
// In midas-app (NOT midas-chart):
fn fill_to_marker(fill: &FillEvent) -> Annotation {
    let is_buy = fill.action == OrderAction::Buy;
    Annotation {
        id: AnnotationId(0), // store assigns real ID
        kind: AnnotationKind::Marker(MarkerAnnotation {
            price: fill.price,
            timestamp: fill.timestamp,
            icon: if is_buy { MarkerIcon::Triangle } else { MarkerIcon::InvTriangle },
            color: if is_buy {
                [0.15, 0.70, 0.30, 0.90]  // green for buys
            } else {
                [0.70, 0.15, 0.15, 0.90]  // red for sells
            },
            size: 8.0,
            label: Some(format!(
                "{} {} @ {:.2}",
                if is_buy { "Buy" } else { "Sell" },
                fill.quantity,
                fill.price,
            )),
        }),
        presence: Presence::Active,
        created_at: fill.timestamp,
        modified_at: fill.timestamp,
        locked: true,     // historical fills are immutable
        visible_timeframes: None,
    }
    // Note: the order_id → annotation_id mapping is maintained by
    // OrderAnnotationLink in the app layer, not on the Annotation struct.
}
```

### 9.6 Visual Reference

```
                          Trade fills on a chart:
+-----------------------------------------------------------------+
|                                                                 |
|           |  |                                                  |
|        |--+--|    ^  <- Buy fill (green triangle, "Buy 100 @    |
|        |  |  |   *       185.48")                               |
|     |--+--|                                                     |
|     |  |  |                    v  <- Sell fill (red inv-tri,    |
|  |--+--|      |  |             *     "Sell 100 @ 190.05")       |
|  |  |  |   |--+--|         |--+--|                              |
|             |  |  |         |  |  |                             |
+-----------------------------------------------------------------+
```

---

## 10. Trendline

**Category**: Annotation | **Priority**: v3 | **Status**: Not yet designed

### 10.1 Data Model

```rust
/// A diagonal line between two price/time points.
/// Requires the LineInstance pipeline (not available in v1).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TrendlineAnnotation {
    /// Start point.
    pub start_price: f64,
    pub start_timestamp: i64,

    /// End point.
    pub end_price: f64,
    pub end_timestamp: i64,

    /// Whether the line extends infinitely past the end point.
    pub extend_right: bool,

    /// Whether the line extends infinitely past the start point.
    pub extend_left: bool,

    /// Line color.
    pub color: [f32; 4],

    /// Line width in logical pixels.
    pub line_width: f32,

    /// Line style.
    pub style: LineStyle,
}
```

### 10.2 Why v3

Trendlines require diagonal line rendering, which depends on the
`LineInstance` pipeline. This pipeline is also needed for:
- Proper moving average lines (Section 6)
- Fibonacci retracement lines (future)
- Channel tools (future)
- Measured moves (future)

Implementing the `LineInstance` pipeline is a prerequisite for all of these.
It represents a significant chunk of GPU work (new shader, new vertex
expansion logic, new buffer management) and is therefore deferred to v3.

### 10.3 Interaction (Planned)

**Drawing**: Click start point, click end point. Preview line follows cursor
between clicks.

**Dragging**: Grab either endpoint to adjust. Grab the middle to translate
the entire line. Alt key constrains to 45-degree angle increments.

**Extension**: Toggle "extend right" / "extend left" via context menu or
keyboard shortcut. Extended portion renders with reduced alpha.

---

## 11. Catalog Summary Table

| Widget | Category | Storage | Interactive | GPU Primitives | Priority | Implementation Status |
|---|---|---|---|---|---|---|
| **HorizontalLevel** | Annotation | AnnotationStore | Full (drag, snap, delete) | GridLine (1-N per level) | v1 | **Migrate** from levels.rs |
| **OrderBracket** | Annotation | AnnotationStore | Full + broker bridge | GridLine (3-8 per bracket) | v1 | **New** |
| **G.ATR** | Indicator | Per-chart config | None | Label only | v1 | **Migrate** from gerchik_atr.rs |
| **VolumeProfile** | Indicator | Per-chart config | Hover tooltip (future) | GridLine (50-200 per profile) | v1 | **Migrate** from volume_profile.rs |
| **MovingAverage** | Indicator | Per-chart config | None | GridLine (step approx) / LineInstance (v2) | v2 | **New** |
| **Velocity** | Indicator | Per-chart config | None | GridLine (fills layer) | v2 | **New** |
| **TextNote** | Annotation | AnnotationStore | Edit text, drag | GridLine (bg rect) + Overlay | v2 | **New** |
| **Marker** | Annotation | AnnotationStore | Select, move, tooltip | GridLine (shape approx) | v2 | **New** |
| **Trendline** | Annotation | AnnotationStore | Full (two-point drag) | LineInstance (future) | v3 | **Blocked** on LineInstance pipeline |

### Instance Budget Estimates

| Widget | Instances per widget | Typical count per chart | Total instances |
|---|---|---|---|
| HorizontalLevel | 1-100 (dashed) | 5-20 | 100-2000 |
| OrderBracket | 3-8 | 1-5 | 15-40 |
| G.ATR | 0 (label only) | 1 | 0 |
| VolumeProfile | 50-200 | 1 | 50-200 |
| MovingAverage | vis_candles | 1-3 | 500-1500 |
| Velocity | vis_candles | 1 | 500 |
| TextNote | 5-10 | 1-5 | 25-50 |
| Marker | 1-16 (scanlines) | 5-50 | 80-800 |
| **Total** | | | **~1300-5000** |

At 32 bytes per `GridLineInstance`, 5000 instances = 160 KB of GPU upload per
frame. This is well within budget (the existing candle + volume pipeline
uploads significantly more data for 5000 candles).

---

## 12. Cross-Cutting Concerns

### 12.1 Presence Enum

Every annotation supports a presence state that controls visibility:

```rust
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Presence {
    /// Fully visible and interactive.
    #[default]
    Active,
    /// Visible on other charts synced to the same symbol, but rendered
    /// with reduced opacity and no hit zones. Used for cross-chart sync.
    Ghost,
    /// Not rendered at all. Stored in the annotation store but skipped
    /// during compute and hit-testing.
    Hidden,
}
```

Ghost presence is used for cross-timeframe visibility: an annotation
created on an M5 chart appears dimmed on a D1 chart of the same symbol
if timeframe filtering restricts it.

### 12.2 Cross-Chart Sync

Annotations are stored per-symbol in `AnnotationStore` (see
`02-storage-and-sync.md` Section 2). All charts displaying the same symbol
read from the same `&[Annotation]` slice -- there is no per-chart ownership
of annotations. Cross-chart sync happens automatically via generation
counter polling: each chart panel tracks the store's generation and
recomputes when it changes.

Ghost presence applies to **cross-timeframe** scenarios: if an annotation
has `visible_timeframes: Some(set)` and the current chart's timeframe is
not in that set, it may render as Ghost (dimmed, no hit zones) rather than
being fully hidden, depending on user preference. Same-symbol, same-timeframe
charts always see the same annotations as Active.

Ghost annotations render with `alpha *= 0.3` and produce no hit zones.

### 12.3 Serialization Rules

All annotation data types derive `Serialize + Deserialize` via serde.
The serialization format is JSON (see `06-persistence.md`).

Key rules:
- `AnnotationId` serializes as a plain `u64`.
- `[f32; 4]` colors serialize as JSON arrays: `[0.15, 0.65, 0.35, 0.9]`.
- `LineStyle` uses serde's default enum representation (tagged).
- `Option<T>` serializes as `null` when `None`.
- New fields added to structs must have `#[serde(default)]` for backward
  compatibility with older JSON files.

### 12.4 Dirty Flag Integration

Annotation changes increment a generation counter in `DirtyFlags`:

```rust
pub struct DirtyFlags {
    // ...existing fields...
    pub annotations: u64,
}
```

Any mutation to `AnnotationStore` (insert, remove, update) bumps
`annotations`. The renderer checks this counter and re-uploads annotation
instance buffers only when changed.

However, annotations also depend on camera state (for coordinate transforms).
When the camera changes, annotation instances must be recomputed even if the
annotation data hasn't changed. The practical approach:

- **Phase 1**: Always-upload annotation instances (like crosshair and VP
  today). The instance count is small (~5000 max) and the upload cost is
  negligible (~160 KB).
- **Phase 2** (optimization): Gate uploads behind
  `dirty.annotations != last_annotations || dirty.camera != last_camera`.

### 12.5 Testing Strategy

Each widget's compute function is a pure function that takes data and camera
state and returns `WidgetOutput`. This makes every widget independently
unit-testable without GPU context.

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn test_camera() -> Camera2D {
        Camera2D {
            time_start: 1_000_000.0,
            time_end: 2_000_000.0,
            price_low: 100.0,
            price_high: 200.0,
            viewport_width: 1920,
            viewport_height: 1080,
            dpi_scale: 1.0,
        }
    }

    #[test]
    fn level_offscreen_returns_none() {
        let camera = test_camera();
        let level = HorizontalLevel {
            price: 50.0, // below visible range
            color: [1.0, 0.0, 0.0, 1.0],
            line_width: 1.0,
            style: LineStyle::Solid,
            label: None,
            extend: LevelExtend::FullWidth,
        };
        let annotation = make_test_annotation(AnnotationKind::Level(level.clone()));
        assert!(compute_level(&level, &annotation, &camera, 1920, false, false).is_none());
    }

    #[test]
    fn bracket_produces_correct_instance_count() {
        let camera = test_camera();
        let bracket = OrderBracket {
            entry: BracketLeg { price: 150.0, /* ... */ },
            take_profit: Some(BracketLeg { price: 165.0, /* ... */ }),
            stop_loss: Some(BracketLeg { price: 142.0, /* ... */ }),
            side: BracketSide::Long,
            status: BracketStatus::Active,
            quantity: Some(100.0),
        };
        let annotation = make_test_annotation(AnnotationKind::OrderBracket(bracket.clone()));
        let output = compute_bracket(&bracket, &annotation, &camera, 1920, false);

        // Active bracket: 3 solid lines + 2 zone fills
        assert_eq!(output.lines.len(), 3);
        assert_eq!(output.fills.len(), 2);
        // 3 price labels + R:R label = 4 labels
        assert!(output.labels.len() >= 4);
        // 3 leg hit zones + 2 zone hit zones = 5
        assert_eq!(output.hit_zones.len(), 5);
    }

    #[test]
    fn risk_reward_computation() {
        let bracket = OrderBracket {
            entry: BracketLeg { price: 100.0, /* ... */ },
            take_profit: Some(BracketLeg { price: 110.0, /* ... */ }),
            stop_loss: Some(BracketLeg { price: 95.0, /* ... */ }),
            side: BracketSide::Long,
            status: BracketStatus::Draft,
            quantity: None,
        };
        let rr = compute_risk_reward(&bracket).unwrap();
        // Reward = 110 - 100 = 10, Risk = 100 - 95 = 5, R:R = 2.0
        assert!((rr - 2.0).abs() < f64::EPSILON);
    }

    #[test]
    fn volume_profile_cache_invalidation() {
        let cache = VolumeProfileCache::default();
        // Empty cache always needs recompute
        assert!(cache.needs_recompute(100.0, 200.0, 0, 500));
    }

    #[test]
    fn marker_circle_produces_scanlines() {
        let instances = marker_to_instances(
            MarkerIcon::Circle, 100.0, 100.0, 8.0,
            [1.0, 0.0, 0.0, 1.0],
        );
        // 8px diameter = ~8 scanlines
        assert!(instances.len() >= 6);
        assert!(instances.len() <= 10);
    }
}
```

### 12.6 Performance Budget

| Metric | Budget | Rationale |
|---|---|---|
| Max annotations per chart | 500 | Plenty for manual trading workflows |
| Max indicator overlays per chart | 10 | MA + VP + GATR + Velocity + headroom |
| Max GridLineInstances total | 5000 | All widgets combined |
| GPU upload per frame | ~160 KB | 5000 x 32 bytes |
| Compute time (all widgets) | < 1.0ms | Simple coordinate transforms |
| Memory per annotation (Rust heap) | < 256 bytes | Small structs, few allocations |
| VP recompute (5000 candles, 50 bins) | < 0.2ms | O(N * avg_bins_per_candle) |
| MA compute (5000 candles, SMA) | < 0.05ms | O(N) rolling sum |

### 12.7 Implementation Priority Roadmap

```
Phase 1: Foundation (v1)
  |
  +-- Migrate HorizontalLevel -> AnnotationStore
  |     (Section 2: levels.rs + level_tool.rs -> annotations/)
  |
  +-- Migrate G.ATR -> chart config + WidgetLabel
  |     (Section 4: minimal change, already computes correctly)
  |
  +-- Migrate VolumeProfile -> chart config
  |     (Section 5: already computes correctly, add caching)
  |
  +-- Implement OrderBracket drawing + rendering
        (Section 3: new, largest v1 item)

Phase 2: Extended annotations (v2)
  |
  +-- MovingAverage (step-line workaround)
  +-- Velocity/Momentum overlay
  +-- TextNote
  +-- MarkerAnnotation
  +-- Order fill markers (app layer bridge)

Phase 3: Diagonal lines (v3)
  |
  +-- LineInstance GPU pipeline (new shader)
  +-- Trendline
  +-- Proper MA diagonal rendering
  +-- Fibonacci retracements (future)
  +-- Channel tool (future)
```

### 12.8 File Layout After Implementation

```
midas-chart/src/
  annotations/
    mod.rs                 # Re-exports, AnnotationId, Anchor, Presence
    types.rs               # Annotation, AnnotationKind, HorizontalLevel, LineStyle
    store.rs               # AnnotationStore: CRUD, iteration, ID generation
    bracket.rs             # OrderBracket, BracketLeg, BracketSide, BracketStatus
    note.rs                # TextNote
    marker.rs              # MarkerAnnotation, MarkerIcon
    hit_test.rs            # hit_test_annotations() -> Option<(AnnotationId, HitZone)>
    render.rs              # compute_level(), compute_bracket(), compute_marker(), compute_note()
  indicators/
    mod.rs                 # Indicator config types, WidgetCategory
    gatr.rs                # GerchikAtrConfig, compute (moved from gerchik_atr.rs)
    volume_profile.rs      # VolumeProfileConfig, compute, cache (moved from volume_profile.rs)
    moving_average.rs      # MovingAverageConfig, compute (new, v2)
    velocity.rs            # VelocityConfig, compute (new, v2)
  levels.rs                # DEPRECATED after Phase 1 migration, then deleted
  level_tool.rs            # LevelTool state machine (kept, works with AnnotationStore)
  gerchik_atr.rs           # DEPRECATED after migration, then deleted
  volume_profile.rs        # DEPRECATED after migration, then deleted
```

---

*Document version: 1.0 -- 2026-03-30*
*Covers: v1 migration + new widgets, v2 extensions, v3 pipeline prerequisites*
