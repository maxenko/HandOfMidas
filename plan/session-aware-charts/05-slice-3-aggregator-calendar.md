# Slice 3 — Calendar-aware aggregator

**Goal.** Teach `BarAggregatorRegistry` to use `ExchangeCalendar::bar_window` for session-sensitive timeframes (D1/W1/MN1/H4). Intraday UTC-epoch-modular timeframes (S5..H1) keep their current alignment.

## Scope

### Aggregator task changes

`crates/midas-market-data/src/aggregator/task.rs::run_aggregator`:

```rust
async fn run_aggregator(
    mut rt_handle: SubscriptionHandle<Bar>,
    tf: Timeframe,
    symbol: SymbolKey,
    calendar: &'static dyn ExchangeCalendar,  // NEW
    bars_tx: broadcast::Sender<Arc<Bar>>,
    last_bar_slot: Arc<RwLock<Option<Bar>>>,
) {
    // ... as today but window alignment uses calendar.bar_window ...
    let (window_open, window_close) = match calendar.bar_window(rt_bar.ts_open, tf) {
        Ok(w) => w,
        Err(CalendarError::UnsupportedTimeframe(_)) => {
            // Fall back to UTC-epoch modular for intraday tfs
            align_to_window_utc(rt_bar.ts_open, tf)?
        }
        Err(e) => { tracing::error!("calendar error: {e:?}"); return; }
    };
    // ...
    let session = calendar.classify(window_open);
    bar.session_kind = Some(session);
    // ...
}
```

### Registry signature change

`BarAggregatorRegistry::subscribe` gains a `calendar` param:

```rust
pub async fn subscribe(
    &self,
    symbol: SymbolKey,
    tf: Timeframe,
    calendar: &'static dyn ExchangeCalendar,
) -> Result<SubscriptionHandle<Bar>, MarketDataError>;
```

Or: router injects the calendar based on symbol (simpler for callers):

```rust
// router/mod.rs
pub async fn subscribe_bars(
    &self,
    symbol: SymbolKey,
    tf: Timeframe,
) -> Result<SubscriptionHandle<Bar>, MarketDataError> {
    let cal = calendar_for(&symbol);
    self.state.aggregator_registry.subscribe(symbol, tf, cal).await
}
```

Choose the second — router resolves calendar.

### Supported-timeframe extension

`BarAggregatorRegistry::subscribe` no longer blanket-rejects `D1`/`W1`/`H4`. It calls `calendar.bar_window(ts_now, tf)` — if the calendar supports the tf, aggregation proceeds; if `Err(UnsupportedTimeframe)`, rejection with the same error surface as today.

`CryptoSpotCalendar::bar_window` supports all timeframes (UTC-epoch for everything).
`XnysCalendar::bar_window` supports S5..H1 (UTC-epoch), H4/D1/W1/MN1 (session-aligned). Rejects exotic tfs like `S1` (no upstream).

### Historical seam

`history_then_live` already calls `router.source().historical_bars(sym, con_id, end, duration, tf, what_to_show, use_rth)`. Add `use_rth` plumbing — today it's hardcoded `true`. Pass through from caller, defaulting to `true` for session-aware calendars, `false` (no filter) for crypto.

## Files touched

- `crates/midas-market-data/src/aggregator/task.rs` — calendar-aware alignment.
- `crates/midas-market-data/src/aggregator/registry.rs` — calendar param, drop `is_unsupported_tf` hard-coded list.
- `crates/midas-market-data/src/router/mod.rs` — resolve calendar for symbol.
- `crates/midas-market-data/Cargo.toml` — add `midas-calendar` dep.

## Tests

- **XNYS M1 aggregation**: sim emits 5s RT bars during RTH (14:30–21:00 UTC); assert M1 aggregator produces minute-aligned bars, each with `session_kind = Regular`.
- **XNYS M1 during pre-market**: sim emits at 11:00 UTC (06:00 ET, pre-market); assert bar's `session_kind = PreMarket`.
- **XNYS D1 boundary**: sim emits across 14:30 UTC (09:30 ET) boundary; the pre-boundary bar closes at 14:30 UTC, the post opens at 14:30 UTC with `session_kind = Regular`.
- **XNYS W1 alignment**: advance time across a weekend; W1 bar closes Friday 16:00 ET, next opens Monday 09:30 ET.
- **Crypto D1 alignment**: bar boundaries at 00:00 UTC regardless of wall-clock.
- **Calendar-unsupported tf**: `subscribe_bars(AAPL, Timeframe::S1)` returns `UnsupportedTimeframe`.

## Acceptance

- All new tests pass under `tokio::test(start_paused = true)`.
- `cargo clippy --workspace --all-targets -- -D warnings` clean.
- Existing aggregator tests continue to pass (defaulting calendar behavior via an XNYS test fixture).

## Commit

Single commit: `feat(aggregator): calendar-aware D1/H4/W1 bar alignment`.
