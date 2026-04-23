# Architecture Reference

## Layering

```
chrono + chrono-tz (external)
          ↑
midas-calendar (new root-workspace crate)
  trait ExchangeCalendar, SessionKind, Session, TradingCalendar
          ↑
midas-broker-core (existing)
  market_data::Bar gains Option<SessionKind>
  MarketDataSource keeps its current trait surface; bar producers
  call the symbol's calendar to classify ticks before emitting.
          ↑
midas-core (desktop)
  CandleData trait gains fn session(idx) -> SessionKind (default Regular)
  CandleBuffer gains Vec<u8> sessions backing that method
          ↑
midas-chart (sans-IO)
  compute/build_candle_instances applies session tint via existing
  CandleInstance.color attribute (no shader change).
  compute/build_grid_instances emits background band rectangles
  for pre/post sessions as GridLineInstance.
  Existing detect_session_boundaries reused for RTH-close separator.
          ↑
midas-render (wgpu)
  Unchanged. All session visuals use existing pipelines.
          ↑
midas-app (iced)
  ChartPanel gains show_extended_hours: bool
  ChartViewStore persists it
  Per-chart toggle chip + settings checkbox
```

## `midas-calendar` crate layout

New crate at `crates/midas-calendar/`. Root workspace (same tier as `midas-broker-core`).

```
crates/midas-calendar/
  Cargo.toml          — deps: chrono, chrono-tz, smallvec, serde (opt), thiserror
                       dev-deps: nyse-holiday-cal (cross-check only)
  src/
    lib.rs            — re-exports + docs
    trait.rs          — ExchangeCalendar trait
    types.rs          — SessionKind, Session, TradingDay, Mic, CalendarError
    registry.rs       — CalendarRegistry: Mic -> &'static dyn ExchangeCalendar
    xnys.rs           — XnysCalendar (US equities)
    crypto_spot.rs    — CryptoSpotCalendar (24/7, UTC-aligned)
    holidays/
      nyse_rules.rs   — rule-based regulars (Good Friday, Thanksgiving…)
      nyse_adhoc.rs   — dated exceptions (9/11, Sandy, state funerals)
      nyse_early.rs   — early-close rules + ad-hoc
      tests.rs        — cross-check against nyse-holiday-cal
```

## Core types

```rust
// midas-calendar::types

/// ISO-10383 Market Identifier Code.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct Mic(pub &'static str);

/// Coarse session classification — what's visually / behaviourally
/// distinct on a chart. Labels (e.g. "Tokyo" for forex) live on the
/// `Session` struct, not here.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Default)]
pub enum SessionKind {
    #[default]
    Regular,        // primary auction window
    PreMarket,
    PostMarket,
    Break,          // intraday gap (CME maintenance, lunch breaks)
    Overnight,      // labeled overnight / non-RTH blocks
    Closed,         // weekend, holiday, between-session
}

/// A single contiguous session window.
#[derive(Clone, Debug, PartialEq)]
pub struct Session {
    pub kind: SessionKind,
    pub open: DateTime<Utc>,
    pub close: DateTime<Utc>,
    pub label: Option<&'static str>, // e.g. "NY" for forex overlays
}

/// Trading-day description: 0..N sessions + holiday flags.
#[derive(Clone, Debug)]
pub struct TradingDay {
    pub date: NaiveDate,                         // in the calendar's tz
    pub sessions: SmallVec<[Session; 4]>,        // ordered by open_ts
    pub is_early_close: bool,
    pub is_holiday: bool,
    pub holiday_name: Option<&'static str>,
}

#[derive(Debug, thiserror::Error)]
pub enum CalendarError {
    #[error("date {0} out of calendar coverage range")]
    OutOfRange(NaiveDate),
    #[error("unsupported timeframe {0:?}")]
    UnsupportedTimeframe(Timeframe),
}
```

## `ExchangeCalendar` trait

```rust
pub trait ExchangeCalendar: Send + Sync {
    fn mic(&self) -> Mic;
    fn tz(&self) -> Tz;
    fn covers(&self) -> Range<NaiveDate>;

    // Day-level.
    fn is_trading_day(&self, date: NaiveDate) -> bool;
    fn trading_day(&self, date: NaiveDate) -> Result<TradingDay, CalendarError>;

    // Point classification.
    fn classify(&self, ts: DateTime<Utc>) -> SessionKind;
    fn session_for(&self, ts: DateTime<Utc>) -> Option<Session>;

    // Navigation.
    fn next_open(&self, ts: DateTime<Utc>, kind: SessionKind) -> Option<DateTime<Utc>>;
    fn prev_close(&self, ts: DateTime<Utc>, kind: SessionKind) -> Option<DateTime<Utc>>;

    // Aggregator hook — the bar window this timeframe would produce for ts.
    fn bar_window(
        &self,
        ts: DateTime<Utc>,
        tf: Timeframe,
    ) -> Result<(DateTime<Utc>, DateTime<Utc>), CalendarError>;

    // Iterator for chart band overlays.
    fn sessions_between<'a>(
        &'a self,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
    ) -> Box<dyn Iterator<Item = Session> + 'a>;

    // Time-axis policy.
    fn time_axis(&self) -> TimeAxisPolicy;
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum TimeAxisPolicy {
    Continuous,        // crypto: never compress closed time
    CompressedClosed,  // stocks/futures/FX: skip closed in collapsed mode
}
```

Calendars are **stateless + `Send + Sync`**; one `&'static dyn ExchangeCalendar` per venue, registered in a `CalendarRegistry: HashMap<Mic, &'static dyn ExchangeCalendar>`.

## `XnysCalendar` (US equities reference impl)

- `tz()` = `America/New_York`.
- `covers()` = 2000-01-01 .. 2031-12-31 (32 years, patch-bumped annually).
- Sessions per trading day:
  - `PreMarket`: 04:00 – 09:30 ET.
  - `Regular`: 09:30 – 16:00 ET (13:00 on early-close days).
  - `PostMarket`: 16:00 – 20:00 ET (17:00 on early-close days).
- `bar_window(ts, Timeframe::D1)` = (09:30 ET open, 16:00 ET close) of the trading day containing `ts` (or the next trading day if `ts` falls on a weekend / holiday). For `use_rth=false` consumers, a separate method `bar_window_eth` returns the 04:00–20:00 ET window.
- `bar_window(ts, Timeframe::H4)` = `09:30, 13:30` or `13:30, 17:30` — aligned to regular session open.
- `bar_window(ts, Timeframe::M1 | M5 | M15 | M30 | H1)` = UTC-epoch modular as today. Intraday granularities do not respect session boundaries (no bar "crosses" 09:30 because the feed naturally aligns).
- `bar_window(ts, Timeframe::W1 | MN1)` = ISO-week / calendar-month aligned to regular session open of the first trading day.

## `CryptoSpotCalendar`

- `tz()` = `UTC`.
- `covers()` = `NaiveDate::MIN..NaiveDate::MAX` (always answers).
- `session_for(ts)` = `Session { kind: Regular, open: UTC midnight of ts, close: UTC midnight of ts + 1d }`.
- `classify(ts)` = always `Regular`.
- `is_trading_day(_)` = `true`.
- `bar_window(ts, tf)` = `(epoch_floor(ts, tf), epoch_floor(ts, tf) + tf_duration)`.
- `sessions_between` = iterator yielding nothing (no chrome to render — single Regular session spans the whole range).
- `time_axis()` = `Continuous`.

## Bar-producer flow

```rust
// Aggregator / sim:
let cal = calendar_for(&symbol);                  // registry lookup
let kind = cal.classify(tick.ts);                 // session classification
let bar = Bar { ts_open, ts_close, .. , session_kind: Some(kind) };
bars_tx.send(Arc::new(bar));
```

Aggregator uses `cal.bar_window(ts, tf)` instead of the current `epoch % secs` for supported session-sensitive timeframes. For timeframes UTC-epoch modular is valid, use the simpler path.

## `CandleData` extension

```rust
// midas-core::candle_data
pub trait CandleData {
    // ...existing methods...

    /// Session classification for the candle at `idx`.
    ///
    /// Returns `SessionKind::Regular` as a neutral default — consumers
    /// that don't track session metadata get the current behaviour
    /// unchanged.
    fn session(&self, _idx: usize) -> SessionKind {
        SessionKind::Regular
    }
}

// midas-core::candle_buffer::CandleBuffer adds:
pub sessions: Vec<u8>,     // repr of SessionKind; synced with opens/closes
```

`apply_bar()` and `push()` are updated to accept a session kind; existing call sites pass `SessionKind::Regular` until the calendar-aware producer is wired in.

## Rendering integration

### Session-aware candle coloring

In `midas-chart/src/compute/mod.rs::build_candle_instances()`:

```rust
let kind = data.session(i);
let base_color = if is_bull { params.bull_color } else { params.bear_color };
let color = apply_session_tint(base_color, kind, params);

fn apply_session_tint(base: [f32;4], kind: SessionKind, params: &Params) -> [f32;4] {
    match kind {
        SessionKind::Regular => base,
        SessionKind::PreMarket => multiply_alpha(base, params.pre_market_alpha_mult),  // default 0.65
        SessionKind::PostMarket => multiply_alpha(base, params.post_market_alpha_mult), // default 0.55
        _ => base,
    }
}
```

No shader change — reuses `CandleInstance.color`.

### Session background bands

In `build_grid_instances()`:

```rust
if input.show_extended_hours {
    for session in cal.sessions_between(visible_start, visible_end) {
        if !matches!(session.kind, PreMarket | PostMarket) { continue; }
        let x_start = camera.time_to_x(session.open.timestamp_millis() as f64);
        let x_end = camera.time_to_x(session.close.timestamp_millis() as f64);
        let color = band_color(session.kind, params);
        out.push(GridLineInstance {
            rect: [x_start, 0.0, x_end, separator_y],
            color, // RGBA with ~8% alpha
        });
    }
}

fn band_color(kind: SessionKind, params: &Params) -> [f32;4] {
    match kind {
        SessionKind::PreMarket => params.pre_market_bg,   // default [0.85, 0.35, 0.35, 0.06]
        SessionKind::PostMarket => params.post_market_bg, // default [0.35, 0.55, 0.95, 0.06]
        _ => [0.0, 0.0, 0.0, 0.0],
    }
}
```

Bands are drawn before candles in the render order (already the case — `grid_instances` precede `candles` in `ChartRenderer::render`).

### RTH-close separator

Reuse existing `detect_session_boundaries()`. The current implementation detects gaps > 1.5× candle duration; session boundaries fall into this class naturally. Extend the result to carry a `boundary_kind` so we can distinguish day-break (existing behaviour, thin blue) from RTH-close within an intraday view (slightly thicker, different tint).

## UI integration

### `ChartPanel` additions

```rust
pub struct ChartPanel {
    // ...existing fields...
    pub show_extended_hours: bool,
    pub calendar_mic: Mic,
}
```

`show_extended_hours` defaults based on timeframe:
- Intraday (S1..H1): `true` by default.
- D1+: effectively ignored (feed-driven; sessions fold into the daily bar).

### `ChartViewStore` persistence

`ChartViewStore`'s per-(symbol, timeframe) record gains `show_extended_hours`. Serde-persisted; migration adds the field with `serde(default)`.

### Toggle UI

- **Bottom-right "EH" chip** on the chart (one-letter). Click toggles `show_extended_hours`. Visual state: blue when on, gray when off. Placed next to the existing timeframe chip.
- **Settings checkbox**: Chart settings menu → "Show extended-hours trading" (checkbox).

Both write through to `ChartViewStore`.

## Calendar selection (MIC from symbol)

For MVP, every symbol gets `Mic("XNYS")` unless the ticker matches a crypto heuristic (`BTC/USD`, `ETH/USD`, `SPOT-BTC-USD`, etc. — tiny hard-coded list). Future work: a proper symbol → MIC resolver backed by the broker's contract metadata.

```rust
fn calendar_for(symbol: &SymbolKey) -> &'static dyn ExchangeCalendar {
    if is_crypto_symbol(symbol) {
        &CRYPTO_SPOT
    } else {
        &XNYS
    }
}
```

## Migration from today

The single largest migration step is **Slice 0 (historical unification)** — chart historical loads stop using `DataProvider::get_candles` (legacy `midas-feed::TestProvider`) and start using `MarketDataSource::historical_bars` (router + sim). This happens before any calendar work because:

- It fixes the watchlist/chart price-mismatch bug immediately.
- It removes the legacy code path that would otherwise need to be taught about calendars too.
- It is a clean prerequisite — surgical and revertable.

Slices 1–12 follow on top of the unified source.

## Session tints — default palette

- **Regular candles**: unchanged `bull_color` / `bear_color`.
- **Pre-market candles**: `base × 0.65` on RGB, alpha = 1.0. Visually dimmer, still clearly bull/bear.
- **Post-market candles**: `base × 0.55`. Dimmer than pre (typically lower liquidity).
- **Pre-market background band**: `[0.85, 0.35, 0.35, 0.06]` (faint reddish; TradingView-adjacent).
- **Post-market background band**: `[0.35, 0.55, 0.95, 0.06]` (faint bluish).
- **RTH-close vertical separator**: `[0.3, 0.3, 0.5, 0.30]` (existing session-boundary color; thin, 1px DPI-scaled).

All values surfaced via `ChartParams` (the existing style struct in `midas-chart`), configurable.

## What the user sees after this lands

1. Watchlist + chart historical + chart live all driven by the same sim price curve. Numbers match exactly.
2. Default chart timeframe stays D1 (user preference preserved). D1 bars now aggregate correctly from sim ticks through calendar-aware `bar_window`.
3. Switch to M1 on a stock chart: pre-market candles (04:00–09:30 ET) render with reduced brightness against a faint reddish background. Regular-hours candles are full-brightness. Post-market (16:00–20:00 ET) dims again with a bluish tint. A thin vertical line at 16:00 ET marks RTH close.
4. Switch to a crypto symbol: chart is session-less. No tints, no bands, no separator. Time axis is continuous.
5. Toggle "EH" off: pre/post candles and bands disappear. Only RTH candles remain, time axis compresses to 09:30–16:00 ET per day.
6. Holiday early-close day: the RTH session is drawn 09:30–13:00 ET, post-market runs 13:00–17:00 ET. Separator at 13:00 ET.
