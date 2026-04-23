# Slice 1 — `midas-calendar` crate

**Goal.** Create the `midas-calendar` crate with the `ExchangeCalendar` trait, shared types, and two concrete impls: `XnysCalendar` (US equities) and `CryptoSpotCalendar` (24/7).

## Crate layout

```
crates/midas-calendar/
  Cargo.toml
  src/
    lib.rs
    trait.rs          — ExchangeCalendar trait
    types.rs          — SessionKind, Session, TradingDay, Mic, CalendarError
    registry.rs       — CalendarRegistry
    xnys/
      mod.rs          — XnysCalendar impl
      holidays.rs     — rule-based regulars + dated ad-hoc + early-close (table)
      tests.rs        — cross-check vs nyse-holiday-cal
    crypto_spot.rs    — CryptoSpotCalendar impl
```

## `Cargo.toml`

```toml
[package]
name = "midas-calendar"
version = "0.1.0"
edition = "2021"

[dependencies]
chrono = { version = "0.4", features = ["serde"] }
chrono-tz = "0.9"
smallvec = "1"
thiserror = "1"
serde = { version = "1", features = ["derive"], optional = true }

[dev-dependencies]
nyse-holiday-cal = "0.2"   # cross-check only, never runtime
```

Root workspace `Cargo.toml`: add `crates/midas-calendar` to `[workspace] members`.

## Types

Per `01-architecture.md` §"Core types":

```rust
// types.rs
pub struct Mic(pub &'static str);

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Default)]
pub enum SessionKind {
    #[default] Regular,
    PreMarket, PostMarket, Break, Overnight, Closed,
}

pub struct Session {
    pub kind: SessionKind,
    pub open: DateTime<Utc>,
    pub close: DateTime<Utc>,
    pub label: Option<&'static str>,
}

pub struct TradingDay { /* ... */ }

#[derive(Debug, thiserror::Error)]
pub enum CalendarError {
    #[error("date {0} out of calendar coverage range")]
    OutOfRange(NaiveDate),
    #[error("unsupported timeframe {0:?}")]
    UnsupportedTimeframe(Timeframe),
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum TimeAxisPolicy { Continuous, CompressedClosed }
```

**Note**: `Timeframe` is imported from `midas-broker-core`. `midas-calendar` takes `midas-broker-core` as a dep (same workspace). Verify direction — `midas-broker-core` must NOT depend on `midas-calendar`. The dep edge is `midas-calendar -> midas-broker-core`.

## Trait

```rust
// trait.rs
#[async_trait]  // actually synchronous; no async methods
pub trait ExchangeCalendar: Send + Sync {
    fn mic(&self) -> Mic;
    fn tz(&self) -> Tz;
    fn covers(&self) -> Range<NaiveDate>;
    fn time_axis(&self) -> TimeAxisPolicy;

    fn is_trading_day(&self, date: NaiveDate) -> bool;
    fn trading_day(&self, date: NaiveDate) -> Result<TradingDay, CalendarError>;

    fn classify(&self, ts: DateTime<Utc>) -> SessionKind;
    fn session_for(&self, ts: DateTime<Utc>) -> Option<Session>;

    fn next_open(&self, ts: DateTime<Utc>, kind: SessionKind) -> Option<DateTime<Utc>>;
    fn prev_close(&self, ts: DateTime<Utc>, kind: SessionKind) -> Option<DateTime<Utc>>;

    fn bar_window(
        &self, ts: DateTime<Utc>, tf: Timeframe,
    ) -> Result<(DateTime<Utc>, DateTime<Utc>), CalendarError>;

    fn sessions_between<'a>(
        &'a self, from: DateTime<Utc>, to: DateTime<Utc>,
    ) -> Box<dyn Iterator<Item = Session> + 'a>;
}
```

No `async` — all methods are pure computation over in-memory tables. Saves a dep on `async-trait`.

## `XnysCalendar`

Per `01-architecture.md` §"XnysCalendar". Key invariants:

- `tz()` = `chrono_tz::America::New_York`.
- `covers()` = 2000-01-01 .. 2031-12-31.
- Regular session: 09:30–16:00 ET (13:00 ET on early-close days).
- Pre-market: 04:00–09:30 ET.
- Post-market: 16:00–20:00 ET (17:00 ET on early-close).
- Holidays: rule-based (New Year, MLK, Presidents, Good Friday, Memorial, Juneteenth, Independence, Labor, Thanksgiving, Christmas) + early-close rules (Black Friday, Christmas Eve if weekday, July 3 if weekday) + dated ad-hoc (9/11, Hurricane Sandy, state funerals — source: `exchange_calendars` USExchangeCalendar + NYSE archival).
- `bar_window(ts, Timeframe::D1)` = regular session window of the trading day containing or next-after `ts`.
- `bar_window(ts, Timeframe::W1)` = regular session of first trading day of the ISO week (or next trading day if Monday is a holiday) → close of the regular session of the last trading day of that ISO week.
- `bar_window(ts, Timeframe::H4)` = `09:30, 13:30` or `13:30, 17:30` ET (halved session; handle early-close days with a single `09:30–13:00` H4 window).
- Intraday `(S5..H1)` → UTC-epoch modular; calendar is not consulted.

Implementation spread:
- `xnys/mod.rs` — trait impl, cached static table.
- `xnys/holidays.rs` — all holiday + early-close logic.
- `xnys/tests.rs` — cross-check against `nyse-holiday-cal` for every date in the coverage range, assert matches.

## `CryptoSpotCalendar`

Trivial; 24/7 UTC-aligned. 30 lines.

## `CalendarRegistry`

```rust
pub struct CalendarRegistry {
    calendars: HashMap<Mic, &'static dyn ExchangeCalendar>,
}

impl CalendarRegistry {
    pub fn new_default() -> Self {
        let mut calendars: HashMap<_, _> = HashMap::new();
        calendars.insert(Mic("XNYS"), &XNYS as &'static dyn ExchangeCalendar);
        calendars.insert(Mic("XNAS"), &XNYS);  // NASDAQ shares NYSE calendar (accurate enough for MVP)
        calendars.insert(Mic("CRYPTO"), &CRYPTO_SPOT);
        Self { calendars }
    }

    pub fn get(&self, mic: Mic) -> Option<&'static dyn ExchangeCalendar> {
        self.calendars.get(&mic).copied()
    }
}

pub static XNYS: XnysCalendar = XnysCalendar::new();
pub static CRYPTO_SPOT: CryptoSpotCalendar = CryptoSpotCalendar::new();
```

## Tests

- **Session classification**: every hour of a known Wednesday maps to correct `SessionKind` (04:00 → PreMarket, 09:30 → Regular, 16:00 → PostMarket, 20:00 → Closed, 02:00 → Closed).
- **Holidays**: 2024-07-04 returns `is_holiday=true`, `is_trading_day=false`.
- **Early close**: 2024-11-29 (Black Friday) has `is_early_close=true`, regular session 09:30–13:00 ET.
- **Cross-check**: iterate 2020..2031, assert `XnysCalendar::is_trading_day(d) == !nyse_holiday_cal::is_holiday(d)` for every `d`.
- **`bar_window`**:
  - `bar_window(2024-01-15 14:00 UTC, D1)` → (`2024-01-15 14:30 UTC`, `2024-01-15 21:00 UTC`) (09:30/16:00 ET on a regular winter day).
  - DST transition: `bar_window(2024-11-04 14:30 UTC, D1)` → post-DST, returns 14:30/21:00 UTC (09:30/16:00 ET with EST offset).
  - On a Saturday: `bar_window(2024-04-20 18:00 UTC, D1)` → Monday's window.
- **Out-of-range**: `trading_day(2032-01-01)` → `Err(OutOfRange)`.
- **Crypto**: `classify(any_ts)` = `Regular`, `time_axis()` = `Continuous`.

## Acceptance

- `cargo test -p midas-calendar` all tests pass.
- `cargo clippy -p midas-calendar -- -D warnings` clean.
- `cargo fmt --all`.
- Cross-check test with `nyse-holiday-cal` dev-dep passes on every day 2020-01-01..2031-12-31.

## Commit

Single commit: `feat(calendar): midas-calendar crate with XNYS + CryptoSpot impls`.

## Risks

- **Holiday table completeness.** Ad-hoc closures (9/11, Sandy, state funerals) are historical and well-known; `nyse-holiday-cal` cross-check catches drift. Future-dated early closes beyond 2031 aren't in scope.
- **DST correctness.** Trust `chrono-tz`'s `America/New_York` zone. `naive_local → utc` conversion uses the zone's DST rules automatically.
- **NASDAQ == NYSE simplification.** True for holidays. Sessions are slightly different in practice (NASDAQ has pre-market from 04:00, NYSE from 06:30 officially for orders, but both accept pre-market activity from 04:00). MVP treats them identically; real split can be added later without API churn.
