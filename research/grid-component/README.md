# Grid Component Research

Research for building a professional-grade grid/table widget for Hand of Midas watchlists.
Conducted 2026-04-01. Six research tracks covering the best grid implementations across web, desktop, and native ecosystems.

## Research Documents

| Document | Focus | Key Takeaways |
|---|---|---|
| [ag-grid.md](ag-grid.md) | AG Grid (JavaScript) | Gold standard feature set: ColDef API, virtual scrolling, transaction API for real-time updates, column state persistence, custom cell renderers |
| [wpf-grids.md](wpf-grids.md) | WPF DataGrid + DevExpress/Telerik | XAML DataTemplate system, DragAdorner for custom drag visuals, Star/Auto/Pixel sizing, ICollectionView for sort/filter, virtualization modes |
| [react-tables.md](react-tables.md) | TanStack Table, MUI DataGrid, React Data Grid | **Headless architecture** (logic separated from rendering), row model pipeline, dnd-kit integration, CSS-variable column sizing |
| [trading-grids.md](trading-grids.md) | Bloomberg, ThinkOrSwim, TWS, TradingView | Flash-on-tick, conditional formatting, symbol linking, column presets, real-time re-sorting, keyboard navigation |
| [drag-patterns.md](drag-patterns.md) | DnD across platforms + Unreal UMG | Custom drag visuals (UMG DragWidget, WPF DragAdorner), GPU overlay layers, drop indicators, animated reorder, spring physics |
| [rust-native-grids.md](rust-native-grids.md) | iced, egui, Slint, Qt, GTK4, Dear ImGui | Specs-only sorting (ImGui), trait-based columns (iced_table), factory/recycling (GTK4), role-based data (Qt), per-column clip rects |

## Top Patterns for Hand of Midas

### Architecture
- **Headless core** (TanStack pattern): Separate `GridState` (columns, sort, selection, scroll) from rendering. Pure functions transform state. Renderer reads state to draw.
- **Specs-only sorting** (ImGui pattern): Grid tracks sort column + direction, emits a message. App sorts its own data. Grid never owns the data.
- **Trait-based columns** (iced_table / Qt delegate): Each column defines how to extract, format, compare, and render its data.

### Must-Have Features (Phase 1)
- Fixed header with synchronized column widths
- Column resizing via drag handles (min/max constraints)
- Column reordering via header drag
- Click-to-sort with direction indicators
- Row selection with highlight
- Symbol linking (click row -> update chart)
- Scrollable body with fixed header
- Custom cell content (text, buttons, icons)

### Should-Have Features (Phase 2)
- Row drag-and-drop reordering with custom drag visual
- Flash-on-tick animation (GPU shader)
- Conditional cell formatting (green/red/gradient)
- Column presets (save/load configurations)
- Virtual scrolling for 1000+ rows
- Keyboard navigation (arrow keys, Enter to select)
- Right-click context menu

### Could-Have Features (Phase 3)
- Custom computed columns (formulas)
- Column groups / band headers
- Multi-watchlist tabs
- Copy/paste, export
- Inline cell editing
- Auto-refreshing scanner integration

### Drag Visual Design (from drag-patterns.md)
- Drag ghost: 90% opacity, 2px drop shadow, 1.02x scale
- Source row: dim to 30% opacity
- Drop indicator: 2px colored line at insertion point
- Animated reorder: items slide out of the way (200ms ease-out)
- Column header drag: floating header ghost + vertical drop indicator between columns

### GPU Rendering Approach (from rust-native-grids.md)
- 7-layer rendering: background -> grid lines -> cells -> selection -> header -> overlays -> drag layer
- Per-column clip rects for text truncation
- Batch text rendering with tabular figures
- Flash animation via GPU shader (color interpolation over 300ms)
- Virtual scrolling: only emit GPU primitives for visible rows
