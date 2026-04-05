use super::*;

use std::ops::Range;

/// Simple test CandleData implementation for unit tests.
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
        let mut lo = f32::MAX;
        let mut hi = f32::MIN;
        for i in range {
            lo = lo.min(self.lows[i]);
            hi = hi.max(self.highs[i]);
        }
        (lo, hi)
    }
    fn find_index_by_time(&self, ts: i64) -> usize {
        self.timestamps
            .binary_search(&ts)
            .unwrap_or_else(|i| i.min(self.len().saturating_sub(1)))
    }
}

fn make_test_candles() -> TestCandles {
    // 5 candles spanning price range 100..110
    TestCandles {
        timestamps: vec![1000, 2000, 3000, 4000, 5000],
        opens: vec![100.0, 102.0, 105.0, 107.0, 103.0],
        highs: vec![103.0, 106.0, 108.0, 110.0, 106.0],
        lows: vec![99.0, 101.0, 104.0, 105.0, 101.0],
        closes: vec![102.0, 105.0, 107.0, 106.0, 104.0],
        volumes: vec![1000, 2000, 1500, 3000, 2500],
    }
}

fn test_camera() -> Camera2D {
    Camera2D {
        time_start: 0.0,
        time_end: 6000.0,
        price_low: 98.0,
        price_high: 112.0,
        viewport_width: 1920,
        viewport_height: 1080,
        dpi_scale: 1.0,
    }
}

#[test]
fn compute_returns_none_for_empty_range() {
    let data = make_test_candles();
    assert!(compute_volume_profile(&data, 0, 0, 100.0, 110.0, 10).is_none());
}

#[test]
fn compute_returns_none_for_zero_bins() {
    let data = make_test_candles();
    assert!(compute_volume_profile(&data, 0, 5, 100.0, 110.0, 0).is_none());
}

#[test]
fn compute_returns_none_for_zero_price_range() {
    let data = make_test_candles();
    assert!(compute_volume_profile(&data, 0, 5, 100.0, 100.0, 10).is_none());
}

#[test]
fn compute_produces_correct_bin_count() {
    let data = make_test_candles();
    let profile = compute_volume_profile(&data, 0, 5, 98.0, 112.0, 14).unwrap();
    assert_eq!(profile.bins.len(), 14);
}

#[test]
fn compute_total_volume_matches() {
    let data = make_test_candles();
    let profile = compute_volume_profile(&data, 0, 5, 98.0, 112.0, 14).unwrap();
    // Total volume = 1000 + 2000 + 1500 + 3000 + 2500 = 10000
    assert_eq!(profile.total_volume, 10000);
}

#[test]
fn poc_has_highest_volume() {
    let data = make_test_candles();
    let profile = compute_volume_profile(&data, 0, 5, 98.0, 112.0, 14).unwrap();
    let poc_vol = profile.bins[profile.poc_index].total();
    for (i, bin) in profile.bins.iter().enumerate() {
        assert!(
            bin.total() <= poc_vol,
            "bin {} has volume {} > POC volume {}",
            i,
            bin.total(),
            poc_vol,
        );
    }
}

#[test]
fn bull_bear_volume_split() {
    let data = make_test_candles();
    // Candles: 0=bull(close>=open), 1=bull, 2=bull, 3=bear(close<open), 4=bull
    let profile = compute_volume_profile(&data, 0, 5, 98.0, 112.0, 14).unwrap();
    let total_buy: u64 = profile.bins.iter().map(|b| b.buy_volume).sum();
    let total_sell: u64 = profile.bins.iter().map(|b| b.sell_volume).sum();
    // Bull candles: 0(1000) + 1(2000) + 2(1500) + 4(2500) = 7000
    // Bear candles: 3(3000) = 3000
    // Due to integer division, some volume is lost -- but most should be allocated.
    assert!(total_buy > 0, "buy volume should be > 0");
    assert!(total_sell > 0, "sell volume should be > 0");
    assert!(
        total_buy > total_sell,
        "buy volume should exceed sell volume"
    );
}

#[test]
fn profile_to_instances_produces_output() {
    let data = make_test_candles();
    let camera = test_camera();
    let profile = compute_volume_profile(&data, 0, 5, 98.0, 112.0, 14).unwrap();
    let instances = profile_to_instances(&profile, &camera, 1920);
    // Should have at least some instances (buy/sell bars + POC line)
    assert!(
        !instances.is_empty(),
        "should produce at least one instance"
    );
}

#[test]
fn profile_to_instances_includes_poc_dot() {
    let data = make_test_candles();
    let camera = test_camera();
    let profile = compute_volume_profile(&data, 0, 5, 98.0, 112.0, 14).unwrap();
    let instances = profile_to_instances(&profile, &camera, 1920);
    // The POC dot is a small circle (stacked scanlines) after the histogram bars.
    // Its instances should be small rects near the POC bar's right edge, not full-width.
    let last = instances.last().unwrap();
    assert!(
        last.rect[2] < 1920.0 * 0.25 + 20.0,
        "POC dot should be near the bar edge, not full-width, got right={}",
        last.rect[2],
    );
    // Gold color (POC)
    assert!(
        last.color[0] > 0.5 && last.color[1] > 0.4,
        "POC dot should be gold-colored",
    );
}

#[test]
fn profile_to_instances_bar_width_bounded() {
    let data = make_test_candles();
    let camera = test_camera();
    let profile = compute_volume_profile(&data, 0, 5, 98.0, 112.0, 14).unwrap();
    let instances = profile_to_instances(&profile, &camera, 1920);
    let max_bar_px = 1920.0 * 0.25;
    let poc_color_r = 0.70_f32;
    // Histogram bars (excluding POC dot) should have right edge <= max_bar_px.
    for inst in &instances {
        let is_poc_dot = (inst.color[0] - poc_color_r).abs() < 0.01;
        if is_poc_dot {
            continue;
        }
        assert!(
            inst.rect[2] <= max_bar_px + 0.01,
            "bar right edge {} exceeds max {}",
            inst.rect[2],
            max_bar_px,
        );
    }
}

#[test]
fn candles_below_price_range_excluded() {
    // Candle entirely below the visible price range should not contribute.
    let data = TestCandles {
        timestamps: vec![1000],
        opens: vec![50.0],
        highs: vec![55.0],
        lows: vec![48.0],
        closes: vec![53.0],
        volumes: vec![5000],
    };
    let profile = compute_volume_profile(&data, 0, 1, 100.0, 110.0, 10).unwrap();
    let total_in_bins: u64 = profile.bins.iter().map(|b| b.total()).sum();
    assert_eq!(
        total_in_bins, 0,
        "candle below range should contribute zero"
    );
}

#[test]
fn candles_above_price_range_excluded() {
    // Candle entirely above the visible price range should not contribute.
    let data = TestCandles {
        timestamps: vec![1000],
        opens: vec![150.0],
        highs: vec![160.0],
        lows: vec![145.0],
        closes: vec![155.0],
        volumes: vec![5000],
    };
    let profile = compute_volume_profile(&data, 0, 1, 100.0, 110.0, 10).unwrap();
    let total_in_bins: u64 = profile.bins.iter().map(|b| b.total()).sum();
    assert_eq!(
        total_in_bins, 0,
        "candle above range should contribute zero"
    );
}

#[test]
fn candle_partially_overlapping_range() {
    // Candle straddles the bottom of the price range -- only the
    // overlapping portion should contribute to bins.
    let data = TestCandles {
        timestamps: vec![1000],
        opens: vec![98.0],
        highs: vec![105.0],
        lows: vec![95.0],
        closes: vec![103.0],
        volumes: vec![1000],
    };
    let profile = compute_volume_profile(&data, 0, 1, 100.0, 110.0, 10).unwrap();
    let total_in_bins: u64 = profile.bins.iter().map(|b| b.total()).sum();
    // Effective range is 100..105 out of 95..105 (half the candle).
    // 10 bins over 100..110, so 5 bins touched (100-105).
    // 1000 / 5 = 200 per bin x 5 = 1000 total.
    assert!(
        total_in_bins > 0,
        "partially overlapping candle should contribute"
    );
    assert_eq!(
        profile.total_volume, 1000,
        "total_volume tracks original candle volume"
    );
}

#[test]
fn volume_remainder_allocated() {
    // volume=10, 3 bins touched -> 3 per bin (9) + 1 remainder to mid bin.
    let data = TestCandles {
        timestamps: vec![1000],
        opens: vec![100.0],
        highs: vec![103.0],
        lows: vec![100.0],
        closes: vec![102.0],
        volumes: vec![10],
    };
    // 3 bins of size 1.0 each (100-101, 101-102, 102-103)
    let profile = compute_volume_profile(&data, 0, 1, 100.0, 103.0, 3).unwrap();
    let total_in_bins: u64 = profile.bins.iter().map(|b| b.total()).sum();
    assert_eq!(
        total_in_bins, 10,
        "integer remainder should be fully allocated"
    );
}
