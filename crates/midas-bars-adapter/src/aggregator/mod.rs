//! Session-aware tick → `BarStream<Candle>` aggregator.
//!
//! Slice S7 of `plan/session-aware-charts/00b-integration-strategy.md`.
//!
//! This module bridges the legacy `MarketDataSource::subscribe_ticks`
//! surface into the ideal-design `BarStream<Candle>` world. It folds
//! trade ticks into in-progress bars whose windows are resolved through
//! the calendar — session boundaries, early-close truncations
//! (R2-G-8), and clock-interval rollovers all flow through
//! `ExchangeCalendar::bar_window` without hand-rolled arithmetic.
//!
//! See:
//! - [`SessionedBarAggregator`] — single-consumer state machine
//!   (`accept_tick`, `flush_if_due`, `snapshot_current_partial`).
//! - [`SessionedBarStream`] — [`BarStream`](midas_stream::BarStream)
//!   adapter driven by an mpsc tick feed.
//! - [`subscribe_aggregated_bars`] — convenience wiring from
//!   `MarketDataSource` + `SymbolResolver` to a ready-to-drain stream.

mod config;
mod core;
mod stream;
mod subscribe;

pub use self::config::{AggregatorConfig, DEFAULT_PARTIAL_EMIT_RATE_HZ};
pub use self::core::{AggregatorError, AggregatorOutput, SessionedBarAggregator};
pub use self::stream::SessionedBarStream;
pub use self::subscribe::{subscribe_aggregated_bars, subscribe_aggregated_bars_with_timeout};
