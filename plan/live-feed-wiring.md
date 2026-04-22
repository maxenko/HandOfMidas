# Live feed wiring (follow-up to live-sim-and-free-brackets.md)

Sim now emits ticks, but the pipeline past the broker bridge has holes:

1. Watchlist symbols loaded from config may not get subscribed (timing race with synthetic 250 ms Ready).
2. Chart symbols are never subscribed unless they're also in a watchlist.
3. Ticks update `market_cache` but not `TickerState.last_price`.
4. Ticks update `market_cache` but do not mutate the chart's `CandleBuffer` — the chart renders static historical data.

User requirement: "if any ticker is in the watchlist it should be watched on at least last price level, if its in the chart it should be drawing the chart" and "our inline charts should also be always updating".

## Changes

### A — Unified subscription ensurer

- Rename `MidasApp::ensure_watchlist_subscriptions` → `ensure_market_subscriptions` in `desktop/win/crates/midas-app/src/app/handlers.rs`.
- It must collect the union of: every watchlist ticker + every `self.charts[*].bound_symbol` + every `self.floating_charts[*].bound_symbol`. (Thumbnails reuse watchlist data, so covered via watchlist.)
- Keep the `SymbolKey` set + `active_market_subs` diff logic.
- Update all existing call sites (connection-ready, add-ticker, etc.) to the new name.

### B — Subscribe on chart bind

- In `desktop/win/crates/midas-app/src/app/ticker_wiring.rs::bind_chart_to_symbol` (and the floating-chart equivalent if distinct), call `self.ensure_market_subscriptions()` after the bound symbol is set. Idempotent, so free.

### C — Subscribe after workspace/watchlist hydration

- Search for where config-loaded watchlists land (likely a `Message::WorkspaceLoaded` / `WatchlistHydrated` / similar handler in `app/handlers.rs`) and add a final `self.ensure_market_subscriptions()` call there, so the synthetic-Ready timing race is irrelevant.

### D — Tick → TickerState

- In `app/handlers.rs::handle_broker_msg` around line 3712 (BrokerEvent::Tick branch), after `self.market_cache.insert(key, merged)`, also dispatch:
  ```rust
  let _ = self.update(Message::Ticker(
      key.clone(),
      crate::ticker_state::TickerMsg::UpdateMarketData {
          last_price: price,
          gatr_abs: merged.gatr_abs, // preserve prior GATR
      },
  ));
  ```
- This updates `TickerState.last_price` so decorators/labels/brackets see the live price.

### E — Tick → live candle

- In the same Tick branch, after updating market_cache + TickerState, walk every chart (main + floating) bound to this symbol. For each one with `data: Some(Arc<CandleBuffer>)`:
  - `Arc::make_mut(&mut arc)` to get mutable access (the buffer will clone only if shared).
  - Mutate the **last** candle's `close` to `price`, update `high = high.max(price)` and `low = low.min(price)`.
  - Bump the buffer's `version` so the chart scene knows it's dirty.
- For inline / thumbnail widgets: if they read from `market_cache` (watchlist snapshot) they are already covered by market_cache updates. If they read from a `CandleBuffer` Arc, the same Arc::make_mut path applies.

### F — Optional: `BarClosed` when the clock rolls

- Out of scope for this pass. Roll-over to a new candle requires per-chart timeframe context. Updating the live candle's close is enough to make the chart visually "move" for the V1.

## Files to touch

- `desktop/win/crates/midas-app/src/app/handlers.rs` (rename + Tick branch)
- `desktop/win/crates/midas-app/src/app/ticker_wiring.rs` (bind_chart_to_symbol)
- Any hydration handler (to be discovered)
- Possibly `midas-data` or `midas-core` if `CandleBuffer` needs a `push_tick` or `version_bump` helper

## Acceptance

- Launch `cargo run -p midas-app` from desktop workspace, open on a clean config:
  - Watchlist symbols show live-moving last price.
  - Open a chart for a symbol that's NOT in any watchlist → chart's last candle close drifts visibly over ~5 s.
  - Open an inline chart / thumbnail → same.
- All existing tests pass; clippy clean; fmt clean.
