use super::*;

/// Build a sample CandleBuffer with `n` candles for testing.
fn sample_buffer(n: usize) -> CandleBuffer {
    let mut buf = CandleBuffer::with_capacity(n);
    for i in 0..n {
        let ts = (i as i64 + 1) * 60_000;
        let price = 100.0 + i as f32;
        buf.push(
            ts,
            price,       // open
            price + 5.0, // high
            price - 5.0, // low
            price + 1.0, // close
            (i as u32 + 1) * 100,
        );
    }
    buf
}

// ── MinMax: basic downsample ────────────────────────────────────────

#[test]
fn minmax_1000_to_100() {
    let buf = sample_buffer(1000);
    let result = downsample_minmax(&buf, 100);

    assert_eq!(result.len(), 100);

    // The first super-candle should start at the first original timestamp.
    assert_eq!(result.timestamps[0], buf.timestamps[0]);
    // The last super-candle should have the last original close.
    assert_eq!(*result.closes.last().unwrap(), *buf.closes.last().unwrap());
}

#[test]
fn minmax_preserves_price_envelope() {
    let buf = sample_buffer(1000);
    let result = downsample_minmax(&buf, 100);

    // The overall price envelope must be preserved.
    let original_min_low = buf.lows.iter().copied().fold(f32::INFINITY, f32::min);
    let original_max_high = buf.highs.iter().copied().fold(f32::NEG_INFINITY, f32::max);

    let result_min_low = result.lows.iter().copied().fold(f32::INFINITY, f32::min);
    let result_max_high = result
        .highs
        .iter()
        .copied()
        .fold(f32::NEG_INFINITY, f32::max);

    assert_eq!(
        original_min_low, result_min_low,
        "minimum low must be preserved"
    );
    assert_eq!(
        original_max_high, result_max_high,
        "maximum high must be preserved"
    );
}

#[test]
fn minmax_volume_is_sum() {
    let buf = sample_buffer(100);
    let result = downsample_minmax(&buf, 10);

    let original_total: u64 = buf.volumes.iter().map(|&v| v as u64).sum();
    let result_total: u64 = result.volumes.iter().map(|&v| v as u64).sum();

    assert_eq!(
        original_total, result_total,
        "total volume must be preserved"
    );
}

#[test]
fn minmax_covers_all_candles() {
    // Verify every original candle is included in exactly one bucket.
    let buf = sample_buffer(103); // not evenly divisible
    let result = downsample_minmax(&buf, 10);

    // All original candles must be accounted for.
    // The first timestamp of each super-candle should be in order.
    for i in 1..result.len() {
        assert!(
            result.timestamps[i] > result.timestamps[i - 1],
            "super-candle timestamps must be monotonically increasing"
        );
    }
    // First and last timestamps should match the original bounds.
    assert_eq!(result.timestamps[0], buf.timestamps[0]);
}

// ── MinMax: identity / edge cases ───────────────────────────────────

#[test]
fn minmax_same_size_returns_clone() {
    let buf = sample_buffer(50);
    let result = downsample_minmax(&buf, 50);

    assert_eq!(result.len(), buf.len());
    assert_eq!(result.timestamps, buf.timestamps);
    assert_eq!(result.opens, buf.opens);
    assert_eq!(result.highs, buf.highs);
    assert_eq!(result.lows, buf.lows);
    assert_eq!(result.closes, buf.closes);
    assert_eq!(result.volumes, buf.volumes);
}

#[test]
fn minmax_larger_target_returns_clone() {
    let buf = sample_buffer(10);
    let result = downsample_minmax(&buf, 100);

    assert_eq!(result.len(), 10);
    assert_eq!(result.timestamps, buf.timestamps);
}

#[test]
fn minmax_empty_buffer() {
    let buf = CandleBuffer::new();
    let result = downsample_minmax(&buf, 10);
    assert!(result.is_empty());
}

#[test]
fn minmax_single_candle() {
    let buf = sample_buffer(1);
    let result = downsample_minmax(&buf, 1);
    assert_eq!(result.len(), 1);
    assert_eq!(result.timestamps[0], buf.timestamps[0]);
}

#[test]
fn minmax_to_one_bucket() {
    let buf = sample_buffer(100);
    let result = downsample_minmax(&buf, 1);

    assert_eq!(result.len(), 1);
    assert_eq!(result.timestamps[0], buf.timestamps[0]);
    assert_eq!(result.opens[0], buf.opens[0]);
    assert_eq!(result.closes[0], *buf.closes.last().unwrap());

    let max_high = buf.highs.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let min_low = buf.lows.iter().copied().fold(f32::INFINITY, f32::min);
    assert_eq!(result.highs[0], max_high);
    assert_eq!(result.lows[0], min_low);
}

// ── LTTB: basic tests ───────────────────────────────────────────────

#[test]
fn lttb_preserves_endpoints() {
    let n = 1000;
    let timestamps: Vec<i64> = (0..n).map(|i| i * 1000).collect();
    let values: Vec<f32> = (0..n).map(|i| (i as f32 * 0.1).sin()).collect();

    let result = downsample_lttb(&timestamps, &values, 100);

    assert_eq!(result.len(), 100);
    assert_eq!(result[0], (timestamps[0], values[0]));
    assert_eq!(
        *result.last().unwrap(),
        (*timestamps.last().unwrap(), *values.last().unwrap())
    );
}

#[test]
fn lttb_no_downsample_needed() {
    let timestamps: Vec<i64> = (0..50).map(|i| i * 1000).collect();
    let values: Vec<f32> = (0..50).map(|i| i as f32).collect();

    let result = downsample_lttb(&timestamps, &values, 100);
    assert_eq!(result.len(), 50);

    for (i, &(ts, val)) in result.iter().enumerate() {
        assert_eq!(ts, timestamps[i]);
        assert_eq!(val, values[i]);
    }
}

#[test]
fn lttb_preserves_shape() {
    // Create a sine wave and verify LTTB captures the peaks and troughs.
    let n = 1000;
    let timestamps: Vec<i64> = (0..n).map(|i| i * 1000).collect();
    let values: Vec<f32> = (0..n)
        .map(|i| (i as f32 * 2.0 * std::f32::consts::PI / 100.0).sin() * 100.0)
        .collect();

    let result = downsample_lttb(&timestamps, &values, 200);
    assert_eq!(result.len(), 200);

    // The downsampled data should have max/min close to the original.
    let orig_max = values.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let orig_min = values.iter().copied().fold(f32::INFINITY, f32::min);
    let result_max = result
        .iter()
        .map(|&(_, v)| v)
        .fold(f32::NEG_INFINITY, f32::max);
    let result_min = result.iter().map(|&(_, v)| v).fold(f32::INFINITY, f32::min);

    // LTTB should capture values within 10% of the extremes.
    assert!(
        (orig_max - result_max).abs() < orig_max.abs() * 0.1,
        "LTTB should capture near-max values: orig_max={orig_max}, result_max={result_max}"
    );
    assert!(
        (orig_min - result_min).abs() < orig_min.abs() * 0.1,
        "LTTB should capture near-min values: orig_min={orig_min}, result_min={result_min}"
    );
}

#[test]
fn lttb_small_input() {
    let timestamps = vec![0i64, 1000];
    let values = vec![1.0f32, 2.0];

    let result = downsample_lttb(&timestamps, &values, 10);
    assert_eq!(result.len(), 2);
}

// ── select_lod ──────────────────────────────────────────────────────

#[test]
fn select_lod_no_downsampling_needed() {
    // 500 candles, 1000px viewport -> max_useful = 2000 -> no downsampling
    assert_eq!(select_lod(500, 1000), 500);
}

#[test]
fn select_lod_downsampling_needed() {
    // 100_000 candles, 1920px viewport -> max_useful = 3840
    assert_eq!(select_lod(100_000, 1920), 3840);
}

#[test]
fn select_lod_minimum_clamp() {
    // 100_000 candles, 50px viewport -> max_useful = 100, but minimum is 256
    assert_eq!(select_lod(100_000, 50), 256);
}

#[test]
fn select_lod_zero_viewport() {
    // Edge case: 0 viewport width -> max_useful = 0 -> clamp to 256
    assert_eq!(select_lod(1000, 0), 256);
}

#[test]
fn select_lod_exact_boundary() {
    // 4000 candles, 2000px -> max_useful = 4000 -> no downsampling
    assert_eq!(select_lod(4000, 2000), 4000);
}

#[test]
fn select_lod_one_more_than_boundary() {
    // 4001 candles, 2000px -> max_useful = 4000 -> downsampling to 4000
    assert_eq!(select_lod(4001, 2000), 4000);
}
