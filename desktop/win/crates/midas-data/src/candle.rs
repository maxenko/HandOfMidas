//! Re-export candle types from `midas-core`.
//!
//! `CandleBuffer` and `CandleSlice` now live in `midas-core` so that the
//! `DataProvider` trait can reference `CandleBuffer` without circular
//! dependencies. This module re-exports them for backward compatibility.

pub use midas_core::{CandleBuffer, CandleSlice};
