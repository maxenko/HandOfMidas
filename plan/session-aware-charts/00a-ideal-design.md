# Ideal Design — Session-Aware Charting System

> **Scope of this document.** Describe the ideal architecture with no concessions to the existing codebase. Integration is a separate concern documented in `00b-integration-strategy.md`. If a current type, API, or abstraction conflicts with the ideal, the ideal wins and integration figures out migration.

## Guiding principles

1. **Sessions are first-class, not metadata.** A candle belongs to a session. There is no candle without a session.
2. **Calendar is mandatory at every time boundary.** Calendar-less code cannot exist. You cannot produce a bar without a calendar. You cannot render a chart without a calendar.
3. **Timeframes are calendar-scoped.** "D1 on XNYS" ≠ "D1 on crypto". The type system enforces this, not convention.
4. **Time is a first-class abstraction with pluggable axes.** Continuous wall-clock, session-compressed, and index-based axes are interchangeable behind one trait. Chart arithmetic routes through the axis.
5. **History and live are one stream.** A `BarStream` produces bars — the seam between "I loaded these from cold storage" and "these are live aggregations" is internal to the stream, invisible to consumers.
6. **Scenes are composable layer stacks, not hard-coded field lists.** Each visual concern (candles, session bands, separators, annotations, volume, holiday markers) is an independent layer with its own state and z-order.
7. **Illegal states are unrepresentable.** If a candle must have a session, its type requires one at construction. If a period is calendar-scoped, its type carries the calendar. Smart constructors only, no public field-struct constructors for domain types.
8. **UTC on the wire, exchange-tz only at the edges.** Every stored `Timestamp` is UTC. Exchange timezone is a calendar concern, applied only for (a) aligning bar windows and (b) UI rendering.
9. **Unified shape across asset classes.** Equities, crypto, forex, futures all use the same types. The calendar is the sole discriminant.
10. **Deterministic over clever.** A calendar out-of-range returns `Err(OutOfRange)`. A session boundary ambiguity resolves through the calendar's documented rules, never heuristics.

---

## Load-bearing invariants from diagnostic review

Review of the initial bolt-on plan surfaced cross-cutting concerns the ideal design must bake in from day one:

1. **`Clock` is a first-class dependency.** `tokio::time::pause()` does NOT affect `chrono::Utc::now()`. Every module that today calls `Utc::now()` must instead consume a `Clock` trait object. Tests inject a `MockClock`. Prod injects `SystemClock`. No exceptions.
2. **`classify()` is infallible and saturating.** Out-of-range timestamps return `SessionKind::Closed`, not `Err`. Calendars expose their `covers()` range for query-time checks but never panic or bubble errors from classification.
3. **Calendar is captured at subscription time, not resolved per tick.** `BarStream::meta().calendar` is immutable for the lifetime of the stream. No `calendar_for(&symbol)` calls on the hot path.
4. **Coverage-range validation happens at construction boundaries.** `CandleSeries::new` / stream subscribe validate the intended query range against `calendar.covers()`; ongoing operations trust the validation.
5. **Holiday rule correctness is enforced by golden-fixture test**, not by hand-wave. The NYSE holiday table's `Friday after the 4th Thursday of November` (Black Friday) is explicitly not "4th Friday of November." State funerals are enumerated including Reagan (2004-06-11) and Ford (2007-01-02). Juneteenth rule gates on `year >= 2022`.

## Type system

### Time

```rust
/// UTC is the only stored representation. Anything else is a lens.
pub type Timestamp = chrono::DateTime<chrono::Utc>;

/// A discrete session on a specific calendar. Always concrete;
/// no "unknown" state. Produced only by calendar methods.
pub struct Session {
    calendar: CalendarId,                        // &'static str, MIC-ish identifier
    kind: SessionKind,
    label: Option<std::borrow::Cow<'static, str>>, // "Tokyo"/"NY" static; Cow so future
                                                 // user-defined overlays (deferred) can
                                                 // carry owned Strings without a type
                                                 // change. Static variants incur zero
                                                 // allocation.
    open: Timestamp,
    close: Timestamp,
}

#[non_exhaustive]
pub enum SessionKind {
    Regular,
    PreMarket,
    PostMarket,
    Break,          // intra-session (CME maintenance, lunch breaks)
    Overnight,
    Closed,         // not a session proper; represents "no session here"
}

// Session can't be constructed publicly; calendars emit them.
impl Session {
    pub(crate) fn new(calendar: CalendarId, kind: SessionKind, open: Timestamp, close: Timestamp) -> Self { ... }
}
```

### Clock abstraction

The `Clock` trait exposes BOTH wall-clock (UTC) and monotonic time. `Instant::now()` is used
throughout the codebase for timeouts, toast expirations, pacing, idle detection; pausing the
wall-clock alone (as with `tokio::time::pause()`) does not suffice. The trait carries both.

```rust
/// First-class clock dependency. Every `Utc::now()` AND every `Instant::now()` in the
/// codebase routes through this. See the `midas-ib-sim/src/engine/clock.rs` prior-art
/// for the split rationale.
pub trait Clock: Send + Sync + 'static {
    /// Wall-clock (UTC). Replaces `chrono::Utc::now()`.
    fn now(&self) -> Timestamp;
    /// Monotonic. Replaces `std::time::Instant::now()`.
    fn now_monotonic(&self) -> std::time::Instant;
}

pub struct SystemClock;
impl Clock for SystemClock {
    fn now(&self) -> Timestamp { chrono::Utc::now() }
    fn now_monotonic(&self) -> std::time::Instant { std::time::Instant::now() }
}

/// Test-only. Exposes `advance_to` / `advance_by` for deterministic session boundary
/// transitions. `advance_by` also calls `tokio::time::advance()` internally (when the
/// tokio test runtime is active) so integration tests see BOTH wall-clock and tokio
/// timers advance together. `inner` stores epoch-nanoseconds (i64, same width as
/// `DateTime<Utc>` NaiveDateTime nanos) to avoid precision loss vs. the aggregator.
#[cfg(any(test, feature = "mock_clock"))]
pub struct MockClock {
    wall_ns: std::sync::atomic::AtomicI64,    // epoch-nanos
    mono_ns: std::sync::atomic::AtomicU64,    // nanos since start
    base_instant: std::time::Instant,
}
#[cfg(any(test, feature = "mock_clock"))]
impl Clock for MockClock {
    fn now(&self) -> Timestamp { /* atomic load wall_ns → Utc */ }
    fn now_monotonic(&self) -> std::time::Instant {
        self.base_instant + std::time::Duration::from_nanos(self.mono_ns.load(..))
    }
}

// Ownership rule: every consumer takes `Arc<dyn Clock>` at construction. This is the ONE
// exception to the "prefer `&'static dyn`" rule used for calendars — clocks are genuinely
// runtime-swappable for tests and per-test isolation; calendars are process-global singletons.
```

**Migration surface.** S0 ships `midas-clock`; migrating the codebase is an auditable
enumeration of every `now()` call:
- `chrono::Utc::now()`: ~60 source sites across 39 files.
- `std::time::Instant::now()`: ~91 sites across 40 files.
- `std::time::SystemTime::now()`: ~8 sites.

S0 includes a forbidden-imports lint (clippy `disallowed_methods` or a repo-local xtask) so
no future code introduces a raw `Utc::now()` / `Instant::now()`. Migration happens per-crate
in later slices; S0 only lands the crate + lint.

### Calendars & Identity

**Ownership rule (R2-NB-1 resolution).** Calendars are process-global singletons behind
`LazyLock`. The canonical reference type is `&'static dyn ExchangeCalendar` everywhere —
`Candle`, `Chart`, `BarStreamMeta`, `SessionedBarAggregator`, `SymbolResolver` all speak
`&'static dyn`. `Arc<dyn>` is rejected to avoid a split ownership model. Tests that need a
per-test mock calendar use `Box::leak` or a per-test `LazyLock`; the `'static` lifetime is
acceptable in tests because each test's calendar outlives all test-scoped streams anyway.

```rust
#[derive(Copy, Clone, Debug, Hash, Eq, PartialEq)]
pub struct CalendarId(pub &'static str);   // "XNYS", "XCME", "CRYPTO", "FX_OTC"

pub trait ExchangeCalendar: Send + Sync + 'static {
    fn id(&self) -> CalendarId;
    fn tz(&self) -> chrono_tz::Tz;

    /// Half-open range `[start, end)`. A date equal to `covers().end` is OUT of coverage.
    /// XNYS covers `2000-01-01 .. 2032-01-01` → 2031-12-31 is covered; 2032-01-01 is not.
    fn covers(&self) -> std::ops::Range<chrono::NaiveDate>;
    fn time_axis_policy(&self) -> TimeAxisPolicy;

    /// Day-level view (one or more non-overlapping sessions per trading day).
    fn trading_day(&self, date: chrono::NaiveDate) -> Result<TradingDay, CalendarError>;
    fn is_trading_day(&self, date: chrono::NaiveDate) -> bool;

    /// Point classification — INFALLIBLE and saturating. Out-of-range returns
    /// `SessionKind::Closed`. Never errors. See load-bearing invariant #2.
    fn classify(&self, ts: Timestamp) -> Session;

    /// Bar window for a (timestamp, period) pair. Calendar-scoped periods
    /// (Session, Calendar) are resolved here; clock-intervals are UTC-epoch modular.
    /// Returns `UnsupportedPeriod` if the (calendar, period) pairing is invalid
    /// (see R2-NB-2 rule below).
    fn bar_window(&self, ts: Timestamp, period: BarPeriod) -> Result<BarWindow, CalendarError>;

    /// Validates a period for this calendar at Chart-construction time. Call ONCE at
    /// chart build; never from the hot path. Fails fast on nonsensical pairings like
    /// `(CryptoSpot, BarPeriod::Session(SessionSpan::Eth))`.
    fn validate_period(&self, period: BarPeriod) -> Result<(), CalendarError>;

    /// Fill `out` with sessions intersecting `[from, to)`. Caller-owned buffer; NO
    /// allocation per call on the render hot path. Pre-reserve `SmallVec<[Session; 16]>`
    /// at Chart build, reuse per frame. Returns the number of sessions written.
    /// For FX overlays this may yield overlapping sessions (Tokyo + London overlap).
    fn sessions_between(
        &self,
        from: Timestamp,
        to: Timestamp,
        out: &mut smallvec::SmallVec<[Session; 16]>,
    ) -> usize;

    /// Navigation primitives.
    fn next_open(&self, ts: Timestamp, kind: SessionKind) -> Option<Timestamp>;
    fn prev_close(&self, ts: Timestamp, kind: SessionKind) -> Option<Timestamp>;
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum TimeAxisPolicy {
    Continuous,                    // crypto: never compress
    CompressedSessionBoundaries,   // stocks/futures/fx: collapse closed time
}

pub struct TradingDay {
    pub date: chrono::NaiveDate,
    pub sessions: smallvec::SmallVec<[Session; 4]>,  // ordered by open; non-overlapping for equities
    pub is_early_close: bool,
    pub is_holiday: bool,
    pub holiday_name: Option<&'static str>,
}

pub struct BarWindow {
    pub open: Timestamp,
    pub close: Timestamp,
    pub session: Session,          // the session this window sits inside
}

#[derive(Debug, thiserror::Error)]
pub enum CalendarError {
    #[error("{0} out of calendar coverage")] OutOfRange(chrono::NaiveDate),
    #[error("unsupported period for {calendar}: {period:?}")]
    UnsupportedPeriod { calendar: CalendarId, period: BarPeriod },
}
```

### Periods — calendar-scoped

```rust
/// A BarPeriod knows what kind of calendar semantics it needs.
/// Construction is via smart constructors; no arbitrary (ClockInterval, SessionScope) combos.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum BarPeriod {
    /// Clock-interval. Same meaning on any calendar. (S5, S15, M1, M5, M15, M30, H1, H2.)
    Clock(ClockInterval),

    /// Session-scoped. Semantics differ per calendar.
    /// XNYS::D1 → the 09:30–16:00 ET regular session.
    /// CRYPTO::D1 → 24 UTC hours.
    /// XCME::D1 → the Globex ETH session (18:00 ET → 17:00 ET with break).
    Session(SessionSpan),

    /// Calendar-scoped. Use the calendar's reckoning of weeks / months / quarters.
    Calendar(CalendarSpan),
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum ClockInterval {
    Seconds(u32), Minutes(u32), Hours(u32),
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum SessionSpan {
    Regular,    // just the regular-hours session (D1 "RTH" for stocks)
    Extended,   // pre + regular + post as one bar (D1 "ETH" for stocks)
    Eth,        // futures electronic session (where applicable)
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum CalendarSpan {
    Week, Month, Quarter, Year,
}

impl BarPeriod {
    pub fn m1() -> Self { Self::Clock(ClockInterval::Minutes(1)) }
    pub fn m5() -> Self { Self::Clock(ClockInterval::Minutes(5)) }
    pub fn h1() -> Self { Self::Clock(ClockInterval::Hours(1)) }
    pub fn d1_rth() -> Self { Self::Session(SessionSpan::Regular) }
    pub fn d1_eth() -> Self { Self::Session(SessionSpan::Extended) }
    pub fn w1() -> Self { Self::Calendar(CalendarSpan::Week) }
    pub fn mn1() -> Self { Self::Calendar(CalendarSpan::Month) }
}
```

**Calendar × Period compatibility matrix (R2-NB-2 resolution).** The type system permits
all (calendar, period) pairings; validity is enforced at `Chart::new` time via
`calendar.validate_period(period)`. This is a FATAL configuration error — the Chart fails
construction, never emits a stream that might fail mid-render. Hot-path `bar_window()` may
still return `UnsupportedPeriod` as a defensive check, but consumers can treat it as
`unreachable!` if construction validation passed.

| Calendar | `Clock(*)` | `Session(Regular)` | `Session(Extended)` | `Session(Eth)` | `Calendar(Week/Month/…)` |
|----------|:---:|:---:|:---:|:---:|:---:|
| XNYS (equities) | ✓ | ✓ RTH 09:30–16:00 ET | ✓ 04:00–20:00 ET | ✗ (ERR) | ✓ |
| CryptoSpot (24h) | ✓ | ✓ 00:00–24:00 UTC | ✓ aliases Regular | ✗ (ERR) | ✓ |
| XCME (futures, future) | ✓ | ✓ RTH pit-hours | ✓ ETH + RTH | ✓ Globex ETH | ✓ |
| FX_OTC (future) | ✓ | ✓ (NY close-anchored day) | ✓ aliases Regular | ✗ (ERR) | ✓ |

Rule: if an asset class has no meaningful distinction between Regular and Extended,
Extended aliases Regular (same window, same session tag). `Eth` is reserved for
calendars that expose a true electronic-session distinct from a pit/regular session —
currently XCME only. CryptoSpot rejects `Eth` with `UnsupportedPeriod` at `validate_period`.

### Candles

**Storage vs. view split (R2-NM-3 resolution).** `Candle` is the API/wire type — a
self-contained, session-tagged bar consumers receive from `BarStream::next()`. `CandleSeries`
stores the MINIMUM (columns per-row, calendar+period once). A `CandleRef` reconstructs the
`Session` lazily from `(calendar, period, timestamps[idx], sessions[idx])` at access time.
The three redundant paths to `CalendarId` (`candle.calendar`, `candle.session.calendar`,
`candle.window.session.calendar`) are internally consistent by construction — `Candle::new`
is the single smart constructor that reconciles them.

```rust
/// A candle IS a session-tagged, calendar-bound OHLCV. Non-optional session.
/// This is the WIRE/API type; `CandleSeries` stores the minimum and reconstructs.
#[derive(Clone, Debug)]
pub struct Candle {
    pub symbol: Symbol,
    pub calendar: CalendarId,
    pub period: BarPeriod,
    pub session: Session,           // ALWAYS present; calendar == self.calendar by ctor
    pub window: BarWindow,          // (open_ts, close_ts, session); session == self.session
    pub o: f64, pub h: f64, pub l: f64, pub c: f64,
    pub volume: u64,
    pub trade_count: u32,
    pub wap: Option<f64>,
    pub completeness: Completeness,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Completeness { Completed, Partial }

impl Candle {
    /// Smart constructor — the only way to produce a `Candle`. Panics (in debug) or
    /// returns Err (in release) if the OHLCV invariants are violated (h < l, c not in
    /// [l, h], etc.). Never fails for valid OHLCV.
    pub fn new(
        symbol: Symbol,
        calendar: &'static dyn ExchangeCalendar,
        period: BarPeriod,
        window: BarWindow,
        ohlcv: Ohlcv,
        completeness: Completeness,
    ) -> Self { ... }
}

pub struct Ohlcv {
    pub o: f64, pub h: f64, pub l: f64, pub c: f64,
    pub volume: u64, pub trade_count: u32, pub wap: Option<f64>,
}
```

### Candle series (storage)

```rust
/// Replaces `CandleBuffer`. SoA for SIMD / GPU affinity, but session identity
/// is baked in — there's no "session-less" variant.
pub struct CandleSeries {
    calendar: CalendarId,
    period: BarPeriod,
    symbol: Symbol,

    // SoA columns, all same length.
    timestamps: Vec<i64>,
    opens: Vec<f32>,
    highs: Vec<f32>,
    lows: Vec<f32>,
    closes: Vec<f32>,
    volumes: Vec<u32>,
    sessions: Vec<SessionKind>,     // parallel; SessionKind is 1 byte via repr(u8)
    completeness: Vec<Completeness>,
    version: AtomicU64,
}

impl CandleSeries {
    pub fn new(calendar: CalendarId, period: BarPeriod, symbol: Symbol) -> Self;
    pub fn push(&mut self, candle: Candle);                 // validates calendar + period match
    pub fn apply(&mut self, candle: Candle);                // overwrite last if ts_open matches
    pub fn at(&self, idx: usize) -> Option<CandleRef<'_>>;  // borrowed view, not a clone
    pub fn iter(&self) -> impl Iterator<Item = CandleRef<'_>>;
}

pub struct CandleRef<'a> {
    pub series: &'a CandleSeries,
    pub idx: usize,
}
```

### BarStream — unified history + live

**Seek split (R2-NB-4 resolution).** Not all streams seek. `BarStream` is the minimum trait
(meta + next + snapshot). `SeekableBarStream: BarStream` is the opt-in sub-trait for
streams that expose time-travel (cold historical, fixture replay). Live streams do NOT
implement `SeekableBarStream` — they cannot rewind a broadcast subscription. `HistoryThenLive`
implements `SeekableBarStream` only while cursor < handoff, delegating to the history half.

`BarStreamMeta.calendar` is `&'static dyn ExchangeCalendar` (not `CalendarId`) so per-tick
consumers have direct access without a registry lookup — the whole point of pinning at
subscribe time.

```rust
/// One stream type. The implementation decides whether to source from cold storage,
/// a live fan-out, a sim, or a file.
#[async_trait]
pub trait BarStream: Send {
    fn meta(&self) -> &BarStreamMeta;
    async fn next(&mut self) -> Option<Candle>;
    async fn snapshot(&mut self, range: TimeRange) -> Result<Vec<Candle>, StreamError>;
}

/// Opt-in for streams that support historical replay / time-travel.
#[async_trait]
pub trait SeekableBarStream: BarStream {
    async fn seek(&mut self, to: Timestamp) -> Result<(), StreamError>;
}

pub struct BarStreamMeta {
    pub symbol: Symbol,
    pub calendar: &'static dyn ExchangeCalendar,
    pub period: BarPeriod,
}

#[derive(Debug, thiserror::Error)]
pub enum StreamError {
    #[error("stream closed")] Closed,
    #[error("not seekable (live stream)")] NotSeekable,
    #[error("timestamp {0} outside stream range")] OutOfRange(Timestamp),
    #[error("upstream error: {0}")] Upstream(String),
}

/// Combinators:
pub struct HistoryThenLive<H, L> { ... }   // chain history → live; SeekableBarStream only
                                           // while cursor sits in the history half
pub struct Filtered<S, P> { ... }          // filter by EhPolicy etc.
pub struct Resampled<S> { ... }            // upsample M1 → M5 (aggregation through calendar.bar_window)
```

### Time axis (first-class)

**`from_x` policy (R2-NM-5 resolution).** `from_x` returns `Option<Timestamp>` — `None`
iff the x-coordinate lies inside a compressed gap on `CompressedAxis`. Consumers of
`from_x` get a SECOND method `from_x_snapped` that always succeeds by snapping to the
nearest session-edge timestamp. Rule:
- Crosshair tooltip / axis label: use `from_x_snapped`.
- Click-to-place (BracketTool, annotation placement): use `from_x_snapped` with
  `SnapDirection::Forward` (place at next-session-open, never mid-gap).
- Hit-testing for hover over existing elements: use `from_x` (None = no hit).

```rust
/// Chart arithmetic routes through the axis. Pluggable: continuous / compressed / indexed.
pub trait TimeAxis: Send + Sync {
    fn to_x(&self, ts: Timestamp) -> f32;
    fn from_x(&self, x: f32) -> Option<Timestamp>;   // None inside compressed gaps
    fn from_x_snapped(&self, x: f32, dir: SnapDirection) -> (Timestamp, bool);
    //   returns (snapped_ts, was_snapped); was_snapped=true means caller sat in a gap
    fn ticks(&self, density: TickDensity) -> Vec<TimeTick>;
    fn width_px(&self) -> f32;
    fn policy(&self) -> TimeAxisPolicy;
}

#[derive(Copy, Clone, Debug)]
pub enum SnapDirection { Nearest, Forward, Backward }

pub struct ContinuousAxis { start: Timestamp, end: Timestamp, width: f32 }
pub struct CompressedAxis { sessions: smallvec::SmallVec<[Session; 16]>, width: f32, gap_px: f32 }
pub struct SessionIndexAxis { timestamps: Arc<[Timestamp]>, width: f32 }

pub struct TimeTick {
    pub x: f32,
    pub ts: Timestamp,
    pub label: TickLabel,      // primary label ("Jan"), optional secondary ("2025")
    pub importance: Importance, // thin tick vs strong tick
}
```

### Scene — composable layers

**`PaintContext` (R2-NB-3 resolution).** Sans-IO primitive emitter. Layers receive a
read-only view of axis + viewport + price-range and a mutable `ScenePrimitives` vector
bank to emit into. No GPU types, no iced types, no wgpu types. The renderer post-processes
`ScenePrimitives` into batched GPU draw calls. This keeps layers testable and mockable.

**Z-ordering (R2-NB-5 resolution).** Each concrete layer type carries a compile-time
`LAYER_Z: LayerZ` associated constant enumerated below. Collisions are structurally
impossible. Builder sorts by `(LAYER_Z, insertion_index)` so same-Z sibling layers (e.g.,
two `LevelLayer`s for different price-levels) render in insertion order.

```rust
pub struct ChartScene {
    axis: Box<dyn TimeAxis>,
    price_range: PriceRange,
    viewport: Viewport,
    layers: Vec<Box<dyn SceneLayer>>,
}

pub trait SceneLayer: Send + Sync {
    fn id(&self) -> LayerId;
    fn z(&self) -> LayerZ;                       // replaces i32 z_order
    fn paint(&self, ctx: &mut PaintContext<'_>);
}

/// Fixed, enumerated z-ordinals. Adding a new layer = adding a new variant here. Keeps
/// render order explicit and change-auditable in code review.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum LayerZ {
    SessionBand     = 0,   // darkest background — trading-hour tint
    Grid            = 1,   // gridlines on top of bands
    SessionSeparator= 2,   // thin vertical rule between sessions
    Volume          = 3,   // bottom-pane volume bars
    Candle          = 4,   // candles render over volume + grid
    HolidayMarker   = 5,   // holiday day markers
    PriceLine       = 6,   // user + order-bracket price lines
    OrderBracket    = 7,   // bracket badges / drag handles
    Level           = 8,   // named level annotations (above brackets for edit affordance)
    Decorator       = 9,   // decorator-tree interactive elements
    Crosshair       = 10,  // always-on-top crosshair
}

/// Primitive vocabulary — layers emit into this; GPU renderer batches post-hoc.
pub struct PaintContext<'a> {
    pub axis: &'a dyn TimeAxis,
    pub viewport: Viewport,
    pub price_range: PriceRange,
    pub palette: &'a ThemePalette,
    pub out: &'a mut ScenePrimitives,
}

#[derive(Default)]
pub struct ScenePrimitives {
    pub candles: Vec<CandleInstance>,
    pub quads:   Vec<QuadInstance>,          // bands, highlights, handles
    pub lines:   Vec<LineInstance>,          // separators, gridlines, price lines
    pub badges:  Vec<BadgeInstance>,         // bracket badges, holiday markers
    pub text:    Vec<TextInstance>,
}

// Concrete layers, each independently testable:
pub struct CandleLayer { candles: Arc<CandleSeries>, style: CandleStyle }
pub struct VolumeLayer { candles: Arc<CandleSeries>, style: VolumeStyle }
pub struct GridLayer { style: GridStyle }
pub struct SessionBandLayer {
    sessions: smallvec::SmallVec<[Session; 16]>,   // reused buffer, not reallocated
    palette: SessionPalette,
}
pub struct SessionSeparatorLayer {
    boundaries: smallvec::SmallVec<[SessionBoundary; 32]>,
    style: SeparatorStyle,
}
pub struct HolidayMarkerLayer { holidays: Vec<(chrono::NaiveDate, &'static str)> }
pub struct CrosshairLayer { position: Option<(f32, f32)> }

// Annotation layers (R2-NM-4 resolution): concrete per kind, NOT a god-enum.
// Each owns its own state machine. The rejected "generic AnnotationLayer" remains rejected.
pub struct OrderBracketLayer { brackets: Vec<OrderBracketView> }
pub struct PriceLineLayer    { lines: Vec<PriceLineView> }
pub struct LevelLayer        { levels: Vec<LevelView> }
pub struct DecoratorLayer    { tree: DecoratorTree }       // existing decorator system wrapped

/// Layer stack is declarative; order is by `LayerZ` then insertion.
impl ChartScene {
    pub fn builder() -> ChartSceneBuilder;
}

pub struct ChartSceneBuilder { ... }

impl ChartSceneBuilder {
    pub fn axis<A: TimeAxis + 'static>(self, axis: A) -> Self;
    pub fn price_range(self, range: PriceRange) -> Self;
    pub fn viewport(self, vp: Viewport) -> Self;
    pub fn layer<L: SceneLayer + 'static>(self, layer: L) -> Self;
    pub fn build(self) -> ChartScene;
}
```

### Chart — composition root

**Camera demotion (R2-NM-7 resolution).** `Camera2D` is deleted. Its fields decompose to:
- `time_start` / `time_end` → owned by `TimeAxis` (each axis impl stores its own range).
- `price_low` / `price_high` → `PriceRange`.
- `viewport_width` / `viewport_height` / `dpi_scale` → `Viewport`.
- `projection_matrix()` → computed by the renderer from `(axis, price_range, viewport)`.

Nothing called "Camera" survives. Pan/zoom state is axis-domain (pan = shift axis range;
zoom = scale axis range), not a separate concern. Axis + PriceRange + Viewport is the
full projection state.

```rust
pub struct Chart {
    pub symbol: Symbol,
    pub calendar: &'static dyn ExchangeCalendar,
    pub period: BarPeriod,
    pub eh_policy: EhPolicy,
    pub stream: Box<dyn BarStream>,
    pub series: CandleSeries,
    pub axis: Box<dyn TimeAxis>,         // owns time-range; no Camera
    pub price_range: PriceRange,
    pub viewport: Viewport,
    pub interaction: InteractionState,   // pan/zoom/drag/hover (R2-G-3)
    pub layer_config: LayerConfig,       // which layers are enabled
}

/// Interaction is per-chart mutable state; not part of rendering. Paint is pure;
/// event handlers mutate `interaction` and dirty the axis/price_range/viewport.
/// (R2-G-3 resolution.)
pub struct InteractionState {
    pub hover: Option<HoverTarget>,
    pub drag: Option<DragSession>,
    pub crosshair_px: Option<(f32, f32)>,
    pub last_wheel_ts: Option<std::time::Instant>,
}

pub enum EhPolicy {
    ShowAll,              // pre+RTH+post, full chrome
    HideExtended,         // RTH only
    ShowBarsOnly,         // pre/post candles visible, no band/separator
}

pub struct LayerConfig {
    pub candles: bool,
    pub volume: bool,
    pub grid: bool,
    pub session_bands: bool,
    pub session_separators: bool,
    pub holidays: bool,
    pub annotations: bool,
    pub crosshair: bool,
}
```

---

## Rendering model

### Render pass

```
for each frame:
    1. Chart applies any pending stream Candles to its CandleSeries.
    2. Chart builds a ChartScene via ChartScene::builder()
       - axis from calendar.time_axis_policy() + camera state
       - layers from layer_config + series + calendar.sessions_between(viewport)
    3. GPU renderer traverses layers by z_order, calling paint(ctx).
    4. Each layer emits its own GPU primitives (not a god struct of instances).
```

### Layer independence

Each `SceneLayer` owns its state and its paint logic. No layer reaches into another. Z-order is the only contract.

Adding a new layer (e.g., "earnings beats marker") is: implement `SceneLayer`, register in `LayerConfig`. Zero modifications to existing layers.

### GPU pipeline mapping

Each layer may use one or more GPU pipelines, but layers compose cleanly:
- `CandleLayer` → candle pipeline (instanced).
- `SessionBandLayer` → quad pipeline (one quad per band).
- `SessionSeparatorLayer` → line pipeline.
- `GridLayer` → line pipeline.

Under the hood, the GPU renderer can batch pipelines across layers for efficiency, but the user-facing layer API stays clean.

---

## Data flow

```
┌─────────────────────────────────────────────────────────┐
│  Provider (Sim / IB)                                    │
│  emits ticks + raw bars                                 │
└─────────────────────────────────────────────────────────┘
                    ↓
┌─────────────────────────────────────────────────────────┐
│  CalendarClassifier                                     │
│  each tick/bar tagged with calendar.classify(ts)        │
└─────────────────────────────────────────────────────────┘
                    ↓
┌─────────────────────────────────────────────────────────┐
│  SessionedBarAggregator                                 │
│  uses calendar.bar_window(ts, period) to align          │
│  emits Candle (always session-tagged)                   │
└─────────────────────────────────────────────────────────┘
                    ↓
┌─────────────────────────────────────────────────────────┐
│  BarStream                                              │
│  HistoryThenLive chains cold → live on the same timeline│
└─────────────────────────────────────────────────────────┘
                    ↓
┌─────────────────────────────────────────────────────────┐
│  CandleSeries                                           │
│  per-(symbol, calendar, period). Session identity baked│
└─────────────────────────────────────────────────────────┘
                    ↓
┌─────────────────────────────────────────────────────────┐
│  ChartScene (layer stack)                               │
│  CandleLayer + SessionBandLayer + SessionSeparatorLayer │
│  + GridLayer + VolumeLayer + AnnotationLayer + ...      │
└─────────────────────────────────────────────────────────┘
                    ↓
             GPU rendering
```

---

## Ideal behaviours

### Session classification always succeeds

`calendar.classify(ts)` returns `Session` (may be `SessionKind::Closed`). No Option. No "unknown." The calendar is authoritative — if it doesn't know, its `covers()` range is wrong and `CalendarError::OutOfRange` bubbles from upstream construction.

### Bar aggregation honours the calendar

`SessionedBarAggregator` takes `(Arc<dyn ExchangeCalendar>, BarPeriod)`. For each tick:
1. `window = calendar.bar_window(tick.ts, period)` → `BarWindow { open, close, session }`.
2. If `window.open` matches current bar → fold.
3. Else → close current bar, open new with `session: window.session`.

The aggregator cannot produce a session-less bar.

### `BarPeriod::Clock` intraday bars across session boundaries

An M1 bar at 09:29 ET is PreMarket. An M1 bar at 09:30 ET is Regular. No bar "straddles" 09:30 — the calendar's `bar_window` ensures the bar closing at 09:30 ET is discrete from the bar opening at 09:30 ET.

This is enforced in the aggregator: when a tick crosses a session boundary, the current bar is closed immediately with `Completeness::Completed` and a new bar opens in the new session.

### Calendar-scoped periods

`BarPeriod::Session(SessionSpan::Regular)` on XNYS produces one bar per trading day, from 09:30–16:00 ET (or 13:00 ET on early-close days).

`BarPeriod::Calendar(CalendarSpan::Week)` on XNYS produces one bar per ISO week, from Monday 09:30 ET to Friday 16:00 ET (respecting holidays — if Monday is a holiday, window opens Tuesday 09:30 ET).

`BarPeriod::Session(SessionSpan::Regular)` on CryptoSpot produces one bar per 24 UTC hours (the crypto calendar maps Regular→24h).

### Time axis continuity

`ContinuousAxis`: every pixel maps to a UTC timestamp linearly. Use for crypto.

`CompressedAxis`: spans list only active sessions; gaps between sessions render with a configurable pixel gap (default 0). A session close at 16:00 ET visually butts against the next pre-market open at 04:00 ET next day with a thin separator. `from_x()` returns `None` for pixels inside the visual gap.

`SessionIndexAxis`: x-coordinate is fractional bar index. Use for extreme zoom, indicator alignment, or analytical work. Wall-clock labels on the axis require a `timestamps` lookup but bar spacing is uniform.

All three implement `TimeAxis`. Chart swaps between them without touching candle data.

### EhPolicy

Determines what gets rendered:
- `ShowAll`: candles, bands, separators, all layers normal.
- `HideExtended`: the `SessionedBarAggregator` filters out `PreMarket`/`PostMarket` candles; the time axis compresses those windows out; bands don't render (nothing to band).
- `ShowBarsOnly`: candles render (tinted by session), but `SessionBandLayer` and `SessionSeparatorLayer` don't emit.

EhPolicy is per-chart, persisted.

---

## Holiday + early-close rules (XNYS)

Per diagnostic review, the following rules are binding:

- **Regular observed holidays** (rule-based): New Year's Day (with weekend-to-Monday observance), Martin Luther King Jr. Day (3rd Monday Jan), Presidents Day (3rd Monday Feb), Good Friday (Easter Sunday – 2 days), Memorial Day (last Monday May), **Juneteenth (gated on `year >= 2022`, with weekend-to-Monday observance)**, Independence Day (with weekend observance), Labor Day (1st Monday Sep), Thanksgiving (4th Thursday Nov), Christmas Day (with weekend observance).
- **Early-close rules**:
  - **Day after Thanksgiving**: `thanksgiving_date + 1 day` — CLOSES AT 13:00 ET. Do NOT define as "4th Friday of November"; coincidentally wrong in ~40% of years in coverage range.
  - **Day before Independence Day (July 3) if weekday**: CLOSES AT 13:00 ET.
  - **Christmas Eve (Dec 24) if weekday and markets open**: CLOSES AT 13:00 ET.
  - Day before Good Friday: NOT an NYSE equity early close. (The SIFMA bond-market convention that Treasuries close early on Maundy Thursday is a different calendar; do not conflate.)
- **Ad-hoc closures** (enumerated):
  - 9/11 attacks: 2001-09-11, 2001-09-12, 2001-09-13, 2001-09-14 (markets closed).
  - Hurricane Sandy: 2012-10-29, 2012-10-30.
  - Reagan state funeral: 2004-06-11.
  - Ford state funeral: 2007-01-02.
  - George H.W. Bush state funeral: 2018-12-05.
  - Jimmy Carter day of mourning: 2025-01-09.
- **Pre-market window 04:00 ET**: reflects the ECN/ARCA-wide convention used by TradingView, Bloomberg, IBKR TWS. NYSE floor formally accepts orders from 06:30 ET; documenting this in the `XnysCalendar` docstring to avoid surprise.

Coverage: 2000-01-01..2031-12-31. Dates outside this range: `classify()` returns `SessionKind::Closed`; `trading_day()` returns `Err(OutOfRange)`.

Test: iterate 2020-01-01..2031-12-31, cross-check `is_trading_day(d) == !nyse_holiday_cal::is_holiday(d)` for every day.

## Explicit rejections of the current codebase's shape

These are conscious breaks from today's architecture:

1. **`CandleBuffer` is retired.** Replaced by `CandleSeries` which requires `(calendar, period, symbol)` at construction. No session-less candles representable.

2. **`Bar` gets `session: Session` (not `Option`).** Every producer classifies. If a producer doesn't know, it's not a producer.

3. **`Timeframe` enum is retired.** Replaced by `BarPeriod` enum with three variants (Clock/Session/Calendar). Semantic meaning depends on the calendar.

4. **`Camera2D::time_to_x` goes away.** Replaced by `TimeAxis::to_x` with pluggable policies. Camera owns the pan/zoom state; axis owns the projection.

5. **`ChartScene` as a god struct with 15+ fields is retired.** Replaced by a `Vec<Box<dyn SceneLayer>>`. New visual primitives land by adding a layer, not by extending the struct.

6. **`DataProvider::get_candles` is retired.** Replaced by `BarStream::snapshot(range)`. Historical and live are one trait.

7. **Aggregator's calendar param is not optional.** No default-Regular fallback. Construction requires a calendar.

8. **`CandleData::session(idx)` default method is retired.** There's no default behaviour — callers either have session data (always, since `CandleSeries` stores it) or they're working on the wrong type.

9. **`detect_session_boundaries()` as a generic gap detector is retired.** `SessionSeparatorLayer` iterates `calendar.sessions_between(viewport)` and emits separators based on session transitions. No heuristics about "gap > 1.5× candle duration."

10. **Symbol → calendar resolution is explicit, not heuristic.** Every Symbol carries a `calendar: CalendarId` field or is resolved through a `SymbolResolver` trait. No hard-coded string matching for "BTC."

---

## Round-2 resolutions appendix

Round-2 plan-eval (`99-diagnostic-findings-r2.md`) surfaced blockers, majors, minors, and
gaps. Fixes are folded inline above. This appendix indexes them by finding-id for
traceability. Gaps not resolved inline:

- **G-1 Volume pane**: Volume renders in the main pane as `VolumeLayer` at `LayerZ::Volume`
  (z=3) occupying the bottom 20% of the viewport by default. Multi-pane split (separate
  volume pane with its own price-range) is deferred to a follow-up; single-pane w/ bottom
  strip is MVP.
- **G-2 Indicators**: `midas-indicators` integration is deferred to a sibling plan.
  Indicators become `ComputedSeriesLayer<I: Indicator>` at a new `LayerZ::Indicator` slot
  inserted between `Candle` and `PriceLine`. Not in this plan's scope.
- **G-4 Persistence & re-hydration**: `ChartViewStore` persists
  `(symbol, calendar_id, period, axis_snapshot, price_range, viewport, eh_policy,
  layer_config)`. On restart, `SessionChart::rehydrate(stored)` creates a new `BarStream`,
  calls `snapshot(stored.axis.time_range())` to backfill history, then transitions to
  live. Open: whether to persist interaction state; default NO (fresh hover/crosshair
  each session).
- **G-5 Multi-source overlay**: Deferred. A single `CandleSeries` per Chart is MVP.
  Overlay (SPY + ES) is a Phase F future plan; requires a `CompositeAxis` that picks a
  "primary calendar" for session compression and plots non-primary series with a
  timestamp-alignment rule.
- **G-6 Thumbnail renderer**: A thumbnail is a `ChartScene` with
  `LayerConfig { candles: true, grid: false, session_bands: true, ..all_false }` and a
  tiny `Viewport`. `ThumbnailDataStore` is retired; thumbnails read from the same
  `CandleSeries` as the main chart.
- **G-7 `DumpState` projection**: Auto-derived via `#[derive(Serialize)]` on all new
  types; no hand-maintained projection struct. Phase D has one commit that replaces the
  manual projection struct with a serde_json::to_value call.
- **G-8 Early-close + Clock(H1)**: On an early-close day (13:00 ET close), an H1 bar
  opening at 13:00 ET is CLOSED immediately at 13:00 ET and marked
  `Completeness::Completed`. The next pre-market opens a new bar at 04:00 ET next day.
  Matches TV convention. Enforced by the aggregator: `bar_window` returns a truncated
  window, and session-crossing forces bar close.
- **G-9 `SymbolResolver`**: Specified as:
  ```rust
  pub trait SymbolResolver: Send + Sync + 'static {
      fn resolve(&self, ticker: &str) -> Result<ResolvedSymbol, ResolveError>;
  }
  pub struct ResolvedSymbol {
      pub symbol: Symbol,
      pub calendar: &'static dyn ExchangeCalendar,
      pub provider_id: ProviderSymbolId,   // sim stable-hash; IB con_id; etc.
  }
  ```
  Provider-specific: `SimSymbolResolver` synthesizes a stable-hash id; `IbSymbolResolver`
  does a `reqContractDetails` round-trip. Lives in the provider crate (not `midas-calendar`).
  S6 (BarStream adapter) depends on this; S2's `Symbol` type does NOT carry calendar
  directly — resolution is always through `SymbolResolver`.

**Deferred to Phase F (not this plan):**
- Auto-switching compressed↔continuous axis at a zoom threshold (R2-NM-6). MVP is
  user-explicit toggle.
- Multi-calendar overlays on one chart (R2-G-5).
- Indicator layer infrastructure (R2-G-2).
- Volume pane split (R2-G-1 deferred half).

---

## What this enables

- **Adding a new calendar (XCME futures, FX, exotic exchanges)**: implement `ExchangeCalendar`, register. Every layer, every stream, every aggregator works automatically.
- **Adding a new visual (earnings markers, news, regime shading)**: implement `SceneLayer`, add to layer config. Zero changes to existing code.
- **Adding a new period type (e.g., Renko, Range, Volume-weighted bars)**: extend `BarPeriod` or introduce `AdaptivePeriod` alongside. Aggregator dispatches on the discriminant.
- **Multiple calendars on one chart** (e.g., overlaying SPX on ES): two `CandleSeries`, two calendars, one `TimeAxis`. The axis chooses a common compression policy.
- **Backtesting on any asset class**: `BarStream` driven by a cold `HistoricalStream` is indistinguishable from live to the chart. Calendar drives both.
- **Replay**: `BarStream` reads from a `.jsonl` fixture. Same chart code.
- **Arbitrary visual compositions**: remove `VolumeLayer` for pure price charts; add a `DepthLayer` for L2 data; etc.

---

## Non-goals of the ideal design

- **No attempt to preserve `CandleBuffer`'s mmap binary layout.** If users want persistence, `CandleSeries` gets a `to_mmap` / `from_mmap` helper; but the on-disk shape matches the in-memory shape, not legacy.
- **No backward-compat shims.** Smart constructors everywhere. Legacy call sites either migrate or delete.
- **No "lightweight mode" for session-less operation.** Every chart has a calendar. If there's no calendar available, there's no chart.
- **No generic "annotation layer can do anything" escape hatch.** Each visual concern is its own concrete layer type with a specific state. Generic flexibility means less compile-time guarantee.

---

## Summary

A world where the type system enforces session + calendar identity on every bar, every axis, every layer. Periods are calendar-scoped. Scenes are composable. History and live are one stream. Crypto, stocks, futures, forex share one shape. Integration work is significant — but the result is a chart architecture that's correct by construction, not correct by convention.

Next doc: `00b-integration-strategy.md` — how to get from the current codebase to this design without shipping a broken intermediate.
