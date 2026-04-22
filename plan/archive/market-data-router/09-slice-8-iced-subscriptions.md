# Slice 8 — Iced Subscriptions Polish

**Goal.** Tighten the iced subscription layer landed in S7. Extract common patterns into helpers, ensure subscription identity is stable, add resync-from-snapshot for `Lagged` errors, verify frame-rate coalescing under load.

## Scope

Most of the behavior was landed in S7; this slice is about hardening, deduplication, and perf correctness.

BR-17 (already addressed in S0 prep): the iced 0.14 subscription API was verified via a scratch POC before any real subscription was wired. If the POC revealed API drift, this slice incorporates that adaptation. Otherwise, `iced::subscription::channel(key, cap, closure)` is the shape used throughout S7 and S8.

### A. Extract `FrameCoalescer` helper

`midas-app/src/app/subscription_helpers.rs`:

```rust
pub struct FrameCoalescer<T> {
    pending: Vec<T>,
    interval: tokio::time::Interval,
    max_batch_size: usize,   // M-30: flush when reached, before interval
}

impl<T> FrameCoalescer<T> {
    pub fn new(frame_ms: u64, max_batch: usize) -> Self { ... }

    /// Run a coalescing loop over `rx`, emitting batches via `out`.
    /// M-30: flushes when `pending.len() >= max_batch_size` OR the interval
    /// ticks — whichever comes first. Prevents unbounded growth if the producer
    /// briefly outruns the interval cadence.
    /// Returns when `rx` closes.
    pub async fn drive<Msg, F>(
        mut self,
        mut rx: broadcast::Receiver<Arc<T>>,
        mut out: iced::futures::channel::mpsc::Sender<Msg>,
        make_batch: F,
        on_lag: impl Fn(u64) -> Msg,
    ) where
        T: Clone,
        Msg: Send,
        F: Fn(Vec<T>) -> Msg,
    { ... }
}
```

Used by chart, watchlist, and ticker subscriptions.

### B. Stable subscription keys

Verify every `iced::subscription::channel(key, ...)` uses a hashable, stable key that changes ONLY when the subscription should be torn down:

- Chart: `("chart-bars", chart_id, symbol, tf)` — tears down on symbol/tf change (correct).
- Watchlist: `("watchlist-quotes", sorted Vec<SymbolKey>)` — sorted to stabilize hash across re-render when HashSet ordering varies.
- Ticker: `("ticker-ticks", symbol)` — one subscription per active symbol.

### C. Snapshot-based resync

When a consumer receives `RecvError::Lagged(n)`:

- Chart: `Message::ChartResync { chart_id }` → handler calls `router.history_then_live(sym, tf, snapshot_lookback)` and replaces `chart.data`.
- Watchlist: `Message::QuoteResync { symbol }` → handler reads `router.last_quote(sym).borrow().clone()` and updates `market_cache`.
- Ticker: ignore `Lagged` for TickerState (it's already batched at frame rate; lagging briefly is fine).

### D. Visibility-aware subscriptions

Only subscribe to bars for *visible* charts. `chart_subscriptions` iterates `self.charts.values().filter(|c| c.is_visible())`. When a chart becomes hidden (e.g., minimized), its subscription disappears and the aggregator refcount decrements.

Add `is_visible()` method to ChartPanel. For now returns `true`; future slices can plumb window visibility.

### E. Memory management

Verify that dropping an iced subscription triggers the subscription task's future to be dropped, which triggers the `SubscriptionHandle`'s `_guard` Drop, which triggers the router's `DecRef` message. Test this end-to-end in a unit test using `tokio::test` + a mock iced subscription harness.

### F. No N×N subscription fan-out

Make sure we don't end up with N charts × M symbols = N×M subscriptions. Each chart has ONE symbol at a time, so one subscription per chart.

For watchlists: one global watchlist subscription that multiplexes all symbols (already designed as such).

For TickerState: one per active ticker (small N).

## Tests

1. `coalescer_emits_at_most_once_per_frame` — push 100 items in 10 ms, coalescer emits 1 batch.
2. `coalescer_emits_during_idle` — push 1 item, no more. After 16 ms, 1 batch with 1 item.
3. `chart_subscription_tears_down_on_symbol_change` — open chart on AAPL, switch to MSFT, assert AAPL tick subscription is cancelled upstream within 100 ms.
4. `hidden_chart_has_no_subscription` — open chart, set visible=false, assert 0 active subscriptions for that chart.
5. `lagged_consumer_resyncs` — artificially cause `Lagged` on a chart subscription (by not draining long enough), assert `Message::ChartResync` fires and chart data is refreshed.

## Acceptance

- All 5 tests pass.
- `cargo clippy`, `cargo fmt`, full test suite green.
- Manual: run app, open 5 charts, each on different symbols, watch CPU. Target: idle < 2% CPU, active chart update < 8% CPU.

## Risks

- iced subscription API may have changed between versions (we're on 0.14). Verify `subscription::channel` signature.
- `HashSet` key hashing is not deterministic; sort before hashing.
- Subscription teardown should be "drop" not "explicit cancel" — make sure no explicit cancel calls leak into public API.
