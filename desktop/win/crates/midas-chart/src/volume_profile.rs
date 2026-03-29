//! Volume Profile computation.
//!
//! Distributes candle volume across price bins to build a horizontal
//! histogram overlay. Uses uniform distribution: each candle's volume
//! is spread equally across bins from its Low to its High.

use crate::camera::Camera2D;
use crate::instances::GridLineInstance;
use midas_core::CandleData;

/// A single price bin in the volume profile.
#[derive(Clone, Debug, Default)]
pub struct VolumeProfileBin {
    /// Lower price boundary of this bin.
    pub price_low: f32,
    /// Upper price boundary of this bin.
    pub price_high: f32,
    /// Volume from bullish candles (close >= open).
    pub buy_volume: u64,
    /// Volume from bearish candles (close < open).
    pub sell_volume: u64,
}

impl VolumeProfileBin {
    /// Total volume in this bin.
    pub fn total(&self) -> u64 {
        self.buy_volume + self.sell_volume
    }
}

/// Computed volume profile for a visible range.
#[derive(Clone, Debug)]
pub struct VolumeProfile {
    /// Price bins from lowest to highest.
    pub bins: Vec<VolumeProfileBin>,
    /// Index of the bin with the highest total volume (Point of Control).
    pub poc_index: usize,
    /// Total volume across all bins.
    pub total_volume: u64,
}

/// Compute volume profile bins from visible candle data.
///
/// Returns `None` if the visible range is empty or price range is zero.
pub fn compute_volume_profile(
    data: &dyn CandleData,
    vis_start: usize,
    vis_end: usize,
    price_low: f32,
    price_high: f32,
    num_bins: usize,
) -> Option<VolumeProfile> {
    if vis_start >= vis_end || num_bins == 0 || price_high <= price_low {
        return None;
    }

    let bin_size = (price_high - price_low) / num_bins as f32;
    let mut bins: Vec<VolumeProfileBin> = (0..num_bins)
        .map(|i| VolumeProfileBin {
            price_low: price_low + i as f32 * bin_size,
            price_high: price_low + (i + 1) as f32 * bin_size,
            ..Default::default()
        })
        .collect();

    let mut total_volume: u64 = 0;

    for i in vis_start..vis_end {
        let high = data.high(i);
        let low = data.low(i);
        let volume = data.volume(i) as u64;
        if volume == 0 || high <= low {
            continue;
        }

        // Skip candles entirely outside the visible price range.
        if high < price_low || low > price_high {
            continue;
        }

        let is_bull = data.close(i) >= data.open(i);

        // Clamp candle range to visible price range, then map to bins.
        let effective_low = low.max(price_low);
        let effective_high = high.min(price_high);
        let bin_lo = ((effective_low - price_low) / bin_size).floor().max(0.0) as usize;
        let bin_hi = ((effective_high - price_low) / bin_size)
            .ceil()
            .min(num_bins as f32) as usize;
        let bin_hi = bin_hi.min(num_bins).saturating_sub(1);

        if bin_lo > bin_hi {
            continue;
        }

        let bins_touched = (bin_hi - bin_lo + 1) as u64;
        let vol_per_bin = volume / bins_touched.max(1);
        let remainder = volume - vol_per_bin * bins_touched;

        for b in bin_lo..=bin_hi {
            if is_bull {
                bins[b].buy_volume += vol_per_bin;
            } else {
                bins[b].sell_volume += vol_per_bin;
            }
        }

        // Allocate integer-division remainder to the midpoint bin.
        if remainder > 0 {
            let mid = (bin_lo + bin_hi) / 2;
            if is_bull {
                bins[mid].buy_volume += remainder;
            } else {
                bins[mid].sell_volume += remainder;
            }
        }

        total_volume += volume;
    }

    // Find POC (bin with max volume)
    let poc_index = bins
        .iter()
        .enumerate()
        .max_by_key(|(_, b)| b.total())
        .map(|(i, _)| i)
        .unwrap_or(0);

    Some(VolumeProfile {
        bins,
        poc_index,
        total_volume,
    })
}

/// Convert a VolumeProfile into GPU-ready GridLineInstance rectangles.
///
/// Renders horizontal bars from the left edge of the chart. Each bar's
/// width is proportional to its volume relative to the POC bin (widest).
/// Buy volume is rendered in one color, sell volume stacked to the right.
pub fn profile_to_instances(
    profile: &VolumeProfile,
    camera: &Camera2D,
    viewport_width: u32,
) -> Vec<GridLineInstance> {
    let max_vol = profile
        .bins
        .iter()
        .map(|b| b.total())
        .max()
        .unwrap_or(1)
        .max(1);
    // VP histogram occupies up to 25% of chart width
    let max_bar_px = viewport_width as f32 * 0.25;
    let buy_color = [0.10, 0.55, 0.55, 0.30]; // teal, semi-transparent
    let sell_color = [0.65, 0.20, 0.20, 0.30]; // red, semi-transparent
    let poc_color = [0.70, 0.58, 0.08, 0.45]; // muted gold, for POC dot

    let mut instances = Vec::with_capacity(profile.bins.len() * 2 + 1);

    for bin in &profile.bins {
        let total = bin.total();
        if total == 0 {
            continue;
        }

        let y_top = camera.price_to_y(bin.price_high as f64);
        let y_bottom = camera.price_to_y(bin.price_low as f64);

        // Skip bins outside viewport
        if y_top > camera.viewport_height as f32 || y_bottom < 0.0 {
            continue;
        }

        let bar_width = (total as f32 / max_vol as f32) * max_bar_px;
        let buy_frac = bin.buy_volume as f32 / total as f32;
        let buy_width = bar_width * buy_frac;

        // Buy portion (left)
        if buy_width > 0.5 {
            instances.push(GridLineInstance {
                rect: [0.0, y_top, buy_width, y_bottom],
                color: buy_color,
            });
        }

        // Sell portion (right of buy)
        let sell_width = bar_width - buy_width;
        if sell_width > 0.5 {
            instances.push(GridLineInstance {
                rect: [buy_width, y_top, bar_width, y_bottom],
                color: sell_color,
            });
        }
    }

    // POC dot: small filled circle at the left edge of the highest-volume bar.
    // Approximated as stacked horizontal scanlines (same technique as the
    // volume handle triangle).
    if !profile.bins.is_empty() {
        let poc = &profile.bins[profile.poc_index];
        let poc_y_mid = {
            let yt = camera.price_to_y(poc.price_high as f64);
            let yb = camera.price_to_y(poc.price_low as f64);
            (yt + yb) / 2.0
        };
        let poc_bar_width = (poc.total() as f32 / max_vol as f32) * max_bar_px;
        let radius: f32 = 2.0;
        let x_center = poc_bar_width + radius + 3.0; // 3px gap after bar
        let slices = (radius * 2.0) as i32;
        for s in 0..slices {
            let y = poc_y_mid - radius + s as f32;
            let dist = ((s as f32 + 0.5) - radius).abs();
            // Circle equation: half-width = sqrt(r² - d²)
            let half_w = (radius * radius - dist * dist).sqrt();
            if half_w < 0.5 {
                continue;
            }
            instances.push(GridLineInstance {
                rect: [x_center - half_w, y, x_center + half_w, y + 1.0],
                color: poc_color,
            });
        }
    }

    instances
}

#[cfg(test)]
mod tests {
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
        assert_eq!(total_in_bins, 0, "candle below range should contribute zero");
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
        assert_eq!(total_in_bins, 0, "candle above range should contribute zero");
    }

    #[test]
    fn candle_partially_overlapping_range() {
        // Candle straddles the bottom of the price range — only the
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
        // 1000 / 5 = 200 per bin × 5 = 1000 total.
        assert!(total_in_bins > 0, "partially overlapping candle should contribute");
        assert_eq!(
            profile.total_volume, 1000,
            "total_volume tracks original candle volume"
        );
    }

    #[test]
    fn volume_remainder_allocated() {
        // volume=10, 3 bins touched → 3 per bin (9) + 1 remainder to mid bin.
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
        assert_eq!(total_in_bins, 10, "integer remainder should be fully allocated");
    }
}
