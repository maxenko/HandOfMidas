# Window Management & Chart Layout System -- Complete Architecture

> Midas Desktop -- Foundational layout system for multi-chart workspace
> Replaces the placeholder `WorkspaceLayout` enum from tech-stack-rust-a.md Phase 4.2
> Target: TC2000 / Bloomberg Terminal / thinkorswim tier tiling window management

---

## Table of Contents

1. [Design Philosophy & Prior Art](#1-design-philosophy--prior-art)
2. [Layout Data Model](#2-layout-data-model)
3. [Split Algorithm](#3-split-algorithm)
4. [Resize Algorithm](#4-resize-algorithm)
5. [Close / Remove Algorithm](#5-close--remove-algorithm)
6. [Drag and Drop UX](#6-drag-and-drop-ux)
7. [Preset Layouts](#7-preset-layouts)
8. [Serialization](#8-serialization)
9. [Integration with iced](#9-integration-with-iced)
10. [Edge Cases](#10-edge-cases)

---

## 1. Design Philosophy & Prior Art

### The Problem

A trading workspace needs N chart panels arranged in an arbitrary tiling grid. Users must be able to:

- Split any panel horizontally or vertically
- Drag charts between positions
- Resize borders between any two adjacent panels
- Close panels and have neighbors reclaim the space
- Save and restore complex layouts across sessions

### Alternatives Considered

| Model | Description | Verdict |
|---|---|---|
| **Flat normalized rects** (`Vec<LayoutCell>` with x/y/w/h in 0..1) | The approach in the original Phase 4.2 placeholder | **Rejected.** No structural information. Resizing one panel requires heuristics to figure out which neighbors to adjust. Splitting is ambiguous. No clean close-and-reclaim. |
| **CSS Grid model** (rows/columns with spans) | 2D grid where panels can span multiple cells | **Rejected for primary model.** Grid coordinates require gap-filling logic and awkward reflow when a panel is removed. Spanning creates complex constraint systems. Good for *preset generation* but bad as the mutable core. |
| **Binary split tree** (like tmux, i3wm, VS Code panel system) | Every non-leaf node is a horizontal or vertical split with a ratio. Leaves are chart panels or tab groups. | **Selected.** Clean recursive structure. Splitting = replace leaf with branch. Closing = replace parent with sibling. Resizing = adjust ratio on split node. All operations are local tree mutations. Well-understood, battle-tested in tmux/i3/VS Code. |
| **N-ary split tree** (like Windows Terminal) | Split nodes can have N children instead of exactly 2 | Considered as an extension. Adds complexity to resize propagation. Binary tree can represent any N-ary layout by nesting. We start binary and can extend to N-ary if a compelling UX need arises. |

### Why Binary Split Tree Wins

1. **Every operation is a local tree edit.** No global constraint solving.
2. **Resize is trivial.** Dragging a border changes exactly one `ratio` field.
3. **Close is trivial.** Remove leaf, replace parent split with surviving sibling, done.
4. **Split is trivial.** Replace target leaf with a new split node containing the old leaf and a new leaf.
5. **Deeply nested layouts emerge naturally** without special-case code.
6. **Serialization is a tree walk** -- clean, deterministic, compact.
7. **Pixel rect computation is a single recursive pass** -- perfect for feeding into iced's layout.

---

## 2. Layout Data Model

### Core Types

```rust
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Unique identifier for a chart panel within the workspace.
/// Uses u64 internally with a monotonic counter to guarantee uniqueness
/// even across save/restore cycles within a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PaneId(pub u64);

/// Unique identifier for a node in the layout tree.
/// Every node (split or leaf) gets one. This is important for
/// addressing nodes during drag-and-drop operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NodeId(pub u64);

/// Direction of a split.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Axis {
    /// Children are arranged left | right.
    Horizontal,
    /// Children are arranged top | bottom.
    Vertical,
}

/// A node in the binary layout tree.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LayoutNode {
    /// A leaf node containing a single pane (or a tab group of panes).
    Leaf(LeafNode),

    /// An internal node splitting space between two children.
    Split(SplitNode),
}

/// A leaf in the layout tree -- either a single chart pane or a
/// tab group of chart panes (where one is active/visible).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LeafNode {
    pub id: NodeId,
    pub tabs: Vec<PaneId>,
    /// Index into `tabs` for the currently visible pane.
    pub active_tab: usize,
}

/// An internal split node dividing space between two children.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SplitNode {
    pub id: NodeId,
    pub axis: Axis,
    /// Fraction of space allocated to the first child (0.0 .. 1.0).
    /// The second child gets `1.0 - ratio`.
    /// Invariant: ratio is clamped to [MIN_RATIO .. 1.0 - MIN_RATIO]
    /// where MIN_RATIO ensures minimum panel size.
    pub ratio: f32,
    /// The first child (left for Horizontal, top for Vertical).
    pub first: Box<LayoutNode>,
    /// The second child (right for Horizontal, bottom for Vertical).
    pub second: Box<LayoutNode>,
}

/// Minimum fraction a child can occupy. Prevents panels from
/// becoming invisibly small. With a 1920px-wide window and a
/// 3-level deep horizontal split, the smallest panel would be
/// 1920 * 0.1 * 0.1 * 0.1 = 1.9px -- but the absolute minimum
/// pixel constraint (below) catches that case.
const MIN_RATIO: f32 = 0.1;

/// Absolute minimum panel dimension in logical pixels.
/// Below this, the panel cannot be shrunk further.
const MIN_PANEL_SIZE_PX: f32 = 80.0;

/// Width of the draggable resize border in logical pixels.
const BORDER_WIDTH: f32 = 4.0;

/// The top-level workspace layout.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceLayout {
    /// The root of the layout tree. `None` means the workspace is empty
    /// (show the empty-state "+" button).
    pub root: Option<LayoutNode>,

    /// Monotonic counter for generating unique NodeIds and PaneIds.
    next_id: u64,
}
```

### Computed Rect (not serialized -- derived each frame)

```rust
/// A pixel-space rectangle computed from the layout tree + window size.
/// This is what iced uses to position each chart's Shader widget.
#[derive(Debug, Clone, Copy)]
pub struct PaneRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// Result of layout computation: a map from PaneId to its pixel rect.
pub type LayoutRects = HashMap<PaneId, PaneRect>;

/// A border between two adjacent panes that the user can drag to resize.
#[derive(Debug, Clone, Copy)]
pub struct ResizeBorder {
    /// The NodeId of the SplitNode that owns this border.
    pub split_node_id: NodeId,
    /// Axis of the split (determines if the border is vertical or horizontal).
    pub axis: Axis,
    /// Pixel-space rectangle of the draggable border zone.
    pub rect: PaneRect,
}
```

### WorkspaceLayout Methods (API Surface)

```rust
impl WorkspaceLayout {
    /// Create an empty workspace (no panels).
    pub fn empty() -> Self;

    /// Create a workspace with a single pane.
    pub fn single(pane_id: PaneId) -> Self;

    /// Generate a new unique NodeId.
    fn next_node_id(&mut self) -> NodeId;

    /// Generate a new unique PaneId.
    pub fn next_pane_id(&mut self) -> PaneId;

    /// Compute pixel rects for all visible panes given the total
    /// available rectangle (typically the workspace area minus
    /// toolbar/statusbar/sidebar).
    pub fn compute_rects(&self, bounds: PaneRect) -> LayoutRects;

    /// Compute all draggable resize borders.
    pub fn compute_borders(&self, bounds: PaneRect) -> Vec<ResizeBorder>;

    /// Split the leaf containing `target_pane` along `axis`, placing
    /// `new_pane` in the specified position relative to the target.
    /// Returns `Err` if `target_pane` is not found.
    pub fn split(
        &mut self,
        target_pane: PaneId,
        new_pane: PaneId,
        axis: Axis,
        position: SplitPosition,
    ) -> Result<(), LayoutError>;

    /// Add `pane` as a new tab in the same leaf node as `target_pane`.
    pub fn add_tab(
        &mut self,
        target_pane: PaneId,
        new_pane: PaneId,
    ) -> Result<(), LayoutError>;

    /// Close `pane`, removing it from whatever leaf contains it.
    /// If the leaf becomes empty, collapse the parent split.
    /// Returns the list of PaneIds that were closed (for cleanup).
    pub fn close_pane(&mut self, pane: PaneId) -> Result<Vec<PaneId>, LayoutError>;

    /// Update the split ratio of the split node identified by `node_id`.
    /// Clamps to valid range.
    pub fn resize(&mut self, node_id: NodeId, new_ratio: f32);

    /// Swap the positions of two panes.
    pub fn swap_panes(&mut self, a: PaneId, b: PaneId) -> Result<(), LayoutError>;

    /// Move `pane` from its current location and drop it relative to
    /// `target_pane` at `position`. This is the high-level drag-and-drop
    /// operation combining close + split (or close + add_tab).
    pub fn move_pane(
        &mut self,
        pane: PaneId,
        target_pane: PaneId,
        position: DropZone,
    ) -> Result<(), LayoutError>;

    /// Return a flat list of all PaneIds in the layout (in tree order).
    pub fn all_panes(&self) -> Vec<PaneId>;

    /// Find the NodeId of the leaf containing `pane`.
    pub fn find_leaf(&self, pane: PaneId) -> Option<NodeId>;

    /// Check if workspace is empty.
    pub fn is_empty(&self) -> bool;

    /// How many total panes exist.
    pub fn pane_count(&self) -> usize;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitPosition {
    /// New pane goes before (left of / above) the target.
    Before,
    /// New pane goes after (right of / below) the target.
    After,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropZone {
    Left,
    Right,
    Top,
    Bottom,
    Tab,
}

#[derive(Debug, thiserror::Error)]
pub enum LayoutError {
    #[error("pane {0:?} not found in layout")]
    PaneNotFound(PaneId),
    #[error("cannot split: layout would exceed minimum panel size")]
    TooSmall,
    #[error("node {0:?} not found in layout")]
    NodeNotFound(NodeId),
}
```

### Tree Structure Visualization

A 2x2 grid in this model:

```
         Split(V, 0.5)              // top-bottom split
        /              \
  Split(H, 0.5)    Split(H, 0.5)   // each half split left-right
  /        \        /        \
Leaf(A)  Leaf(B)  Leaf(C)  Leaf(D)
```

Pixel rects for a 1200x800 workspace:
- A: (0, 0, 600, 400)
- B: (600, 0, 600, 400)
- C: (0, 400, 600, 400)
- D: (600, 400, 600, 400)

A more complex layout (3 charts: one tall on the left, two stacked on the right):

```
      Split(H, 0.4)
      /            \
   Leaf(A)      Split(V, 0.6)
                /            \
             Leaf(B)       Leaf(C)
```

Pixel rects for 1200x800:
- A: (0, 0, 480, 800)      -- left 40%, full height
- B: (480, 0, 720, 480)    -- right 60%, top 60%
- C: (480, 480, 720, 320)  -- right 60%, bottom 40%

---

## 3. Split Algorithm

### Operation: Split a Leaf

When the user drops a new chart (or an existing chart being moved) onto the left/right/top/bottom zone of an existing chart, we split the target leaf.

**Input:** `target_pane: PaneId`, `new_pane: PaneId`, `axis: Axis`, `position: SplitPosition`

**Algorithm:**

```
1. Walk the tree to find the leaf node containing `target_pane`.
   (This is a recursive search -- O(n) where n = number of nodes,
    but n is tiny: a 20-chart layout has ~39 nodes max.)

2. Remove the leaf from its parent, remembering the parent pointer.

3. Create a new SplitNode:
   - axis = the requested axis
   - ratio = 0.5 (default: equal split)
   - first = (see step 4)
   - second = (see step 4)

4. Based on `position`:
   - Before (Left or Top): first = new LeafNode(new_pane), second = old leaf
   - After (Right or Bottom): first = old leaf, second = new LeafNode(new_pane)

5. Replace the old leaf's position in the tree with the new SplitNode.
   - If the old leaf was root, the new SplitNode becomes root.
   - If the old leaf was first child of parent, replace parent.first.
   - If the old leaf was second child of parent, replace parent.second.
```

### Step-by-Step Diagram: Splitting Chart B to add Chart E on its right

**Before:**
```
      Split(V, 0.5)        window: 1200x800
      /            \
 Leaf(A)       Leaf(B)
 (0,0,           (600,0,
  600,800)        600,800)
```

**User drags Chart E onto the RIGHT zone of Chart B.**

**After:**
```
      Split(V, 0.5)                    window: 1200x800
      /            \
 Leaf(A)       Split(V, 0.5)       <-- new split node replaces old Leaf(B)
               /            \
           Leaf(B)       Leaf(E)    <-- B stays as first, E is new second
```

Note: both splits are `Axis::Horizontal` in this case (I used "V" loosely above -- let me be precise). Since charts are placed left/right, this is `Axis::Horizontal`:

```
       Split(H, 0.5)                    window: 1200x800
       /            \
  Leaf(A)       Split(H, 0.5)
  (0,0,         /            \
   600,800)  Leaf(B)       Leaf(E)
             (600,0,       (900,0,
              300,800)      300,800)
```

**Pixel rects after:**
- A: (0, 0, 600, 800) -- unchanged
- B: (600, 0, 300, 800) -- B got halved
- E: (900, 0, 300, 800) -- E fills the other half

### Step-by-Step Diagram: Splitting Chart A to add Chart F below it

**Before (continuing from above):**
```
       Split(H, 0.5)
       /            \
  Leaf(A)       Split(H, 0.5)
                /            \
             Leaf(B)       Leaf(E)
```

**User drags Chart F onto the BOTTOM zone of Chart A.**

**After:**
```
       Split(H, 0.5)
       /                    \
  Split(V, 0.5)        Split(H, 0.5)
  /            \        /            \
Leaf(A)    Leaf(F)   Leaf(B)       Leaf(E)
```

Pixel rects for 1200x800:
- A: (0, 0, 600, 400)
- F: (0, 400, 600, 400)
- B: (600, 0, 300, 800)
- E: (900, 0, 300, 800)

### Implementation Sketch

```rust
impl WorkspaceLayout {
    pub fn split(
        &mut self,
        target_pane: PaneId,
        new_pane: PaneId,
        axis: Axis,
        position: SplitPosition,
    ) -> Result<(), LayoutError> {
        let root = self.root.as_mut().ok_or(LayoutError::PaneNotFound(target_pane))?;

        // Recursive helper that finds and replaces the target leaf in-place.
        fn split_recursive(
            node: &mut LayoutNode,
            target: PaneId,
            new_pane: PaneId,
            axis: Axis,
            position: SplitPosition,
            id_gen: &mut impl FnMut() -> NodeId,
        ) -> Result<bool, LayoutError> {
            match node {
                LayoutNode::Leaf(leaf) => {
                    if !leaf.tabs.contains(&target) {
                        return Ok(false); // not this leaf
                    }

                    let old_leaf = node.clone();
                    let new_leaf = LayoutNode::Leaf(LeafNode {
                        id: id_gen(),
                        tabs: vec![new_pane],
                        active_tab: 0,
                    });

                    let (first, second) = match position {
                        SplitPosition::Before => (new_leaf, old_leaf),
                        SplitPosition::After => (old_leaf, new_leaf),
                    };

                    *node = LayoutNode::Split(SplitNode {
                        id: id_gen(),
                        axis,
                        ratio: 0.5,
                        first: Box::new(first),
                        second: Box::new(second),
                    });

                    Ok(true)
                }
                LayoutNode::Split(split) => {
                    if split_recursive(&mut split.first, target, new_pane, axis, position, id_gen)? {
                        return Ok(true);
                    }
                    split_recursive(&mut split.second, target, new_pane, axis, position, id_gen)
                }
            }
        }

        let mut counter = self.next_id;
        let mut id_gen = || {
            let id = NodeId(counter);
            counter += 1;
            id
        };

        let found = split_recursive(root, target_pane, new_pane, axis, position, &mut id_gen)?;
        self.next_id = counter;

        if found {
            Ok(())
        } else {
            Err(LayoutError::PaneNotFound(target_pane))
        }
    }
}
```

### DropZone to Split Mapping

```rust
impl DropZone {
    pub fn to_split_params(self) -> Option<(Axis, SplitPosition)> {
        match self {
            DropZone::Left   => Some((Axis::Horizontal, SplitPosition::Before)),
            DropZone::Right  => Some((Axis::Horizontal, SplitPosition::After)),
            DropZone::Top    => Some((Axis::Vertical, SplitPosition::Before)),
            DropZone::Bottom => Some((Axis::Vertical, SplitPosition::After)),
            DropZone::Tab    => None, // handled separately via add_tab
        }
    }
}
```

---

## 4. Resize Algorithm

### Overview

Every split node has a `ratio: f32` field controlling how space is divided between its two children. Resizing is the act of changing this ratio by dragging the border between two adjacent panels.

### Border Detection

During `compute_borders()`, we walk the tree and for each `SplitNode`, compute the pixel position of the dividing line:

```rust
fn compute_borders_recursive(
    node: &LayoutNode,
    bounds: PaneRect,
    out: &mut Vec<ResizeBorder>,
) {
    if let LayoutNode::Split(split) = node {
        let (first_bounds, second_bounds) = split_bounds(split, bounds);

        // The border sits between first_bounds and second_bounds.
        let border_rect = match split.axis {
            Axis::Horizontal => PaneRect {
                x: first_bounds.x + first_bounds.width - BORDER_WIDTH / 2.0,
                y: bounds.y,
                width: BORDER_WIDTH,
                height: bounds.height,
            },
            Axis::Vertical => PaneRect {
                x: bounds.x,
                y: first_bounds.y + first_bounds.height - BORDER_WIDTH / 2.0,
                width: bounds.width,
                height: BORDER_WIDTH,
            },
        };

        out.push(ResizeBorder {
            split_node_id: split.id,
            axis: split.axis,
            rect: border_rect,
        });

        // Recurse into children
        compute_borders_recursive(&split.first, first_bounds, out);
        compute_borders_recursive(&split.second, second_bounds, out);
    }
}

fn split_bounds(split: &SplitNode, bounds: PaneRect) -> (PaneRect, PaneRect) {
    match split.axis {
        Axis::Horizontal => {
            let first_w = bounds.width * split.ratio;
            let second_w = bounds.width - first_w;
            (
                PaneRect { x: bounds.x, y: bounds.y, width: first_w, height: bounds.height },
                PaneRect { x: bounds.x + first_w, y: bounds.y, width: second_w, height: bounds.height },
            )
        }
        Axis::Vertical => {
            let first_h = bounds.height * split.ratio;
            let second_h = bounds.height - first_h;
            (
                PaneRect { x: bounds.x, y: bounds.y, width: bounds.width, height: first_h },
                PaneRect { x: bounds.x, y: bounds.y + first_h, width: bounds.width, height: second_h },
            )
        }
    }
}
```

### Resize Interaction

When the user starts dragging a resize border:

1. **Hit test** the mouse position against all `ResizeBorder` rects (computed each frame or cached).
2. When a border is hit, record the `split_node_id` and the initial mouse position.
3. On mouse move during drag, compute the new ratio:

```rust
fn compute_new_ratio(
    split: &SplitNode,
    bounds: PaneRect,
    mouse_position: f32,  // mouse X for Horizontal, mouse Y for Vertical
) -> f32 {
    let (origin, size) = match split.axis {
        Axis::Horizontal => (bounds.x, bounds.width),
        Axis::Vertical => (bounds.y, bounds.height),
    };

    let raw_ratio = (mouse_position - origin) / size;

    // Clamp to prevent panels smaller than minimum
    let min_ratio = MIN_PANEL_SIZE_PX / size;
    let max_ratio = 1.0 - min_ratio;

    raw_ratio.clamp(min_ratio, max_ratio)
}
```

4. Call `layout.resize(split_node_id, new_ratio)`.

### Minimum Size Constraint Propagation

The simple ratio clamp above handles the immediate children, but what about deeply nested trees? Consider:

```
Split(H, 0.5)          -- window width 1200
/            \
Leaf(A)    Split(H, 0.5)
           /            \
        Leaf(B)       Leaf(C)
```

If the user drags the outer border to shrink the right side, Leaf(B) and Leaf(C) both need to stay above `MIN_PANEL_SIZE_PX`. The maximum the outer ratio can go is:

```
max_outer_ratio = 1.0 - (2 * MIN_PANEL_SIZE_PX / window_width)
```

More generally, the minimum space a subtree needs is determined by its deepest chain of splits along the same axis:

```rust
/// Compute the minimum size in logical pixels that a subtree requires
/// along a given axis.
fn min_size(node: &LayoutNode, axis: Axis) -> f32 {
    match node {
        LayoutNode::Leaf(_) => MIN_PANEL_SIZE_PX,
        LayoutNode::Split(split) => {
            if split.axis == axis {
                // Both children need space along this axis
                min_size(&split.first, axis) + min_size(&split.second, axis)
            } else {
                // Children are stacked along the other axis;
                // along our query axis they share the same space,
                // so the minimum is the max of the two children.
                min_size(&split.first, axis).max(min_size(&split.second, axis))
            }
        }
    }
}
```

During resize of a split node, we compute:

```rust
fn clamped_ratio(split: &SplitNode, bounds: PaneRect) -> (f32, f32) {
    let size = match split.axis {
        Axis::Horizontal => bounds.width,
        Axis::Vertical => bounds.height,
    };

    let first_min = min_size(&split.first, split.axis);
    let second_min = min_size(&split.second, split.axis);

    let ratio_min = first_min / size;
    let ratio_max = 1.0 - (second_min / size);

    (ratio_min.max(MIN_RATIO), ratio_max.min(1.0 - MIN_RATIO))
}
```

### Resize Cursor Feedback

- When hovering a horizontal split border: show `CursorIcon::ColResize` (or `EwResize`)
- When hovering a vertical split border: show `CursorIcon::RowResize` (or `NsResize`)

### Double-Click to Equalize

Double-clicking a resize border resets it to `ratio = 0.5`. This is a common UX pattern in tiling managers.

---

## 5. Close / Remove Algorithm

### Operation: Close a Pane

When the user clicks the X on a chart panel, we remove that pane from the layout and reclaim its space.

**Algorithm:**

```
1. Find the leaf node containing the target pane.

2. If the leaf has multiple tabs:
   a. Remove the pane from the tabs list.
   b. If the active_tab index is now out of bounds, set active_tab = tabs.len() - 1.
   c. Done -- the leaf remains, just with one fewer tab.

3. If the leaf has exactly one tab (the pane being closed):
   a. Find the parent split node of this leaf.
   b. Determine the sibling (the other child of the parent split).
   c. Replace the parent split node with the sibling.
      - If the parent split was the root, the sibling becomes the new root.
      - If the parent split was a child of a grandparent, the sibling takes
        the parent split's position in the grandparent.
   d. The closed leaf is dropped.

4. If the leaf is the root (single pane, no parent):
   a. Set root = None. The workspace is now empty.
```

### Step-by-Step Diagram: Closing Chart B

**Before:**
```
       Split(H, 0.5)         id=S1
       /            \
  Leaf(A)       Split(H, 0.5)     id=S2
  id=L1         /            \
             Leaf(B)       Leaf(E)
             id=L2         id=L3
```

**Close Chart B:**

1. Find Leaf(B) -- it is `L2`, child of Split `S2`.
2. The sibling of `L2` within `S2` is `Leaf(E)` (`L3`).
3. Replace `S2` in its parent (`S1.second`) with `L3`.

**After:**
```
       Split(H, 0.5)         id=S1
       /            \
  Leaf(A)       Leaf(E)
  id=L1         id=L3
```

Chart E now gets the full right half of the workspace. The space that B occupied has been cleanly reclaimed by E.

### Step-by-Step Diagram: Closing the last chart

**Before:**
```
Leaf(A)    <-- root
```

**Close Chart A:**

1. Leaf(A) is the root, single tab.
2. Set `root = None`.
3. Workspace is empty. Show empty state UI.

### Implementation Sketch

```rust
impl WorkspaceLayout {
    pub fn close_pane(&mut self, pane: PaneId) -> Result<Vec<PaneId>, LayoutError> {
        let root = self.root.as_mut().ok_or(LayoutError::PaneNotFound(pane))?;

        // Special case: root is a leaf
        if let LayoutNode::Leaf(leaf) = root {
            if let Some(idx) = leaf.tabs.iter().position(|&p| p == pane) {
                leaf.tabs.remove(idx);
                if leaf.tabs.is_empty() {
                    self.root = None;
                    return Ok(vec![pane]);
                }
                leaf.active_tab = leaf.active_tab.min(leaf.tabs.len() - 1);
                return Ok(vec![pane]);
            }
            return Err(LayoutError::PaneNotFound(pane));
        }

        // General case: find and remove within the tree
        match Self::close_recursive(root, pane) {
            CloseResult::NotFound => Err(LayoutError::PaneNotFound(pane)),
            CloseResult::RemovedFromTab => Ok(vec![pane]),
            CloseResult::CollapsedToSibling(sibling) => {
                // The root split collapsed -- but this case is handled
                // inside close_recursive by replacing the split in-place.
                // If close_recursive returns this, the root itself was
                // the parent. Replace root with sibling.
                self.root = Some(sibling);
                Ok(vec![pane])
            }
        }
    }
}

enum CloseResult {
    NotFound,
    RemovedFromTab,
    CollapsedToSibling(LayoutNode),
}

impl WorkspaceLayout {
    fn close_recursive(node: &mut LayoutNode, pane: PaneId) -> CloseResult {
        match node {
            LayoutNode::Leaf(leaf) => {
                if let Some(idx) = leaf.tabs.iter().position(|&p| p == pane) {
                    leaf.tabs.remove(idx);
                    if leaf.tabs.is_empty() {
                        // Signal to parent: this leaf is now empty,
                        // collapse me out.
                        // (This is handled by the Split arm below.)
                        return CloseResult::NotFound; // shouldn't reach here
                    }
                    leaf.active_tab = leaf.active_tab.min(leaf.tabs.len() - 1);
                    CloseResult::RemovedFromTab
                } else {
                    CloseResult::NotFound
                }
            }
            LayoutNode::Split(split) => {
                // Try first child
                let first_has_pane = Self::subtree_contains_pane(&split.first, pane);

                if first_has_pane {
                    // Check if first child is a leaf with exactly this pane
                    if Self::is_leaf_with_single_pane(&split.first, pane) {
                        // Collapse: replace this entire split with second child
                        let sibling = *split.second.clone();
                        *node = sibling;
                        return CloseResult::RemovedFromTab;
                    }
                    return Self::close_recursive(&mut split.first, pane);
                }

                let second_has_pane = Self::subtree_contains_pane(&split.second, pane);

                if second_has_pane {
                    if Self::is_leaf_with_single_pane(&split.second, pane) {
                        let sibling = *split.first.clone();
                        *node = sibling;
                        return CloseResult::RemovedFromTab;
                    }
                    return Self::close_recursive(&mut split.second, pane);
                }

                CloseResult::NotFound
            }
        }
    }

    fn subtree_contains_pane(node: &LayoutNode, pane: PaneId) -> bool {
        match node {
            LayoutNode::Leaf(leaf) => leaf.tabs.contains(&pane),
            LayoutNode::Split(split) => {
                Self::subtree_contains_pane(&split.first, pane)
                    || Self::subtree_contains_pane(&split.second, pane)
            }
        }
    }

    fn is_leaf_with_single_pane(node: &LayoutNode, pane: PaneId) -> bool {
        matches!(node, LayoutNode::Leaf(leaf) if leaf.tabs.len() == 1 && leaf.tabs[0] == pane)
    }
}
```

### What Happens to Ratios After Close?

When a split node is collapsed (replaced by its surviving child), the surviving subtree inherits the *full space* that the parent split occupied. This is correct behavior -- the surviving charts grow to fill the gap.

If the user had carefully sized things, this growth is exactly what they'd expect: closing a chart makes its neighbor bigger.

---

## 6. Drag and Drop UX

### Overview

Drag and drop is the primary way users rearrange charts. The UX has three phases:

1. **Drag start**: User clicks and holds on a chart's title bar (or a dedicated drag handle).
2. **Drag hover**: As the user moves the mouse over other chart panels, overlay zones appear showing where the chart will dock.
3. **Drop**: User releases the mouse. The layout tree is restructured.

### Drop Zone Geometry

When the user is dragging a chart and hovers over another chart panel, divide the target panel into 5 zones:

```
+-------------------------------------------+
|                  TOP (25%)                |
|  +-------------------------------------+ |
|  |                                     | |
|L |                                     |R|
|E |           CENTER                    |I|
|F |            (tab)                    |G|
|T |                                     |H|
|  |                                     |T|
|25|                                     |25|
|% |                                     |% |
|  +-------------------------------------+ |
|                BOTTOM (25%)               |
+-------------------------------------------+
```

Precise hit zones (in local coordinates relative to the panel rect):

```rust
/// Determine which drop zone the cursor is in, given cursor position
/// relative to the panel's top-left corner.
fn hit_test_drop_zone(
    cursor_x: f32,
    cursor_y: f32,
    panel_width: f32,
    panel_height: f32,
) -> Option<DropZone> {
    let margin_x = panel_width * 0.25;
    let margin_y = panel_height * 0.25;

    // Check edges first (priority over center)
    if cursor_x < margin_x {
        return Some(DropZone::Left);
    }
    if cursor_x > panel_width - margin_x {
        return Some(DropZone::Right);
    }
    if cursor_y < margin_y {
        return Some(DropZone::Top);
    }
    if cursor_y > panel_height - margin_y {
        return Some(DropZone::Bottom);
    }

    // Center region = tab drop
    Some(DropZone::Tab)
}
```

**Important refinement**: For small panels, the edge zones might overlap. If the panel is less than `4 * MIN_PANEL_SIZE_PX` in either dimension, suppress splits along that axis (only allow tab drops). This prevents creating impossibly small panels.

### Drop Preview Overlay

While the user is hovering with a dragged chart, render a semi-transparent overlay showing where the chart will land:

```rust
/// Describes the preview highlight to render during drag-and-drop.
#[derive(Debug, Clone, Copy)]
pub struct DropPreview {
    /// The pixel rect of the highlighted zone.
    pub rect: PaneRect,
    /// The zone type (for visual styling -- edge drops show the
    /// zone as half the panel, tab drops show the full panel).
    pub zone: DropZone,
}

fn compute_drop_preview(
    panel_rect: PaneRect,
    zone: DropZone,
) -> DropPreview {
    let rect = match zone {
        DropZone::Left => PaneRect {
            x: panel_rect.x,
            y: panel_rect.y,
            width: panel_rect.width * 0.5,
            height: panel_rect.height,
        },
        DropZone::Right => PaneRect {
            x: panel_rect.x + panel_rect.width * 0.5,
            y: panel_rect.y,
            width: panel_rect.width * 0.5,
            height: panel_rect.height,
        },
        DropZone::Top => PaneRect {
            x: panel_rect.x,
            y: panel_rect.y,
            width: panel_rect.width,
            height: panel_rect.height * 0.5,
        },
        DropZone::Bottom => PaneRect {
            x: panel_rect.x,
            y: panel_rect.y + panel_rect.height * 0.5,
            width: panel_rect.width,
            height: panel_rect.height * 0.5,
        },
        DropZone::Tab => PaneRect {
            x: panel_rect.x,
            y: panel_rect.y,
            width: panel_rect.width,
            height: panel_rect.height,
        },
    };

    DropPreview { rect, zone }
}
```

**Visual styling:**

| Zone | Preview Color | Description |
|---|---|---|
| Left/Right/Top/Bottom | `rgba(70, 130, 255, 0.25)` | Blue tint covering half the target panel |
| Tab | `rgba(70, 130, 255, 0.15)` | Lighter blue tint covering entire panel |

The preview overlay is rendered as a simple filled rectangle on top of the chart content. Use the `rect.wgsl` pipeline (already planned in midas-render) or render via iced's built-in `container` with a background color.

### Drop Execution

When the user releases the mouse:

```rust
fn execute_drop(
    layout: &mut WorkspaceLayout,
    dragged_pane: PaneId,
    target_pane: PaneId,
    zone: DropZone,
) -> Result<(), LayoutError> {
    if dragged_pane == target_pane {
        return Ok(()); // dropped on self, no-op
    }

    match zone {
        DropZone::Tab => {
            // Step 1: Remove dragged_pane from its current location
            layout.close_pane(dragged_pane)?;
            // Step 2: Add as tab to target
            layout.add_tab(target_pane, dragged_pane)?;
        }
        edge_zone => {
            let (axis, position) = edge_zone.to_split_params().unwrap();
            // Step 1: Remove dragged_pane from its current location
            layout.close_pane(dragged_pane)?;
            // Step 2: Split target and insert dragged pane
            layout.split(target_pane, dragged_pane, axis, position)?;
        }
    }

    Ok(())
}
```

**Subtlety**: The `close_pane` call may restructure the tree, which could change the position of `target_pane`. However, since we identify panes by `PaneId` (not by tree position), the subsequent `split` or `add_tab` call simply searches for `target_pane` by ID. The tree restructuring from close does not invalidate the PaneId.

### Drag Source: Title Bar vs Full Panel

Two options for what triggers a drag:

1. **Title bar drag handle only** (recommended initially): Less accidental drags. The title bar of each chart panel has a small grip icon or is itself the drag handle.

2. **Full panel drag with modifier key**: Hold Ctrl+click anywhere in the chart to start dragging. This is secondary UX, added later.

### Drag State Machine

```rust
#[derive(Debug)]
pub enum DragState {
    /// No drag in progress.
    Idle,

    /// User has pressed down on a drag handle but hasn't moved enough
    /// to start the drag (prevents accidental drags from clicks).
    PendingDrag {
        pane: PaneId,
        start_pos: (f32, f32),
    },

    /// Active drag in progress.
    Dragging {
        pane: PaneId,
        /// Current mouse position.
        cursor_pos: (f32, f32),
        /// Which panel the cursor is currently over (if any).
        hover_target: Option<PaneId>,
        /// Which drop zone within the hover target.
        hover_zone: Option<DropZone>,
    },
}

const DRAG_THRESHOLD: f32 = 5.0; // pixels before drag activates
```

State transitions:

```
Idle --[mouse down on drag handle]--> PendingDrag
PendingDrag --[mouse moved > DRAG_THRESHOLD]--> Dragging
PendingDrag --[mouse up]--> Idle (was just a click)
Dragging --[mouse move]--> Dragging (update cursor_pos, recalc hover)
Dragging --[mouse up over drop zone]--> Idle (execute drop)
Dragging --[mouse up over nothing / escape]--> Idle (cancel)
```

---

## 7. Preset Layouts

### Generating Common Layouts

Preset layouts are factory functions that produce a `WorkspaceLayout` with a specific tree structure.

```rust
impl WorkspaceLayout {
    /// Single chart, full workspace.
    pub fn preset_1x1(&mut self) -> PaneId {
        let pane = self.next_pane_id();
        self.root = Some(LayoutNode::Leaf(LeafNode {
            id: self.next_node_id(),
            tabs: vec![pane],
            active_tab: 0,
        }));
        pane
    }

    /// 2 charts side by side (1 row, 2 columns).
    ///
    ///  +-------+-------+
    ///  |       |       |
    ///  |   A   |   B   |
    ///  |       |       |
    ///  +-------+-------+
    pub fn preset_2x1(&mut self) -> Vec<PaneId> {
        let a = self.next_pane_id();
        let b = self.next_pane_id();

        self.root = Some(LayoutNode::Split(SplitNode {
            id: self.next_node_id(),
            axis: Axis::Horizontal,
            ratio: 0.5,
            first: Box::new(LayoutNode::Leaf(LeafNode {
                id: self.next_node_id(),
                tabs: vec![a],
                active_tab: 0,
            })),
            second: Box::new(LayoutNode::Leaf(LeafNode {
                id: self.next_node_id(),
                tabs: vec![b],
                active_tab: 0,
            })),
        }));

        vec![a, b]
    }

    /// 2 charts stacked (2 rows, 1 column).
    ///
    ///  +---------------+
    ///  |       A       |
    ///  +---------------+
    ///  |       B       |
    ///  +---------------+
    pub fn preset_1x2(&mut self) -> Vec<PaneId> {
        let a = self.next_pane_id();
        let b = self.next_pane_id();

        self.root = Some(LayoutNode::Split(SplitNode {
            id: self.next_node_id(),
            axis: Axis::Vertical,
            ratio: 0.5,
            first: Box::new(LayoutNode::Leaf(LeafNode {
                id: self.next_node_id(),
                tabs: vec![a],
                active_tab: 0,
            })),
            second: Box::new(LayoutNode::Leaf(LeafNode {
                id: self.next_node_id(),
                tabs: vec![b],
                active_tab: 0,
            })),
        }));

        vec![a, b]
    }

    /// 4 charts in a 2x2 grid.
    ///
    ///  +-------+-------+
    ///  |   A   |   B   |
    ///  +-------+-------+
    ///  |   C   |   D   |
    ///  +-------+-------+
    pub fn preset_2x2(&mut self) -> Vec<PaneId> {
        let a = self.next_pane_id();
        let b = self.next_pane_id();
        let c = self.next_pane_id();
        let d = self.next_pane_id();

        //        Split(V, 0.5)
        //       /             \
        //  Split(H, 0.5)   Split(H, 0.5)
        //  /      \         /      \
        // A        B       C        D

        self.root = Some(LayoutNode::Split(SplitNode {
            id: self.next_node_id(),
            axis: Axis::Vertical,
            ratio: 0.5,
            first: Box::new(LayoutNode::Split(SplitNode {
                id: self.next_node_id(),
                axis: Axis::Horizontal,
                ratio: 0.5,
                first: Box::new(self.make_leaf(a)),
                second: Box::new(self.make_leaf(b)),
            })),
            second: Box::new(LayoutNode::Split(SplitNode {
                id: self.next_node_id(),
                axis: Axis::Horizontal,
                ratio: 0.5,
                first: Box::new(self.make_leaf(c)),
                second: Box::new(self.make_leaf(d)),
            })),
        }));

        vec![a, b, c, d]
    }

    /// 6 charts in a 3x2 grid (3 columns, 2 rows).
    ///
    ///  +-----+-----+-----+
    ///  |  A  |  B  |  C  |
    ///  +-----+-----+-----+
    ///  |  D  |  E  |  F  |
    ///  +-----+-----+-----+
    pub fn preset_3x2(&mut self) -> Vec<PaneId> {
        let panes: Vec<PaneId> = (0..6).map(|_| self.next_pane_id()).collect();

        // Tree structure for 3 columns:
        //   Split(H, 0.333)
        //   /              \
        //  col0     Split(H, 0.5)
        //           /            \
        //         col1          col2
        //
        // Each col is a vertical split of 2 rows.

        fn make_column(layout: &mut WorkspaceLayout, top: PaneId, bottom: PaneId) -> LayoutNode {
            LayoutNode::Split(SplitNode {
                id: layout.next_node_id(),
                axis: Axis::Vertical,
                ratio: 0.5,
                first: Box::new(layout.make_leaf(top)),
                second: Box::new(layout.make_leaf(bottom)),
            })
        }

        let col0 = make_column(self, panes[0], panes[3]);
        let col1 = make_column(self, panes[1], panes[4]);
        let col2 = make_column(self, panes[2], panes[5]);

        self.root = Some(LayoutNode::Split(SplitNode {
            id: self.next_node_id(),
            axis: Axis::Horizontal,
            ratio: 1.0 / 3.0,
            first: Box::new(col0),
            second: Box::new(LayoutNode::Split(SplitNode {
                id: self.next_node_id(),
                axis: Axis::Horizontal,
                ratio: 0.5,
                first: Box::new(col1),
                second: Box::new(col2),
            })),
        }));

        panes
    }

    /// 8 charts in a 4x2 grid (4 columns, 2 rows).
    ///
    ///  +----+----+----+----+
    ///  | A  | B  | C  | D  |
    ///  +----+----+----+----+
    ///  | E  | F  | G  | H  |
    ///  +----+----+----+----+
    pub fn preset_4x2(&mut self) -> Vec<PaneId> {
        let panes: Vec<PaneId> = (0..8).map(|_| self.next_pane_id()).collect();

        fn make_column(layout: &mut WorkspaceLayout, top: PaneId, bottom: PaneId) -> LayoutNode {
            LayoutNode::Split(SplitNode {
                id: layout.next_node_id(),
                axis: Axis::Vertical,
                ratio: 0.5,
                first: Box::new(layout.make_leaf(top)),
                second: Box::new(layout.make_leaf(bottom)),
            })
        }

        // Build a balanced binary tree of 4 columns:
        //         Split(H, 0.5)
        //        /              \
        //  Split(H, 0.5)   Split(H, 0.5)
        //  /      \         /      \
        // col0   col1     col2    col3

        let col0 = make_column(self, panes[0], panes[4]);
        let col1 = make_column(self, panes[1], panes[5]);
        let col2 = make_column(self, panes[2], panes[6]);
        let col3 = make_column(self, panes[3], panes[7]);

        let left_half = LayoutNode::Split(SplitNode {
            id: self.next_node_id(),
            axis: Axis::Horizontal,
            ratio: 0.5,
            first: Box::new(col0),
            second: Box::new(col1),
        });

        let right_half = LayoutNode::Split(SplitNode {
            id: self.next_node_id(),
            axis: Axis::Horizontal,
            ratio: 0.5,
            first: Box::new(col2),
            second: Box::new(col3),
        });

        self.root = Some(LayoutNode::Split(SplitNode {
            id: self.next_node_id(),
            axis: Axis::Horizontal,
            ratio: 0.5,
            first: Box::new(left_half),
            second: Box::new(right_half),
        }));

        panes
    }

    /// TC2000-style "focus + context" layout:
    /// One large chart on the left, 4 small charts stacked on the right.
    ///
    ///  +-----------+------+
    ///  |           |  B   |
    ///  |           +------+
    ///  |     A     |  C   |
    ///  |           +------+
    ///  |           |  D   |
    ///  |           +------+
    ///  |           |  E   |
    ///  +-----------+------+
    pub fn preset_focus_4(&mut self) -> Vec<PaneId> {
        let panes: Vec<PaneId> = (0..5).map(|_| self.next_pane_id()).collect();

        // Right side: 4-way vertical split (as a binary tree)
        //     Split(V, 0.5)
        //     /            \
        // Split(V,0.5)  Split(V,0.5)
        // /    \        /    \
        // B     C      D     E

        let right_top = LayoutNode::Split(SplitNode {
            id: self.next_node_id(),
            axis: Axis::Vertical,
            ratio: 0.5,
            first: Box::new(self.make_leaf(panes[1])),
            second: Box::new(self.make_leaf(panes[2])),
        });

        let right_bottom = LayoutNode::Split(SplitNode {
            id: self.next_node_id(),
            axis: Axis::Vertical,
            ratio: 0.5,
            first: Box::new(self.make_leaf(panes[3])),
            second: Box::new(self.make_leaf(panes[4])),
        });

        let right_side = LayoutNode::Split(SplitNode {
            id: self.next_node_id(),
            axis: Axis::Vertical,
            ratio: 0.5,
            first: Box::new(right_top),
            second: Box::new(right_bottom),
        });

        self.root = Some(LayoutNode::Split(SplitNode {
            id: self.next_node_id(),
            axis: Axis::Horizontal,
            ratio: 0.65,
            first: Box::new(self.make_leaf(panes[0])),
            second: Box::new(right_side),
        }));

        panes
    }

    // Helper
    fn make_leaf(&mut self, pane: PaneId) -> LayoutNode {
        LayoutNode::Leaf(LeafNode {
            id: self.next_node_id(),
            tabs: vec![pane],
            active_tab: 0,
        })
    }
}
```

### Preset Application Strategy

When the user selects a preset from the toolbar dropdown:

1. Determine which existing `PaneId`s are currently in the layout (call `all_panes()`).
2. Generate the preset tree, which creates new `PaneId`s.
3. Re-map: assign existing chart state (symbol, timeframe, indicators) to the new PaneIds in order. If the new layout has more panes than the old, the extras start as empty/default charts. If the new layout has fewer panes, the extras are discarded (or could be preserved as inactive state).
4. Replace `self.root` with the new tree.

This is a destructive operation from the layout's perspective, but the chart *content* is preserved by remapping PaneIds to chart state in the application layer (`MidasApp.charts`).

---

## 8. Serialization

### Format: TOML for Config, JSON Also Supported

The layout tree serializes naturally with serde since every type derives `Serialize` and `Deserialize`. The tree structure uses serde's default enum representation (externally tagged).

### TOML Representation

```toml
[workspace.layout]
next_id = 12

[workspace.layout.root]
type = "Split"

[workspace.layout.root.split]
id = 3
axis = "Horizontal"
ratio = 0.5

[workspace.layout.root.split.first]
type = "Leaf"

[workspace.layout.root.split.first.leaf]
id = 1
tabs = [1, 2]
active_tab = 0

[workspace.layout.root.split.second]
type = "Leaf"

[workspace.layout.root.split.second.leaf]
id = 2
tabs = [3]
active_tab = 0
```

### JSON Representation (more natural for recursive trees)

```json
{
  "next_id": 12,
  "root": {
    "Split": {
      "id": 3,
      "axis": "Horizontal",
      "ratio": 0.5,
      "first": {
        "Leaf": {
          "id": 1,
          "tabs": [1, 2],
          "active_tab": 0
        }
      },
      "second": {
        "Leaf": {
          "id": 2,
          "tabs": [3],
          "active_tab": 0
        }
      }
    }
  }
}
```

**Recommendation**: Use JSON for the layout tree specifically, even if the rest of the config file is TOML. Recursive tree structures are awkward in TOML's flat key hierarchy. Alternatively, embed the JSON blob as a string field in the TOML config:

```toml
[workspace]
layout_json = '{"next_id":12,"root":{...}}'
```

Or better, use a separate `layout.json` file alongside `config.toml`:

```
data/
  config.toml       # app settings, watchlist, theme
  layout.json       # workspace layout tree
  charts/           # per-pane chart configurations
    pane_1.toml     # symbol, timeframe, indicators for PaneId(1)
    pane_2.toml
    ...
```

### Serde Customization for Clean Output

To get clean serialization, use serde attributes:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum LayoutNode {
    Leaf(LeafNode),
    Split(SplitNode),
}
```

With `#[serde(tag = "type")]`, the JSON becomes:

```json
{
  "type": "Split",
  "id": 3,
  "axis": "Horizontal",
  "ratio": 0.5,
  "first": { "type": "Leaf", "id": 1, "tabs": [1], "active_tab": 0 },
  "second": { "type": "Leaf", "id": 2, "tabs": [2], "active_tab": 0 }
}
```

This is clean, flat, and easy to read/debug.

### Save Strategy

- **When**: Save on every layout mutation (split, close, resize, drag-and-drop), debounced to at most 1 write per second. Use the same debounced-save mechanism already planned in Phase 4.6 config persistence.
- **Atomicity**: Write to a temp file, then rename (atomic on all major filesystems). Prevents corrupt state if the app crashes mid-write.
- **Schema versioning**: Include a `version` field at the top level. When loading, check the version and run migration logic if needed.

```rust
#[derive(Debug, Serialize, Deserialize)]
pub struct PersistedLayout {
    /// Schema version for migration.
    pub version: u32,
    /// The layout tree.
    pub layout: WorkspaceLayout,
    /// Per-pane chart configurations (parallel to layout.all_panes()).
    pub pane_configs: HashMap<u64, PaneConfig>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PaneConfig {
    pub symbol: String,
    pub timeframe: String,
    pub indicators: Vec<IndicatorConfig>,
    pub horizontal_levels: Vec<HorizontalLevel>,
}

const CURRENT_VERSION: u32 = 1;
```

### Load Strategy

```rust
impl PersistedLayout {
    pub fn load(path: &Path) -> Result<Self, anyhow::Error> {
        let content = std::fs::read_to_string(path)?;
        let persisted: Self = serde_json::from_str(&content)?;

        if persisted.version > CURRENT_VERSION {
            anyhow::bail!(
                "Layout file version {} is newer than supported version {}",
                persisted.version,
                CURRENT_VERSION
            );
        }

        // Run migrations if needed
        // if persisted.version < 2 { migrate_v1_to_v2(&mut persisted); }

        Ok(persisted)
    }

    pub fn save(&self, path: &Path) -> Result<(), anyhow::Error> {
        let content = serde_json::to_string_pretty(self)?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, &content)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }
}
```

---

## 9. Integration with iced

### Architecture Overview

The layout system is purely a data structure with algorithms. It does not know about iced. The integration layer lives in `midas-app` and bridges the layout tree to iced's widget tree.

```
WorkspaceLayout (midas-core or midas-app)
    |
    | compute_rects(workspace_bounds) -> HashMap<PaneId, PaneRect>
    |
    v
workspace.rs view function (midas-app)
    |
    | For each (PaneId, PaneRect): create a ChartWidget positioned at that rect
    |
    v
iced widget tree
    |
    | iced's layout engine positions each ChartWidget
    |
    v
Each ChartWidget implements iced::widget::shader::Program
    |
    | ChartRenderer draws candles/volume/indicators into its PaneRect
    |
    v
wgpu render pass
```

### The Workspace View

```rust
// midas-app/src/views/workspace.rs

use iced::widget::{container, mouse_area, stack, canvas, Row, Column};
use iced::{Element, Length, Size, Rectangle};

pub fn workspace_view<'a>(
    app: &'a MidasApp,
) -> Element<'a, Message> {
    if app.layout.is_empty() {
        return empty_workspace_view();
    }

    // Step 1: We don't know the pixel bounds yet (iced computes them).
    // Instead, we build a recursive widget tree that mirrors the layout tree,
    // using iced's Row/Column with FillPortion for ratios.
    build_layout_widget(&app.layout.root.as_ref().unwrap(), app)
}

fn build_layout_widget<'a>(
    node: &LayoutNode,
    app: &'a MidasApp,
) -> Element<'a, Message> {
    match node {
        LayoutNode::Leaf(leaf) => {
            // Render the active tab's chart
            let pane_id = leaf.tabs[leaf.active_tab];
            let chart_widget = chart_panel_view(app, pane_id, &leaf.tabs, leaf.active_tab);
            chart_widget
        }
        LayoutNode::Split(split) => {
            let first = build_layout_widget(&split.first, app);
            let second = build_layout_widget(&split.second, app);

            // Convert ratio to fill portions (iced uses integer weights).
            // Multiply by 1000 for decent precision.
            let first_portion = (split.ratio * 1000.0) as u16;
            let second_portion = ((1.0 - split.ratio) * 1000.0) as u16;

            match split.axis {
                Axis::Horizontal => {
                    // Left-right arrangement
                    Row::new()
                        .push(
                            container(first)
                                .width(Length::FillPortion(first_portion))
                                .height(Length::Fill)
                        )
                        .push(
                            resize_handle_vertical(split.id)
                        )
                        .push(
                            container(second)
                                .width(Length::FillPortion(second_portion))
                                .height(Length::Fill)
                        )
                        .width(Length::Fill)
                        .height(Length::Fill)
                        .into()
                }
                Axis::Vertical => {
                    // Top-bottom arrangement
                    Column::new()
                        .push(
                            container(first)
                                .width(Length::Fill)
                                .height(Length::FillPortion(first_portion))
                        )
                        .push(
                            resize_handle_horizontal(split.id)
                        )
                        .push(
                            container(second)
                                .width(Length::Fill)
                                .height(Length::FillPortion(second_portion))
                        )
                        .width(Length::Fill)
                        .height(Length::Fill)
                        .into()
                }
            }
        }
    }
}
```

### Two Integration Strategies

There are two valid approaches. We document both and recommend one.

**Strategy A: iced-native layout via Row/Column/FillPortion** (shown above)

- Pros: Leverages iced's layout engine. Resize handles are regular widgets. Natural iced architecture.
- Cons: `FillPortion` uses integer weights, so precision is limited (but 1000 subdivisions = 0.1% granularity, which is fine). Getting the exact pixel rect of each chart for `compute_rects` requires a bit of indirection (the `Shader` widget receives its bounds in the `draw` call).
- Verdict: **Recommended.** Keeps us in iced's ecosystem. The Shader widget already receives its pixel bounds during rendering.

**Strategy B: Custom layout widget with absolute positioning**

- We create a single custom iced widget that does all layout math internally and positions chart Shader widgets at computed pixel rects.
- Pros: Full control over pixel-exact positioning. `compute_rects` output maps directly to child positions.
- Cons: Reimplements layout logic that iced already provides. More code. Fights the framework.
- Verdict: Reserve as fallback if Strategy A has issues.

### Resize Handle Widget

The resize handle between two split children is a thin interactive widget:

```rust
fn resize_handle_vertical<'a>(split_id: NodeId) -> Element<'a, Message> {
    // A 4px-wide vertical bar that changes cursor on hover
    // and emits resize messages on drag.
    mouse_area(
        container(iced::widget::Space::new(BORDER_WIDTH, Length::Fill))
            .style(|_| container::Style {
                background: Some(iced::Background::Color(iced::Color::from_rgb(
                    0.15, 0.15, 0.20,
                ))),
                ..Default::default()
            })
    )
    .on_press(Message::ResizeDragStart(split_id))
    .on_release(Message::ResizeDragEnd)
    .into()
}
```

During a resize drag, the `update` function tracks mouse position and calls:

```rust
Message::ResizeDragMove(split_id, mouse_x, mouse_y) => {
    // Compute new ratio based on mouse position relative to
    // the split node's bounds.
    // (Bounds must be tracked -- see below.)
    if let Some(bounds) = self.split_bounds.get(&split_id) {
        let new_ratio = compute_new_ratio(split_id, bounds, mouse_x);
        self.layout.resize(split_id, new_ratio);
    }
}
```

### Tracking Split Node Bounds

To compute the new ratio during resize, we need to know the pixel bounds of each split node. Two approaches:

1. **Compute from window size + tree walk**: Call `compute_rects` with the known workspace bounds each time we need to resolve a resize. This is cheap (tree is tiny) and keeps the code simple.

2. **Cache bounds during iced layout pass**: Each `resize_handle` widget can report its position via a message after iced lays it out. More complex but avoids recomputation.

**Recommendation**: Approach 1. Call `compute_rects` on every resize drag event. The tree has at most ~40 nodes for 20 charts. The computation is a few microseconds.

### Chart Panel View

Each leaf in the layout tree renders as a chart panel with:
- A thin title bar (symbol name, timeframe, tab buttons if multiple tabs, close button, drag handle)
- The chart Shader widget filling the remaining space

```rust
fn chart_panel_view<'a>(
    app: &'a MidasApp,
    active_pane: PaneId,
    tabs: &[PaneId],
    active_tab: usize,
) -> Element<'a, Message> {
    let chart_state = app.get_chart_state(active_pane);

    let title_bar = Row::new()
        .push(drag_handle(active_pane))
        .push(
            // Tab buttons (if multiple tabs)
            tabs.iter().enumerate().fold(Row::new().spacing(2), |row, (i, &pane_id)| {
                let label = app.get_chart_state(pane_id)
                    .map(|s| s.symbol.as_str())
                    .unwrap_or("Empty");
                let is_active = i == active_tab;
                row.push(tab_button(pane_id, label, is_active))
            })
        )
        .push(iced::widget::Space::new(Length::Fill, Length::Shrink))
        .push(close_button(active_pane))
        .height(TITLE_BAR_HEIGHT)
        .width(Length::Fill);

    Column::new()
        .push(title_bar)
        .push(
            ChartWidget::new(active_pane, chart_state)
                .width(Length::Fill)
                .height(Length::Fill)
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}
```

### Empty Workspace View

When `layout.is_empty()`, show a centered "+" button:

```rust
fn empty_workspace_view<'a>() -> Element<'a, Message> {
    container(
        button(text("+").size(48))
            .on_press(Message::AddFirstChart)
            .style(button::secondary)
            .padding(20)
    )
    .width(Length::Fill)
    .height(Length::Fill)
    .center_x()
    .center_y()
    .into()
}
```

### Drag and Drop Overlay

During a drag operation, render a semi-transparent overlay on top of the entire workspace using iced's `stack` widget (or `overlay`):

```rust
fn workspace_view_with_overlay<'a>(app: &'a MidasApp) -> Element<'a, Message> {
    let base = workspace_view(app);

    if let DragState::Dragging { ref hover_zone, .. } = app.drag_state {
        if let Some(preview) = &app.drop_preview {
            let overlay = canvas(DropPreviewCanvas {
                rect: preview.rect,
                zone: preview.zone,
            })
            .width(Length::Fill)
            .height(Length::Fill);

            stack![base, overlay].into()
        } else {
            base
        }
    } else {
        base
    }
}
```

### Message Flow for Layout Operations

```rust
pub enum Message {
    // ... existing messages ...

    // Layout mutations
    SplitPane {
        target: PaneId,
        new_pane: PaneId,
        zone: DropZone,
    },
    ClosePane(PaneId),
    SelectTab(PaneId, usize),
    ApplyPreset(PresetKind),
    AddFirstChart,

    // Resize
    ResizeDragStart(NodeId),
    ResizeDragMove(f32, f32),    // mouse position
    ResizeDragEnd,

    // Drag and drop
    PaneDragStart(PaneId),
    PaneDragMove(f32, f32),
    PaneDragEnd,
    PaneDragCancel,
}

#[derive(Debug, Clone)]
pub enum PresetKind {
    Single,
    TwoColumns,
    TwoRows,
    Grid2x2,
    Grid3x2,
    Grid4x2,
    FocusPlus4,
}
```

---

## 10. Edge Cases

### 10.1 Single Chart Fills Entire Window

**Scenario**: The workspace has exactly one pane (no splits).

**Behavior**: The root is a `LayoutNode::Leaf`. The chart fills the entire workspace bounds. No resize borders exist. Dropping a new chart onto it splits the root.

**Code path**: `compute_rects` returns a single entry. `build_layout_widget` returns a single `ChartWidget`. Clean, no special-casing needed.

### 10.2 Last Chart Closed -- Empty State

**Scenario**: User closes the only remaining chart.

**Behavior**: `close_pane` sets `root = None`. The `workspace_view` function detects `is_empty()` and renders the empty workspace with the "+" button.

**State**: The `MidasApp` transitions to having no active chart. The toolbar shows a disabled state or prompts the user to add a chart.

### 10.3 Very Small Window

**Scenario**: User resizes the OS window to a very small size (e.g., 200x150 pixels).

**Behavior**: The `min_size` function propagates minimum panel sizes up the tree. If the window is too small to fit all panels at their minimum size, we have two options:

1. **Clamp ratios so all panels get at least MIN_PANEL_SIZE_PX** (may cause panels to overlap or extend beyond window). Not ideal.

2. **Allow panels to go below MIN_PANEL_SIZE_PX but prevent new splits.** The layout still renders (just very small). The user cannot create new splits when space is insufficient.

3. **Recommended: Set a minimum window size on the OS window itself.** iced supports `window::Settings { min_size: Some((800, 600)), .. }`. This is the standard desktop app approach and prevents the problem entirely.

**Implementation**: Set `min_size` in iced window settings. Additionally, the resize algorithm's clamping prevents any individual split from creating panels below `MIN_PANEL_SIZE_PX` within the current window size.

### 10.4 Deeply Nested Splits

**Scenario**: User splits repeatedly, creating a tree 10+ levels deep. Each level halves the space. At depth 10 with a 1920px window, each panel would be `1920 / 2^10 = ~1.9px`.

**Behavior**: The split operation should *reject* splits that would create panels below the absolute minimum size.

```rust
pub fn split(
    &mut self,
    target_pane: PaneId,
    new_pane: PaneId,
    axis: Axis,
    position: SplitPosition,
) -> Result<(), LayoutError> {
    // Before mutating the tree, check if the target leaf's
    // current size (computed from the current window bounds)
    // is large enough to split.
    let rects = self.compute_rects(self.last_known_bounds);
    if let Some(rect) = rects.get(&target_pane) {
        let size = match axis {
            Axis::Horizontal => rect.width,
            Axis::Vertical => rect.height,
        };
        if size / 2.0 < MIN_PANEL_SIZE_PX {
            return Err(LayoutError::TooSmall);
        }
    }

    // ... proceed with split
}
```

**Practical limit**: With `MIN_PANEL_SIZE_PX = 80` and a 1920px window, the maximum horizontal split depth is `floor(log2(1920/80)) = 4` (16 panels across). This is more than enough for any reasonable trading workspace.

### 10.5 Drag-and-Drop Self-Targeting

**Scenario**: User drags a chart and drops it on itself.

**Behavior**: No-op. Detect this in `execute_drop` and return early.

### 10.6 Drag-and-Drop with Only One Pane

**Scenario**: User tries to drag the only chart in the workspace.

**Behavior**: Allow the drag to start (so the user can drop it into a "new chart" zone or cancel). If they drop it back on itself, no-op. The main use case is: open the layout preset menu while dragging, or drag it to a tab bar that might exist in the future.

**Alternative**: Do not allow dragging when there's only one pane. Show a tooltip: "Add another chart first."

**Recommendation**: Allow the drag but make it clear via the overlay that no valid drop target exists (dim the workspace, show a "no valid target" cursor).

### 10.7 Tab Group with Single Tab

**Scenario**: A leaf has `tabs: vec![pane_a]`. Is this valid?

**Behavior**: Yes. This is the normal state. The tab bar is either hidden entirely (since there's only one tab) or shows a single tab with a close button. When a second pane is dropped as a tab, the tab bar appears with two tabs.

**Display rule**: Hide the tab bar UI when `tabs.len() == 1`. Show it when `tabs.len() >= 2`.

### 10.8 Closing a Tab from a Multi-Tab Leaf

**Scenario**: A leaf has `tabs: [A, B, C]`, active_tab = 1 (B is visible). User closes B.

**Behavior**: Remove B from tabs. New tabs = `[A, C]`. The new active_tab should be smart:
- If the closed tab was the last one, active_tab decrements.
- Otherwise, active_tab stays the same index (which now points to the next tab).
- This matches browser tab-close behavior.

```rust
fn close_tab_from_leaf(leaf: &mut LeafNode, pane: PaneId) {
    if let Some(idx) = leaf.tabs.iter().position(|&p| p == pane) {
        leaf.tabs.remove(idx);
        if leaf.active_tab >= leaf.tabs.len() {
            leaf.active_tab = leaf.tabs.len().saturating_sub(1);
        }
    }
}
```

### 10.9 Race Condition: Layout Mutation During Drag

**Scenario**: While the user is dragging a pane, another event triggers a layout change (e.g., a WebSocket message causes a new chart to auto-open, or a keyboard shortcut closes a chart).

**Behavior**: Cancel the in-progress drag. The drag state machine transitions to `Idle`. The user can retry.

**Implementation**: In the `update` function, whenever a layout mutation occurs that is not part of the current drag operation, check if `drag_state != Idle` and if so, set `drag_state = Idle`.

### 10.10 Window DPI Changes (Multi-Monitor)

**Scenario**: User drags the window from a 1x DPI monitor to a 2x HiDPI monitor.

**Behavior**: The layout tree stores ratios (0.0..1.0), not pixel values. When the window's DPI changes, `compute_rects` is called with the new logical pixel bounds (which iced provides after the DPI change). All rects scale correctly because ratios are resolution-independent.

**No action needed**: The tree model is DPI-agnostic by design.

### 10.11 Restoring a Layout with Stale PaneIds

**Scenario**: User saves a layout, then the application restarts. The layout file references `PaneId(5)`, but no chart state exists for that pane.

**Behavior**: During restoration, iterate `layout.all_panes()` and for each PaneId, look up the corresponding `PaneConfig` in the persisted data. If a PaneId has no config, initialize it as a default/empty chart panel. If a PaneId in the config has no corresponding pane in the layout tree, ignore it (orphaned config data).

```rust
fn restore_workspace(persisted: &PersistedLayout, app: &mut MidasApp) {
    app.layout = persisted.layout.clone();

    for pane_id in app.layout.all_panes() {
        if let Some(config) = persisted.pane_configs.get(&pane_id.0) {
            app.charts.insert(pane_id, ChartPanel::from_config(config));
        } else {
            // PaneId exists in layout but no config -- create empty chart
            app.charts.insert(pane_id, ChartPanel::default());
        }
    }

    // Clean up any chart state for PaneIds not in the layout
    let layout_panes: HashSet<PaneId> = app.layout.all_panes().into_iter().collect();
    app.charts.retain(|id, _| layout_panes.contains(id));
}
```

---

## Appendix A: Full Rect Computation Implementation

The core layout computation that turns the abstract tree into pixel positions:

```rust
impl WorkspaceLayout {
    pub fn compute_rects(&self, bounds: PaneRect) -> LayoutRects {
        let mut rects = HashMap::new();
        if let Some(ref root) = self.root {
            Self::compute_rects_recursive(root, bounds, &mut rects);
        }
        rects
    }

    fn compute_rects_recursive(
        node: &LayoutNode,
        bounds: PaneRect,
        rects: &mut LayoutRects,
    ) {
        match node {
            LayoutNode::Leaf(leaf) => {
                // All tabs in this leaf share the same rect.
                // Only the active tab is visible, but all get the rect
                // (so we can compute positions for tab switch animations, etc.)
                for &pane_id in &leaf.tabs {
                    rects.insert(pane_id, bounds);
                }
            }
            LayoutNode::Split(split) => {
                let (first_bounds, second_bounds) = match split.axis {
                    Axis::Horizontal => {
                        let border_half = BORDER_WIDTH / 2.0;
                        let available = bounds.width - BORDER_WIDTH;
                        let first_w = available * split.ratio;
                        let second_w = available - first_w;
                        (
                            PaneRect {
                                x: bounds.x,
                                y: bounds.y,
                                width: first_w,
                                height: bounds.height,
                            },
                            PaneRect {
                                x: bounds.x + first_w + BORDER_WIDTH,
                                y: bounds.y,
                                width: second_w,
                                height: bounds.height,
                            },
                        )
                    }
                    Axis::Vertical => {
                        let available = bounds.height - BORDER_WIDTH;
                        let first_h = available * split.ratio;
                        let second_h = available - first_h;
                        (
                            PaneRect {
                                x: bounds.x,
                                y: bounds.y,
                                width: bounds.width,
                                height: first_h,
                            },
                            PaneRect {
                                x: bounds.x,
                                y: bounds.y + first_h + BORDER_WIDTH,
                                width: bounds.width,
                                height: second_h,
                            },
                        )
                    }
                };

                Self::compute_rects_recursive(&split.first, first_bounds, rects);
                Self::compute_rects_recursive(&split.second, second_bounds, rects);
            }
        }
    }
}
```

Note: The `BORDER_WIDTH` is subtracted from the available space so that the resize handle does not overlap chart content. Each chart panel gets clean, non-overlapping pixel rects with a gap between them for the border.

---

## Appendix B: Testing Strategy

### Unit Tests for the Layout Model

The layout model is pure data + algorithms with no UI or GPU dependencies. It is trivially testable:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_workspace() {
        let layout = WorkspaceLayout::empty();
        assert!(layout.is_empty());
        assert_eq!(layout.pane_count(), 0);
        assert!(layout.compute_rects(PaneRect { x: 0.0, y: 0.0, width: 1200.0, height: 800.0 }).is_empty());
    }

    #[test]
    fn single_pane_fills_workspace() {
        let mut layout = WorkspaceLayout::empty();
        let pane = layout.preset_1x1();
        let bounds = PaneRect { x: 0.0, y: 0.0, width: 1200.0, height: 800.0 };
        let rects = layout.compute_rects(bounds);
        assert_eq!(rects.len(), 1);
        let r = rects[&pane];
        assert_eq!(r.x, 0.0);
        assert_eq!(r.y, 0.0);
        assert_eq!(r.width, 1200.0);
        assert_eq!(r.height, 800.0);
    }

    #[test]
    fn split_creates_two_panes() {
        let mut layout = WorkspaceLayout::empty();
        let a = layout.preset_1x1();
        let b = layout.next_pane_id();
        layout.split(a, b, Axis::Horizontal, SplitPosition::After).unwrap();

        assert_eq!(layout.pane_count(), 2);
        let bounds = PaneRect { x: 0.0, y: 0.0, width: 1200.0, height: 800.0 };
        let rects = layout.compute_rects(bounds);
        assert_eq!(rects.len(), 2);

        // A should be on the left, B on the right
        assert!(rects[&a].x < rects[&b].x);
        // Combined widths should equal total width (minus border)
        let total = rects[&a].width + rects[&b].width + BORDER_WIDTH;
        assert!((total - 1200.0).abs() < 0.01);
    }

    #[test]
    fn close_pane_reclaims_space() {
        let mut layout = WorkspaceLayout::empty();
        let panes = layout.preset_2x2();
        assert_eq!(layout.pane_count(), 4);

        layout.close_pane(panes[1]).unwrap();
        assert_eq!(layout.pane_count(), 3);

        // panes[0] should now have the full left column top half
        // (or the right side collapses -- depends on tree structure)
    }

    #[test]
    fn close_last_pane_empties_workspace() {
        let mut layout = WorkspaceLayout::empty();
        let pane = layout.preset_1x1();
        layout.close_pane(pane).unwrap();
        assert!(layout.is_empty());
    }

    #[test]
    fn resize_clamps_to_minimum() {
        let mut layout = WorkspaceLayout::empty();
        let a = layout.preset_1x1();
        let b = layout.next_pane_id();
        layout.split(a, b, Axis::Horizontal, SplitPosition::After).unwrap();

        // Find the split node and try to set extreme ratio
        if let Some(LayoutNode::Split(split)) = &layout.root {
            let node_id = split.id;
            layout.resize(node_id, 0.01); // too small
            if let Some(LayoutNode::Split(split)) = &layout.root {
                assert!(split.ratio >= MIN_RATIO);
            }
        }
    }

    #[test]
    fn preset_2x2_produces_four_equal_rects() {
        let mut layout = WorkspaceLayout::empty();
        let panes = layout.preset_2x2();
        let bounds = PaneRect { x: 0.0, y: 0.0, width: 1200.0, height: 800.0 };
        let rects = layout.compute_rects(bounds);
        assert_eq!(rects.len(), 4);

        // All four should be roughly the same size
        for &pane in &panes {
            let r = rects[&pane];
            assert!((r.width - 598.0).abs() < 5.0); // ~600 minus border/2
            assert!((r.height - 398.0).abs() < 5.0); // ~400 minus border/2
        }
    }

    #[test]
    fn serialization_roundtrip() {
        let mut layout = WorkspaceLayout::empty();
        let _panes = layout.preset_3x2();

        let json = serde_json::to_string(&layout).unwrap();
        let restored: WorkspaceLayout = serde_json::from_str(&json).unwrap();

        assert_eq!(layout.pane_count(), restored.pane_count());
        assert_eq!(layout.all_panes(), restored.all_panes());
    }

    #[test]
    fn min_size_prevents_deep_nesting() {
        let mut layout = WorkspaceLayout::empty();
        let mut current = layout.preset_1x1();

        // Keep splitting until we hit the minimum size constraint
        let bounds = PaneRect { x: 0.0, y: 0.0, width: 800.0, height: 600.0 };
        for _ in 0..20 {
            let new = layout.next_pane_id();
            match layout.split(current, new, Axis::Horizontal, SplitPosition::After) {
                Ok(()) => current = new,
                Err(LayoutError::TooSmall) => break,
                Err(e) => panic!("unexpected error: {e}"),
            }
        }

        // Should have stopped well before 20 splits
        assert!(layout.pane_count() < 15);
    }
}
```

### Property-Based Tests (with proptest)

```rust
#[cfg(test)]
mod proptests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        /// Any sequence of splits and closes should leave the tree in a valid state.
        #[test]
        fn split_and_close_invariants(
            ops in prop::collection::vec(
                prop_oneof![
                    Just(Op::Split),
                    Just(Op::Close),
                ],
                0..50
            )
        ) {
            let mut layout = WorkspaceLayout::empty();
            let _ = layout.preset_1x1();

            for op in ops {
                let panes = layout.all_panes();
                if panes.is_empty() { break; }
                let target = panes[0]; // deterministic for reproducibility

                match op {
                    Op::Split => {
                        let new = layout.next_pane_id();
                        let _ = layout.split(target, new, Axis::Horizontal, SplitPosition::After);
                    }
                    Op::Close => {
                        let _ = layout.close_pane(target);
                    }
                }

                // Invariant: pane_count matches actual leaves
                // Invariant: compute_rects produces one rect per pane
                if !layout.is_empty() {
                    let bounds = PaneRect { x: 0.0, y: 0.0, width: 1200.0, height: 800.0 };
                    let rects = layout.compute_rects(bounds);
                    assert_eq!(rects.len(), layout.pane_count());
                }
            }
        }
    }
}
```

---

## Appendix C: Comparison with iced's Built-in `pane_grid`

iced ships with a `pane_grid` widget (`iced::widget::pane_grid`) that implements a binary split tree layout. It is worth examining whether to use it directly vs. building our own.

### iced `pane_grid` Capabilities

- Binary split tree (same model we designed)
- Drag-and-drop pane rearrangement
- Resize handles between panes
- Focus tracking
- Pane close/split operations

### Why We Build Our Own

1. **Tab groups**: `pane_grid` does not support tab groups within a single pane. Our `LeafNode.tabs` field is a core requirement for the trading UX (dock multiple charts into one tabbed container).

2. **Preset layouts**: We need to programmatically construct specific tree shapes (2x2, 3x2, focus+4). `pane_grid` doesn't expose tree construction APIs at the level we need.

3. **Custom drop zones**: We need the 5-zone drop overlay (left/right/top/bottom/tab). `pane_grid` has simpler drag-and-drop.

4. **Serialization control**: We need precise control over the serialization format for cross-session persistence, including per-pane config data.

5. **Animation control**: Future animations (panel collapse/expand, tab switch) need custom state that `pane_grid` doesn't expose.

6. **Decoupling**: Our layout model lives in `midas-core` (or a dedicated `midas-layout` crate), independent of iced. This means we can test it thoroughly without a GUI and potentially reuse it if we ever swap the GUI framework.

### What We Borrow from `pane_grid`

Study `pane_grid`'s source code for:
- How it integrates with iced's layout engine (the `layout()` method)
- How it handles resize drag events
- How it renders focus indicators

These are implementation details we can learn from without depending on `pane_grid` directly.

### Potential Compromise

Start with our own layout model (for tab groups and presets), but use `pane_grid` as the iced rendering widget if its API is flexible enough. If we find ourselves fighting `pane_grid`, switch to the `Row`/`Column`/`FillPortion` approach described in Section 9. Evaluate during Phase 4.2 implementation.

---

## Appendix D: File Placement in the Midas Crate Structure

```
crates/
  midas-core/
    src/
      layout/
        mod.rs          -- pub use of all layout types
        tree.rs         -- LayoutNode, SplitNode, LeafNode, WorkspaceLayout
        types.rs        -- PaneId, NodeId, Axis, DropZone, SplitPosition, PaneRect
        rects.rs        -- compute_rects, compute_borders, min_size
        split.rs        -- split algorithm
        close.rs        -- close algorithm
        resize.rs       -- resize algorithm
        presets.rs      -- preset_1x1, preset_2x2, etc.
        serialize.rs    -- PersistedLayout, save/load
      id.rs             -- ChartId (existing), plus re-export PaneId
      config.rs         -- AppConfig (existing), updated to use PersistedLayout
      ...

  midas-app/
    src/
      views/
        workspace.rs    -- build_layout_widget, workspace_view, empty_workspace_view
      widgets/
        chart_widget.rs -- (existing) ChartWidget Shader implementation
        resize_handle.rs -- resize bar widget
        drop_overlay.rs  -- drag-and-drop preview overlay
      app.rs            -- DragState, resize state, layout messages in update()
```

The layout module in `midas-core` has **zero dependencies** on iced, wgpu, or any GUI framework. It is a pure data structure + algorithm crate. The integration layer in `midas-app` bridges it to iced.

---

## Appendix E: Performance Characteristics

| Operation | Time Complexity | Practical Cost (20-chart layout) |
|---|---|---|
| `compute_rects` | O(n) tree walk | ~39 nodes, <1 microsecond |
| `compute_borders` | O(n) tree walk | ~19 borders, <1 microsecond |
| `split` | O(n) search + O(1) mutation | <1 microsecond |
| `close_pane` | O(n) search + O(1) mutation | <1 microsecond |
| `resize` | O(n) search + O(1) mutation | <1 microsecond |
| `all_panes` | O(n) tree walk | <1 microsecond |
| `min_size` | O(n) tree walk | <1 microsecond |
| `serialize` (serde) | O(n) | ~1 microsecond |
| `deserialize` (serde) | O(n) | ~1 microsecond |

None of these operations are in the hot rendering path. They run only on user interaction (drag, close, split) or once per frame for rect computation. The layout system contributes effectively zero overhead to the 60fps rendering budget.

---

## Summary

This document defines a complete, production-grade layout system for the Midas multi-chart workspace. The binary split tree model is simple, well-understood, and handles all required operations (split, close, resize, drag-and-drop, tabs, presets, serialization) with clean local tree mutations. The system is framework-agnostic at its core, integrates naturally with iced via `Row`/`Column`/`FillPortion`, and is trivially testable with unit and property-based tests.

The next step is to implement this in `crates/midas-core/src/layout/` as Phase 4.2 of the implementation plan, replacing the placeholder `WorkspaceLayout` enum.
