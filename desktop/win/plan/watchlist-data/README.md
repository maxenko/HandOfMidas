# Feature: Watchlist Market Data Cache

## Overview

The watchlist grid currently shows `"--"` for price, change%, and G.ATR columns because `compute_all_market_data()` scrapes data from loaded chart candle buffers — it only has data for symbols with an active chart pane. When the user clicks a different ticker, the chart switches symbols and the previous symbol's data vanishes.

This plan introduces a `MarketDataCache` that holds market snapshots for **all watchlist symbols**, persists across chart switches, and provides the subscription interface needed for future IB streaming integration.

**Who benefits**: Every user with a watchlist. Currently the watchlist is blind — it shows ticker names but no live data unless a chart happens to be open for that symbol.

## Codebase Analysis

### Current Data Flow (broken)
```
User adds "AAPL" to watchlist
  → No data loaded (shows "--")
User clicks "AAPL" row
  → Symbol linking loads AAPL chart via DataProvider::get_candles()
  → Chart stores Arc<CandleBuffer>
  → compute_all_market_data() finds chart, extracts last_close/change/GATR
  → Watchlist shows values
User clicks "MSFT" row
  → Chart switches to MSFT, drops AAPL buffer
  → AAPL goes back to "--"
```

### Desired Data Flow
```
User adds "AAPL" to watchlist
  → MarketDataCache loads snapshot for AAPL (via DataProvider)
  → Watchlist shows values immediately
User clicks "MSFT" row
  → Cache already has MSFT data
  → AAPL data persists in cache
  → Both show values
```

### Key Integration Points

| Component | File | Role |
|-----------|------|------|
| `DataProvider` trait | `midas-core/src/provider.rs` | Async candle data source (`get_candles()`) |
| `TestProvider` | `midas-feed/src/test_provider.rs` | Deterministic test data, always connected |
| `ProviderRegistry` | `midas-app/src/registry.rs` | Active provider selection |
| `CandleBuffer` | `midas-core/src/candle_buffer.rs` | SoA candle storage |
| `WatchlistPanel` | `midas-app/src/watchlist.rs` | Ticker list, grid state |
| `WatchlistRow` | `midas-app/src/watchlist_columns.rs` | Pre-computed row data for grid |
| `compute_all_market_data()` | `midas-app/src/app/views.rs` | Current (broken) data source |
| `view_watchlist_body()` | `midas-app/src/app/views.rs` | Builds WatchlistRow from market data |
| `MidasApp` | `midas-app/src/app.rs` | App state, message handling |

### Existing Patterns to Follow

- **Async data loading**: `load_chart_async()` uses `Task::perform()` with `Arc<dyn DataProvider>` — same pattern for cache population
- **Arc sharing**: `ChartPanel.data: Option<Arc<CandleBuffer>>` — cache will store `Arc<CandleBuffer>` too
- **Message dispatch**: `Message::DataLoaded(ChartId, Result<...>)` pattern for async results
- **Config persistence**: `WatchlistPanel::to_config()` / `from_config()` for state roundtrip
- **Gerchik ATR computation**: `midas_chart::gerchik_atr::compute_gerchik_atr()` — reuse for cache

### Broker Plan Alignment

The broker plan (`broker/plan/04-market-data-and-events.md`) specifies:
- L1 streaming via `tokio::broadcast` channels (4096 buffer, lossy)
- `TickSnapshot` struct (bid/ask/last/volume/high/low)
- `SubscriptionManager` with ref-counting and line-limit enforcement
- 50ms coalescing window for tick aggregation

This cache design intentionally **does not implement streaming** — it provides the data model and subscription interface that streaming will plug into. Phase 1 (IB integration) will replace the pull-based population with push-based tick updates.

## Design Decisions

### Decision 1: Where does MarketDataCache live?

**Context**: The cache needs to be accessible from `view_watchlist_body()` (views) and updated from async tasks (app).

**Options**:
1. **Field on `MidasApp`** — simple, co-located with `watchlists` and `providers`
2. **Field on `WatchlistPanel`** — per-watchlist isolation
3. **Separate service/actor** — decoupled, message-driven

**Recommendation**: Option 1 — field on `MidasApp`. The cache is shared across all watchlists (same AAPL data for all lists). It's populated via the app's `DataProvider` and consumed in `view()`. No need for actor complexity yet.

**Confidence**: High

### Decision 2: What data does the cache store?

**Context**: The watchlist needs last price, change%, and G.ATR. These are derived from candle data, not raw quotes.

**Options**:
1. **Store raw `CandleBuffer`** per symbol — compute derived values on demand
2. **Store pre-computed `MarketSnapshot`** — last_price, prev_close, change_pct, gatr_text, gatr_color
3. **Store both** — buffer for chart use + snapshot for fast watchlist access

**Recommendation**: Option 2 — store pre-computed `MarketSnapshot`. The watchlist only needs 5 derived values per symbol. Storing full candle buffers (~100KB each) for 50+ watchlist symbols wastes memory. The snapshot is computed once when data arrives and read on every frame.

For Phase 1 (IB streaming), the snapshot will be updated incrementally from tick data without needing the full buffer.

**Confidence**: High

### Decision 3: How is the cache populated?

**Context**: With `TestProvider`, data must be fetched via `get_candles()`. With IB, it will come from streaming ticks.

**Options**:
1. **Eager load on watchlist change** — fetch data for all symbols when a ticker is added/removed
2. **Lazy load in view** — check cache on each frame, request missing data
3. **Background refresh loop** — periodic timer re-fetches all symbols

**Recommendation**: Option 1 with background refresh. When a ticker is added to any watchlist, fire an async `get_candles()` for that symbol (daily timeframe, 30 days). Store the computed snapshot. A subscription timer (every 60s for test provider, replaced by streaming in Phase 1) refreshes all cached symbols.

**Confidence**: Medium — the refresh interval is a guess; IB streaming replaces it entirely.

### Decision 4: Cache key — symbol only or symbol + timeframe?

**Context**: The watchlist shows daily price data. Charts may show different timeframes.

**Recommendation**: **Symbol only** for the watchlist cache. The cache always uses D1 (daily) timeframe for price/change computation. Chart-specific timeframe data stays on `ChartPanel`. This keeps the cache simple and small.

**Confidence**: High

## Implementation Plan

### Slice 1: MarketSnapshot type in midas-core

**Goal**: Define the cache's value type in the leaf crate so it's available everywhere.
**Depends on**: None

**Files to create or modify**:
- `crates/midas-core/src/market_data.rs` (new) — `MarketSnapshot` struct
- `crates/midas-core/src/lib.rs` — add `pub mod market_data;` and re-export

**Key implementation details**:
```rust
/// Point-in-time market data snapshot for one symbol.
/// Computed from daily candle data; updated by streaming ticks in Phase 1.
#[derive(Debug, Clone, Default)]
pub struct MarketSnapshot {
    /// Last closing price (or last trade price when streaming).
    pub last_price: Option<f64>,
    /// Previous close (for change% computation).
    pub prev_close: Option<f64>,
    /// Percentage change from previous close.
    pub change_pct: Option<f64>,
    /// Gerchik ATR display text (e.g., "3.45%").
    pub gatr_text: Option<String>,
    /// Gerchik ATR color [r, g, b, a].
    pub gatr_color: Option<[f32; 4]>,
}
```

**Testing**: Unit test for `Default` (all fields `None`).

**Done when**: `MarketSnapshot` compiles, is exported from `midas-core`, and is usable from `midas-app`.

---

### Slice 2: MarketDataCache on MidasApp

**Goal**: Add a `HashMap<String, MarketSnapshot>` cache to the app and a function to compute snapshots from candle data.
**Depends on**: Slice 1

**Files to create or modify**:
- `crates/midas-app/src/market_cache.rs` (new) — cache struct and snapshot computation
- `crates/midas-app/src/main.rs` — add `mod market_cache;`
- `crates/midas-app/src/app.rs` — add `market_cache: MarketDataCache` field to `MidasApp`, initialize in `new()`

**Key implementation details**:
```rust
use std::collections::HashMap;
use midas_core::MarketSnapshot;

/// In-memory cache of market data snapshots keyed by uppercase symbol.
///
/// Populated asynchronously from the active DataProvider. The watchlist
/// reads from this cache instead of scraping chart candle buffers.
#[derive(Debug, Default)]
pub struct MarketDataCache {
    snapshots: HashMap<String, MarketSnapshot>,
}

impl MarketDataCache {
    pub fn get(&self, symbol: &str) -> Option<&MarketSnapshot> {
        self.snapshots.get(symbol)
    }

    pub fn insert(&mut self, symbol: String, snapshot: MarketSnapshot) {
        self.snapshots.insert(symbol, snapshot);
    }

    pub fn remove(&mut self, symbol: &str) {
        self.snapshots.remove(symbol);
    }

    pub fn symbols(&self) -> impl Iterator<Item = &String> {
        self.snapshots.keys()
    }
}

/// Compute a MarketSnapshot from a CandleBuffer (daily timeframe).
pub fn snapshot_from_candles(buffer: &midas_core::CandleBuffer) -> MarketSnapshot {
    let len = buffer.len();
    if len == 0 {
        return MarketSnapshot::default();
    }
    let last_close = buffer.closes[len - 1] as f64;
    let prev_close = if len >= 2 {
        Some(buffer.closes[len - 2] as f64)
    } else {
        None
    };
    let change_pct = prev_close.map(|prev| {
        if prev != 0.0 { ((last_close - prev) / prev) * 100.0 } else { 0.0 }
    });
    let candle_duration = midas_chart::estimate_candle_duration(buffer);
    let gatr = midas_chart::gerchik_atr::compute_gerchik_atr(buffer, candle_duration);

    MarketSnapshot {
        last_price: Some(last_close),
        prev_close,
        change_pct,
        gatr_text: gatr.as_ref().map(|g| g.text.clone()),
        gatr_color: gatr.as_ref().map(|g| g.color),
    }
}
```

**Testing**:
- `snapshot_from_candles` with empty buffer → all `None`
- `snapshot_from_candles` with 1 candle → `last_price` set, `change_pct` None
- `snapshot_from_candles` with 2+ candles → `change_pct` computed correctly
- `MarketDataCache::get/insert/remove` roundtrip

**Done when**: `MidasApp` has a `market_cache` field, `snapshot_from_candles` works, tests pass.

---

### Slice 3: Populate cache on ticker add/remove

**Goal**: When a ticker is added to any watchlist, fetch its daily candle data and populate the cache. When removed from all watchlists, remove from cache.
**Depends on**: Slice 2

**Files to modify**:
- `crates/midas-app/src/app.rs` — modify `WatchlistAddTicker` and `WatchlistRemoveTicker` handlers

**Key implementation details**:

In `WatchlistAddTicker` handler, after `wl.add_ticker(&input)` succeeds:
```rust
// Check if this symbol needs data loaded into the cache.
let symbol = input.trim().to_uppercase();
if self.market_cache.get(&symbol).is_none() {
    // Fire async load for this symbol's daily data.
    let task = self.load_market_snapshot(&symbol);
    return Task::batch([self.flush_config(), task]);
}
```

New method on `MidasApp`:
```rust
fn load_market_snapshot(&self, symbol: &str) -> Task<Message> {
    let provider = match self.providers.active_data_provider() {
        Some(p) => p,
        None => return Task::none(),
    };
    let sym = symbol.to_uppercase();
    Task::perform(
        async move {
            provider.get_candles(&sym, Timeframe::D1, 30).await
        },
        move |result| Message::MarketSnapshotLoaded(sym, result.map_err(|e| e.to_string())),
    )
}
```

New message variant:
```rust
MarketSnapshotLoaded(String, Result<CandleBuffer, String>),
```

Handler:
```rust
Message::MarketSnapshotLoaded(symbol, Ok(buffer)) => {
    let snapshot = crate::market_cache::snapshot_from_candles(&buffer);
    self.market_cache.insert(symbol, snapshot);
    Task::none()
}
Message::MarketSnapshotLoaded(symbol, Err(e)) => {
    tracing::warn!("Failed to load market data for {symbol}: {e}");
    Task::none()
}
```

In `WatchlistRemoveTicker` handler, after removing the ticker:
```rust
// Remove from cache if no watchlist still has this symbol.
let symbol_upper = symbol.to_uppercase();
let still_used = self.watchlists.values().any(|wl| wl.has_ticker(&symbol_upper));
if !still_used {
    self.market_cache.remove(&symbol_upper);
}
```

**Testing**: Manual — add ticker, verify snapshot loads after a frame; remove ticker, verify cache entry removed.

**Done when**: Adding a ticker to a watchlist triggers async data load and populates the cache. Removing the last reference cleans it up.

---

### Slice 4: Wire watchlist view to read from cache

**Goal**: Replace `compute_all_market_data()` with cache reads in `view_watchlist_body()`.
**Depends on**: Slice 3

**Files to modify**:
- `crates/midas-app/src/app/views.rs` — rewrite `WatchlistRow` construction to read from `self.market_cache`

**Key implementation details**:

Replace the market data section in `view_watchlist_body()`:
```rust
// OLD: let market_data = self.compute_all_market_data();
// NEW: read directly from the cache
let empty_snapshot = midas_core::MarketSnapshot::default();
let mut grid_rows: Vec<WatchlistRow> = wl.tickers.iter().map(|ticker| {
    let snap = self.market_cache
        .get(&ticker.symbol)
        .unwrap_or(&empty_snapshot);

    let price_text = snap.last_price
        .map(|p| format!("{p:.2}"))
        .unwrap_or_else(|| "--".into());
    let change_text = snap.change_pct
        .map(|c| format!("{c:+.2}%"))
        .unwrap_or_else(|| "--".into());
    let change_color = match snap.change_pct {
        Some(c) if c > 0.0 => Color::from_rgb(0.2, 0.8, 0.3),
        Some(c) if c < 0.0 => Color::from_rgb(0.9, 0.25, 0.2),
        _ => Color::from_rgb(0.6, 0.6, 0.6),
    };
    let gatr_text = snap.gatr_text.clone().unwrap_or_else(|| "--".into());
    let gatr_color = snap.gatr_color
        .map(|c| Color::from_rgba(c[0], c[1], c[2], c[3]))
        .unwrap_or(Color::from_rgb(0.6, 0.6, 0.6));

    WatchlistRow { symbol: ticker.symbol.clone(), favorite: ticker.favorite,
        price_text, change_text, change_color, gatr_text, gatr_color,
        wl_id, price_value: snap.last_price, change_value: snap.change_pct }
}).collect();
```

Remove `compute_all_market_data()` function and `TickerMarketData` struct (dead code after this slice).

**Testing**: Manual — verify watchlist shows data for all tickers, data persists when clicking different rows.

**Done when**: Watchlist reads from cache. `compute_all_market_data()` is deleted. Values persist across chart switches.

---

### Slice 5: Populate cache on startup for existing watchlists

**Goal**: When the app starts, load market data for all symbols across all watchlists.
**Depends on**: Slice 3

**Files to modify**:
- `crates/midas-app/src/app.rs` — after watchlists are restored from config, fire snapshot loads

**Key implementation details**:

In `MidasApp::new()` or the startup task, after watchlists are loaded from config:
```rust
// Collect all unique symbols across all watchlists.
let symbols: HashSet<String> = self.watchlists.values()
    .flat_map(|wl| wl.tickers.iter().map(|t| t.symbol.clone()))
    .collect();

// Fire async loads for all symbols.
let tasks: Vec<Task<Message>> = symbols.into_iter()
    .map(|sym| self.load_market_snapshot(&sym))
    .collect();

Task::batch(tasks)
```

**Testing**: Manual — restart app with saved watchlists, verify market data appears without clicking each ticker.

**Done when**: All watchlist symbols have cached data within a few seconds of app startup.

---

### Slice 6: Periodic refresh (subscription stub)

**Goal**: Refresh cached snapshots periodically so data stays current. This is the subscription interface that IB streaming will replace.
**Depends on**: Slice 5

**Files to modify**:
- `crates/midas-app/src/app.rs` — add `subscription()` timer and refresh handler

**Key implementation details**:

Add a subscription that fires every 60 seconds:
```rust
fn subscription(&self) -> iced::Subscription<Message> {
    // ... existing subscriptions ...

    // Refresh market data cache every 60 seconds.
    let market_refresh = iced::time::every(std::time::Duration::from_secs(60))
        .map(|_| Message::RefreshMarketData);

    iced::Subscription::batch([existing_subs, market_refresh])
}
```

Handler:
```rust
Message::RefreshMarketData => {
    let symbols: Vec<String> = self.market_cache.symbols().cloned().collect();
    let tasks: Vec<Task<Message>> = symbols.into_iter()
        .map(|sym| self.load_market_snapshot(&sym))
        .collect();
    Task::batch(tasks)
}
```

**Testing**: Manual — watch the watchlist, verify values refresh after 60s.

**Done when**: Cache auto-refreshes. With `TestProvider`, values stay the same (deterministic). With a real provider, values would update.

---

### Dependency Summary

```
Slice 1 (MarketSnapshot type)
    ↓
Slice 2 (Cache struct + snapshot computation)
    ↓
  ┌─┴──────────────┐
  ↓                ↓
Slice 3            Slice 5
(add/remove)       (startup load)
  ↓
Slice 4
(wire to view)
  ↓
Slice 6
(periodic refresh)
```

Slices 3 and 5 can be done in parallel. Slice 4 depends on 3. Slice 6 depends on 5.

## Risks & Unknowns

### R1: TestProvider data is static — refresh won't show changes
**Mitigation**: Expected. TestProvider returns deterministic data. The refresh loop validates the plumbing. Real changes require IB streaming (Phase 1).

### R2: 50+ symbols × `get_candles()` on startup could be slow
**Mitigation**: `TestProvider` generates data in ~1-5ms per symbol, so 50 symbols ≈ 250ms. For IB, pacing rules (60 requests/10min) would require batching. The cache should show a loading indicator per symbol until data arrives.

### R3: Race condition — ticker removed while async load is in-flight
**Mitigation**: The `MarketSnapshotLoaded` handler does a blind `insert()`. If the ticker was removed between request and response, the cache holds an orphan entry. This is harmless — it wastes a few bytes and gets cleaned up on the next add/remove cycle. For correctness, the handler could check `still_used` before inserting.

## Testing Strategy

- **Unit tests**: `MarketSnapshot::default()`, `snapshot_from_candles()` with various buffer sizes, `MarketDataCache` CRUD
- **Integration**: Manual — add tickers, verify data appears, switch charts, verify data persists, restart app, verify data loads
- **Follows existing pattern**: Inline `#[cfg(test)]` modules, `#[tokio::test]` for async, `TestProvider` as data source

## Out of Scope

- **IB streaming / real-time ticks** — Phase 1 IB integration, not this feature
- **Bid/ask/volume display** — requires L1 streaming data not available from candle providers
- **DuckDB persistent cache integration** — `midas-store` exists but wiring it to the market cache is a separate task
- **Multiple timeframe snapshots** — cache uses D1 only; chart-specific timeframes stay on ChartPanel
- **Loading indicators per symbol** — nice-to-have, not blocking
- **Cache eviction / LRU** — unnecessary until symbol count exceeds hundreds
