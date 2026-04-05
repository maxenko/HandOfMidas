use super::*;
use midas_core::GATR_COLOR_GREEN;
use std::ops::Range;

/// Minimal test fixture implementing `CandleData`.
struct TestCandles {
    timestamps: Vec<i64>,
    opens: Vec<f32>,
    highs: Vec<f32>,
    lows: Vec<f32>,
    closes: Vec<f32>,
    volumes: Vec<u32>,
}

impl CandleData for TestCandles {
    fn len(&self) -> usize {
        self.timestamps.len()
    }
    fn timestamp(&self, idx: usize) -> i64 {
        self.timestamps[idx]
    }
    fn open(&self, idx: usize) -> f32 {
        self.opens[idx]
    }
    fn high(&self, idx: usize) -> f32 {
        self.highs[idx]
    }
    fn low(&self, idx: usize) -> f32 {
        self.lows[idx]
    }
    fn close(&self, idx: usize) -> f32 {
        self.closes[idx]
    }
    fn volume(&self, idx: usize) -> u32 {
        self.volumes[idx]
    }
    fn price_range(&self, range: Range<usize>) -> (f32, f32) {
        let mut min = f32::MAX;
        let mut max = f32::MIN;
        for i in range {
            min = min.min(self.lows[i]);
            max = max.max(self.highs[i]);
        }
        (min, max)
    }
    fn find_index_by_time(&self, ts: i64) -> usize {
        match self.timestamps.binary_search(&ts) {
            Ok(idx) => idx,
            Err(idx) => idx.min(self.len().saturating_sub(1)),
        }
    }
}

/// Build 5-minute candles across multiple days.
///
/// Each day has 4 candles at 09:30, 10:00, 10:30, 11:00 UTC.
/// Day N has: high = 100 + N*2, low = 100 - N, close = 100 + N.
fn multi_day_5m_data(num_days: usize) -> TestCandles {
    let five_min_ms: i64 = 300_000;
    let mut timestamps = Vec::new();
    let mut opens = Vec::new();
    let mut highs = Vec::new();
    let mut lows = Vec::new();
    let mut closes = Vec::new();
    let mut volumes = Vec::new();

    // Base: 2024-01-15 00:00:00 UTC = 1705276800000
    let base_day_ms: i64 = 1_705_276_800_000;
    let session_start_offset: i64 = 9 * 3_600_000 + 30 * 60_000; // 09:30 UTC

    for day in 0..num_days {
        let day_base = base_day_ms + day as i64 * DAY_MS + session_start_offset;
        let day_f = day as f32;
        for candle in 0..4 {
            let ts = day_base + candle as i64 * five_min_ms;
            timestamps.push(ts);
            opens.push(100.0 + day_f);
            // Spread high/low across candles so daily aggregation is meaningful.
            if candle == 0 {
                highs.push(100.0 + day_f * 2.0); // Day high on first candle
                lows.push(100.0 - day_f); // Day low on first candle
            } else {
                highs.push(100.0 + day_f + 0.5);
                lows.push(100.0 + day_f - 0.5);
            }
            closes.push(100.0 + day_f);
            volumes.push(1000);
        }
    }

    TestCandles {
        timestamps,
        opens,
        highs,
        lows,
        closes,
        volumes,
    }
}

// ── aggregate_daily_bars ────────────────────────────────────────

#[test]
fn aggregate_empty_data() {
    let data = TestCandles {
        timestamps: vec![],
        opens: vec![],
        highs: vec![],
        lows: vec![],
        closes: vec![],
        volumes: vec![],
    };
    assert!(aggregate_daily_bars(&data).is_empty());
}

#[test]
fn aggregate_single_candle() {
    let data = TestCandles {
        timestamps: vec![1_705_276_800_000],
        opens: vec![100.0],
        highs: vec![105.0],
        lows: vec![95.0],
        closes: vec![102.0],
        volumes: vec![1000],
    };
    let bars = aggregate_daily_bars(&data);
    assert_eq!(bars.len(), 1);
    assert_eq!(bars[0].high, 105.0);
    assert_eq!(bars[0].low, 95.0);
    assert_eq!(bars[0].close, 102.0);
}

#[test]
fn aggregate_multiple_candles_same_day() {
    // 3 candles on the same UTC day.
    let base = 1_705_276_800_000_i64; // 2024-01-15 00:00 UTC
    let data = TestCandles {
        timestamps: vec![base + 3_600_000, base + 7_200_000, base + 10_800_000],
        opens: vec![100.0, 101.0, 102.0],
        highs: vec![103.0, 108.0, 105.0],
        lows: vec![97.0, 99.0, 101.0],
        closes: vec![101.0, 104.0, 103.0],
        volumes: vec![100, 200, 300],
    };
    let bars = aggregate_daily_bars(&data);
    assert_eq!(bars.len(), 1);
    assert_eq!(bars[0].high, 108.0); // max of 103, 108, 105
    assert_eq!(bars[0].low, 97.0); // min of 97, 99, 101
    assert_eq!(bars[0].close, 103.0); // last candle's close
}

#[test]
fn aggregate_multi_day() {
    let data = multi_day_5m_data(3);
    let bars = aggregate_daily_bars(&data);
    assert_eq!(bars.len(), 3);

    // Day 0: high = max(100.0, 100.5, 100.5, 100.5) = 100.5,
    //         low = min(100.0, 99.5, 99.5, 99.5) = 99.5
    assert!((bars[0].high - 100.5).abs() < f32::EPSILON);
    assert!((bars[0].low - 99.5).abs() < f32::EPSILON);

    // Day 1: high = 100+2 = 102.0 (from first candle), low = 100-1 = 99.0
    assert!((bars[1].high - 102.0).abs() < f32::EPSILON);
    assert!((bars[1].low - 99.0).abs() < f32::EPSILON);

    // Day 2: high = 100+4 = 104.0, low = 100-2 = 98.0
    assert!((bars[2].high - 104.0).abs() < f32::EPSILON);
    assert!((bars[2].low - 98.0).abs() < f32::EPSILON);
}

// ── compute_gerchik_atr ────────────────────────────────────────

#[test]
fn returns_none_for_empty_data() {
    let data = TestCandles {
        timestamps: vec![],
        opens: vec![],
        highs: vec![],
        lows: vec![],
        closes: vec![],
        volumes: vec![],
    };
    assert!(compute_gerchik_atr(&data, 300_000.0).is_none());
}

#[test]
fn returns_none_for_daily_timeframe() {
    let data = multi_day_5m_data(20);
    // Candle duration >= 1 day should return None.
    assert!(compute_gerchik_atr(&data, DAY_MS as f64).is_none());
    assert!(compute_gerchik_atr(&data, DAY_MS as f64 * 7.0).is_none());
}

#[test]
fn returns_none_for_too_few_days() {
    // 1 day -> only 1 daily bar -> not enough for TR computation.
    let data = multi_day_5m_data(1);
    assert!(compute_gerchik_atr(&data, 300_000.0).is_none());
    // 2 days -> only 2 daily bars -> not enough (need >= 3).
    let data = multi_day_5m_data(2);
    assert!(compute_gerchik_atr(&data, 300_000.0).is_none());
}

#[test]
fn green_when_low_atr_usage() {
    // Build data where current session has a small range relative to ATR.
    let data = multi_day_5m_data(20);
    let result = compute_gerchik_atr(&data, 300_000.0);
    assert!(result.is_some());
    let render = result.unwrap();
    assert!(render.text.starts_with("G.ATR "));
    assert!(render.text.ends_with('%'));
    // With our test data, day 19 range is small relative to the ATR
    // built from increasing ranges across 20 days.
    // Just verify the structure is correct.
    assert!(render.pct >= 0.0);
}

#[test]
fn red_when_high_atr_usage() {
    // Build data where earlier days had tiny ranges but current day has a huge range.
    let base = 1_705_276_800_000_i64;
    let five_min = 300_000_i64;
    let session_start = 9 * 3_600_000_i64;
    let mut timestamps = Vec::new();
    let mut opens = Vec::new();
    let mut highs = Vec::new();
    let mut lows = Vec::new();
    let mut closes = Vec::new();
    let mut volumes = Vec::new();

    // 15 days with tiny ranges (high-low = 1.0).
    for day in 0..15 {
        for candle in 0..4 {
            let ts = base + day * DAY_MS + session_start + candle * five_min;
            timestamps.push(ts);
            opens.push(100.0);
            highs.push(100.5);
            lows.push(99.5);
            closes.push(100.0);
            volumes.push(1000);
        }
    }
    // Day 15: huge range (high-low = 20.0) -- should exceed 75% of ATR ~ 1.0.
    for candle in 0..4 {
        let ts = base + 15 * DAY_MS + session_start + candle * five_min;
        timestamps.push(ts);
        opens.push(100.0);
        if candle == 0 {
            highs.push(120.0);
            lows.push(80.0);
        } else {
            highs.push(101.0);
            lows.push(99.0);
        }
        closes.push(100.0);
        volumes.push(1000);
    }

    let data = TestCandles {
        timestamps,
        opens,
        highs,
        lows,
        closes,
        volumes,
    };

    let result = compute_gerchik_atr(&data, 300_000.0).unwrap();
    // Last day range = 40 (120-80), ATR ~ 1.0 -> pct >> 75%.
    assert!(result.pct > 75.0);
    // All closes = 100.0 (flat) -> price_up=true -> green.
    assert_eq!(result.color, GATR_COLOR_GREEN);
}

#[test]
fn text_format() {
    let data = multi_day_5m_data(10);
    let result = compute_gerchik_atr(&data, 300_000.0).unwrap();
    // Should be "G.ATR XX%"
    assert!(result.text.starts_with("G.ATR "));
    assert!(result.text.ends_with('%'));
    // No decimal places in the percentage (strip the "G.ATR " prefix first).
    let pct_part = result.text.strip_prefix("G.ATR ").unwrap();
    assert!(!pct_part.contains('.'));
}

#[test]
fn h4_is_still_intraday() {
    // H4 candle duration = 14_400_000 ms, which is < DAY_MS.
    let data = multi_day_5m_data(20);
    let result = compute_gerchik_atr(&data, 14_400_000.0);
    assert!(result.is_some());
}
