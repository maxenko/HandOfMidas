# TODO — Hand of Midas

Current state as of 2026-04-04. Pick up from here.

## What's Working

- GPU-rendered candlestick charts (20+ at 60fps), zoom/pan/auto-scale
- Watchlist panels with grid widget, symbol linking, drag-to-chart
- Order entry panel (UI only — not wired to broker)
- Chart bracket visualization (TP/SL lines, drag to modify)
- Market order bracket engine (create, cancel, modify legs)
- Full-simulation test broker (fills, OCA, positions, ticks)
- G.ATR indicator: 7-session paranormal-filtered, D1 bars, shared between chart and grid
- Horizontal levels with per-symbol persistence
- Crosshair with axis labels
- Multi-window support (pop-out charts)
- SQLite order persistence with audit trail
- DuckDB data cache (midas-store, wired but unused)
- Provider registry (TestProvider active, IB slot empty)
- 1000+ tests across both workspaces

## Immediate Next Steps

### 1. Wire Broker Bridge to Desktop App

The order panel validates and builds bracket params, but never sends
`BrokerCommand::CreateMarketBracket` to the engine. Two `tracing::warn!`
calls in `app.rs` (lines ~2674, ~2794) mark the gaps.

**What's needed:**
- Add `midas-broker` as a dependency (or use the channel-based bridge)
- Start `BrokerEngine` in `MidasApp::new()`, store `BrokerHandle`
- Route `OrderPanelConfirmYes` → `BrokerCommand::CreateMarketBracket`
- Route `BracketContextCancel` → `BrokerCommand::CancelBracket`
- Subscribe to `BrokerEvent`s and update chart annotations on status changes

**Reference:** `plan/broker-trait-redesign.md` (implemented), `plan/archive/market-order-bracket/`

### 2. Grid Component Polish

`midas-grid` is wired into the watchlist but needs:
- Column sorting (SortColumn/SortDirection types exist, handler not wired)
- Drag-and-drop row reorder
- Conditional cell formatting (flash on tick)
- Context menu (right-click ticker)

**Reference:** `desktop/win/plan/grid-component/`

### 3. IB Paper Trading Connection (Phase 1)

No real IB adapter exists yet. `BrokerClient` trait is widened and ready.

**What's needed:**
- Implement `BrokerClient` for a real `IbClient` wrapper around `ibapi` crate
- Implement `MarketDataSource` for IB historical bars
- Connection lifecycle (connect/disconnect/reconnect on daily restart)
- Paper trading validation (port 4002)

**Reference:** `plan/broker/01-architecture.md`, `research/provider-ib.md`

### 4. Test Data Provider Consistency

The `TestDataProvider` generates daily and intraday data via separate
Brownian motion paths. The daily bar H-L doesn't perfectly match the
aggregated intraday H-L for the same day. Not a bug in production (IB
data is consistent), but confusing during development.

**Fix:** Derive daily bars from aggregated intraday data, or constrain
the intraday Brownian bridge to exactly hit daily H/L/O/C.

## Known Gaps (Not Blocking)

- **Duplicate `aggregate_daily_bars` / `DailyBar`** — Two copies exist:
  `midas-chart/src/gerchik_atr.rs` (f32) and `midas-chart/src/indicators/gerchik_atr.rs` (f64).
  Consolidate into a single shared implementation to reduce duplication.
- **BrokerClient trait ~35% IB parity** — market data subs, positions,
  account queries are on the trait now. Missing: trailing stops, TIF
  enforcement, margin, bar streaming, algo orders.
- **Order panel not submitted** — UI exists, broker bridge not connected.
- **Annotation lifecycle** — bracket annotations created on order submit
  but not cleaned up on bracket close/cancel via broker events.
- **Chart G.ATR on D1 charts** — shows value from market_cache (D1 bars).
  Works correctly but the chart's own daily candles may visually differ
  from the 30-day D1 snapshot if the chart loads a different date range.

## Architecture Notes

- Two Cargo workspaces: root (broker) + `desktop/win/` (app)
- Desktop can't depend on `midas-broker` (pulls in `ibapi`). Types are
  mirrored in `desktop/win/crates/midas-core/src/broker.rs`.
- `BrokerClient` trait: `crates/midas-broker/src/client.rs`
- `MarketDataSource` trait: `crates/midas-broker/src/market_data.rs`
- G.ATR single source: `midas_core::gerchik_gatr_pct()` in
  `desktop/win/crates/midas-core/src/atr.rs`
- Market data cache: `desktop/win/crates/midas-app/src/market_cache.rs`
- All plans: `plan/` (active), `plan/archive/` (implemented)
- All research: `research/`
