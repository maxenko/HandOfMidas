# Professional Trading Platform Watchlist/Grid Components

Research into how Bloomberg, ThinkOrSwim, Interactive Brokers TWS, and TradingView
implement their watchlist and data grid components. Focus is on UX patterns, visual
design, and features specific to real-time financial data grids.

---

## 1. Bloomberg Terminal

### The Monitor / MOST Screen

Bloomberg's watchlist equivalent is the **Monitor** (and specialized views like the
**MOST** -- Most Active -- screen). Monitors are the primary tool for tracking a
portfolio of securities in real time.

#### Columns and Data Fields

- **Default columns**: Symbol, Name, Last Price, Change, Change %, Bid, Ask, Volume,
  Open, High, Low, Previous Close, VWAP.
- **Custom fields**: Users click "Fields" in the top-right corner, open the "Custom"
  tab, and type a data field name in the "Add column" search bar. Hundreds of data
  points are available -- fundamentals, technicals, credit metrics, options greeks.
- **Column management**: Drag-and-drop reordering. Right-click column headers to
  insert, remove, or resize. Columns can be moved between panes.
- **Panes**: Large watchlists can be split into multiple panes within a single Monitor
  window, allowing the user to view more securities at once without scrolling.
- **Sorting**: Click column headers to sort ascending/descending. Securities can also
  be sorted by industrial sector, alphabetically, or by any numeric column.

#### Real-Time Updates and Color Coding

- **Amber-on-black**: Bloomberg's iconic color scheme. The base font color is amber
  (`#F39F41` approximate) on a pure black (`#000000`) background. The custom typeface
  ("Bloomberg Prop Unicode N," designed by Matthew Carter) is optimized for dense
  numerical data and legibility at small sizes.
- **Semantic colors**: Price increases display in **green**; decreases in **red**.
  Non-semantic information remains in amber. The active panel shows a blue flashing
  cursor.
- **Color accessibility (CVD)**: Bloomberg offers alternative color schemes for users
  with color vision deficiency. The Deuteranopia scheme uses **blue** for "up" and
  **red** for "down" instead of green/red. A separate Protanomaly scheme is also
  available.
- **Tick flash**: When a price updates, the cell briefly highlights to indicate the
  direction of the change before reverting to its base color.
- **Color palette reference** (approximate hex values from community sources):
  - Background: `#000000`
  - Amber text: `#F39F41`
  - Positive: `#4AF6C3` (teal-green)
  - Negative: `#FF433D` (red)
  - Accent blue: `#0068FF`
  - Accent orange: `#FB8B1E`

#### Layout and Navigation

- **Four-panel workspace**: The core Bloomberg Terminal typically runs four windows,
  each with its own command line. Users type function mnemonics (e.g., `MOST <GO>`,
  `WEI <GO>`) and the panel populates.
- **Keyboard-driven**: Bloomberg is fundamentally keyboard-first. The physical keyboard
  is color-coded -- yellow keys for market sectors, green for GO/Enter, red for Cancel.
  Users navigate almost entirely via typed commands.
- **Launchpad**: The modern overlay (Launchpad) allows drag-and-drop arrangement of
  monitors, charts, and news panels on a multi-monitor setup. Monitors can be linked
  so clicking a security in one panel updates others.
- **Saved configurations**: Users can save entire monitor layouts, field selections,
  and sorting criteria for quick recall.

### Key Takeaway

Bloomberg prioritizes **information density** and **keyboard-driven navigation**.
The amber-on-black scheme with semantic green/red coloring is purpose-built for
rapid scanning of large datasets. The system handles thousands of securities via
pane splitting rather than infinite scroll.

---

## 2. ThinkOrSwim (Charles Schwab / TDA)

### Watchlist Gadget

ThinkOrSwim's watchlist is delivered as a "gadget" on the left sidebar. It supports
personal watchlists, public/curated lists, portfolio lists, and dynamic scan results.

#### Column System

- **Pre-built columns**: Last, Net Change, % Change, Volume, Bid, Ask, Bid Size,
  Ask Size, Open, High, Low, Close, Mark, Market Cap, P/E, Dividend Yield, 52-Week
  High/Low, and dozens more.
- **Custom columns via ThinkScript**: The defining feature. Users write ThinkScript
  code to create computed columns. For example:
  ```thinkscript
  def pctChange = (close - close[1]) / close[1];
  AddLabel(yes, AsPercent(pctChange), 
      if pctChange > 0.1 then CreateColor(0,180,180) 
      else if pctChange > 0 then Color.GREEN
      else Color.RED);
  AssignBackgroundColor(
      if pctChange > 0 then CreateColor(209, 243, 235) 
      else CreateColor(227, 208, 178));
  ```
- **Condition Wizard**: A no-code interface for building conditional column logic
  without writing ThinkScript directly.

#### Conditional Coloring

- **`AssignBackgroundColor()`**: Sets the background color of a watchlist cell based
  on conditions. Accepts standard colors (`Color.DARK_GREEN`, `Color.RED`) or custom
  RGB via `CreateColor(r, g, b)`.
- **`AddLabel()` text coloring**: Controls the foreground text color independently
  from the background.
- **Multi-level color coding**: Common patterns include gradient coloring --
  e.g., cyan for highest gap values, red for second highest, green for third, yellow
  for lowest. This lets traders scan columns visually for outliers.
- **Cell-level formatting**: Each custom column independently controls its own
  text color, background color, and displayed text/number format.

#### Scanning Integration

- **Stock Hacker scanner**: Users define filter criteria (price range, volume,
  custom ThinkScript conditions) and the results populate in a watchlist-style grid.
- **Static vs. dynamic watchlists**: "Save as Watchlist" captures a snapshot.
  "Save scan query" creates a dynamic watchlist that auto-refreshes every 3-7 minutes.
- **Scanner-to-watchlist pipeline**: Scan results display in the same grid format
  with the same custom columns, making the transition seamless.

#### Multiple Watchlists and Tabs

- **Watchlist selector**: Click the watchlist name to create, delete, or switch
  between lists. Up/down arrows cycle through lists in a group.
- **Public watchlists**: Built-in lists like "Top 10 by Volume," "Losers,"
  industry-based lists. These update in real time.
- **Portfolio watchlist**: Auto-populated from current account positions.

#### Right-Click Context Menu

- Trade actions: Buy, Sell, Buy Custom, Sell Custom.
- Analysis: Send to Chart, Analyze Trade, Option Chain.
- List management: Add to Watchlist, Remove from Watchlist, Move Up/Down.
- Copy symbol to clipboard.

### Key Takeaway

ThinkOrSwim's competitive advantage is **programmable columns via ThinkScript**.
The ability to write arbitrary code that controls cell value, text color, and
background color -- with full access to price, volume, and indicator data -- is
unmatched among retail platforms. The scanning-to-watchlist pipeline creates a
complete workflow from discovery to monitoring.

---

## 3. Interactive Brokers TWS

### Monitor Panel and Watchlist

TWS provides two interfaces: the **Classic TWS** (spreadsheet-style) and the
**Mosaic** interface (modern, panel-based). Both share the same underlying data grid.

#### Column Customization

- **Insert Column**: Hover over a column header for 1 second, then select "Insert
  Column" from the popup. Alternatively, right-click a header.
- **Available columns**: Last, Change, Change %, Bid, Ask, Bid Size, Ask Size,
  Volume, Average Volume, Open, High, Low, Close, VWAP, Market Cap, P/E, Dividend,
  52-Week Range, Position, Unrealized P&L, Options columns (IV, Greeks), and many more.
- **Views (Column Presets)**: Columns are organized into "views" -- named sets of
  columns that can be switched instantly. Example views: "Market Data," "Fundamentals,"
  "Options," "Portfolio." Users create custom views.
- **No custom computed columns**: Unlike ThinkOrSwim, TWS does not support
  user-defined formula columns. Column choices are limited to IB's predefined set.

#### Sorting

- **Right-click sort**: Right-click any column header to choose ascending or
  descending sort.
- **Continuous sort**: When enabled, the grid automatically re-sorts in real time
  as values change. For example, sorting by "Change %" keeps the biggest movers at
  the top as prices update.
- **One-time sort**: Sorts once and holds position even as values change.

#### Real-Time Streaming

- **Streaming market data**: All data cells update in real time via IB's streaming
  feed. Last price, bid/ask, volume -- all tick continuously.
- **Color coding**: Positive changes in green, negative in red. Customizable via
  Global Configuration > Display > Colors.
- **Cell background flash**: Price changes trigger a brief background color flash
  on the affected cell before reverting to the base color.

#### Symbol Linking (Window Groups)

- **Color-coded groups**: Each window (watchlist, chart, order entry, etc.) has a
  colored "linking block" in its top-right corner.
- **Group behavior**: All windows sharing the same color group update together.
  Click "AAPL" in the watchlist (Group Green) and the chart window (also Group Green)
  immediately shows AAPL.
- **Multiple groups**: Red, Green, Blue, Yellow, etc. Traders create independent
  linked groups for different workflows (e.g., one group for large-caps, another for
  options research).
- **Unlinked mode**: Setting a window to "unlinked" (no color) prevents it from
  changing when other windows change.

#### Right-Click Context Menu

- **Trading**: Buy, Sell, Close Position, Adjust Position.
- **Analysis**: Open Chart, Open Option Chain, Fundamentals, Analyst Ratings.
- **Configuration**: Set Alert, Add to Watchlist, Configure Columns, Market Depth.
- **Order management**: Attach bracket orders, modify existing orders from grid.

#### Cloud Sync

Watchlists are saved to IB's cloud, so the same lists appear across desktop, web,
and mobile platforms.

### Key Takeaway

TWS excels at **institutional-grade column variety** and **continuous real-time
sorting**. The window grouping system with color-coded linking is a best-in-class
implementation of symbol linking. The lack of custom computed columns is a notable
gap compared to ThinkOrSwim.

---

## 4. TradingView Watchlist

### Web-Based Watchlist Widget

TradingView's watchlist is a sidebar panel in their web-based charting platform.
It supports multiple view modes and is designed for a modern, clean aesthetic.

#### View Modes

- **Table View**: Full columnar display with headers, sortable columns, and
  resizable column widths. Resembles a traditional data grid.
- **Compact / Minimalist View**: Strips away column headers and extra data. Shows
  only the symbol name, last price, and change -- all on a single row. Designed for
  maximum vertical density when monitoring many symbols.
- **Advanced View**: Adds specialized column sets:
  - **Price**: Last, Change, Change %, Volume, Market Cap.
  - **Performance**: Price change percentages over 1D, 1W, 1M, 3M, 6M, YTD, 1Y.
  - **Financials**: Revenue, EPS, P/E, Market Cap, Dividend Yield.
  - **Risk**: Beta and volatility across different timeframes.
- **Summary row**: In advanced view, displays min, max, average, and median values
  for each column across the entire watchlist.

#### Column Selection and Customization

- **Add/remove columns**: Click the three-dot menu next to the watchlist name,
  toggle columns on/off.
- **Add Column (+)**: Insert new data fields; **Remove (trash icon)**: Delete columns.
- **Resize columns**: Drag the right border of any column header.
- **Reorder**: Drag-and-drop column headers to rearrange.
- **No custom formula columns**: Like TWS, TradingView does not support
  user-defined computed columns in the watchlist (though Pine Script exists for
  chart indicators).

#### Symbol Linking and Navigation

- **Click-to-chart**: Clicking any symbol in the watchlist immediately updates the
  main chart to that symbol.
- **Keyboard navigation**: Up/Down arrows (or Space/Shift+Space) move through the
  list. Each press updates the chart.
- **Quick add**: `Alt+W` adds the currently charted symbol to the watchlist.
  Typing in the watchlist opens a symbol search overlay.
- **Flag/color coding**: `Alt+Enter` toggles a flag on the selected symbol.
  Users can color-code individual symbols for visual categorization (e.g., red for
  high-priority, blue for research candidates).
- **Sections**: Watchlists can be subdivided into named sections for grouping
  (e.g., "Tech," "Energy," "Earnings This Week").

#### Organization

- **Multiple watchlists**: Create unlimited named watchlists. Switch between them
  via a dropdown.
- **Import/Export**: Symbols can be imported from text files.
- **Watchlist alerts**: Set alerts on watchlist symbols that trigger when conditions
  are met.
- **Drag-and-drop reorder**: Symbols can be manually reordered within the list.

### Key Takeaway

TradingView's strength is **clean modern UX** with multiple density modes. The
advanced view with performance/risk column sets and summary statistics is unique.
The keyboard-driven navigation (arrow keys immediately update the chart) creates
a fluid workflow for scanning through symbols.

---

## 5. Common Features Across All Platforms

### 5.1 Real-Time Cell Updates

All four platforms stream market data in real time. The universal pattern:

| Aspect | Implementation |
|--------|---------------|
| **Update frequency** | Tick-by-tick for Level 1 data (last, bid, ask, volume) |
| **Cell flash** | Brief background color change (green/red) on the cell that updated, fading back to base color over ~500ms-1500ms |
| **Flash scope** | Only the changed cell flashes, not the entire row |
| **Animation** | CSS-style transition: apply highlight color, then fade. Two phases: "changed" state (~500ms) then "fade" state (~1000ms) |
| **Performance** | Must handle hundreds of updates per second across thousands of cells. AG Grid benchmarks: 150,000+ updates/second |

### 5.2 Conditional Cell Formatting

| Pattern | Usage |
|---------|-------|
| **Green/Red for direction** | Universal. Positive change = green text or background; negative = red |
| **Gradient intensity** | Some platforms shade color intensity proportional to magnitude (darker green = larger gain) |
| **Heat map coloring** | Bloomberg and ThinkOrSwim support heat-map-style gradients across a column |
| **Position-aware coloring** | Cells showing P&L or position size use distinct colors (e.g., blue for long, orange for short) |
| **Threshold-based** | ThinkOrSwim excels here: custom thresholds trigger distinct colors (e.g., RSI > 70 = red background, RSI < 30 = green) |
| **CVD accessibility** | Bloomberg offers blue/red alternatives for color-blind users |

### 5.3 Custom Computed Columns

| Platform | Support | Mechanism |
|----------|---------|-----------|
| Bloomberg | Limited | Predefined field library (hundreds of fields) but no user-defined formulas |
| ThinkOrSwim | **Full** | ThinkScript -- arbitrary code with access to all market data, indicators, and color control |
| TWS | None | Predefined columns only |
| TradingView | None | Predefined columns only (Pine Script is chart-only) |

ThinkOrSwim is the clear leader. Its ThinkScript columns can compute ratios, moving
averages, multi-timeframe analysis, and arbitrary boolean conditions -- all displayed
inline in the watchlist grid.

### 5.4 Column Presets / Profiles

| Platform | Feature |
|----------|---------|
| Bloomberg | Save entire Monitor configurations (columns + sorting + filters) |
| ThinkOrSwim | Column sets per watchlist gadget; custom columns persist |
| TWS | Named "views" -- switchable column presets (Market Data, Options, Portfolio, custom) |
| TradingView | Advanced view modes (Price, Performance, Financials, Risk) serve as presets |

TWS has the most explicit implementation: views are first-class objects that can be
created, named, saved, and switched with a single click.

### 5.5 Right-Click Context Menus

Every platform provides context menus on both **rows** (symbol-level actions) and
**column headers** (grid configuration).

**Row context menu** (common items across platforms):
- Buy / Sell / Close Position
- Open Chart
- Open Option Chain
- Set Alert / Notification
- Add to / Remove from Watchlist
- Copy Symbol
- View Fundamentals / News

**Header context menu** (common items):
- Sort Ascending / Descending
- Insert Column (Before / After)
- Remove Column
- Auto-fit Column Width
- Reset Column Widths
- Hide / Show Columns

### 5.6 Multi-Watchlist Tabs

All platforms support multiple independent watchlists:

- **Bloomberg**: Multiple Monitor windows, each with its own security list and column set
- **ThinkOrSwim**: Watchlist selector dropdown, up/down arrows to cycle, personal + public lists
- **TWS**: Tabbed monitor pages, each page is an independent watchlist with its own view
- **TradingView**: Watchlist dropdown, unlimited named lists, sections within lists

### 5.7 Symbol Linking

| Platform | Mechanism |
|----------|-----------|
| Bloomberg | Launchpad panel linking; click security in monitor to update linked panels |
| ThinkOrSwim | Gadget linking via colored chain-link icons; multiple independent link groups |
| TWS | **Window Groups** -- color-coded blocks (Red, Green, Blue, Yellow, etc.) in window corners. Best-in-class implementation |
| TradingView | Implicit -- clicking a watchlist symbol always updates the main chart |

TWS and ThinkOrSwim both support multiple independent link groups, allowing traders
to run parallel workflows (e.g., one link group for scalping, another for swing
research). TradingView uses a simpler single-chart-linked model.

---

## 6. UX Patterns

### Handling Thousands of Tickers

| Technique | Platforms |
|-----------|-----------|
| **Virtualized scrolling** | All. Only rows visible in the viewport are rendered. The DOM contains ~30-50 row elements regardless of list size |
| **Pane splitting** | Bloomberg. Split the Monitor into 2-4 panes showing different sections of the same list |
| **Sections / Groups** | TradingView (named sections), ThinkOrSwim (separate watchlists per category) |
| **Dynamic filtering** | ThinkOrSwim (scan queries auto-filter), Bloomberg (criteria-based filtering) |
| **Continuous sort** | TWS. Sort by "Change %" and the top movers always float to the top |

### Keyboard Navigation

| Action | Bloomberg | ThinkOrSwim | TWS | TradingView |
|--------|-----------|-------------|-----|-------------|
| Move up/down | Arrow keys | Arrow keys | Arrow keys | Arrow / Space |
| Select symbol | Enter / GO | Click or Enter | Click or Enter | Click (auto-links) |
| Quick-add symbol | Type in command line | Type in blank row | Type in blank row at bottom | `Alt+W` or type in search |
| Search symbols | Type mnemonic + GO | Symbol search field | Type in blank row | `/` or type anywhere |
| Remove symbol | Context menu | Delete key / context menu | Delete / context menu | Context menu |

### Quick-Add Symbol Patterns

1. **Blank row at bottom** (TWS, ThinkOrSwim): The grid always has an empty row at
   the bottom. Type a ticker and press Enter to add.
2. **Command line** (Bloomberg): Type the ticker in the command line, then a function
   key to route it to the Monitor.
3. **Overlay search** (TradingView): Start typing anywhere and a search popup appears.
   Select a result to add it to the watchlist.
4. **Hotkey add** (TradingView): `Alt+W` adds the currently displayed chart symbol.

---

## 7. Visual Design

### Typography

| Platform | Font | Style |
|----------|------|-------|
| Bloomberg | Bloomberg Prop Unicode N (custom, by Matthew Carter) | Monospace-like proportional with tabular figures. Optimized for dense numerical grids |
| ThinkOrSwim | System sans-serif (appears to use a variant of Segoe UI / Arial) | Proportional with tabular numerals for number columns |
| TWS | System sans-serif | Proportional text, tabular figures in numeric columns |
| TradingView | Trebuchet MS / system sans-serif | Clean proportional, tabular figures for prices |

**Critical typographic requirement for trading grids**: All numeric columns must use
**tabular (monospaced) figures** so that digits align vertically. This ensures that
columns of numbers are scannable -- the decimal points line up, and a price changing
from `142.50` to `143.50` does not cause the column to shift horizontally.

### Spacing and Density

| Aspect | Bloomberg | ThinkOrSwim | TWS | TradingView |
|--------|-----------|-------------|-----|-------------|
| Row height | ~16-18px (extremely dense) | ~22-24px | ~20-22px | ~28-32px (table), ~22px (compact) |
| Cell padding | 2-4px horizontal | 4-6px | 4-6px | 8-12px |
| Information density | Highest | High | High | Medium (prioritizes readability) |
| Target audience | Professionals on multi-monitor setups | Active retail traders | Institutional and active retail | Broad retail audience |

### Row Styling

| Feature | Bloomberg | ThinkOrSwim | TWS | TradingView |
|---------|-----------|-------------|-----|-------------|
| Alternating row colors | No (uniform black background) | Yes (subtle alternation) | Yes (light/dark alternation) | Yes (very subtle in table view) |
| Grid lines | Subtle or none (density is key) | Horizontal separators | Both horizontal and vertical | Horizontal only in table view |
| Selected row highlight | Bright blue bar | Blue/dark highlight | Blue highlight bar | Light gray/blue background |
| Hover highlight | Subtle brightness change | Row highlight on hover | Row highlight on hover | Row highlight on hover |

### Color Schemes

| Platform | Background | Text | Positive | Negative | Neutral |
|----------|-----------|------|----------|----------|---------|
| Bloomberg | Black `#000000` | Amber `#F39F41` | Green/Teal `#4AF6C3` | Red `#FF433D` | Amber `#F39F41` |
| ThinkOrSwim | Dark gray `#1E1E1E` area | White/Light gray | Green `#00C805` | Red `#FF0000` | White |
| TWS | Dark `#2B2B2B` or Light themes | Theme-dependent | Green | Red | Default text |
| TradingView | White `#FFFFFF` or Dark `#1E222D` | `#131722` or `#D1D4DC` | Green `#089981` | Red `#F23645` | Gray |

---

## 8. What Makes a Trading Grid Special vs. a Generic Data Grid

A trading watchlist grid is not merely a sortable table. The following characteristics
distinguish it from a generic component like AG Grid, Handsontable, or a Material UI
DataGrid:

### 8.1 Real-Time Streaming (Not Request-Response)

Generic grids are designed for static or paginated data fetched via REST APIs. A
trading grid must:

- Accept a **continuous stream** of updates (WebSocket, broadcast channel, or
  shared-memory ring buffer).
- Apply **individual cell updates** without re-rendering the entire row or table.
- Handle **burst traffic** -- market open can produce hundreds of updates per second
  across hundreds of symbols simultaneously.
- **Batch updates** -- accumulate ticks received within a single frame (~16ms) and
  apply them in one render pass to avoid layout thrashing.

### 8.2 Flash-on-Tick

The signature UX pattern of trading grids. When a price changes:

1. The cell's background color changes to a **highlight color** (green for uptick,
   red for downtick).
2. After a short hold period (~300-500ms), the color **fades back** to the base
   color over ~500-1000ms.
3. If another tick arrives during the fade, the animation **resets** -- the new
   direction's color takes over immediately.

Implementation approaches:
- **CSS transitions**: Apply a class (e.g., `flash-up`), then remove it after a
  timeout. CSS handles the fade. Lightweight but requires careful class management.
- **Inline style animation**: Set `backgroundColor` directly, then use
  `requestAnimationFrame` to interpolate back. More control, heavier.
- **GPU shader** (for GPU-rendered grids like Hand of Midas): Write the flash as
  a uniform that decays over time in the fragment shader. Zero CPU cost per cell.

### 8.3 Position-Aware Coloring

Trading grids are aware of the user's **portfolio positions**:

- Symbols where the user has an open position may be highlighted (e.g., bold text,
  left-border accent, or background tint).
- P&L columns are colored based on whether the position is profitable (green) or
  at a loss (red).
- Position size can drive color intensity (larger positions = more saturated colors).

This is fundamentally different from a generic grid where every row is treated
identically.

### 8.4 Semantic Column Types

Trading grids understand the **meaning** of their data:

| Column type | Behavior |
|-------------|----------|
| **Price** | Right-aligned, tabular figures, flash on change, green/red for direction |
| **Change / Change %** | Signed value, green/red coloring, often with up/down arrow icon |
| **Volume** | Right-aligned, abbreviated (e.g., "1.2M"), often with bar chart background |
| **Bid / Ask** | Flash independently, may show spread highlight |
| **P&L** | Colored based on sign, may show absolute and percentage |
| **Symbol** | Left-aligned, may include logo/icon, flag indicator, link-group color dot |
| **Sparkline** | Inline mini chart showing intraday or multi-day price movement |
| **Status** | Pre-market, market-hours, after-hours indicators with distinct styling |

Generic grids treat all columns as strings or numbers with optional formatting.
Trading grids build domain-specific rendering into the column type itself.

### 8.5 Order Entry Integration

Professional trading grids allow **trading directly from the grid**:

- Double-click a bid/ask cell to pre-populate a limit order.
- Right-click for a full order ticket.
- Drag a row to an order entry panel.
- Inline "Buy" / "Sell" buttons that appear on hover.

The grid is not just a display -- it is an **input surface** for the trading workflow.

### 8.6 Multi-Source Data Fusion

A single row in a trading grid may fuse data from multiple sources:

- **Real-time feed**: Last, Bid, Ask, Volume (streaming, sub-second latency).
- **Delayed/snapshot**: Fundamentals, earnings dates, analyst ratings (refreshed
  periodically).
- **Local computation**: P&L, position size, custom indicator values (computed
  client-side).
- **User annotations**: Flags, notes, color tags (persisted locally or to cloud).

Generic grids typically bind to a single data source per row.

### 8.7 Performance Budget

| Metric | Trading Grid Target | Generic Grid |
|--------|-------------------|--------------|
| Visible rows | 50-200 | 10-50 (paginated) |
| Update latency | < 16ms (one frame) | Not applicable |
| Updates per second | 1,000-150,000+ | Batch / on-demand |
| Memory per row | Minimal (virtualized) | Full DOM per row (or virtualized) |
| Scroll performance | 60fps with live updates | 60fps static |

---

## Appendix: Implementation Patterns for Hand of Midas

Based on this research, the following patterns are most relevant for a GPU-rendered
Rust desktop trading grid:

### Must-Have Features (Phase 1)

1. **Real-time streaming cell updates** with per-cell flash animation.
2. **Tabular (monospaced) figures** in all numeric columns.
3. **Green/red conditional coloring** for price direction.
4. **Column resizing and reordering** via drag-and-drop.
5. **Single-click symbol linking** to update linked chart panels.
6. **Right-click context menu** on rows (trade actions) and headers (column config).
7. **Keyboard navigation** -- arrow keys to move selection, type-to-search for quick add.
8. **Virtualized rendering** -- only visible rows rendered, supporting 1000+ symbols.
9. **Continuous sort** -- grid re-sorts in real time as values change.

### Should-Have Features (Phase 2)

1. **Column presets / views** -- save and switch named column configurations.
2. **Multiple watchlist tabs** with independent column sets and sort orders.
3. **Advanced view modes** -- switch between Price, Performance, Fundamentals.
4. **Compact mode** -- reduced row height for maximum density.
5. **Alternating row backgrounds** with configurable grid lines.
6. **Symbol flag/color tagging** for user categorization.
7. **Inline sparkline column** -- mini price chart per row.

### Could-Have Features (Phase 3)

1. **Custom computed columns** via a formula/expression language.
2. **Dynamic watchlists** from scan/filter queries.
3. **Heat map column backgrounds** proportional to value magnitude.
4. **Position-aware row decoration** (border accent for open positions).
5. **Inline order entry** -- click bid/ask to create orders.
6. **CVD-accessible color schemes** (blue/red alternative).
7. **Cloud sync** of watchlist configuration.

### GPU Rendering Considerations

Since Hand of Midas uses GPU-rendered UI:

- **Flash animation** can be implemented as a per-cell uniform (flash_intensity,
  flash_color) that decays over time in the fragment shader. This is zero-cost
  compared to CSS-based approaches.
- **Text rendering** with tabular figures requires a font atlas with fixed-width
  digit glyphs. The font need not be fully monospaced -- only digits 0-9, decimal
  point, comma, minus sign, and percent must be fixed-width.
- **Virtualized rendering** is natural in a GPU context -- compute visible row
  range from scroll offset and viewport height, emit quads only for visible rows.
- **Batch updates** -- accumulate all ticks received since the last frame, apply
  them to the data model, then re-render only affected cells in the next draw call.
- **Column sorting** should happen on the CPU (data model layer), with the GPU
  receiving only the final sorted visible slice.
