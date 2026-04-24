//! Cross-workspace `impl CandleData for CandleSeries`.
//!
//! Per design decision D7 of the chart-transition plan, the
//! `midas-indicators` compute path (ATR + Gerchik ATR) consumes the
//! `CandleData` trait, not concrete `CandleBuffer`. Making the same
//! compute work against the session-aware [`midas_bars::CandleSeries`]
//! reduces to a single trait impl — no indicator rewrite.
//!
//! ## Orphan rule
//!
//! The impl lives in `midas-core` (owner of [`CandleData`]), against
//! a foreign type ([`midas_bars::CandleSeries`], defined in the
//! root-workspace `midas-bars` crate). This is permitted because one
//! of the two (the trait) is local. The adapter crate
//! (`midas-bars-adapter`) would NOT be allowed to carry this impl —
//! neither the trait nor the type is local there.
//!
//! ## Conversion notes
//!
//! [`CandleSeries`] stores timestamps as UTC nanos (`i64`), whereas
//! [`CandleData::timestamp`] returns epoch milliseconds. The adapter
//! divides by `1_000_000` (ns → ms). All floating-point columns
//! match ([`f32`] on both sides); conversions are a plain field copy.
//!
//! ## `find_index_by_time`
//!
//! [`CandleSeries`] does not expose a search helper; this adapter
//! walks the series linearly to find the closest bar by timestamp.
//! Complexity is `O(n)` per call — acceptable because indicator
//! compute already scans the full series (not called from the hot
//! paint path). A future optimisation could expose a binary-search
//! helper on `CandleSeries` if profiling demands it.

use std::ops::Range;

use midas_bars::CandleSeries;

use crate::candle_data::CandleData;

/// Nanoseconds per millisecond.
const NS_PER_MS: i64 = 1_000_000;

impl CandleData for CandleSeries {
    fn len(&self) -> usize {
        self.len()
    }

    fn timestamp(&self, idx: usize) -> i64 {
        // Series stores UTC nanos; `CandleData` contract returns
        // epoch-millis. Division truncates toward zero for positive
        // timestamps, matching the legacy `CandleBuffer` conversion
        // path where `.timestamp_millis()` was used.
        let ts = self
            .at(idx)
            .expect("timestamp: idx out of bounds")
            .ts_open();
        ts.timestamp_millis()
    }

    fn open(&self, idx: usize) -> f32 {
        self.at(idx).expect("open: idx out of bounds").open() as f32
    }

    fn high(&self, idx: usize) -> f32 {
        self.at(idx).expect("high: idx out of bounds").high() as f32
    }

    fn low(&self, idx: usize) -> f32 {
        self.at(idx).expect("low: idx out of bounds").low() as f32
    }

    fn close(&self, idx: usize) -> f32 {
        self.at(idx).expect("close: idx out of bounds").close() as f32
    }

    fn volume(&self, idx: usize) -> u32 {
        let v = self.at(idx).expect("volume: idx out of bounds").volume();
        u32::try_from(v).unwrap_or(u32::MAX)
    }

    fn price_range(&self, range: Range<usize>) -> (f32, f32) {
        // Mirror `CandleBuffer::price_range`: panics on empty or
        // out-of-bounds range. Callers on the hot path already clamp
        // via `visible_range`.
        assert!(!range.is_empty(), "price_range: empty range");
        assert!(
            range.end <= self.len(),
            "price_range: range.end={} > len={}",
            range.end,
            self.len()
        );

        let mut max_high = f32::MIN;
        let mut min_low = f32::MAX;
        for idx in range {
            let row = self.at(idx).expect("price_range: idx out of bounds");
            let h = row.high() as f32;
            let l = row.low() as f32;
            if h > max_high {
                max_high = h;
            }
            if l < min_low {
                min_low = l;
            }
        }
        (min_low, max_high)
    }

    fn find_index_by_time(&self, ts: i64) -> usize {
        // Linear scan for the bar whose ts_open (in ms) matches the
        // query most closely. `CandleBuffer` uses binary search (its
        // `timestamps` field is directly available); `CandleSeries`
        // does not expose the underlying column, so we scan. The
        // method is not called from the paint hot path.
        if self.is_empty() {
            return 0;
        }
        // Binary-search-equivalent: find the first index whose
        // timestamp is >= the query. Walk in order; break as soon as
        // we pass it.
        let target_ns = ts.saturating_mul(NS_PER_MS);
        let len = self.len();
        let mut idx = 0;
        for i in 0..len {
            let row_ns = self
                .at(i)
                .expect("find_index_by_time: bounded loop")
                .ts_open()
                .timestamp_nanos_opt()
                .expect("timestamp within chrono nanos range");
            if row_ns >= target_ns {
                idx = i;
                return idx;
            }
            idx = i;
        }
        idx.min(len - 1)
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use midas_bars::{Candle, CandleSeries, Completeness, Ohlcv, Symbol};
    use midas_calendar::{xnys, BarPeriod, Timestamp};

    use super::*;
    use crate::candle_buffer::CandleBuffer;

    fn utc(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> Timestamp {
        chrono::Utc.with_ymd_and_hms(y, mo, d, h, mi, 0).unwrap()
    }

    fn mk_candle(ts: Timestamp, o: f64, h: f64, l: f64, c: f64, v: u64) -> Candle {
        let cal = xnys();
        let sym = Symbol::new("SPY", cal.id());
        let session = cal.classify(ts);
        let window = cal.bar_window(ts, BarPeriod::m1()).unwrap();
        let ohlcv = Ohlcv::new(o, h, l, c, v, 1, None).unwrap();
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

    fn seed_series_and_buffer() -> (CandleSeries, CandleBuffer) {
        // Seed both stores with the same 10 bars so the round-trip
        // tests can compare `CandleData` outputs side-by-side.
        let cal = xnys();
        let mut series = CandleSeries::new(cal.id(), BarPeriod::m1(), Symbol::new("SPY", cal.id()));
        let mut buffer = CandleBuffer::new();
        let start = utc(2024, 1, 17, 14, 30);
        for i in 0..10 {
            let ts = start + chrono::Duration::minutes(i as i64);
            let o = 100.0 + (i as f64) * 0.1;
            let h = o + 0.5;
            let l = o - 0.4;
            let c = o + 0.2;
            let v = 1_000 + (i as u64) * 100;
            series.push(mk_candle(ts, o, h, l, c, v));
            buffer.push(
                ts.timestamp_millis(),
                o as f32,
                h as f32,
                l as f32,
                c as f32,
                v as u32,
            );
        }
        (series, buffer)
    }

    #[test]
    fn len_matches_between_series_and_buffer() {
        let (series, buffer) = seed_series_and_buffer();
        assert_eq!(CandleData::len(&series), CandleData::len(&buffer));
    }

    #[test]
    fn timestamp_matches_between_series_and_buffer() {
        let (series, buffer) = seed_series_and_buffer();
        for i in 0..10 {
            assert_eq!(
                CandleData::timestamp(&series, i),
                CandleData::timestamp(&buffer, i),
                "timestamp mismatch at i={i}",
            );
        }
    }

    #[test]
    fn open_high_low_close_volume_match() {
        let (series, buffer) = seed_series_and_buffer();
        for i in 0..10 {
            assert_eq!(CandleData::open(&series, i), CandleData::open(&buffer, i));
            assert_eq!(CandleData::high(&series, i), CandleData::high(&buffer, i));
            assert_eq!(CandleData::low(&series, i), CandleData::low(&buffer, i));
            assert_eq!(CandleData::close(&series, i), CandleData::close(&buffer, i));
            assert_eq!(
                CandleData::volume(&series, i),
                CandleData::volume(&buffer, i),
            );
        }
    }

    #[test]
    fn price_range_matches_between_series_and_buffer() {
        let (series, buffer) = seed_series_and_buffer();
        let got = CandleData::price_range(&series, 0..10);
        let expected = CandleData::price_range(&buffer, 0..10);
        assert_eq!(got, expected);
    }

    #[test]
    fn find_index_by_time_matches_between_series_and_buffer_at_boundaries() {
        let (series, buffer) = seed_series_and_buffer();
        // First bar's timestamp should return 0 on both stores.
        let first_ts = CandleData::timestamp(&series, 0);
        assert_eq!(
            CandleData::find_index_by_time(&series, first_ts),
            CandleData::find_index_by_time(&buffer, first_ts),
        );
        // Last bar's timestamp should return 9 on both.
        let last_ts = CandleData::timestamp(&series, 9);
        assert_eq!(
            CandleData::find_index_by_time(&series, last_ts),
            CandleData::find_index_by_time(&buffer, last_ts),
        );
    }

    #[test]
    fn candle_data_can_be_used_as_trait_object() {
        // Object-safety regression: the `CandleData` trait already
        // promises dyn-compatibility (see its doc-comment). Exercising
        // a `&dyn CandleData` path through `CandleSeries` confirms
        // every method lands on the vtable.
        let (series, _) = seed_series_and_buffer();
        let dynref: &dyn CandleData = &series;
        assert_eq!(dynref.len(), 10);
        assert!(dynref.open(0).is_finite());
        let (lo, hi) = dynref.price_range(0..10);
        assert!(lo < hi);
    }

    #[test]
    fn empty_series_reports_zero_len_and_find_index_zero() {
        let cal = xnys();
        let empty = CandleSeries::new(cal.id(), BarPeriod::m1(), Symbol::new("SPY", cal.id()));
        assert_eq!(CandleData::len(&empty), 0);
        assert!(CandleData::is_empty(&empty));
        // The `CandleData` contract says "If ts is before all data,
        // returns 0. If after all data, returns len-1 (or 0 if empty)".
        assert_eq!(CandleData::find_index_by_time(&empty, 0), 0);
    }
}
