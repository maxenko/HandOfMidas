//! Construction-time configuration for [`SessionedBarAggregator`].
//!
//! Groups the immutable inputs the aggregator takes at construction
//! (symbol, calendar, period, clock, partial-emit rate-limit) so the
//! type-signature of [`SessionedBarAggregator::new`] stays flat even as
//! new knobs land.
//!
//! [`SessionedBarAggregator`]: super::core::SessionedBarAggregator

use std::sync::Arc;

use midas_bars::Symbol;
use midas_calendar::{BarPeriod, ExchangeCalendar};
use midas_clock::Clock;

/// Default ceiling on `Partial` emits per wall-clock second when ticks
/// arrive faster than the consumer needs. 4 Hz ≈ one partial every 250ms
/// which comfortably stays under a 60 Hz render budget.
pub const DEFAULT_PARTIAL_EMIT_RATE_HZ: u32 = 4;

/// Construction-time config for [`SessionedBarAggregator`].
///
/// [`SessionedBarAggregator`]: super::core::SessionedBarAggregator
pub struct AggregatorConfig {
    /// Symbol this aggregator folds ticks for.
    pub symbol: Symbol,
    /// Calendar used for every `bar_window` / session lookup. Pinned at
    /// construction; the hot path never re-resolves.
    pub calendar: &'static dyn ExchangeCalendar,
    /// Target period for emitted candles.
    pub period: BarPeriod,
    /// Clock used for partial-emit rate-limiting and `flush_if_due`
    /// wall-time checks. `Arc<dyn Clock>` so tests can inject a
    /// [`MockClock`](midas_clock::MockClock).
    pub clock: Arc<dyn Clock>,
    /// Maximum `Partial` emits per wall-clock second. Folded ticks that
    /// arrive more frequently than this coalesce into a single emit.
    /// Completed bars (session rollover / window crossover) always emit
    /// immediately regardless of the rate limit.
    pub partial_emit_rate_hz: u32,
}

impl AggregatorConfig {
    /// Build a config with `partial_emit_rate_hz` defaulted to
    /// [`DEFAULT_PARTIAL_EMIT_RATE_HZ`].
    pub fn new(
        symbol: Symbol,
        calendar: &'static dyn ExchangeCalendar,
        period: BarPeriod,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            symbol,
            calendar,
            period,
            clock,
            partial_emit_rate_hz: DEFAULT_PARTIAL_EMIT_RATE_HZ,
        }
    }

    /// Override the partial-emit rate limit. Zero disables rate-limiting
    /// entirely (every folded tick emits a Partial).
    pub fn with_partial_emit_rate_hz(mut self, hz: u32) -> Self {
        self.partial_emit_rate_hz = hz;
        self
    }
}

impl std::fmt::Debug for AggregatorConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AggregatorConfig")
            .field("symbol", &self.symbol)
            .field("calendar", &self.calendar.id())
            .field("period", &self.period)
            .field("partial_emit_rate_hz", &self.partial_emit_rate_hz)
            .finish()
    }
}
