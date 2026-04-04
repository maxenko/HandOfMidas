# 03 -- Column System and Data Model

> Grid component specification for Hand of Midas  
> Depends on: research documents in `research/grid-component/`  
> Target crate: `midas-grid` (standalone grid widget crate)  
> Consumer crate: `midas-app` (watchlist implementation)  
> **This document is the canonical source for the `GridColumn` trait definition and `ColumnId` type.**

---

## Table of Contents

1. [GridColumn Trait](#1-gridcolumn-trait)
2. [ColumnWidth System](#2-columnwidth-system)
3. [Column Configuration and State](#3-column-configuration-and-state)
4. [Data Access Pattern](#4-data-access-pattern)
5. [Concrete Column Types](#5-concrete-column-types)
6. [Watchlist Column Definitions](#6-watchlist-column-definitions)
7. [Row Data Model](#7-row-data-model)
8. [Multi-Sort Support](#8-multi-sort-support)
9. [Type Safety Considerations](#9-type-safety-considerations)

---

## 1. GridColumn Trait

### 1.1 Core Trait Definition

The `GridColumn` trait is the fundamental abstraction for every column in the grid. It is generic over two type parameters:

- `T` -- the row data type (e.g., `WatchlistRow`). The grid never constrains this beyond what columns require.
- `Message` -- the iced message type emitted by interactive cells.

The trait lives in `midas-grid` so that both the grid widget and application code can reference it.

```rust
use iced::Element;
use std::cmp::Ordering;

/// Unique, stable identifier for a column within a grid instance.
///
/// Used for column state persistence, sort specs, and programmatic
/// column references. Must be unique within a single grid's column set.
/// Uses `&'static str` for zero-cost `Copy`, `Hash`, `Eq`, and natural
/// TOML serialization. All watchlist columns have known names at compile
/// time. If runtime-generated column IDs are needed in the future,
/// a `ColumnId::Dynamic(String)` variant can be introduced.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Ord, PartialOrd)]
pub struct ColumnId(pub &'static str);

impl std::fmt::Display for ColumnId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

impl serde::Serialize for ColumnId {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.0)
    }
}

/// The core column trait. Every column in the grid implements this.
///
/// `T` is the row data type. `Message` is the iced message type.
/// The grid calls these methods during layout and rendering but never
/// owns the data -- it borrows `&T` from the application.
pub trait GridColumn<T, Message> {
    /// Stable identifier for persistence and sort references.
    /// Returns by value since `ColumnId` is `Copy`.
    fn id(&self) -> ColumnId;

    /// Render the header cell content.
    ///
    /// The grid provides the sort indicator separately; this method
    /// returns only the label/widget content. The grid composites
    /// the header content with sort arrows and resize handles.
    fn header(&self) -> Element<'_, Message>;

    /// Render a data cell for the given row.
    ///
    /// Called once per visible row during `view()`. The returned
    /// `Element` can be any iced widget: text, button, toggle,
    /// container with custom styling, etc.
    ///
    /// The explicit lifetime ties `Element` to both `&self` and `&T`,
    /// enabling zero-copy borrowing from row data (e.g., `text(&row.symbol)`).
    fn cell<'a>(&'a self, row: &'a T, row_index: usize) -> Element<'a, Message>;

    /// Width specification for this column.
    fn width(&self) -> ColumnWidth {
        ColumnWidth::Flex(1.0)
    }

    /// Minimum width in logical pixels. The column cannot be resized
    /// below this value.
    fn min_width(&self) -> f32 {
        20.0
    }

    /// Maximum width in logical pixels. `None` means unbounded.
    fn max_width(&self) -> Option<f32> {
        None
    }

    /// Whether the user can sort by this column.
    ///
    /// When `true`, the grid renders a clickable header and includes
    /// this column in sort-spec messages. When `false`, header clicks
    /// on this column are ignored for sorting purposes.
    fn sortable(&self) -> bool {
        false
    }

    /// Compare two rows for sorting on this column.
    ///
    /// Only called when `sortable()` returns `true` and the user has
    /// activated sorting on this column. The grid handles direction
    /// reversal for descending sort -- this method always returns the
    /// natural (ascending) ordering.
    ///
    /// Default: `Ordering::Equal` (stable no-op for non-sortable columns).
    fn compare(&self, _a: &T, _b: &T) -> Ordering {
        Ordering::Equal
    }

    /// Whether the user can resize this column by dragging.
    fn resizable(&self) -> bool {
        true
    }

    /// Whether the user can reorder this column by dragging its header.
    fn reorderable(&self) -> bool {
        true
    }

    /// Horizontal alignment of cell content.
    fn align(&self) -> Alignment {
        Alignment::Start
    }
}

/// Horizontal alignment for cell content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Alignment {
    Start,
    Center,
    End,
}
```

### 1.2 Design Rationale for Default Implementations

Every method except `id()`, `header()`, and `cell()` has a default implementation. This follows the principle of progressive complexity:

- **Simple columns** (display-only text) only need three method implementations.
- **Sortable columns** override `sortable()` to return `true` and provide `compare()`.
- **Fixed-width columns** (drag handle, delete button) override `width()` to return `ColumnWidth::Fixed(...)`, `resizable()` to return `false`, and `reorderable()` to return `false`.

The `compare` method returns `Ordering::Equal` by default rather than panicking. This means if a developer marks a column as `sortable()` but forgets to implement `compare()`, the sort will be a stable no-op rather than a crash. Clippy can warn about this via a custom lint in the future, but correctness-by-default is more important than strictness here.

### 1.3 Handling the Drag Handle Column

The drag handle column is not "special" at the trait level. It is a normal column whose `cell()` method returns a drag-grip button. The grid's row-drag system uses a separate mechanism to detect that a drag has started on a particular row.

```rust
/// The drag handle column renders a grip icon and emits a drag-start
/// message when pressed. The grid widget intercepts this message to
/// enter row-drag mode.
///
/// Key properties:
/// - `sortable()` -> false (cannot sort by drag handle)
/// - `resizable()` -> false (fixed width)
/// - `reorderable()` -> false (always first)
/// - `width()` -> Fixed(26.0)
```

The grid widget checks if a `Message` variant indicates a drag-start. This is communicated through the application's message enum, not through the trait. The column simply emits the message; the grid's `update()` handler enters drag mode.

### 1.4 Handling Action Columns

Action columns (delete button, favorite toggle) are display columns with interactive widgets. They are not sortable and do not participate in data extraction. Their `cell()` method returns a button or toggle that emits an application-level message.

```rust
/// Action columns:
/// - `sortable()` -> false
/// - `compare()` -> default (Equal)
/// - `cell()` returns a button/toggle widget
/// - The button's on_press emits an application Message
///   (e.g., Message::WatchlistRemoveTicker(wl_id, symbol))
```

There is no special trait method for "action" behavior. The trait is deliberately minimal. Rich behavior comes from what `cell()` returns, not from additional trait surface area.

---

## 2. ColumnWidth System

### 2.1 Width Variants

Inspired by WPF's Star/Auto/Pixel system, AG Grid's flex, and egui's column modes.

```rust
/// Specifies how a column's width is determined.
///
/// Width resolution happens during the grid's layout phase.
/// The grid first allocates Fixed columns, then distributes
/// remaining space among Flex columns proportionally.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ColumnWidth {
    /// Exact pixel width. The column is always this wide.
    /// Used for drag handles, icon buttons, and columns the
    /// user has manually resized.
    Fixed(f32),

    /// Proportional share of remaining space after Fixed columns
    /// are allocated. A column with `Flex(2.0)` gets twice as
    /// much space as `Flex(1.0)`.
    ///
    /// Analogous to CSS `flex-grow` or WPF `Star` sizing.
    /// The flex value must be > 0.0.
    Flex(f32),

    /// Fit to content. The grid measures the header and all
    /// visible cells, then uses the widest measurement plus padding.
    ///
    /// Expensive for large datasets -- only measures visible rows.
    /// Capped by `min_width()` and `max_width()`.
    Auto,
}

impl Default for ColumnWidth {
    fn default() -> Self {
        ColumnWidth::Flex(1.0)
    }
}
```

### 2.2 Width Resolution Algorithm

The grid resolves column widths during `layout()` in a two-pass process:

```
Pass 1: Measure fixed and auto columns
─────────────────────────────────────────
  total_available = grid_viewport_width - separator_widths
  fixed_used = 0.0

  for each visible column in display order:
      match column.effective_width():  // from config override or trait default
          Fixed(px) => {
              resolved_width = px.clamp(column.min_width(), column.max_width_or(f32::MAX))
              fixed_used += resolved_width
          }
          Auto => {
              measured = measure_content(column, visible_rows)
              resolved_width = measured.clamp(column.min_width(), column.max_width_or(f32::MAX))
              fixed_used += resolved_width
          }
          Flex(_) => { /* skip for now */ }

Pass 2: Distribute remaining space to flex columns
─────────────────────────────────────────
  remaining = total_available - fixed_used
  total_flex = sum of flex values for all Flex columns

  if remaining > 0 and total_flex > 0:
      for each Flex(weight) column:
          proportion = weight / total_flex
          raw_width = remaining * proportion
          resolved_width = raw_width.clamp(column.min_width(), column.max_width_or(f32::MAX))

  if remaining <= 0:
      // All flex columns get their min_width
      for each Flex(_) column:
          resolved_width = column.min_width()
```

### 2.3 Resize Interaction with Width Types

When the user drags a column resize handle:

- **Fixed column**: The width changes directly. Stays `Fixed`.
- **Flex column**: Converts to `Fixed(new_pixel_width)` permanently. This matches AG Grid's behavior: once a user manually resizes a flex column, flex is disabled for that column. The rationale is that the user has expressed a specific intent for that width.
- **Auto column**: Converts to `Fixed(new_pixel_width)`. Same logic -- manual resize overrides automatic measurement.

This conversion is stored in `ColumnConfig.width` (see Section 3), not in the trait implementation. The trait provides the *default* width; the config provides the *current* width.

### 2.4 Effective Width Resolution

The grid determines a column's effective width by checking config first, then falling back to the trait:

```rust
impl ColumnConfig {
    /// The width to use for layout. Config overrides the trait default.
    pub fn effective_width(&self, trait_width: ColumnWidth) -> ColumnWidth {
        if self.width_override.is_some() {
            // User has resized or config has been loaded -- use stored value.
            self.width_override.unwrap()
        } else {
            trait_width
        }
    }
}
```

### 2.5 Double-Click to Auto-Size

Double-clicking a column resize handle triggers auto-sizing:

1. Measure the header width and all visible cell widths for that column.
2. Set the column width to `Fixed(max_measured + padding)`.
3. Store in `ColumnConfig`.

This mirrors AG Grid's `autoSizeColumns` behavior. The column becomes `Fixed` after auto-sizing.

---

## 3. Column Configuration and State

### 3.1 Separation of Definition and State

A column's *definition* is its trait implementation -- how it renders, what it sorts by, its default width. This is code, compiled into the binary.

A column's *state* is its runtime configuration -- current width, display order, visibility, sort direction. This changes at runtime and persists to disk.

```rust
/// Runtime state for a single column. Persisted to TOML.
///
/// Stored in `GridConfig.columns` and keyed by `ColumnId`.
/// When no config exists for a column, the grid uses the trait defaults.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ColumnConfig {
    /// Stable identifier matching `GridColumn::id()`.
    /// Uses `String` for serde compatibility — mapped to `ColumnId(&'static str)`
    /// at load time via a known-columns lookup (see 00-architecture.md §6.3).
    pub id: String,

    /// Whether the column is visible. Hidden columns still exist
    /// in the column list but are not rendered.
    #[serde(default = "default_true")]
    pub visible: bool,

    /// Width override. `None` means "use the trait default".
    /// Set to `Some(Fixed(px))` when the user resizes a column,
    /// or loaded from persisted state.
    #[serde(default)]
    pub width_override: Option<ColumnWidth>,

    /// Display order index. Columns are rendered in ascending
    /// `order` value. When two columns have the same order, they
    /// render in definition order (the order they appear in the
    /// columns Vec passed to the grid).
    pub order: usize,

    /// Pinning (future). Left-pinned columns stay visible during
    /// horizontal scroll. Right-pinned columns stick to the right edge.
    #[serde(default)]
    pub pinned: Option<PinSide>,
}

fn default_true() -> bool {
    true
}

/// Which side a column is pinned to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PinSide {
    Left,
    Right,
}
```

### 3.2 GridConfig (Persistable State)

The persistable grid configuration that survives across sessions. This is
distinct from the runtime `GridState` in 00-architecture.md, which also
includes transient fields (selection, scroll offset, drag/resize state).
At load time, `GridConfig` is converted into `GridState` by adding default
transient fields. At save time, only the persistable subset is written.

```rust
/// Persistable grid configuration for one grid instance.
///
/// Serialized as TOML inside the application config file.
/// Converted to/from the runtime `GridState` at load/save boundaries.
/// Uses `String` for column IDs in serialization; the application maps
/// these to `ColumnId(&'static str)` via a known-columns lookup.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GridConfig {
    /// Per-column configuration. Keyed by ColumnId string.
    /// Columns not present here use trait defaults.
    #[serde(default)]
    pub columns: Vec<ColumnConfig>,

    /// Active sort specification, if any. `None` means no sort.
    /// Single-column sort for Phase 0-3; multi-sort (`Vec<SortSpecConfig>`) in Phase 4.
    /// Matches the runtime `GridState.sort: Option<SortSpec>` cardinality.
    /// Uses `SortSpecConfig` (String-based column IDs) for serde compatibility.
    #[serde(default)]
    pub sort: Option<SortSpecConfig>,

    /// Name of the active column preset, if any.
    #[serde(default)]
    pub active_preset: Option<String>,
}

/// Persistable sort specification for one column.
///
/// This is the **config/persisted** form. The runtime form is `SortSpec`
/// (defined in 00-architecture.md §2.1) which uses `ColumnId(&'static str)`.
/// At load time, `SortSpecConfig.column_id` is mapped to `ColumnId` via
/// a known-columns lookup. At save time, `SortSpec` is converted back
/// to `SortSpecConfig` via `ColumnId::to_string()`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SortSpecConfig {
    /// Which column to sort by.
    /// Uses `String` for serde compatibility — mapped to `ColumnId` at load time.
    pub column_id: String,
    /// Sort direction.
    pub direction: SortDirection,
}

/// Sort direction.
///
/// **Implementation note**: This is the SAME `SortDirection` type defined
/// in 00-architecture.md §2.1. During implementation, use a single type
/// in `column.rs` with both serde derives and `toggle()`/`indicator()`
/// methods. The `SortSpecConfig` struct uses this same type for its
/// `direction` field — no separate "persisted form" is needed since the
/// variants are identical.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SortDirection {
    Ascending,
    Descending,
}

impl SortDirection {
    /// Return the opposite direction.
    pub fn toggle(self) -> Self {
        match self {
            Self::Ascending => Self::Descending,
            Self::Descending => Self::Ascending,
        }
    }

    /// Unicode arrow indicator for header display.
    pub fn indicator(self) -> &'static str {
        match self {
            Self::Ascending => " \u{25B2}",  // ▲
            Self::Descending => " \u{25BC}", // ▼
        }
    }
}
```

### 3.3 Building GridConfig from Column Definitions

When a grid is first created (no persisted state), `GridConfig` is built from the column definitions:

```rust
impl GridConfig {
    /// Create a default GridConfig from a set of column definitions.
    ///
    /// Each column gets its trait-default width and is visible.
    /// Display order matches definition order.
    pub fn from_columns<T, M, C: GridColumn<T, M>>(columns: &[C]) -> Self {
        Self {
            columns: columns
                .iter()
                .enumerate()
                .map(|(i, col)| ColumnConfig {
                    id: col.id().0.to_owned(),
                    visible: true,
                    width_override: None,
                    order: i,
                    pinned: None,
                })
                .collect(),
            sort: None,
            active_preset: None,
        }
    }
}
```

### 3.4 Loading GridState from Persisted Config

When the application starts, `GridConfig` (String-based column IDs from TOML) must be
converted into the runtime `GridState` (`ColumnId(&'static str)`-based). The key challenge
is that `ColumnId(&'static str)` cannot be deserialized directly — serde cannot produce a
`&'static str` from TOML input. The resolution strategy is a known-columns lookup:

```rust
/// Resolve a persisted column ID string to a `ColumnId(&'static str)`.
///
/// Iterates the column definitions and returns the matching `ColumnId`
/// if a column with the given string ID exists. Returns `None` for
/// unknown column names (forward-compatible: if a column is removed,
/// its persisted config entry is silently dropped).
pub fn resolve_column_id<T, M, C: GridColumn<T, M>>(
    columns: &[C],
    name: &str,
) -> Option<ColumnId> {
    columns.iter().find(|c| c.id().0 == name).map(|c| c.id())
}

impl GridState {
    /// Build a runtime `GridState` from a persisted `GridConfig` and
    /// the current column definitions.
    ///
    /// Unknown column IDs in the config are silently ignored.
    /// Columns not present in the config use trait defaults.
    /// **Note**: Hidden columns (`visible: false`) are excluded from
    /// `column_order`. If later made visible, they append to the end
    /// rather than restoring their original position. This is acceptable
    /// for Phase 0; position-preserving visibility can be added later
    /// by maintaining a separate `all_column_order` that includes hidden columns.
    pub fn from_config<T, M, C: GridColumn<T, M>>(
        config: &GridConfig,
        columns: &[C],
    ) -> Self {
        let mut column_order = Vec::new();
        let mut column_widths = HashMap::new();

        // Build ordered config entries, sorted by display order.
        let mut sorted_configs: Vec<_> = config.columns.iter().collect();
        sorted_configs.sort_by_key(|c| c.order);

        for cc in &sorted_configs {
            if let Some(col_id) = resolve_column_id(columns, &cc.id) {
                if cc.visible {
                    column_order.push(col_id);
                }
                if let Some(ColumnWidth::Fixed(px)) = cc.width_override {
                    column_widths.insert(col_id, px);
                }
            }
        }

        // Add any columns not in the config (new columns added since last save).
        for col in columns {
            if !column_order.contains(&col.id()) {
                column_order.push(col.id());
            }
        }

        // Resolve sort spec.
        let sort = config.sort.as_ref().and_then(|s| {
            resolve_column_id(columns, &s.column_id).map(|col_id| SortSpec {
                column_id: col_id,
                direction: s.direction,
            })
        });

        Self {
            column_order,
            column_widths,
            sort,
            selection: SelectionState {
                selected: None,
                focused: None,
            },
            scroll_y: 0.0,
            interaction: ActiveInteraction::None,
        }
    }
}
```

> **Phasing note**: The `from_config()` shown here reflects the Phase 2+ shape. In Phase 0, `interaction` is always `ActiveInteraction::None` (resize/drag not yet implemented). In Phase 1, `ActiveInteraction::Resize` becomes available.

This function is the critical persistence round-trip glue. It is called in
`WatchlistPanel::from_config()` during application startup.

### 3.5 Column State Persistence (TOML)

Grid state serializes into the existing `AppConfig` TOML structure. The `WatchlistConfig` struct gains a new `grid_state` field:

```toml
[[watchlists]]
name = "Main"
symbol_link = "blue"

[watchlists.grid_state]
active_preset = "default"

[[watchlists.grid_state.columns]]
id = "drag"
visible = true
order = 0
width_override = { Fixed = 26.0 }

[[watchlists.grid_state.columns]]
id = "favorite"
visible = true
order = 1
width_override = { Fixed = 30.0 }

[[watchlists.grid_state.columns]]
id = "ticker"
visible = true
order = 2

[watchlists.grid_state.sort]
column_id = "change_pct"
direction = "descending"
```

This replaces the current `column_widths: Vec<f32>` in `WatchlistConfig`. No backward-compatible dual-format loading is needed — no shipped releases exist, so there are no existing config files to migrate (see 04-implementation-roadmap.md Phase 0).

### 3.5 Column Presets

A column preset is a named snapshot of `GridConfig`. Presets are stored in a separate section of the config:

```rust
/// A named column configuration that can be saved, loaded, and switched.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ColumnPreset {
    /// Human-readable name (e.g., "Default", "Compact", "Full").
    pub name: String,
    /// The saved grid state.
    pub state: GridConfig,
}
```

Preset operations:

- **Save current as preset**: Snapshot the current `GridConfig` with a name.
- **Load preset**: Replace current `GridConfig` with the preset's state. The grid re-renders immediately.
- **Reset to default**: Rebuild `GridConfig` from column definitions (trait defaults).
- **Delete preset**: Remove a named preset from the config.

Presets are stored in `AppConfig`:

```toml
[[grid_presets]]
name = "Performance View"
[grid_presets.state]
# ... same structure as watchlists.grid_state
```

### 3.6 Reset to Default

```rust
impl GridConfig {
    /// Reset all column state to trait defaults.
    ///
    /// Clears width overrides, restores definition order,
    /// makes all columns visible, and clears sort.
    pub fn reset<T, M, C: GridColumn<T, M>>(&mut self, columns: &[C]) {
        *self = Self::from_columns(columns);
    }
}
```

---

## 4. Data Access Pattern

### 4.1 Grid Never Owns Data

This is the foundational architectural rule, inherited from ImGui's specs-only sorting pattern and iced's Elm architecture.

```
Application owns:     Vec<WatchlistRow>
Grid receives:        &[WatchlistRow]  (borrowed slice)
Grid calls:           column.cell(&row, index)  for each visible row
Grid emits:           GridMessage::SortToggled(ColumnId)
Application sorts:    its own Vec<WatchlistRow>
Grid re-renders:      with the newly sorted data
```

The grid widget's `view` function receives a slice reference:

```rust
/// Build the grid widget. Returns `Element<'a, M>` where `M` is the
/// application's message type.
///
/// `columns` -- the column definitions (generic, typically an application enum)
/// `rows` -- borrowed slice of row data, already sorted by the application
/// `state` -- grid state (column configs, sort specs)
///
/// Uses generics (`C: GridColumn<T, M>`) rather than trait objects.
/// The primary use pattern is an application enum (e.g., `WatchlistColumn`)
/// implementing `GridColumn`, passed as `&[WatchlistColumn]`.
/// See 00-architecture.md §7.2 for the canonical builder signature.
///
/// Cell content emits `M` directly. Grid chrome (sort, resize, select)
/// maps through the required `on_grid` constructor parameter:
/// ```rust
/// grid(&columns, &rows, &grid_state, move |gm| Message::WatchlistGrid(wl_id, gm))
/// ```
pub fn grid<'a, T, M, C>(
    columns: &'a [C],
    rows: &'a [T],
    state: &'a GridState,
    on_grid: impl Fn(GridMessage) -> M + 'a,
) -> Grid<'a, T, M, C>
where
    C: GridColumn<T, M>,
    M: Clone + 'a,
{
    Grid::new(columns, rows, state, on_grid)
}
```

### 4.2 Cell Rendering Flow

For each visible row, for each visible column (in display order):

```
1. Grid reads ColumnConfig.order to determine column display sequence.
2. Grid reads ColumnConfig.visible to skip hidden columns.
3. Grid calls column.cell(&rows[row_index], row_index).
4. The returned Element is placed into the grid layout at the
   resolved (row, col) position with the resolved column width.
```

The grid does not cache cell elements between frames. In iced's Elm architecture, the entire `view()` tree is rebuilt on every state change. For grids with hundreds of rows, virtual scrolling (rendering only visible rows) is the performance strategy, not caching.

### 4.3 Sort Flow (Specs-Only)

When the user clicks a sortable column header:

```
1. Grid chrome calls: (on_grid)(GridMessage::SortToggled(ColumnId("ticker")))
   This produces: Message::WatchlistGrid(wl_id, grid_msg)

2. Application update() handler:
   a. Calls wl.grid_state.toggle_sort(col_id)
   b. Sorts its own Vec<WatchlistRow> using the column's compare() method
   c. Returns Command::none() (state changed, iced will call view())

3. Grid view() is called with the newly sorted data slice.
   Grid reads GridState.sort to display sort indicators on headers.
```

The grid never calls `compare()` on its own. The application calls it.

**Important**: The application sort handler is where domain-specific sort logic
(favorites-first pinning, custom tiebreakers) belongs. The grid's column-level
`compare()` handles per-column ordering; the application wraps it with any
pre-sort keys needed. The current watchlist floats favorites to the top regardless
of the active sort column — this behavior **must** be preserved in the migration.

```rust
// In MidasApp::update(), handling Message::WatchlistGrid(wl_id, GridMessage::SortToggled(col_id)):
fn apply_sort(
    rows: &mut Vec<WatchlistRow>,
    sort_specs: &[SortSpec],
    columns: &[WatchlistColumn],  // application enum implementing GridColumn
) {
    if sort_specs.is_empty() {
        return; // No sort -- keep insertion order.
    }

    rows.sort_by(|a, b| {
        // Favorites-first: favorites always float to the top, regardless of
        // the active sort column. This preserves the current watchlist behavior.
        let fav_order = b.favorite.cmp(&a.favorite); // true > false => favorites first
        if fav_order != Ordering::Equal {
            return fav_order;
        }

        // Then apply column-level sort specs.
        for spec in sort_specs {
            let col = columns.iter().find(|c| c.id() == spec.column_id);
            if let Some(col) = col {
                let ordering = col.compare(a, b);
                let ordering = match spec.direction {
                    SortDirection::Ascending => ordering,
                    SortDirection::Descending => ordering.reverse(),
                };
                if ordering != Ordering::Equal {
                    return ordering;
                }
            }
            // If Equal, fall through to next sort level.
        }
        Ordering::Equal
    });
}
```

### 4.4 Row Identity

> **Phase 3a**: API surface -- `RowKey`, `RowKeyFn`, and the `.row_key()` builder method are
> introduced. The grid stores keys internally and uses them for single-row selection persistence
> across sorts and data updates.
>
> **Phase 3a**: Required for multi-selection -- `RowKey` replaces `usize` indices in the
> `SelectionState` set. Multi-select (`Ctrl+Click`, `Shift+Click`) is gated on a `row_key`
> function being provided; the grid falls back to `SelectionMode::Single` if it is absent.

Each row needs a stable identity for:
- Tracking selection across sorts and reorders.
- Optimizing re-renders (knowing which row moved vs. which row changed).
- Drag-and-drop (identifying the dragged row after data reorder).

This follows the same pattern used by AG Grid (`getRowId`) and TanStack Table (`getRowId`):
the consumer provides a function that extracts a stable business key from each row, and the
grid uses that key -- not the row's position in the data slice -- as the canonical identity.
Index-based selection is fragile across re-sorts and live data updates; key-based selection
is not.

Rather than adding a trait bound to `T`, the grid uses a key extraction function:

```rust
/// Function that extracts a stable identity from a row.
///
/// The returned value must be unique within the dataset and stable
/// across sort/filter operations. For watchlists, this is the ticker
/// symbol string.
///
/// Phase 3a: API surface (grid stores keys, selection uses them).
/// Phase 3a: Full multi-selection uses RowKey instead of usize indices.
pub type RowKeyFn<T> = Box<dyn Fn(&T) -> RowKey>;

/// Stable identity for a row, independent of its position in the data slice.
/// Wraps a string key (typically extracted from the row's business identity,
/// e.g., a ticker symbol).
///
/// Must be unique within the dataset and stable across sort/filter operations.
/// Used for selection persistence, scroll-to-row, and future multi-select.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RowKey(pub String);

impl RowKey {
    pub fn new(key: impl Into<String>) -> Self {
        Self(key.into())
    }
}
```

The grid constructor accepts an **optional** key function (builder pattern). If not provided,
the grid falls back to index-based selection (backward compatible with Phase 0 grids):

```rust
impl<'a, T, Message> Grid<'a, T, Message> {
    /// Provide a function that extracts a stable identity key from each row.
    ///
    /// Used for selection persistence across sorts and data updates.
    /// Phase 3a: API surface (grid stores keys, selection uses them).
    /// Phase 3a: Full multi-selection uses RowKey instead of usize indices.
    /// If not provided, the grid falls back to index-based selection.
    pub fn row_key(mut self, key_fn: impl Fn(&T) -> RowKey + 'a) -> Self {
        self.row_key_fn = Some(Box::new(key_fn));
        self
    }
}
```

Usage:

```rust
grid(columns, rows, &state)
    .row_key(|row: &WatchlistRow| RowKey::new(&row.symbol))
```

**Fallback behavior**: If no key function is provided, the grid falls back to using row index
as identity. This is unstable across sorts -- acceptable for trivial grids that never re-sort,
but not for selection tracking in production use. Phase 3 multi-selection requires a `row_key`
function; calling `.selection_mode(SelectionMode::Multi)` without a `row_key` function logs a
warning and falls back to `SelectionMode::Single`.

---

## 5. Concrete Column Types

These are pre-built column implementations in `midas-grid` that cover the common patterns. Application code can use these directly or implement `GridColumn` from scratch for custom behavior.

### 5.1 TextColumn

Extracts a string from row data and renders it as a `text()` widget.

```rust
use iced::widget::text;
use iced::{Color, Element};
use std::cmp::Ordering;

/// A column that displays text extracted from the row.
///
/// Supports:
/// - Static or dynamic text color via a closure
/// - Sortable via a comparison closure
/// - Configurable alignment
pub struct TextColumn<T, Message> {
    id: ColumnId,
    header_label: String,
    /// Extracts the display string from a row.
    text_fn: Box<dyn Fn(&T) -> String>,
    /// Optional: extracts a text color from a row.
    color_fn: Option<Box<dyn Fn(&T) -> Color>>,
    /// Optional: comparison function for sorting.
    compare_fn: Option<Box<dyn Fn(&T, &T) -> Ordering>>,
    width: ColumnWidth,
    min_width: f32,
    max_width: Option<f32>,
    alignment: Alignment,
    font_size: f32,
    _message: std::marker::PhantomData<Message>,
}

impl<T, Message> TextColumn<T, Message> {
    /// Create a new text column.
    pub fn new(
        id: &'static str,
        header: impl Into<String>,
        text_fn: impl Fn(&T) -> String + 'static,
    ) -> Self {
        Self {
            id: ColumnId(id),
            header_label: header.into(),
            text_fn: Box::new(text_fn),
            color_fn: None,
            compare_fn: None,
            width: ColumnWidth::Flex(1.0),
            min_width: 40.0,
            max_width: None,
            alignment: Alignment::Start,
            font_size: 13.0,
            _message: std::marker::PhantomData,
        }
    }

    /// Set a conditional text color based on row data.
    pub fn color(mut self, f: impl Fn(&T) -> Color + 'static) -> Self {
        self.color_fn = Some(Box::new(f));
        self
    }

    /// Make the column sortable with a comparison function.
    pub fn sortable_by(mut self, f: impl Fn(&T, &T) -> Ordering + 'static) -> Self {
        self.compare_fn = Some(Box::new(f));
        self
    }

    /// Set the column width.
    pub fn width(mut self, w: ColumnWidth) -> Self {
        self.width = w;
        self
    }

    /// Set the minimum width.
    pub fn min_width(mut self, w: f32) -> Self {
        self.min_width = w;
        self
    }

    /// Set the maximum width.
    pub fn max_width(mut self, w: f32) -> Self {
        self.max_width = Some(w);
        self
    }

    /// Set horizontal alignment.
    pub fn align(mut self, a: Alignment) -> Self {
        self.alignment = a;
        self
    }

    /// Set font size.
    pub fn font_size(mut self, s: f32) -> Self {
        self.font_size = s;
        self
    }
}

impl<T, Message: 'static> GridColumn<T, Message> for TextColumn<T, Message> {
    fn id(&self) -> ColumnId {
        self.id
    }

    fn header(&self) -> Element<'_, Message> {
        text(&self.header_label).size(12).into()
    }

    fn cell<'a>(&'a self, row: &'a T, _row_index: usize) -> Element<'a, Message> {
        let content = (self.text_fn)(row);
        let mut t = text(content).size(self.font_size);
        if let Some(ref color_fn) = self.color_fn {
            t = t.color((color_fn)(row));
        }
        t.into()
    }

    fn width(&self) -> ColumnWidth {
        self.width
    }

    fn min_width(&self) -> f32 {
        self.min_width
    }

    fn max_width(&self) -> Option<f32> {
        self.max_width
    }

    fn sortable(&self) -> bool {
        self.compare_fn.is_some()
    }

    fn compare(&self, a: &T, b: &T) -> Ordering {
        match &self.compare_fn {
            Some(f) => f(a, b),
            None => Ordering::Equal,
        }
    }

    fn align(&self) -> Alignment {
        self.alignment
    }
}
```

### 5.2 NumericColumn

Specialized for numeric data with formatting, sign display, and conditional coloring.

```rust
/// A column optimized for numeric data display.
///
/// Supports:
/// - Configurable decimal precision
/// - Optional +/- sign prefix
/// - Conditional color (green/red/neutral based on value)
/// - Right-alignment by default (correct for numeric data)
/// - Flash-on-tick support (Phase 2)
pub struct NumericColumn<T, Message> {
    id: ColumnId,
    header_label: String,
    /// Extracts the numeric value from a row. Returns None for missing data.
    value_fn: Box<dyn Fn(&T) -> Option<f64>>,
    /// Number of decimal places.
    precision: usize,
    /// Whether to show +/- sign prefix.
    show_sign: bool,
    /// Optional suffix (e.g., "%").
    suffix: &'static str,
    /// Text to display when value is None.
    empty_text: &'static str,
    /// Color function: given the numeric value, return a display color.
    color_fn: Option<Box<dyn Fn(f64) -> Color>>,
    /// Comparison function for sorting.
    compare_fn: Option<Box<dyn Fn(&T, &T) -> Ordering>>,
    width: ColumnWidth,
    min_width: f32,
    max_width: Option<f32>,
    font_size: f32,
    _message: std::marker::PhantomData<Message>,
}

impl<T, Message> NumericColumn<T, Message> {
    pub fn new(
        id: &'static str,
        header: impl Into<String>,
        value_fn: impl Fn(&T) -> Option<f64> + 'static,
    ) -> Self {
        Self {
            id: ColumnId(id),
            header_label: header.into(),
            value_fn: Box::new(value_fn),
            precision: 2,
            show_sign: false,
            suffix: "",
            empty_text: "--",
            color_fn: None,
            compare_fn: None,
            width: ColumnWidth::Flex(1.0),
            min_width: 50.0,
            max_width: None,
            font_size: 13.0,
            _message: std::marker::PhantomData,
        }
    }

    /// Set decimal precision.
    pub fn precision(mut self, p: usize) -> Self {
        self.precision = p;
        self
    }

    /// Show +/- sign prefix for the value.
    pub fn show_sign(mut self) -> Self {
        self.show_sign = true;
        self
    }

    /// Append a suffix to the formatted value (e.g., "%").
    pub fn suffix(mut self, s: &'static str) -> Self {
        self.suffix = s;
        self
    }

    /// Set the text displayed when the value is None.
    pub fn empty_text(mut self, s: &'static str) -> Self {
        self.empty_text = s;
        self
    }

    /// Set conditional coloring based on the numeric value.
    pub fn color(mut self, f: impl Fn(f64) -> Color + 'static) -> Self {
        self.color_fn = Some(Box::new(f));
        self
    }

    /// Convenience: green for positive, red for negative, neutral for zero/missing.
    pub fn green_red(self, neutral: Color) -> Self {
        self.color(move |v| {
            if v > 0.0 {
                Color::from_rgb(0.2, 0.8, 0.3)
            } else if v < 0.0 {
                Color::from_rgb(0.9, 0.25, 0.2)
            } else {
                neutral
            }
        })
    }

    /// Make the column sortable with an explicit comparison function.
    ///
    /// This is the only way to make a `NumericColumn` sortable. A standalone
    /// `sortable()` method is intentionally omitted because `Box<dyn Fn>`
    /// cannot be cloned, so the comparator cannot be auto-derived from
    /// `value_fn`. The caller must always supply the comparator explicitly.
    pub fn sortable_by(mut self, f: impl Fn(&T, &T) -> Ordering + 'static) -> Self {
        self.compare_fn = Some(Box::new(f));
        self
    }

    pub fn width(mut self, w: ColumnWidth) -> Self {
        self.width = w;
        self
    }

    pub fn min_width(mut self, w: f32) -> Self {
        self.min_width = w;
        self
    }

    pub fn max_width(mut self, w: f32) -> Self {
        self.max_width = Some(w);
        self
    }

    pub fn font_size(mut self, s: f32) -> Self {
        self.font_size = s;
        self
    }
}

impl<T, Message: 'static> GridColumn<T, Message> for NumericColumn<T, Message> {
    fn id(&self) -> ColumnId {
        self.id
    }

    fn header(&self) -> Element<'_, Message> {
        text(&self.header_label).size(12).into()
    }

    fn cell<'a>(&'a self, row: &'a T, _row_index: usize) -> Element<'a, Message> {
        match (self.value_fn)(row) {
            Some(val) => {
                let formatted = if self.show_sign {
                    format!(
                        "{:+.prec$}{suffix}",
                        val,
                        prec = self.precision,
                        suffix = self.suffix
                    )
                } else {
                    format!(
                        "{:.prec$}{suffix}",
                        val,
                        prec = self.precision,
                        suffix = self.suffix
                    )
                };
                let mut t = text(formatted).size(self.font_size);
                if let Some(ref color_fn) = self.color_fn {
                    t = t.color((color_fn)(val));
                }
                t.into()
            }
            None => text(self.empty_text)
                .size(self.font_size)
                .color(Color::from_rgb(0.6, 0.6, 0.6))
                .into(),
        }
    }

    fn width(&self) -> ColumnWidth {
        self.width
    }

    fn min_width(&self) -> f32 {
        self.min_width
    }

    fn max_width(&self) -> Option<f32> {
        self.max_width
    }

    fn sortable(&self) -> bool {
        self.compare_fn.is_some()
    }

    fn compare(&self, a: &T, b: &T) -> Ordering {
        match &self.compare_fn {
            Some(f) => f(a, b),
            None => Ordering::Equal,
        }
    }

    fn align(&self) -> Alignment {
        Alignment::End // Numeric columns are right-aligned by default.
    }
}
```

### 5.3 ButtonColumn

Renders an interactive button per row.

```rust
/// A column that renders a button in each cell.
///
/// The button emits a custom message when clicked.
/// Used for delete buttons, action triggers, etc.
pub struct ButtonColumn<T, Message> {
    id: ColumnId,
    header_label: String,
    /// Returns the button label for a given row.
    label_fn: Box<dyn Fn(&T) -> String>,
    /// Returns the message to emit when the button is clicked.
    message_fn: Box<dyn Fn(&T) -> Message>,
    width: ColumnWidth,
    font_size: f32,
}

impl<T, Message> ButtonColumn<T, Message> {
    pub fn new(
        id: &'static str,
        header: impl Into<String>,
        label_fn: impl Fn(&T) -> String + 'static,
        message_fn: impl Fn(&T) -> Message + 'static,
    ) -> Self {
        Self {
            id: ColumnId(id),
            header_label: header.into(),
            label_fn: Box::new(label_fn),
            message_fn: Box::new(message_fn),
            width: ColumnWidth::Fixed(30.0),
            font_size: 12.0,
        }
    }

    pub fn width(mut self, w: ColumnWidth) -> Self {
        self.width = w;
        self
    }

    pub fn font_size(mut self, s: f32) -> Self {
        self.font_size = s;
        self
    }
}

impl<T, Message: Clone + 'static> GridColumn<T, Message> for ButtonColumn<T, Message> {
    fn id(&self) -> ColumnId {
        self.id
    }

    fn header(&self) -> Element<'_, Message> {
        text(&self.header_label).size(12).into()
    }

    fn cell<'a>(&'a self, row: &'a T, _row_index: usize) -> Element<'a, Message> {
        let label = (self.label_fn)(row);
        let msg = (self.message_fn)(row);
        button(text(label).size(self.font_size))
            .on_press(msg)
            .padding([2, 4])
            .style(hover_text_button_style) // reuse existing project style
            .into()
    }

    fn width(&self) -> ColumnWidth {
        self.width
    }

    fn sortable(&self) -> bool {
        false
    }

    fn resizable(&self) -> bool {
        false // Action columns are typically fixed width.
    }

    fn reorderable(&self) -> bool {
        false // Action columns stay in place.
    }
}
```

### 5.4 ToggleColumn

Renders a toggle widget (star, checkbox, etc.) per row.

```rust
/// A column that renders a toggle (binary state) per row.
///
/// Used for favorites (star/unstar), enabled/disabled, etc.
/// The toggle emits a message when clicked.
pub struct ToggleColumn<T, Message> {
    id: ColumnId,
    header_label: String,
    /// Returns the current toggle state for a row.
    state_fn: Box<dyn Fn(&T) -> bool>,
    /// Returns the label text based on toggle state.
    /// e.g., |on| if on { "\u{2605}" } else { "\u{2606}" }  // ★ / ☆
    label_fn: Box<dyn Fn(bool) -> String>,
    /// Returns the message to emit when toggled.
    message_fn: Box<dyn Fn(&T) -> Message>,
    width: ColumnWidth,
    font_size: f32,
}

impl<T, Message> ToggleColumn<T, Message> {
    pub fn new(
        id: &'static str,
        header: impl Into<String>,
        state_fn: impl Fn(&T) -> bool + 'static,
        label_fn: impl Fn(bool) -> String + 'static,
        message_fn: impl Fn(&T) -> Message + 'static,
    ) -> Self {
        Self {
            id: ColumnId(id),
            header_label: header.into(),
            state_fn: Box::new(state_fn),
            label_fn: Box::new(label_fn),
            message_fn: Box::new(message_fn),
            width: ColumnWidth::Fixed(30.0),
            font_size: 12.0,
        }
    }

    pub fn width(mut self, w: ColumnWidth) -> Self {
        self.width = w;
        self
    }

    pub fn font_size(mut self, s: f32) -> Self {
        self.font_size = s;
        self
    }
}

impl<T, Message: Clone + 'static> GridColumn<T, Message> for ToggleColumn<T, Message> {
    fn id(&self) -> ColumnId {
        self.id
    }

    fn header(&self) -> Element<'_, Message> {
        text(&self.header_label).size(12).into()
    }

    fn cell<'a>(&'a self, row: &'a T, _row_index: usize) -> Element<'a, Message> {
        let is_on = (self.state_fn)(row);
        let label = (self.label_fn)(is_on);
        let msg = (self.message_fn)(row);
        button(text(label).size(self.font_size))
            .on_press(msg)
            .padding([2, 4])
            .style(hover_text_button_style)
            .into()
    }

    fn width(&self) -> ColumnWidth {
        self.width
    }

    fn sortable(&self) -> bool {
        false
    }

    fn resizable(&self) -> bool {
        false
    }

    fn reorderable(&self) -> bool {
        false
    }
}
```

### 5.5 DragHandleColumn

Renders the drag grip icon for row reordering.

```rust
/// A column that renders a drag handle (grip icon) for row reordering.
///
/// Pressing the grip initiates a row drag operation. The grid widget
/// detects the drag-start message and enters row-drag mode.
pub struct DragHandleColumn<T, Message> {
    id: ColumnId,
    /// Returns the message to emit when the drag handle is pressed.
    message_fn: Box<dyn Fn(&T) -> Message>,
    width: f32,
    grip_icon: &'static str,
}

impl<T, Message> DragHandleColumn<T, Message> {
    pub fn new(message_fn: impl Fn(&T) -> Message + 'static) -> Self {
        Self {
            id: ColumnId("drag"),
            message_fn: Box::new(message_fn),
            width: 26.0,
            grip_icon: "\u{2807}", // ⠇ (braille six-dot pattern)
        }
    }

    /// Override the default grip icon character.
    pub fn icon(mut self, icon: &'static str) -> Self {
        self.grip_icon = icon;
        self
    }

    /// Override the column width.
    pub fn width(mut self, w: f32) -> Self {
        self.width = w;
        self
    }
}

impl<T, Message: Clone + 'static> GridColumn<T, Message> for DragHandleColumn<T, Message> {
    fn id(&self) -> ColumnId {
        self.id
    }

    fn header(&self) -> Element<'_, Message> {
        // Empty header -- drag column has no label.
        Space::new().into()
    }

    fn cell<'a>(&'a self, row: &'a T, _row_index: usize) -> Element<'a, Message> {
        let msg = (self.message_fn)(row);
        button(text(self.grip_icon).size(12))
            .on_press(msg)
            .padding([2, 4])
            .style(hover_text_button_style)
            .into()
    }

    fn width(&self) -> ColumnWidth {
        ColumnWidth::Fixed(self.width)
    }

    fn sortable(&self) -> bool {
        false
    }

    fn resizable(&self) -> bool {
        false
    }

    fn reorderable(&self) -> bool {
        false // Drag handle column is always first.
    }
}
```

### 5.6 ComputedColumn (Future -- Phase 3)

A column whose value is derived from a user-defined formula operating on other column values within the same row. This is the ThinkOrSwim-inspired custom column feature.

```rust
/// (Phase 3) A column whose value is computed from a formula
/// referencing other columns in the same row.
///
/// The formula language is a simple expression evaluator (not Turing-complete)
/// supporting arithmetic operators, built-in functions (abs, max, min, round),
/// and column references by ID.
///
/// Example formula: "price * volume / 1000000"
/// Example formula: "if(change_pct > 0, 'UP', 'DOWN')"
pub struct ComputedColumn<T, Message> {
    id: ColumnId,
    header_label: String,
    /// The formula expression string (user-authored, persisted).
    formula: String,
    /// Compiled evaluator (parsed from formula on load).
    /// Takes a row reference and a column-value lookup function.
    evaluator: Box<dyn Fn(&T) -> Option<f64>>,
    precision: usize,
    color_fn: Option<Box<dyn Fn(f64) -> Color>>,
    width: ColumnWidth,
    _message: std::marker::PhantomData<Message>,
}
```

Design note: the formula language and evaluator are out of scope for Phase 1. The struct is included here to show that the column system's generic architecture accommodates it without trait changes.

---

## 6. Watchlist Column Definitions

This section shows exactly how the current watchlist's seven columns are defined using the column system. The row data type is `WatchlistRow` (defined in Section 7).

### 6.1 Complete Column Set

> **Phase note**: This section illustrates how pre-built generic column types (`TextColumn`, `NumericColumn`, etc.) compose via trait objects. This is the **Phase 2+** approach, shown here for completeness. During **Phases 0-1**, the watchlist uses the `WatchlistColumn` enum with static dispatch (see `00-architecture.md` Section 3.3). The team should choose between enum dispatch and trait objects at Phase 2 time based on whether exhaustive matching or runtime composability is more valuable.

```rust
use crate::grid::{
    ButtonColumn, ColumnWidth, DragHandleColumn, GridColumn, NumericColumn,
    TextColumn, ToggleColumn,
};
use iced::Color;
use midas_core::WatchlistId;
use std::cmp::Ordering;

/// Build the watchlist column definitions.
///
/// Each column is a concrete type implementing `GridColumn<WatchlistRow, Message>`.
/// The returned Vec is the "column schema" for the watchlist grid.
pub fn watchlist_columns(
    wl_id: WatchlistId,
) -> Vec<Box<dyn GridColumn<WatchlistRow, Message>>> {
    vec![
        // ── Column 0: Drag handle (⠇) ──────────────────────────
        Box::new(
            DragHandleColumn::new(move |row: &WatchlistRow| {
                Message::WatchlistDragStart(wl_id, row.symbol.clone())
            })
        ),

        // ── Column 1: Favorite toggle (★/☆) ────────────────────
        Box::new(
            ToggleColumn::new(
                "favorite",
                "\u{2605}", // ★ as header
                |row: &WatchlistRow| row.favorite,
                |on| {
                    if on {
                        "\u{2605}".to_owned() // ★ filled star
                    } else {
                        "\u{2606}".to_owned() // ☆ empty star
                    }
                },
                move |row: &WatchlistRow| {
                    Message::WatchlistToggleFavorite(wl_id, row.symbol.clone())
                },
            )
            .width(ColumnWidth::Fixed(30.0))
        ),

        // ── Column 2: Ticker symbol ────────────────────────────
        Box::new(
            TextColumn::new(
                "ticker",
                "Ticker",
                |row: &WatchlistRow| row.symbol.clone(),
            )
            .sortable_by(|a: &WatchlistRow, b: &WatchlistRow| {
                a.symbol.cmp(&b.symbol)
            })
            .width(ColumnWidth::Flex(1.0))
            .min_width(60.0)
        ),

        // ── Column 3: Last price ───────────────────────────────
        Box::new(
            NumericColumn::new(
                "price",
                "Price",
                |row: &WatchlistRow| row.last_price,
            )
            .precision(2)
            .sortable_by(|a: &WatchlistRow, b: &WatchlistRow| {
                a.last_price
                    .partial_cmp(&b.last_price)
                    .unwrap_or(Ordering::Equal)
            })
            .width(ColumnWidth::Flex(1.0))
            .min_width(60.0)
        ),

        // ── Column 4: Change % ─────────────────────────────────
        Box::new(
            NumericColumn::new(
                "change_pct",
                "Chg%",
                |row: &WatchlistRow| row.change_pct,
            )
            .precision(2)
            .show_sign()
            .suffix("%")
            .green_red(Color::from_rgb(0.6, 0.6, 0.6))
            .sortable_by(|a: &WatchlistRow, b: &WatchlistRow| {
                a.change_pct
                    .partial_cmp(&b.change_pct)
                    .unwrap_or(Ordering::Equal)
            })
            .width(ColumnWidth::Flex(1.0))
            .min_width(55.0)
        ),

        // ── Column 5: G.ATR ────────────────────────────────────
        Box::new(
            NumericColumn::new(
                "gatr",
                "G.ATR",
                |row: &WatchlistRow| row.gatr,
            )
            .precision(2)
            .color(|val| {
                if val > 0.0 {
                    Color::from_rgb(0.2, 0.8, 0.3)
                } else if val < 0.0 {
                    Color::from_rgb(0.9, 0.25, 0.2)
                } else {
                    Color::from_rgb(0.6, 0.6, 0.6)
                }
            })
            .sortable_by(|a: &WatchlistRow, b: &WatchlistRow| {
                a.gatr
                    .partial_cmp(&b.gatr)
                    .unwrap_or(Ordering::Equal)
            })
            .width(ColumnWidth::Flex(1.0))
            .min_width(55.0)
        ),

        // ── Column 6: Delete button (✕) ────────────────────────
        Box::new(
            ButtonColumn::new(
                "delete",
                "", // empty header
                |_row: &WatchlistRow| "\u{00D7}".to_owned(), // ×
                move |row: &WatchlistRow| {
                    Message::WatchlistRemoveTicker(wl_id, row.symbol.clone())
                },
            )
            .width(ColumnWidth::Fixed(30.0))
        ),
    ]
}
```

### 6.2 Integration with the Grid Widget

The watchlist view function becomes:

```rust
fn view_watchlist_body(&self, wl_id: WatchlistId) -> Element<'_, Message> {
    let wl = match self.watchlists.get(&wl_id) {
        Some(wl) => wl,
        None => return text("Watchlist not found").into(),
    };

    // Build row data (see Section 7 for WatchlistRow).
    let rows = self.build_watchlist_rows(wl);

    // Column definitions.
    let columns = watchlist_columns(wl_id);

    // Grid state (from watchlist panel state).
    let state = &wl.grid_state;

    grid(&columns, &rows, state, move |gm| Message::WatchlistGrid(wl_id, gm))
        .row_key(|row: &WatchlistRow| RowKey::new(&row.symbol))
        .selected_row(
            wl.selected_symbol.as_ref().map(|s| RowKey::new(s))
        )
}
```

### 6.3 Sort Integration with Favorites-First Rule

The watchlist has a domain-specific sorting rule: favorites always float to the top, regardless of the active sort column. This is implemented in the application's sort handler, not in the grid:

```rust
// In MidasApp::update():
Message::WatchlistGrid(wl_id, GridMessage::SortToggled(col_id)) => {
    if let Some(wl) = self.watchlists.get_mut(&wl_id) {
        wl.grid_state.toggle_sort(col_id);

        // Sort the underlying ticker data.
        let columns = watchlist_columns(wl_id);
        let mut rows = self.build_watchlist_rows_mut(wl);

        rows.sort_by(|a, b| {
            // Primary: favorites always on top.
            let fav_ord = b.favorite.cmp(&a.favorite);
            if fav_ord != Ordering::Equal {
                return fav_ord;
            }
            // Secondary: apply grid sort specs.
            for spec in &specs {
                if let Some(col) = columns.iter().find(|c| c.id() == spec.column_id) {
                    let ordering = col.compare(a, b);
                    let ordering = match spec.direction {
                        SortDirection::Ascending => ordering,
                        SortDirection::Descending => ordering.reverse(),
                    };
                    if ordering != Ordering::Equal {
                        return ordering;
                    }
                }
            }
            Ordering::Equal
        });

        // Apply the sorted order back to the watchlist's ticker list.
        self.apply_sorted_order(wl_id, &rows);
    }
}
```

---

## 7. Row Data Model

### 7.1 Generic Row Type

The grid is generic over `T`. It never constrains `T` beyond what columns need. The grid does not know or care whether `T` is a watchlist row, an order entry, or a position summary. All data semantics are encoded in the column definitions.

### 7.2 WatchlistRow

The concrete row type for watchlists. This replaces the current pattern where market data is looked up separately during view rendering.

```rust
/// A single row in the watchlist grid.
///
/// Combines the persisted ticker data (symbol, favorite) with
/// transient market data (price, change, GATR). Built fresh before
/// each view() call from the watchlist state and market data cache.
///
/// This struct is NOT persisted. It exists only for the duration of
/// one view() cycle. Persisted data stays in `WatchlistTicker` and
/// the market data provider.
#[derive(Debug, Clone)]
pub struct WatchlistRow {
    /// Ticker symbol (e.g., "AAPL"). Also serves as the row key.
    pub symbol: String,
    /// Whether this ticker is marked as a favorite.
    pub favorite: bool,
    /// Last traded price. `None` if no market data available.
    pub last_price: Option<f64>,
    /// Daily change percentage. `None` if unavailable.
    pub change_pct: Option<f64>,
    /// G.ATR (Gap Average True Range) value. `None` if unavailable.
    pub gatr: Option<f64>,
    /// Color for the G.ATR cell, derived from the indicator logic.
    /// `None` means use the default neutral color.
    pub gatr_color: Option<[f32; 4]>,
}
```

### 7.3 Building WatchlistRow from State

```rust
impl MidasApp {
    /// Build the WatchlistRow slice for a given watchlist.
    ///
    /// Fuses persisted ticker data with live market data in one pass.
    fn build_watchlist_rows(&self, wl: &WatchlistPanel) -> Vec<WatchlistRow> {
        let market_data = self.compute_all_market_data();

        wl.tickers
            .iter()
            .map(|ticker| {
                let mkt = market_data.get(&ticker.symbol);
                WatchlistRow {
                    symbol: ticker.symbol.clone(),
                    favorite: ticker.favorite,
                    last_price: mkt.and_then(|m| m.last_price),
                    change_pct: mkt.and_then(|m| m.change_pct),
                    gatr: mkt.and_then(|m| {
                        m.gatr_text.as_ref().and_then(|s| s.parse::<f64>().ok())
                    }),
                    gatr_color: mkt.and_then(|m| m.gatr_color),
                }
            })
            .collect()
    }
}
```

### 7.4 Row Data Update Flow

```
Market data update arrives (via broadcast channel or polling)
    |
    v
MidasApp::update() stores new market data in cache
    |
    v
iced calls MidasApp::view() on next frame
    |
    v
view_watchlist_body() calls build_watchlist_rows()
    -> fuses WatchlistTicker + market data cache -> Vec<WatchlistRow>
    |
    v
grid() receives &[WatchlistRow]
    -> for each visible row, calls column.cell(&row, index)
    -> returns Element tree to iced for rendering
```

The grid never stores row data between frames. Each frame, it receives a fresh slice. This is correct for iced's Elm architecture where `view()` is a pure function of state.

### 7.5 Future: Flash-on-Tick Data

> **Flash-on-tick is implemented in Phase 3a using app-side change detection.** The grid receives flash triggers via `GridMessage::FlashCell { row, column, direction }` — it does not perform change detection itself. See `02-rendering.md` Section 4 and `04-implementation-roadmap.md` Phase 3a for the canonical design. The `prev_price` field should NOT be added to `WatchlistRow`.

---

## 8. Multi-Sort Support

### 8.1 Sort State Representation

Multi-sort is represented as an ordered list of `SortSpec`:

```rust
/// Active sort specifications for a grid.
///
/// Index 0 is the primary sort key. Index 1 is the tiebreaker.
/// Index 2 breaks ties within index 1, and so on.
///
/// Empty Vec means no active sort (data displayed in natural/insertion order).
pub sort: Option<SortSpec>,  // Phase 0-3 single sort; Phase 4 upgrades to Vec<SortSpec>
```

### 8.2 Sort Priority List

```rust
/// One level of a multi-column sort (persistable form).
/// See §3.2 for the canonical definition of SortSpecConfig.
/// The runtime form `SortSpec` (in 00-architecture.md) uses `ColumnId`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SortSpecConfig {
    /// Which column to sort by.
    /// Uses `String` for serde compatibility — mapped to `ColumnId` at load time.
    pub column_id: String,
    /// Sort direction for this level.
    pub direction: SortDirection,
}
```

Example: sort by Change% descending (primary), then by Ticker ascending (secondary):

```rust
// Multi-sort example (Phase 4, persisted config form):
vec![
    SortSpecConfig { column_id: "change_pct".to_owned(), direction: SortDirection::Descending },
    SortSpecConfig { column_id: "ticker".to_owned(), direction: SortDirection::Ascending },
]
```

### 8.3 UI Behavior

**Single click** on a column header:
1. If the column is not currently sorted: set it as the sole primary sort (ascending).
2. If the column is the primary sort (ascending): toggle to descending.
3. If the column is the primary sort (descending): clear all sort (return to natural order).

**Shift+click** on a column header (multi-sort):
1. If the column is not in the sort list: append it as the next sort level (ascending).
2. If the column is already in the sort list: toggle its direction.
3. If the column is already descending: remove it from the sort list.

This matches AG Grid's and TanStack Table's behavior exactly.

### 8.4 Sort Indicators

The grid header renders sort indicators for each sorted column:

- **Arrow**: up triangle (ascending) or down triangle (descending) next to the header text.
- **Priority badge**: a small superscript number (1, 2, 3...) indicating sort priority. Only shown when 2+ columns are sorted.

```rust
/// Build the header cell for a sortable column, including sort indicator.
fn render_header_with_sort(
    column: &dyn GridColumn<T, Message>,
    sort_specs: &[SortSpec],
) -> Element<'_, Message> {
    let base = column.header();

    // Find this column in the sort specs.
    let sort_info = sort_specs
        .iter()
        .enumerate()
        .find(|(_, s)| &s.column_id == column.id());

    match sort_info {
        Some((priority, spec)) => {
            let arrow = spec.direction.indicator();
            let priority_label = if sort_specs.len() > 1 {
                format!("{}", priority + 1)
            } else {
                String::new()
            };
            // Composite: [base_header] [arrow] [priority_badge]
            row![
                base,
                text(format!("{arrow}{priority_label}")).size(10),
            ]
            .into()
        }
        None => base,
    }
}
```

### 8.5 Stable Sort Requirement

The sort MUST be stable. When two rows compare as `Equal` on all sort levels, they retain their relative order from before the sort. This is critical for:

- Favorites-first rule: favorites are sorted first, then the sort column applies within favorites and within non-favorites separately. A stable sort preserves the manual order within equal groups.
- Manual drag reorder: if the user has manually arranged rows, a stable sort preserves that arrangement within equal-value groups.

Rust's `sort_by` is guaranteed stable (TimSort). Rust's `sort_unstable_by` is NOT stable. The application must use `sort_by`, not `sort_unstable_by`.

### 8.6 Continuous Sort (Future -- Phase 2)

When market data updates arrive, the sort could be automatically re-applied to keep the grid sorted in real time (like TWS's "continuous sort" feature). This is an opt-in feature:

```rust
/// Whether the grid re-sorts automatically when data changes.
///
/// When enabled, the application re-applies sort specs after every
/// market data update. When disabled, sort order is frozen until the
/// user clicks a header again.
pub continuous_sort: bool,
```

For Phase 1, continuous sort is disabled. The sort runs once when the user clicks a header.

---

## 9. Type Safety Considerations

### 9.1 Compile-Time Column-Row Type Matching

The `GridColumn<T, Message>` trait is generic over `T`. A `TextColumn<WatchlistRow, Message>` cannot be used in a grid that expects `OrderRow` data. The compiler catches this:

```rust
// This compiles:
let cols: Vec<Box<dyn GridColumn<WatchlistRow, Message>>> = vec![
    Box::new(TextColumn::<WatchlistRow, Message>::new("ticker", "Ticker", |r| r.symbol.clone())),
];

// This fails to compile -- type mismatch:
// let cols: Vec<Box<dyn GridColumn<OrderRow, Message>>> = vec![
//     Box::new(TextColumn::<WatchlistRow, Message>::new(...)),
//                           ^^^^^^^^^^^^^ expected OrderRow
// ];
```

The generic `T` propagates through the entire column definition chain, from the closure types (`Fn(&T) -> String`) to the trait implementation. No runtime type checking is needed.

### 9.2 Storing Heterogeneous Columns in a Vec

> **Phasing note**: Phase 0-1 uses enum dispatch (`WatchlistColumn` enum). Phase 2+ may introduce trait objects (`Box<dyn GridColumn<T, M>>`) for heterogeneous column types. The enum approach is simpler and sufficient for the initial watchlist use case. The trait object discussion below describes the Phase 2+ option.

All columns for a given grid share the same `T` and `Message`, so they are stored as `Vec<Box<dyn GridColumn<T, Message>>>`. This uses trait objects (dynamic dispatch) rather than an enum.

**Why trait objects instead of an enum:**

- **Open extension**: Application code can define custom column types without modifying the grid crate's enum. A `SparklineColumn` defined in `midas-app` works alongside `TextColumn` from `midas-grid`.
- **No combinatorial explosion**: An enum would need a variant for every combination of column features. Trait objects compose naturally.
- **Acceptable performance**: Dynamic dispatch adds one vtable pointer indirection per method call. Cell rendering involves building iced Element trees (heap allocations) that dwarf the cost of a vtable lookup.

**Trade-off**: Trait objects require the trait to be object-safe. This constrains the trait design:

- No generic methods (methods cannot have additional type parameters).
- No `Self` in return position.
- No associated types that vary per implementation.

The `GridColumn` trait as designed is object-safe. All methods return concrete types (`Element`, `ColumnWidth`, `Ordering`, `bool`, `f32`).

### 9.3 Lifetime Management for Element References

`Element<'_, Message>` borrows from the column and row for the duration of the view cycle. Key lifetime relationships:

```rust
// The grid widget borrows columns and rows for the lifetime 'a.
pub struct Grid<'a, T, Message> {
    columns: &'a [Box<dyn GridColumn<T, Message> + 'a>],
    rows: &'a [T],
    state: &'a GridState,
    // ...
}

// column.cell() borrows the row for its return Element's lifetime.
// The Element lives until view() returns and iced takes ownership.
fn cell<'a>(&'a self, row: &'a T, row_index: usize) -> Element<'a, Message>;
// Lifetime elision: Element<'_, Message> borrows from &self and &T.
```

The critical constraint: **all closures in concrete column types must be `'static`**. The closures in `TextColumn`, `NumericColumn`, etc. are stored as `Box<dyn Fn(...) + 'static>`. This is because the column objects themselves are typically owned by the application (not borrowed), so non-`'static` closures would create self-referential lifetime issues.

Practical consequence: closures cannot borrow local variables from the calling function. They must capture owned values (via `move`). This is why `wl_id` is captured by value (`Copy`) and symbol is cloned in the message closures.

### 9.4 The `'a` Lifetime on Column Trait Objects

The column trait objects in the Vec need to outlive the grid widget within a single `view()` call. Since `view()` borrows `&self`, and columns are created inside `view_watchlist_body()`, the columns live on the stack for the duration of the `view()` call. This is sufficient.

If columns were expensive to create and needed caching, they could be stored in the application state as `Vec<Box<dyn GridColumn<WatchlistRow, Message> + 'static>>`. The current design creates them per-frame, which is acceptable because column construction only allocates a few closures and strings per column (microseconds, not milliseconds).

### 9.5 Avoiding Orphan Rule Issues

The `GridColumn` trait is defined in `midas-grid`. The concrete column types (`TextColumn`, `NumericColumn`, etc.) are also in `midas-grid`. The row types (`WatchlistRow`) are in `midas-app`. This is fine because `midas-app` uses the trait, it does not implement it on foreign types. The concrete column types are generic over `T`, so `TextColumn<WatchlistRow, Message>` is instantiated by `midas-app` without orphan issues.

If `midas-app` needs a column type not provided by `midas-grid`, it implements `GridColumn<WatchlistRow, Message>` for a new struct defined in `midas-app`. No orphan rule violation because the trait and the implementing type are not both foreign.

---

## Appendix: Migration Path from Current Implementation

The current watchlist implementation in `views.rs` (lines 1135-1430) manually constructs header cells, data cells, resize handles, and sort logic inline. The grid component replaces all of this with:

1. **Column definitions**: Replace the 7 manual header/cell blocks with `watchlist_columns(wl_id)` (Section 6.1).
2. **Sort logic**: Replace the inline `sorted.sort_by(...)` with `apply_sort()` calling `column.compare()` (Section 4.3).
3. **Resize handles**: Move from inline `mouse_area` + `Space` into the grid widget's header rendering.
4. **Column widths**: Replace `column_widths: [f32; 7]` with `GridConfig.columns: Vec<ColumnConfig>`.
5. **Row rendering**: Replace the manual `for ticker in sorted` loop with the grid widget's internal row iteration.
6. **Sort state**: Replace `SortColumn` enum + `SortDirection` with `Option<SortSpec>` (single-sort for Phase 0-3).

The existing `WatchlistTicker` struct and `WatchlistPanel` struct remain. `WatchlistRow` is a new transient struct built from `WatchlistTicker` + market data, replacing the inline market-data-lookup pattern.

---

### Critical Files for Implementation
- D:\GitHub\HandOfMidas\desktop\win\crates\midas-grid\src\lib.rs
- D:\GitHub\HandOfMidas\desktop\win\crates\midas-app\src\watchlist.rs
- D:\GitHub\HandOfMidas\desktop\win\crates\midas-app\src\app\views.rs
- D:\GitHub\HandOfMidas\desktop\win\crates\midas-core\src\config.rs
- D:\GitHub\HandOfMidas\desktop\win\crates\midas-core\src\id.rs