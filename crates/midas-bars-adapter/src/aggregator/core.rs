//! Core session-aware tick → candle aggregator state machine.
//!
//! The aggregator folds trade ticks into a single in-progress bar whose
//! window is resolved by the calendar. When a tick arrives in a
//! different window (clock boundary crossed, session changed, early
//! close fired), the aggregator emits a `Rollover` with the closed bar
//! (marked `Completed`) and the freshly-opened bar (marked `Partial`).
//!
//! R2-G-8 (early-close + Clock(H1)) is honoured by trusting the
//! calendar: `XnysCalendar::bar_window` returns a `BarWindow` whose
//! `session` is re-classified at `ts`. The moment a post-close tick
//! arrives (or a new-day pre-market tick), its `BarWindow` differs from
//! the current one (either on session or on UTC alignment) and the
//! state machine rolls over naturally. The aggregator never hand-rolls
//! session arithmetic.
//!
//! ## Trade ticks only
//!
//! MVP accepts only `TickKind::PriceSize` and `TickKind::Price` with
//! `TickType::Last`. Bid/Ask quote ticks are ignored. `Price+Last` has
//! no size; it contributes to OHLC and `trade_count` but not `volume`
//! or VWAP weighting.

use std::time::Duration;

use midas_bars::{Candle, Completeness, Ohlcv, Symbol};
use midas_broker_core::market_data::{Tick, TickKind, TickType, TickValue};
use midas_calendar::{BarPeriod, BarWindow, CalendarError, ExchangeCalendar, Timestamp};

use super::config::AggregatorConfig;

/// Outcome of feeding a tick to [`SessionedBarAggregator::accept_tick`].
///
/// The variants are ordered by "emit weight" ascending:
/// - `Ignored` — tick doesn't match the aggregator's filter or sits
///   outside calendar coverage. No state change, no emit.
/// - `Folded` — tick was accepted and merged into the current bar, but
///   the partial-emit rate-limit suppressed the update. Caller has no
///   candle to publish.
/// - `Partial` — tick merged and a rate-limited partial update fires.
/// - `Opened` — first-ever tick opened a fresh bar.
/// - `Closed` — window boundary forced the current bar to close; no
///   new bar opens this call (e.g. a `flush_if_due` wall-time flush).
/// - `Rollover` — window boundary crossed in a single `accept_tick`
///   call; the previous bar closes and a new one opens off the same
///   tick in one atomic output.
#[derive(Debug, Clone)]
pub enum AggregatorOutput {
    /// Tick rejected (non-trade kind, out-of-coverage, etc.).
    Ignored,
    /// Tick merged; emit suppressed by the partial-emit rate limit.
    Folded,
    /// Tick merged; emit a partial update for the current bar.
    Partial(Candle),
    /// First tick opened a fresh bar; emit its initial partial snapshot.
    Opened(Candle),
    /// Current bar closed (no new bar opened in this call).
    Closed(Candle),
    /// Current bar closed AND a new bar opened in a single call.
    ///
    /// Both candles are boxed — `Candle` is ~288 bytes, and rollovers
    /// are rare compared to folds/partials. Boxing keeps the enum's
    /// stack footprint small on the hot path without materially
    /// affecting boundary-crossing cost.
    Rollover {
        /// Previous bar, stamped `Completeness::Completed`.
        closed: Box<Candle>,
        /// Newly opened bar, stamped `Completeness::Partial` with the
        /// arriving tick as its sole fold.
        opened: Box<Candle>,
    },
}

/// Error surface for the aggregator.
#[derive(Debug, thiserror::Error)]
pub enum AggregatorError {
    /// `ExchangeCalendar::validate_period` rejected the (calendar,
    /// period) pairing at construction time.
    #[error("invalid period for calendar: {0}")]
    InvalidPeriod(#[from] CalendarError),
}

/// Mutable state for the bar currently being accumulated. Not public —
/// every read goes through the aggregator's inspection methods so
/// external callers can't skip the invariants (window ownership, VWAP
/// reconciliation).
#[derive(Debug, Clone)]
pub(super) struct InProgressBar {
    pub window: BarWindow,
    pub o: f64,
    pub h: f64,
    pub l: f64,
    pub c: f64,
    pub volume: u64,
    pub trade_count: u32,
    /// Σ price·size (VWAP numerator).
    pub vwap_num: f64,
    /// Σ size (VWAP denominator). Zero when only `Price+Last` ticks have
    /// been folded (no size-bearing trades); in that case `wap` is
    /// emitted as `None`.
    pub vwap_den: u64,
    /// Timestamp of the first folded tick. Retained for future
    /// observability (lag tracking) and so `snapshot_current_partial`
    /// can expose it via inspection methods.
    #[allow(dead_code)]
    pub first_tick_ts: Timestamp,
}

impl InProgressBar {
    fn wap(&self) -> Option<f64> {
        if self.vwap_den == 0 {
            None
        } else {
            Some(self.vwap_num / self.vwap_den as f64)
        }
    }

    /// Emit a `Candle` view of the in-progress bar with the given
    /// completeness. `Ohlcv::new` may clamp `wap` if it drifts outside
    /// `[l, h]` due to FP rounding on long-running accumulations — we
    /// defensively drop the wap in that case rather than fail the whole
    /// emit.
    fn to_candle(
        &self,
        symbol: Symbol,
        calendar: &'static dyn ExchangeCalendar,
        period: BarPeriod,
        completeness: Completeness,
    ) -> Candle {
        let raw_wap = self.wap();
        let wap = match raw_wap {
            Some(w) if w.is_finite() && w >= self.l && w <= self.h => Some(w),
            _ => None,
        };
        let ohlcv = Ohlcv::new(
            self.o,
            self.h,
            self.l,
            self.c,
            self.volume,
            self.trade_count,
            wap,
        )
        .expect("in-progress OHLCV is maintained valid by fold()");
        Candle::new(
            symbol,
            calendar,
            period,
            self.window.session.clone(),
            self.window.clone(),
            ohlcv,
            completeness,
        )
        .expect("window session derived from the calendar; candle invariants must hold")
    }
}

/// Session-aware tick → candle aggregator.
///
/// Single-consumer: owns its current in-progress bar and rate-limit
/// state. Wrap in [`SessionedBarStream`](super::stream::SessionedBarStream)
/// to expose as a `BarStream<Candle>`.
pub struct SessionedBarAggregator {
    config: AggregatorConfig,
    current: Option<InProgressBar>,
    last_partial_emit: Option<std::time::Instant>,
}

impl std::fmt::Debug for SessionedBarAggregator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionedBarAggregator")
            .field("config", &self.config)
            .field("has_current", &self.current.is_some())
            .finish()
    }
}

impl SessionedBarAggregator {
    /// Build a new aggregator. Validates the `(calendar, period)` pair
    /// via `ExchangeCalendar::validate_period` — fails fast on the
    /// nonsensical combinations the calendar rejects (e.g.
    /// `(CryptoSpot, Session(Eth))`).
    pub fn new(config: AggregatorConfig) -> Result<Self, AggregatorError> {
        config.calendar.validate_period(config.period)?;
        Ok(Self {
            config,
            current: None,
            last_partial_emit: None,
        })
    }

    /// Borrow the config (read-only).
    pub fn config(&self) -> &AggregatorConfig {
        &self.config
    }

    /// Convenience access to the configured symbol.
    #[inline]
    pub fn symbol(&self) -> Symbol {
        self.config.symbol
    }

    /// Convenience access to the configured calendar.
    #[inline]
    pub fn calendar(&self) -> &'static dyn ExchangeCalendar {
        self.config.calendar
    }

    /// Convenience access to the configured period.
    #[inline]
    pub fn period(&self) -> BarPeriod {
        self.config.period
    }

    /// Returns `true` when a bar is currently in-progress.
    pub fn has_current(&self) -> bool {
        self.current.is_some()
    }

    /// Borrow the current in-progress `BarWindow`, if any.
    pub fn current_window(&self) -> Option<&BarWindow> {
        self.current.as_ref().map(|c| &c.window)
    }

    /// Non-consuming snapshot: build a `Candle` view of the in-progress
    /// bar with `Completeness::Partial`. Returns `None` before the first
    /// tick or after a `Closed` (pre-reopen).
    pub fn snapshot_current_partial(&self) -> Option<Candle> {
        self.current.as_ref().map(|c| {
            c.to_candle(
                self.config.symbol,
                self.config.calendar,
                self.config.period,
                Completeness::Partial,
            )
        })
    }

    /// Feed a tick. See [`AggregatorOutput`] for the semantics.
    pub fn accept_tick(&mut self, tick: &Tick) -> AggregatorOutput {
        let (price, size) = match extract_trade(tick) {
            Some(parts) => parts,
            None => return AggregatorOutput::Ignored,
        };
        let ts = tick.ts;

        let window = match self.config.calendar.bar_window(ts, self.config.period) {
            Ok(w) => w,
            Err(_) => {
                // Out-of-coverage / unsupported period — treat as ignored
                // so transient coverage edges don't crash the stream.
                return AggregatorOutput::Ignored;
            }
        };

        match self.current.as_mut() {
            None => {
                // First tick overall (or reopening after `Closed` flush).
                let inp = new_bar(window, price, size, ts);
                self.current = Some(inp);
                let candle = self.current_candle(Completeness::Partial);
                self.last_partial_emit = Some(self.config.clock.now_monotonic());
                AggregatorOutput::Opened(candle)
            }
            Some(inp) if inp.window == window => {
                fold(inp, price, size);
                if self.should_emit_partial() {
                    self.last_partial_emit = Some(self.config.clock.now_monotonic());
                    AggregatorOutput::Partial(self.current_candle(Completeness::Partial))
                } else {
                    AggregatorOutput::Folded
                }
            }
            Some(_) => {
                // Rollover: close the previous bar, then open a new one
                // seeded with the arriving tick.
                let closed = self.close_current_as_completed();
                let inp = new_bar(window, price, size, ts);
                self.current = Some(inp);
                let opened = self.current_candle(Completeness::Partial);
                self.last_partial_emit = Some(self.config.clock.now_monotonic());
                AggregatorOutput::Rollover {
                    closed: Box::new(closed),
                    opened: Box::new(opened),
                }
            }
        }
    }

    /// Wall-time flush: if the current bar's window has already closed
    /// (per `config.clock.now()`), stamp it `Completed` and return it.
    /// Intended to be called on a low-frequency heartbeat (e.g. 100ms)
    /// so session ends don't wait for a post-close tick that may never
    /// arrive.
    pub fn flush_if_due(&mut self) -> Option<Candle> {
        let now = self.config.clock.now();
        let due = self
            .current
            .as_ref()
            .map(|c| now >= c.window.close)
            .unwrap_or(false);
        if due {
            Some(self.close_current_as_completed())
        } else {
            None
        }
    }

    // -- internals --

    fn should_emit_partial(&self) -> bool {
        let hz = self.config.partial_emit_rate_hz;
        if hz == 0 {
            return true;
        }
        let interval = Duration::from_nanos(1_000_000_000u64 / u64::from(hz));
        match self.last_partial_emit {
            None => true,
            Some(last) => {
                let now = self.config.clock.now_monotonic();
                now.saturating_duration_since(last) >= interval
            }
        }
    }

    fn current_candle(&self, completeness: Completeness) -> Candle {
        self.current
            .as_ref()
            .expect("current_candle called with no bar in progress")
            .to_candle(
                self.config.symbol,
                self.config.calendar,
                self.config.period,
                completeness,
            )
    }

    fn close_current_as_completed(&mut self) -> Candle {
        let inp = self
            .current
            .take()
            .expect("close_current_as_completed called with no bar in progress");
        // Reset rate limit so a subsequent Opened emits its first
        // Partial immediately rather than being suppressed.
        self.last_partial_emit = None;
        inp.to_candle(
            self.config.symbol,
            self.config.calendar,
            self.config.period,
            Completeness::Completed,
        )
    }
}

// ---------------------------------------------------------------------------
// Free helpers
// ---------------------------------------------------------------------------

/// Extract `(price, size_u64)` from a trade tick, or `None` if the tick
/// is not a trade we accept.
///
/// Accepted shapes (IB `reqTickByTickData` + `reqMktData::Last`):
/// - `kind == PriceSize`, `tick_type == Last`, `value == PriceSize { price, size }` —
///   atomic tick-by-tick trade; uses the carried size.
/// - `kind == Price`, `tick_type == Last`, `value == Price(p)` — sampled
///   last trade; size unknown, folded with size=0. OHLC and trade_count
///   still update; volume does not.
///
/// Bug-hunt H2: the previous wildcard `(PriceSize, _, PriceSize)` arm
/// folded BidAsk tick-by-tick quotes (tick_type = Bid / Ask, atomic
/// price+size) as trades. Quote ticks must never move OHLC. Restrict
/// the PriceSize arm to `tick_type == Last` explicitly. `TickType` is
/// `#[non_exhaustive]`; if IB later ships an `AllLast` (reqTickByTickData
/// "AllLast" mode) variant, add it as a peer arm.
///
/// Finite / sanity filter: reject NaN / infinite prices and negative
/// sizes up front so they never propagate into OHLC.
fn extract_trade(tick: &Tick) -> Option<(f64, u64)> {
    // Guard prices and sizes before anything enters OHLC.
    let sane_price_size = |price: f64, size: i64| -> Option<(f64, u64)> {
        if !price.is_finite() {
            return None;
        }
        if size < 0 {
            // Clamp the IB "size unknown" sentinel to zero rather than
            // reject the tick — matches the historical accepted shape.
            return Some((price, 0));
        }
        // Guard against absurd sizes — a single tick with >= 2^62 shares
        // is synthetic / corrupt and would overflow the u64 volume
        // accumulator after a handful of folds.
        if size > i64::MAX / 2 {
            return None;
        }
        Some((price, size as u64))
    };

    match (tick.kind, tick.tick_type, &tick.value) {
        (TickKind::PriceSize, TickType::Last, TickValue::PriceSize { price, size }) => {
            sane_price_size(*price, *size)
        }
        (TickKind::Price, TickType::Last, TickValue::Price(p)) => {
            if p.is_finite() {
                Some((*p, 0))
            } else {
                None
            }
        }
        _ => None,
    }
}

fn new_bar(window: BarWindow, price: f64, size: u64, ts: Timestamp) -> InProgressBar {
    let (vwap_num, vwap_den) = if size == 0 {
        (0.0, 0)
    } else {
        (price * size as f64, size)
    };
    InProgressBar {
        window,
        o: price,
        h: price,
        l: price,
        c: price,
        volume: size,
        trade_count: 1,
        vwap_num,
        vwap_den,
        first_tick_ts: ts,
    }
}

fn fold(inp: &mut InProgressBar, price: f64, size: u64) {
    if price > inp.h {
        inp.h = price;
    }
    if price < inp.l {
        inp.l = price;
    }
    inp.c = price;
    inp.trade_count = inp.trade_count.saturating_add(1);
    if size > 0 {
        inp.volume = inp.volume.saturating_add(size);
        inp.vwap_num += price * size as f64;
        inp.vwap_den = inp.vwap_den.saturating_add(size);
    }
}

// Moved-out test module. See `core_tests.rs` — kept in a sibling file
// per the R6 refactor so this production file stays under ~500 LOC.
#[cfg(test)]
#[path = "core_tests.rs"]
mod core_tests;
