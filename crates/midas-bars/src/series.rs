//! `CandleSeries` — SoA storage for session-tagged bars.
//!
//! Replaces the legacy session-less `CandleBuffer`. Per the R2-NM-3
//! resolution (see `plan/session-aware-charts/00a-ideal-design.md`), the
//! series stores the MINIMUM: calendar and period once per series,
//! `SessionKind` and `Completeness` one byte per row, and SoA OHLCV
//! columns. The full `Session` is reconstructed lazily via `CandleRef`.
//!
//! ## Layout
//!
//! - `timestamps: Vec<i64>` — bar-window open timestamps, UTC nanos.
//!   Same width as `DateTime<Utc>::timestamp_nanos_opt()`.
//! - `opens / highs / lows / closes: Vec<f32>` — GPU-friendly precision
//!   matching the existing desktop `CandleBuffer` layout.
//! - `volumes: Vec<u32>` — consistent with desktop bar storage.
//! - `sessions: Vec<SessionKind>` — 1 byte/row via `#[repr(u8)]`.
//! - `completeness: Vec<Completeness>` — 1 byte/row via `#[repr(u8)]`.
//! - `version: AtomicU64` — monotonically increments on every mutation
//!   so consumers can skip paint when unchanged.
//!
//! All columns are the SAME length — an invariant enforced by
//! `debug_assert_eq!` on every `push` / `apply`.

use std::sync::atomic::{AtomicU64, Ordering};

use midas_calendar::{BarPeriod, CalendarId, ExchangeCalendar, Session, SessionKind, Timestamp};

use crate::candle::{Candle, CandleError, Completeness};
use crate::Symbol;

/// SoA session-tagged candle storage. One series = one
/// `(symbol, calendar, period)` tuple. Series are NOT `Serialize` —
/// wire-format is the `Candle` type.
///
/// ## Capacity cap (app-harden H1)
///
/// `max_rows` is optional. When `Some(cap)`, `push` / `apply` drain
/// oldest rows in lockstep across all columns so `len() <= cap` holds
/// after every mutation. Default is `None` (unbounded, matching the
/// pre-cap behaviour). Intraday intraday session-chart drivers should
/// call [`CandleSeries::new_with_cap`] with a reasonable bound
/// (e.g. 10_000 M1 bars ≈ one week of RTH) so long-running sessions
/// can't OOM the app.
///
/// Trade-off: when `max_rows` is exceeded the OLDEST rows fall off the
/// left. Snapshots and charts that queried a range now absent from the
/// series will observe a gap at the left edge. This is the expected
/// behaviour for a rolling buffer.
pub struct CandleSeries {
    calendar: CalendarId,
    period: BarPeriod,
    symbol: Symbol,

    timestamps: Vec<i64>,
    opens: Vec<f32>,
    highs: Vec<f32>,
    lows: Vec<f32>,
    closes: Vec<f32>,
    volumes: Vec<u32>,
    trade_counts: Vec<u32>,
    /// Stored as `f32` with `f32::NAN` sentinel = `None`. Keeps the
    /// column SoA-friendly (no parallel Option bitmap) and matches the
    /// GPU-friendly precision of the other OHLC columns.
    waps: Vec<f32>,
    sessions: Vec<SessionKind>,
    completeness: Vec<Completeness>,

    /// Optional rolling-cap. When `Some(cap)`, oldest rows are drained
    /// in lockstep so `len() <= cap`. See type-level docs.
    max_rows: Option<usize>,

    version: AtomicU64,
}

impl std::fmt::Debug for CandleSeries {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CandleSeries")
            .field("symbol", &self.symbol)
            .field("calendar", &self.calendar)
            .field("period", &self.period)
            .field("len", &self.len())
            .field("version", &self.version.load(Ordering::Relaxed))
            .finish()
    }
}

impl CandleSeries {
    /// Build an empty series. Symbol calendar must match the `calendar`
    /// arg; debug-asserted because `Symbol` carries its own
    /// `CalendarId` by design (R2-G-9 `SymbolResolver`).
    ///
    /// The series is unbounded; use [`CandleSeries::new_with_cap`] for
    /// a rolling-buffer variant that drops oldest rows past a cap.
    pub fn new(calendar: CalendarId, period: BarPeriod, symbol: Symbol) -> Self {
        Self::with_optional_cap(calendar, period, symbol, None)
    }

    /// Build an empty series with a rolling row cap. When `len()`
    /// reaches `max_rows` the OLDEST row is dropped in lockstep across
    /// every column on the next `push` / `apply`. See
    /// [`CandleSeries`] for trade-offs.
    ///
    /// # Panics
    ///
    /// Panics if `max_rows == 0`.
    pub fn new_with_cap(
        calendar: CalendarId,
        period: BarPeriod,
        symbol: Symbol,
        max_rows: usize,
    ) -> Self {
        assert!(max_rows > 0, "max_rows must be > 0");
        Self::with_optional_cap(calendar, period, symbol, Some(max_rows))
    }

    fn with_optional_cap(
        calendar: CalendarId,
        period: BarPeriod,
        symbol: Symbol,
        max_rows: Option<usize>,
    ) -> Self {
        debug_assert_eq!(
            symbol.calendar(),
            calendar,
            "symbol calendar must match series calendar"
        );
        Self {
            calendar,
            period,
            symbol,
            timestamps: Vec::new(),
            opens: Vec::new(),
            highs: Vec::new(),
            lows: Vec::new(),
            closes: Vec::new(),
            volumes: Vec::new(),
            trade_counts: Vec::new(),
            waps: Vec::new(),
            sessions: Vec::new(),
            completeness: Vec::new(),
            max_rows,
            version: AtomicU64::new(0),
        }
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.timestamps.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.timestamps.is_empty()
    }

    #[inline]
    pub fn symbol(&self) -> Symbol {
        self.symbol
    }

    #[inline]
    pub fn calendar(&self) -> CalendarId {
        self.calendar
    }

    #[inline]
    pub fn period(&self) -> BarPeriod {
        self.period
    }

    /// Monotonically-increasing version. Bumps on every mutating call.
    /// Consumers snapshot this to skip paint when unchanged.
    #[inline]
    pub fn version(&self) -> u64 {
        self.version.load(Ordering::Relaxed)
    }

    /// Append a new bar to the end of the series. **Panics** on a
    /// `(calendar, period, symbol)` mismatch — programmer error, not a
    /// runtime condition.
    pub fn push(&mut self, candle: Candle) {
        self.validate_candle(&candle)
            .expect("CandleSeries::push: candle does not match series");
        self.push_columns(&candle);
        self.enforce_cap();
        self.bump_version();
        self.debug_check_invariant();
    }

    /// Apply an in-progress update. If the last bar's open-timestamp
    /// matches `candle.window.open`, overwrite it in place (the bar is
    /// still accumulating ticks). Otherwise delegate to `push`.
    ///
    /// Panics on the same mismatches as `push`.
    pub fn apply(&mut self, candle: Candle) {
        self.validate_candle(&candle)
            .expect("CandleSeries::apply: candle does not match series");
        let ts_open_ns = candle
            .window
            .open
            .timestamp_nanos_opt()
            .expect("timestamp within chrono nanos range");
        if let Some(&last_ts) = self.timestamps.last() {
            if last_ts == ts_open_ns {
                let idx = self.timestamps.len() - 1;
                self.write_row(idx, &candle);
                self.bump_version();
                self.debug_check_invariant();
                return;
            }
        }
        self.push_columns(&candle);
        self.enforce_cap();
        self.bump_version();
        self.debug_check_invariant();
    }

    /// Optional rolling-cap configured at construction.
    #[inline]
    pub fn max_rows(&self) -> Option<usize> {
        self.max_rows
    }

    /// Borrowed view of a single row. `None` if `idx >= len()`.
    pub fn at(&self, idx: usize) -> Option<CandleRef<'_>> {
        if idx < self.len() {
            Some(CandleRef { series: self, idx })
        } else {
            None
        }
    }

    /// Iterate rows left-to-right.
    pub fn iter(&self) -> impl Iterator<Item = CandleRef<'_>> + '_ {
        (0..self.len()).map(move |idx| CandleRef { series: self, idx })
    }

    // --- internals ---

    fn validate_candle(&self, candle: &Candle) -> Result<(), CandleError> {
        if candle.calendar != self.calendar {
            return Err(CandleError::SeriesCalendarMismatch {
                series_calendar: self.calendar,
                candle_calendar: candle.calendar,
            });
        }
        if candle.period != self.period {
            return Err(CandleError::SeriesPeriodMismatch {
                series_period: self.period,
                candle_period: candle.period,
            });
        }
        if candle.symbol != self.symbol {
            return Err(CandleError::SeriesSymbolMismatch {
                series_symbol: self.symbol,
                candle_symbol: candle.symbol,
            });
        }
        Ok(())
    }

    fn push_columns(&mut self, candle: &Candle) {
        let ts_open_ns = candle
            .window
            .open
            .timestamp_nanos_opt()
            .expect("timestamp within chrono nanos range");
        self.timestamps.push(ts_open_ns);
        self.opens.push(candle.o as f32);
        self.highs.push(candle.h as f32);
        self.lows.push(candle.l as f32);
        self.closes.push(candle.c as f32);
        // Clamp volume into u32 — exchanges that exceed u32 volumes in a
        // single bar (unlikely intraday) get saturated rather than
        // panicking.
        self.volumes
            .push(u32::try_from(candle.volume).unwrap_or(u32::MAX));
        self.trade_counts.push(candle.trade_count);
        self.waps.push(wap_to_column(candle.wap));
        self.sessions.push(candle.session.kind());
        self.completeness.push(candle.completeness);
    }

    fn write_row(&mut self, idx: usize, candle: &Candle) {
        let ts_open_ns = candle
            .window
            .open
            .timestamp_nanos_opt()
            .expect("timestamp within chrono nanos range");
        self.timestamps[idx] = ts_open_ns;
        self.opens[idx] = candle.o as f32;
        self.highs[idx] = candle.h as f32;
        self.lows[idx] = candle.l as f32;
        self.closes[idx] = candle.c as f32;
        self.volumes[idx] = u32::try_from(candle.volume).unwrap_or(u32::MAX);
        self.trade_counts[idx] = candle.trade_count;
        self.waps[idx] = wap_to_column(candle.wap);
        self.sessions[idx] = candle.session.kind();
        self.completeness[idx] = candle.completeness;
    }

    /// If a rolling cap is set and `len() > cap`, drain the oldest rows
    /// in lockstep so every column is the same length after return.
    fn enforce_cap(&mut self) {
        let Some(cap) = self.max_rows else {
            return;
        };
        if self.timestamps.len() <= cap {
            return;
        }
        let drop_n = self.timestamps.len() - cap;
        self.timestamps.drain(0..drop_n);
        self.opens.drain(0..drop_n);
        self.highs.drain(0..drop_n);
        self.lows.drain(0..drop_n);
        self.closes.drain(0..drop_n);
        self.volumes.drain(0..drop_n);
        self.trade_counts.drain(0..drop_n);
        self.waps.drain(0..drop_n);
        self.sessions.drain(0..drop_n);
        self.completeness.drain(0..drop_n);
    }

    fn bump_version(&self) {
        self.version.fetch_add(1, Ordering::Relaxed);
    }

    #[cfg(debug_assertions)]
    fn debug_check_invariant(&self) {
        let n = self.timestamps.len();
        debug_assert_eq!(self.opens.len(), n);
        debug_assert_eq!(self.highs.len(), n);
        debug_assert_eq!(self.lows.len(), n);
        debug_assert_eq!(self.closes.len(), n);
        debug_assert_eq!(self.volumes.len(), n);
        debug_assert_eq!(self.trade_counts.len(), n);
        debug_assert_eq!(self.waps.len(), n);
        debug_assert_eq!(self.sessions.len(), n);
        debug_assert_eq!(self.completeness.len(), n);
    }

    #[cfg(not(debug_assertions))]
    fn debug_check_invariant(&self) {}
}

/// `None` round-trips through `f32::NAN`. Callers MUST clamp incoming
/// `Some(NaN)` to `None` ahead of this; `Ohlcv::new` already rejects
/// NaN values.
#[inline]
fn wap_to_column(wap: Option<f64>) -> f32 {
    match wap {
        Some(v) if v.is_finite() => v as f32,
        _ => f32::NAN,
    }
}

#[inline]
fn column_to_wap(v: f32) -> Option<f64> {
    if v.is_finite() {
        Some(v as f64)
    } else {
        None
    }
}

/// Read-side view into a `CandleSeries` row. The full `Session` is
/// synthesized lazily via `CandleRef::session(calendar)` — the stored
/// representation is just `SessionKind` plus the bar's open timestamp.
#[derive(Copy, Clone)]
pub struct CandleRef<'a> {
    series: &'a CandleSeries,
    idx: usize,
}

impl<'a> CandleRef<'a> {
    #[inline]
    pub fn series(&self) -> &'a CandleSeries {
        self.series
    }

    #[inline]
    pub fn idx(&self) -> usize {
        self.idx
    }

    /// The bar's open timestamp (UTC).
    pub fn ts_open(&self) -> Timestamp {
        ns_to_utc(self.series.timestamps[self.idx])
    }

    /// The bar's close timestamp — reconstructed by routing `ts_open`
    /// through the calendar's `bar_window`. Callers on the render hot
    /// path who need many closes should batch via the calendar directly
    /// rather than calling `ts_close` per row.
    pub fn ts_close(&self, calendar: &'static dyn ExchangeCalendar) -> Timestamp {
        let open = self.ts_open();
        calendar
            .bar_window(open, self.series.period)
            .map(|w| w.close)
            .unwrap_or(open)
    }

    #[inline]
    pub fn open(&self) -> f64 {
        self.series.opens[self.idx] as f64
    }

    #[inline]
    pub fn high(&self) -> f64 {
        self.series.highs[self.idx] as f64
    }

    #[inline]
    pub fn low(&self) -> f64 {
        self.series.lows[self.idx] as f64
    }

    #[inline]
    pub fn close(&self) -> f64 {
        self.series.closes[self.idx] as f64
    }

    #[inline]
    pub fn volume(&self) -> u64 {
        self.series.volumes[self.idx] as u64
    }

    /// Total trade count the bar accumulated. `0` indicates either a
    /// truly-empty bar OR that the feed didn't propagate trade counts
    /// (e.g. older historical imports). Callers should not infer
    /// "no trades" from this value without corroborating evidence.
    #[inline]
    pub fn trade_count(&self) -> u32 {
        self.series.trade_counts[self.idx]
    }

    /// Volume-weighted average price, if the aggregator propagated one.
    /// `None` when the bar had no size-bearing trades (only
    /// `Price+Last` ticks) or when the feed omits WAP.
    #[inline]
    pub fn wap(&self) -> Option<f64> {
        column_to_wap(self.series.waps[self.idx])
    }

    #[inline]
    pub fn session_kind(&self) -> SessionKind {
        self.series.sessions[self.idx]
    }

    #[inline]
    pub fn completeness(&self) -> Completeness {
        self.series.completeness[self.idx]
    }

    /// Lazily reconstruct the full `Session` for this row. The caller
    /// supplies the calendar (which they already hold for the chart);
    /// this routes through `ExchangeCalendar::classify`, which is
    /// infallible and saturating.
    pub fn session(&self, calendar: &'static dyn ExchangeCalendar) -> Session {
        calendar.classify(self.ts_open())
    }
}

fn ns_to_utc(ns: i64) -> Timestamp {
    chrono::DateTime::<chrono::Utc>::from_timestamp_nanos(ns)
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use midas_calendar::{xnys, BarPeriod};

    use super::*;
    use crate::candle::Ohlcv;

    fn utc(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> Timestamp {
        chrono::Utc.with_ymd_and_hms(y, mo, d, h, mi, 0).unwrap()
    }

    /// Build a well-formed Candle for the supplied intraday timestamp.
    fn mk_candle(ts: Timestamp, price: f64) -> Candle {
        let cal = xnys();
        let sym = Symbol::new("SPY", cal.id());
        let session = cal.classify(ts);
        let window = cal.bar_window(ts, BarPeriod::m1()).unwrap();
        let ohlcv = Ohlcv::new(price, price + 0.1, price - 0.1, price, 100, 1, None).unwrap();
        Candle::new(
            sym,
            cal,
            BarPeriod::m1(),
            session,
            window,
            ohlcv,
            Completeness::Completed,
        )
        .unwrap()
    }

    #[test]
    fn new_series_is_empty() {
        let cal = xnys();
        let s = CandleSeries::new(cal.id(), BarPeriod::m1(), Symbol::new("SPY", cal.id()));
        assert!(s.is_empty());
        assert_eq!(s.len(), 0);
        assert_eq!(s.version(), 0);
    }

    #[test]
    fn push_appends_and_bumps_version() {
        let cal = xnys();
        let mut s = CandleSeries::new(cal.id(), BarPeriod::m1(), Symbol::new("SPY", cal.id()));
        let ts = utc(2024, 1, 17, 15, 0);
        s.push(mk_candle(ts, 100.0));
        assert_eq!(s.len(), 1);
        assert_eq!(s.version(), 1);
        s.push(mk_candle(ts + chrono::Duration::minutes(1), 100.5));
        assert_eq!(s.len(), 2);
        assert_eq!(s.version(), 2);
    }

    #[test]
    #[should_panic(expected = "does not match series")]
    fn push_panics_on_period_mismatch() {
        let cal = xnys();
        let mut s = CandleSeries::new(cal.id(), BarPeriod::m5(), Symbol::new("SPY", cal.id()));
        // Build a candle with period m1 — mismatch.
        let ts = utc(2024, 1, 17, 15, 0);
        let candle = mk_candle(ts, 100.0);
        assert_eq!(candle.period, BarPeriod::m1());
        s.push(candle);
    }

    #[test]
    #[should_panic(expected = "does not match series")]
    fn push_panics_on_calendar_mismatch() {
        // Series on CRYPTO, candle on XNYS.
        let mut s = CandleSeries::new(
            midas_calendar::CRYPTO_SPOT_ID,
            BarPeriod::m1(),
            Symbol::new("BTC-USD", midas_calendar::CRYPTO_SPOT_ID),
        );
        let ts = utc(2024, 1, 17, 15, 0);
        let candle = mk_candle(ts, 100.0); // XNYS-scoped
        s.push(candle);
    }

    #[test]
    #[should_panic(expected = "does not match series")]
    fn push_panics_on_symbol_mismatch() {
        let cal = xnys();
        let mut s = CandleSeries::new(cal.id(), BarPeriod::m1(), Symbol::new("SPY", cal.id()));
        // Candle is for SPY in mk_candle; make the series AAPL.
        let mut s2 = CandleSeries::new(cal.id(), BarPeriod::m1(), Symbol::new("AAPL", cal.id()));
        let ts = utc(2024, 1, 17, 15, 0);
        s.push(mk_candle(ts, 100.0)); // OK on s
        s2.push(mk_candle(ts, 100.0)); // panic: candle.symbol=SPY != AAPL
    }

    #[test]
    fn apply_overwrites_last_on_matching_ts() {
        let cal = xnys();
        let mut s = CandleSeries::new(cal.id(), BarPeriod::m1(), Symbol::new("SPY", cal.id()));
        let ts = utc(2024, 1, 17, 15, 0);
        s.push(mk_candle(ts, 100.0));
        let v_after_push = s.version();

        // Same ts_open, updated price — overwrite.
        s.apply(mk_candle(ts, 100.9));
        assert_eq!(s.len(), 1);
        assert!(s.version() > v_after_push);
        let row = s.at(0).unwrap();
        assert!((row.open() - 100.9).abs() < 1e-4);
    }

    #[test]
    fn apply_pushes_when_ts_differs() {
        let cal = xnys();
        let mut s = CandleSeries::new(cal.id(), BarPeriod::m1(), Symbol::new("SPY", cal.id()));
        let ts = utc(2024, 1, 17, 15, 0);
        s.push(mk_candle(ts, 100.0));
        s.apply(mk_candle(ts + chrono::Duration::minutes(1), 101.0));
        assert_eq!(s.len(), 2);
    }

    #[test]
    fn apply_on_empty_series_pushes() {
        let cal = xnys();
        let mut s = CandleSeries::new(cal.id(), BarPeriod::m1(), Symbol::new("SPY", cal.id()));
        let ts = utc(2024, 1, 17, 15, 0);
        s.apply(mk_candle(ts, 100.0));
        assert_eq!(s.len(), 1);
        assert_eq!(s.version(), 1);
    }

    #[test]
    fn column_invariant_holds_after_many_pushes() {
        let cal = xnys();
        let mut s = CandleSeries::new(cal.id(), BarPeriod::m1(), Symbol::new("SPY", cal.id()));
        let start = utc(2024, 1, 17, 14, 30); // 09:30 ET
        for i in 0..100 {
            let ts = start + chrono::Duration::minutes(i);
            s.push(mk_candle(ts, 100.0 + i as f64 * 0.01));
        }
        assert_eq!(s.len(), 100);
        // The invariant is enforced by debug_check_invariant on every
        // push — getting here means it held.
    }

    #[test]
    fn at_bounds_check() {
        let cal = xnys();
        let s = CandleSeries::new(cal.id(), BarPeriod::m1(), Symbol::new("SPY", cal.id()));
        assert!(s.at(0).is_none());
    }

    #[test]
    fn iter_yields_in_order() {
        let cal = xnys();
        let mut s = CandleSeries::new(cal.id(), BarPeriod::m1(), Symbol::new("SPY", cal.id()));
        let start = utc(2024, 1, 17, 14, 30);
        for i in 0..5 {
            let ts = start + chrono::Duration::minutes(i);
            s.push(mk_candle(ts, 100.0 + i as f64));
        }
        let opens: Vec<f64> = s.iter().map(|r| r.open()).collect();
        assert_eq!(opens.len(), 5);
        for w in opens.windows(2) {
            assert!(w[1] > w[0]);
        }
    }

    #[test]
    fn candle_ref_reconstructs_session_xnys_regular() {
        let cal = xnys();
        let mut s = CandleSeries::new(cal.id(), BarPeriod::m1(), Symbol::new("SPY", cal.id()));
        // 14:30 UTC = 09:30 ET = Regular open.
        let ts = utc(2024, 1, 17, 14, 30);
        s.push(mk_candle(ts, 100.0));
        let r = s.at(0).unwrap();
        assert_eq!(r.session_kind(), SessionKind::Regular);
        assert_eq!(r.session(cal).kind(), SessionKind::Regular);
    }

    #[test]
    fn candle_ref_reconstructs_session_xnys_pre_market() {
        let cal = xnys();
        let mut s = CandleSeries::new(cal.id(), BarPeriod::m1(), Symbol::new("SPY", cal.id()));
        // 13:00 UTC = 08:00 ET = PreMarket.
        let ts = utc(2024, 1, 17, 13, 0);
        s.push(mk_candle(ts, 100.0));
        let r = s.at(0).unwrap();
        assert_eq!(r.session_kind(), SessionKind::PreMarket);
        assert_eq!(r.session(cal).kind(), SessionKind::PreMarket);
    }

    #[test]
    fn candle_ref_ts_close_matches_bar_window() {
        let cal = xnys();
        let mut s = CandleSeries::new(cal.id(), BarPeriod::m1(), Symbol::new("SPY", cal.id()));
        let ts = utc(2024, 1, 17, 14, 30);
        s.push(mk_candle(ts, 100.0));
        let r = s.at(0).unwrap();
        let expected = cal.bar_window(ts, BarPeriod::m1()).unwrap().close;
        assert_eq!(r.ts_close(cal), expected);
    }

    #[test]
    fn regular_session_holds_390_m1_bars() {
        // 09:30 ET → 16:00 ET = 390 minutes = 390 m1 bars.
        let cal = xnys();
        let mut s = CandleSeries::new(cal.id(), BarPeriod::m1(), Symbol::new("SPY", cal.id()));
        let start = utc(2024, 1, 17, 14, 30); // 09:30 ET
        for i in 0..390 {
            let ts = start + chrono::Duration::minutes(i);
            s.push(mk_candle(ts, 100.0 + (i as f64) * 0.001));
        }
        assert_eq!(s.len(), 390);
        assert_eq!(s.version(), 390);

        // Every row should be Regular.
        let all_regular = s.iter().all(|r| r.session_kind() == SessionKind::Regular);
        assert!(all_regular);

        // First bar opens at 09:30 ET, last opens at 15:59 ET.
        let first = s.at(0).unwrap().ts_open();
        let last = s.at(389).unwrap().ts_open();
        assert_eq!(first, start);
        assert_eq!(last, start + chrono::Duration::minutes(389));
    }

    #[test]
    fn accessors_match_new() {
        let cal = xnys();
        let sym = Symbol::new("AAPL", cal.id());
        let s = CandleSeries::new(cal.id(), BarPeriod::m5(), sym);
        assert_eq!(s.calendar(), cal.id());
        assert_eq!(s.period(), BarPeriod::m5());
        assert_eq!(s.symbol(), sym);
    }

    /// Regression: bug-hunt H5. `trade_count` and `wap` must round-trip
    /// through the series — prior impl dropped both.
    #[test]
    fn trade_count_and_wap_round_trip() {
        let cal = xnys();
        let mut s = CandleSeries::new(cal.id(), BarPeriod::m1(), Symbol::new("SPY", cal.id()));
        let ts = utc(2024, 1, 17, 15, 0);
        let session = cal.classify(ts);
        let window = cal.bar_window(ts, BarPeriod::m1()).unwrap();
        let ohlcv = Ohlcv::new(100.0, 101.0, 99.0, 100.5, 5_000, 42, Some(100.25)).unwrap();
        let c = Candle::new(
            Symbol::new("SPY", cal.id()),
            cal,
            BarPeriod::m1(),
            session,
            window,
            ohlcv,
            Completeness::Completed,
        )
        .unwrap();
        s.push(c);
        let r = s.at(0).unwrap();
        assert_eq!(r.trade_count(), 42);
        assert_eq!(r.wap(), Some(100.25));
    }

    #[test]
    fn wap_none_round_trips_as_none() {
        let cal = xnys();
        let mut s = CandleSeries::new(cal.id(), BarPeriod::m1(), Symbol::new("SPY", cal.id()));
        let ts = utc(2024, 1, 17, 15, 0);
        s.push(mk_candle(ts, 100.0)); // mk_candle builds None wap
        let r = s.at(0).unwrap();
        assert_eq!(r.wap(), None);
        assert_eq!(r.trade_count(), 1);
    }

    /// Regression: app-harden H1. Rolling-cap `max_rows` drops oldest
    /// rows in lockstep once the cap is exceeded. All columns must
    /// stay equal-length (SoA invariant enforced by
    /// `debug_check_invariant`).
    #[test]
    fn rolling_cap_drops_oldest() {
        let cal = xnys();
        let mut s =
            CandleSeries::new_with_cap(cal.id(), BarPeriod::m1(), Symbol::new("SPY", cal.id()), 50);
        assert_eq!(s.max_rows(), Some(50));
        let start = utc(2024, 1, 17, 14, 30); // 09:30 ET
        for i in 0..100 {
            let ts = start + chrono::Duration::minutes(i);
            s.push(mk_candle(ts, 100.0 + i as f64 * 0.01));
        }
        // After 100 pushes with cap=50, expect len exactly 50.
        assert_eq!(s.len(), 50);
        // First remaining row should be bar 50 (0-indexed), i.e. open at
        // start + 50 minutes.
        let first = s.at(0).unwrap().ts_open();
        assert_eq!(first, start + chrono::Duration::minutes(50));
        // Last remaining row should be bar 99.
        let last = s.at(49).unwrap().ts_open();
        assert_eq!(last, start + chrono::Duration::minutes(99));
    }

    #[test]
    fn rolling_cap_keeps_columns_in_lockstep() {
        let cal = xnys();
        let mut s =
            CandleSeries::new_with_cap(cal.id(), BarPeriod::m1(), Symbol::new("SPY", cal.id()), 10);
        let start = utc(2024, 1, 17, 14, 30);
        for i in 0..25 {
            let ts = start + chrono::Duration::minutes(i);
            s.push(mk_candle(ts, 100.0 + i as f64));
        }
        assert_eq!(s.len(), 10);
        // `debug_check_invariant` runs on every push; reaching this
        // line means every column drained in lockstep. Snapshot-equal
        // them explicitly as belt-and-braces.
        assert_eq!(s.timestamps.len(), 10);
        assert_eq!(s.opens.len(), 10);
        assert_eq!(s.highs.len(), 10);
        assert_eq!(s.lows.len(), 10);
        assert_eq!(s.closes.len(), 10);
        assert_eq!(s.volumes.len(), 10);
        assert_eq!(s.trade_counts.len(), 10);
        assert_eq!(s.waps.len(), 10);
        assert_eq!(s.sessions.len(), 10);
        assert_eq!(s.completeness.len(), 10);
    }

    #[test]
    fn uncapped_default_still_grows_unbounded() {
        let cal = xnys();
        let mut s = CandleSeries::new(cal.id(), BarPeriod::m1(), Symbol::new("SPY", cal.id()));
        assert_eq!(s.max_rows(), None);
        let start = utc(2024, 1, 17, 14, 30);
        for i in 0..200 {
            let ts = start + chrono::Duration::minutes(i);
            s.push(mk_candle(ts, 100.0));
        }
        assert_eq!(s.len(), 200);
    }

    #[test]
    #[should_panic(expected = "max_rows must be > 0")]
    fn new_with_cap_rejects_zero() {
        let cal = xnys();
        let _ =
            CandleSeries::new_with_cap(cal.id(), BarPeriod::m1(), Symbol::new("SPY", cal.id()), 0);
    }

    #[test]
    fn apply_updates_high_on_same_open_ts() {
        let cal = xnys();
        let mut s = CandleSeries::new(cal.id(), BarPeriod::m1(), Symbol::new("SPY", cal.id()));
        let ts = utc(2024, 1, 17, 15, 0);
        s.push(mk_candle(ts, 100.0));
        // Manually construct a fresh partial bar with a higher 'high' at
        // the same ts_open.
        let session = cal.classify(ts);
        let window = cal.bar_window(ts, BarPeriod::m1()).unwrap();
        let ohlcv = Ohlcv::new(100.0, 101.5, 99.9, 101.2, 250, 5, None).unwrap();
        let partial = Candle::new(
            Symbol::new("SPY", cal.id()),
            cal,
            BarPeriod::m1(),
            session,
            window,
            ohlcv,
            Completeness::Partial,
        )
        .unwrap();
        s.apply(partial);
        assert_eq!(s.len(), 1);
        let r = s.at(0).unwrap();
        assert!((r.high() - 101.5).abs() < 1e-3);
        assert_eq!(r.completeness(), Completeness::Partial);
    }
}
