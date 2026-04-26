//! The `CandleData` trait abstracts over candle data sources.
//!
//! It lives in `midas-core` (the leaf crate) so that `midas-chart` can program
//! against it without depending on `midas-data`'s concrete `CandleBuffer` type.
//!
//! This enables:
//! - **Sans-IO chart logic**: `midas-chart` accepts `&dyn CandleData` or
//!   generics bounded by `CandleData`, keeping it free of storage dependencies.
//! - **Testing**: test fixtures can implement `CandleData` with hard-coded data.
//! - **Future streaming**: a real-time adapter wrapping a ring buffer or database
//!   cursor can implement `CandleData` without converting to `CandleBuffer`.

use std::ops::Range;

use midas_bars::SessionKindByte;

/// Trait abstracting over candle data sources.
///
/// Implemented by `CandleBuffer` (in `midas-data`), and potentially by
/// streaming adapters, database cursors, or test fixtures.
///
/// # Object Safety
///
/// This trait is object-safe: all methods take `&self` and return sized types,
/// so it can be used as `&dyn CandleData`.
pub trait CandleData {
    /// Total number of candles in the data source.
    fn len(&self) -> usize;

    /// Whether the data source contains zero candles.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Epoch-millisecond timestamp of the candle at `idx`.
    fn timestamp(&self, idx: usize) -> i64;

    /// Opening price of the candle at `idx`.
    fn open(&self, idx: usize) -> f32;

    /// Highest price of the candle at `idx`.
    fn high(&self, idx: usize) -> f32;

    /// Lowest price of the candle at `idx`.
    fn low(&self, idx: usize) -> f32;

    /// Closing price of the candle at `idx`.
    fn close(&self, idx: usize) -> f32;

    /// Trade volume for the candle at `idx`.
    fn volume(&self, idx: usize) -> u32;

    /// Min and max prices (low, high) across the given index range.
    ///
    /// Returns `(min_low, max_high)` for all candles in `range`.
    fn price_range(&self, range: Range<usize>) -> (f32, f32);

    /// Find the index of the candle whose timestamp is closest to `ts`
    /// (epoch milliseconds).
    ///
    /// If `ts` is before all data, returns 0.
    /// If `ts` is after all data, returns `len() - 1` (or 0 if empty).
    fn find_index_by_time(&self, ts: i64) -> usize;

    /// Trading session kind for the candle at `idx`.
    ///
    /// Default returns [`SessionKindByte::Regular`] so existing
    /// implementations stay compiling — only stores that classify bars
    /// (legacy `CandleBuffer`, session-aware `CandleSeries`) need to
    /// override. Routed through `midas-bars`' re-export so this trait
    /// stays free of a `midas-calendar` production dep.
    fn session_kind(&self, _idx: usize) -> SessionKindByte {
        SessionKindByte::Regular
    }
}

#[cfg(test)]
mod tests;
