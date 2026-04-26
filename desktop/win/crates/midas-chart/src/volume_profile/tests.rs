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

// ─── Slice 2 (VP-anchored, legacy stack) ─────────────────────────────

use chrono_tz::US::Eastern;

const ET: Tz = Eastern;

/// Test #1 — empty input returns empty boundaries.
#[test]
fn boundaries_empty_input() {
    let out = candle_period_boundaries(&[], VolumeProfileAnchor::Daily, ET);
    assert!(out.is_empty());
}

/// Test #2 — `Anchor::Viewport` always returns empty.
#[test]
fn boundaries_viewport_yields_empty() {
    let ts = vec![1_700_000_000_000_i64; 5];
    let out = candle_period_boundaries(&ts, VolumeProfileAnchor::Viewport, ET);
    assert!(out.is_empty());
}

/// Test #3 — `Anchor::Unknown` always returns empty.
#[test]
fn boundaries_unknown_yields_empty() {
    let ts = vec![1_700_000_000_000_i64; 5];
    let out = candle_period_boundaries(&ts, VolumeProfileAnchor::Unknown, ET);
    assert!(out.is_empty());
}

/// Test #4 — three NYSE trading days of M5 timestamps in MILLISECONDS
/// must produce exactly 3 indices. **Critical regression guard for the
/// epoch-ms vs epoch-s unit-confusion bug.**
///
/// Data: 09:30..16:00 ET on 2024-01-17, 2024-01-18, 2024-01-19 → 78
/// candles per day (390 mins / 5min). 234 candles total → 3 boundaries.
#[test]
fn boundaries_daily_three_nyse_days_ms_scale() {
    use chrono::TimeZone;
    let mut ts: Vec<i64> = Vec::new();
    for day in 17..20i64 {
        // 14:30 UTC = 09:30 ET; M5 candles 0..78 across the trading day.
        let day_open = chrono::Utc
            .with_ymd_and_hms(2024, 1, day as u32, 14, 30, 0)
            .unwrap();
        for m in 0..78i64 {
            let dt = day_open + chrono::Duration::minutes(m * 5);
            ts.push(dt.timestamp_millis());
        }
    }
    assert_eq!(ts.len(), 234);
    let out = candle_period_boundaries(&ts, VolumeProfileAnchor::Daily, ET);
    assert_eq!(
        out.len(),
        3,
        "ms-scale 3-day fixture must produce 3 daily boundaries (got {out:?})"
    );
    assert_eq!(out[0], 0);
}

/// Test #5 — DST spring-forward (Sun 2024-03-10 in ET) does not induce
/// a spurious daily boundary. 3 daily timestamps at 14:00 UTC across
/// Mar 9, 10, 11 → 3 boundaries (one per day, not 4).
#[test]
fn boundaries_daily_dst_spring_forward() {
    use chrono::TimeZone;
    let ts: Vec<i64> = [9, 10, 11]
        .iter()
        .map(|&d| {
            chrono::Utc
                .with_ymd_and_hms(2024, 3, d, 19, 0, 0)
                .unwrap()
                .timestamp_millis()
        })
        .collect();
    let out = candle_period_boundaries(&ts, VolumeProfileAnchor::Daily, ET);
    assert_eq!(out.len(), 3, "DST spring-forward induced spurious boundary");
}

/// Test #6 — DST fall-back (first Sunday of November) → same.
#[test]
fn boundaries_daily_dst_fall_back() {
    use chrono::TimeZone;
    // Fall-back Sun 2023-11-05 in ET. 14:00 UTC on Nov 4, 5, 6.
    let ts: Vec<i64> = [4, 5, 6]
        .iter()
        .map(|&d| {
            chrono::Utc
                .with_ymd_and_hms(2023, 11, d, 19, 0, 0)
                .unwrap()
                .timestamp_millis()
        })
        .collect();
    let out = candle_period_boundaries(&ts, VolumeProfileAnchor::Daily, ET);
    assert_eq!(out.len(), 3, "DST fall-back induced spurious boundary");
}

/// Test #7 — ISO week 53/2026 → week 1/2027 boundary at the
/// Mon 2027-01-04 transition. (Dec 28 2026 = Mon, week 53; ISO years
/// 2026-W53 contains Dec 28 2026 - Jan 3 2027.)
#[test]
fn boundaries_iso_week_year_transition() {
    use chrono::TimeZone;
    // 2027-01-01 (Fri) is in ISO week 2026-W53.
    // 2027-01-04 (Mon) starts ISO week 2027-W1.
    let dates = [(2027, 1, 1), (2027, 1, 2), (2027, 1, 3), (2027, 1, 4)];
    let ts: Vec<i64> = dates
        .iter()
        .map(|&(y, m, d)| {
            chrono::Utc
                .with_ymd_and_hms(y, m, d, 15, 0, 0)
                .unwrap()
                .timestamp_millis()
        })
        .collect();
    let out = candle_period_boundaries(&ts, VolumeProfileAnchor::Weekly, ET);
    assert_eq!(out.len(), 2, "ISO-week transition produced {out:?}");
}

/// Test #8 — last bar of December + first bar of January → 2 monthly
/// boundaries.
#[test]
fn boundaries_monthly_year_end() {
    use chrono::TimeZone;
    let ts: Vec<i64> = [(2024, 12, 31), (2025, 1, 2)]
        .iter()
        .map(|&(y, m, d)| {
            chrono::Utc
                .with_ymd_and_hms(y, m, d, 15, 0, 0)
                .unwrap()
                .timestamp_millis()
        })
        .collect();
    let out = candle_period_boundaries(&ts, VolumeProfileAnchor::Monthly, ET);
    assert_eq!(out, vec![0, 1]);
}

/// Test #9 — yearly anchor splits at Jan 1 in the calendar's tz (ET).
#[test]
fn boundaries_yearly_year_transition() {
    use chrono::TimeZone;
    let ts: Vec<i64> = [(2023, 12, 31), (2024, 1, 1), (2024, 1, 2)]
        .iter()
        .map(|&(y, m, d)| {
            chrono::Utc
                .with_ymd_and_hms(y, m, d, 15, 0, 0)
                .unwrap()
                .timestamp_millis()
        })
        .collect();
    let out = candle_period_boundaries(&ts, VolumeProfileAnchor::Yearly, ET);
    assert_eq!(out, vec![0, 1]);
}

/// Test #10 — clamping: out-of-range timestamps are skipped (logged),
/// in-range timestamps are kept. Defends against epoch-second / epoch-
/// millisecond unit confusion regressions.
#[test]
fn boundaries_clamps_out_of_range_timestamps() {
    use chrono::TimeZone;
    // Mix of: pre-epoch (negative), in-range ms (2024), far-future ms
    // (year 9999), i64::MAX. Only the 2024 entry is kept.
    let valid_ms = chrono::Utc
        .with_ymd_and_hms(2024, 6, 1, 14, 30, 0)
        .unwrap()
        .timestamp_millis();
    let ts = vec![-8_520_336_000_000, valid_ms, 253_370_764_800_000, i64::MAX];
    let out = candle_period_boundaries(&ts, VolumeProfileAnchor::Daily, ET);
    // Only one in-range ts → one boundary at index 1 (the valid one).
    assert_eq!(out.len(), 1);
    assert_eq!(out[0], 1);
}

/// Test #11 — `compute_anchored_volume_profiles` slices the data into
/// the `boundaries.windows(2)` ranges plus the trailing range.
/// 3 boundaries `[0, 50, 100]` over a 100-candle visible range yields
/// 2 profiles.
#[test]
fn anchored_compute_two_profiles_from_three_boundaries() {
    let data = make_long_test_candles(100);
    let boundaries = vec![0usize, 50, 100]; // 100 == vis_end; last range is empty
    let profiles = compute_anchored_volume_profiles(&data, 0, 100, &boundaries, 12, 100);
    // Last boundary == vis_end so no trailing range is emitted; only
    // `windows(2)` gives [0..50, 50..100] → 2 profiles.
    assert_eq!(profiles.len(), 2);
    assert_eq!(profiles[0].0, 0);
    assert_eq!(profiles[1].0, 50);
}

/// Test #12 — `max_profiles = 1` keeps only the most-recent partition.
#[test]
fn anchored_compute_max_profiles_cap() {
    let data = make_long_test_candles(50);
    // 5 partitions: 0..10, 10..20, 20..30, 30..40, 40..50.
    let boundaries = vec![0usize, 10, 20, 30, 40, 50];
    let profiles = compute_anchored_volume_profiles(&data, 0, 50, &boundaries, 12, 1);
    assert_eq!(profiles.len(), 1);
    assert_eq!(profiles[0].0, 40, "should keep only the most recent");
}

/// Test #13 — empty inputs / zero-length partitions don't panic and
/// emit no profile.
#[test]
fn anchored_compute_empty_inputs_emit_no_profile() {
    let data = make_long_test_candles(10);
    let no_b = compute_anchored_volume_profiles(&data, 0, 10, &[], 12, 100);
    assert!(no_b.is_empty());
    let zero_bins = compute_anchored_volume_profiles(&data, 0, 10, &[0, 5, 10], 0, 100);
    assert!(zero_bins.is_empty());
    let zero_max = compute_anchored_volume_profiles(&data, 0, 10, &[0, 5, 10], 12, 0);
    assert!(zero_max.is_empty());
}

/// Test #14 — `timeframe_blocks_anchor` matches the specification:
/// daily blocks D1+, weekly blocks W1+, etc.
#[test]
fn timeframe_blocks_anchor_matches_spec() {
    use VolumeProfileAnchor::*;
    // Sub-daily TFs never block.
    assert!(!timeframe_blocks_anchor(300, Daily)); // M5
    assert!(!timeframe_blocks_anchor(3600, Daily)); // H1
                                                    // Daily blocks Daily anchor.
    assert!(timeframe_blocks_anchor(86_400, Daily));
    // Weekly TF blocks Daily and Weekly.
    assert!(timeframe_blocks_anchor(7 * 86_400, Daily));
    assert!(timeframe_blocks_anchor(7 * 86_400, Weekly));
    // Daily TF does NOT block Weekly.
    assert!(!timeframe_blocks_anchor(86_400, Weekly));
    // Viewport / Unknown never blocked.
    assert!(!timeframe_blocks_anchor(365 * 86_400, Viewport));
    assert!(!timeframe_blocks_anchor(365 * 86_400, Unknown));
    // tf_secs == 0 (insufficient data) doesn't trip the gate.
    assert!(!timeframe_blocks_anchor(0, Daily));
}

/// Test — S6 P2 narrow-period 1-px POC tick degradation.
///
/// When a period's raw pixel span is in `[MIN_POC_TICK_PX, 12)`,
/// `anchored_profile_width` returns `None` (period too narrow for a
/// full profile) but `anchored_profiles_to_instances` should still
/// emit a single 1-pixel POC tick rectangle at the period's `left_x`.
/// Below `MIN_POC_TICK_PX` the period is dropped entirely.
#[test]
fn anchored_profiles_renderer_emits_poc_tick_for_narrow_period() {
    use crate::camera::Camera2D;
    use crate::volume_profile::{anchored_profiles_to_instances, VolumeProfile, VolumeProfileBin};

    let camera = Camera2D {
        time_start: 0.0,
        time_end: 1_000.0,
        price_low: 90.0,
        price_high: 110.0,
        viewport_width: 800,
        viewport_height: 600,
        dpi_scale: 1.0,
    };

    // One profile with 3 bins; bin index 1 is the POC.
    let bins = vec![
        VolumeProfileBin {
            price_low: 100.0,
            price_high: 101.0,
            buy_volume: 5,
            sell_volume: 0,
        },
        VolumeProfileBin {
            price_low: 101.0,
            price_high: 102.0,
            buy_volume: 50,
            sell_volume: 50,
        },
        VolumeProfileBin {
            price_low: 102.0,
            price_high: 103.0,
            buy_volume: 5,
            sell_volume: 0,
        },
    ];
    let profile = VolumeProfile {
        bins,
        poc_index: 1,
        total_volume: 110,
    };
    let profiles = vec![(0usize, profile)];

    // raw_period_px = 3.0 (narrow but >= MIN_POC_TICK_PX), width_px = 0
    // (returned None from anchored_profile_width), expect a single POC
    // tick instance with width MIN_POC_TICK_PX.
    let lefts = vec![42.0_f32];
    let widths = vec![0.0_f32];
    let raw_widths = vec![3.0_f32];
    let instances =
        anchored_profiles_to_instances(&profiles, &lefts, &widths, &raw_widths, &camera);
    assert_eq!(
        instances.len(),
        1,
        "narrow period emits exactly one POC tick"
    );
    let inst = &instances[0];
    let inst_w = inst.rect[2] - inst.rect[0];
    assert!(
        (inst_w - MIN_POC_TICK_PX).abs() < 1e-3,
        "POC tick is 1 px wide, got {inst_w}"
    );
    assert!(
        (inst.rect[0] - 42.0).abs() < 1e-3,
        "POC tick anchored to period's left_x"
    );
}

/// Test — S6 P2: sub-`MIN_POC_TICK_PX` periods drop entirely (no
/// POC tick emitted for genuinely sub-pixel periods).
#[test]
fn anchored_profiles_renderer_drops_sub_pixel_period() {
    use crate::camera::Camera2D;
    use crate::volume_profile::{anchored_profiles_to_instances, VolumeProfile, VolumeProfileBin};

    let camera = Camera2D {
        time_start: 0.0,
        time_end: 1_000.0,
        price_low: 90.0,
        price_high: 110.0,
        viewport_width: 800,
        viewport_height: 600,
        dpi_scale: 1.0,
    };
    let bins = vec![VolumeProfileBin {
        price_low: 100.0,
        price_high: 101.0,
        buy_volume: 100,
        sell_volume: 0,
    }];
    let profile = VolumeProfile {
        bins,
        poc_index: 0,
        total_volume: 100,
    };
    let profiles = vec![(0usize, profile)];

    // raw_period_px below MIN_POC_TICK_PX — drop entirely.
    let lefts = vec![10.0_f32];
    let widths = vec![0.0_f32];
    let raw_widths = vec![0.5_f32];
    let instances =
        anchored_profiles_to_instances(&profiles, &lefts, &widths, &raw_widths, &camera);
    assert!(instances.is_empty(), "sub-pixel period emits no instances");
}

/// Test #15 — width clamps: too-narrow periods skip, too-wide periods
/// clamp to MAX_PROFILE_PX, in-range periods scale by width_fraction.
#[test]
fn anchored_profile_width_clamps() {
    // < MIN_PERIOD_PX_TO_RENDER (12) → None (skip).
    assert!(anchored_profile_width(5.0, 1.0).is_none());
    // 100 px raw with width_fraction=1.0 → 100 (within clamp band).
    let w = anchored_profile_width(100.0, 1.0).unwrap();
    assert!((w - 100.0).abs() < 1e-3);
    // 10000 px raw → MAX_PROFILE_PX.
    let w = anchored_profile_width(10_000.0, 1.0).unwrap();
    assert!((w - MAX_PROFILE_PX).abs() < 1e-3);
    // width_fraction halves the result.
    let w = anchored_profile_width(100.0, 0.5).unwrap();
    assert!((w - 50.0).abs() < 1e-3);
}

/// Helper — build a `n`-candle TestCandles with deterministic OHLCV.
fn make_long_test_candles(n: usize) -> TestCandles {
    let mut t = Vec::with_capacity(n);
    let mut o = Vec::with_capacity(n);
    let mut h = Vec::with_capacity(n);
    let mut l = Vec::with_capacity(n);
    let mut c = Vec::with_capacity(n);
    let mut v = Vec::with_capacity(n);
    for i in 0..n {
        t.push(1_000 + i as i64 * 60_000);
        let p = 100.0 + (i % 10) as f32 * 0.5;
        o.push(p);
        h.push(p + 0.5);
        l.push(p - 0.5);
        c.push(p + 0.1);
        v.push(1_000);
    }
    TestCandles {
        timestamps: t,
        opens: o,
        highs: h,
        lows: l,
        closes: c,
        volumes: v,
    }
}
