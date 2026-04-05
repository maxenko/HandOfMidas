//! Level-of-Detail (LOD) downsampling algorithms for candle and line data.
//!
//! - [`downsample_minmax`]: MinMax bucketing that preserves the price envelope
//!   (max high, min low per bucket). Correct for OHLCV candle data.
//! - [`downsample_lttb`]: Largest Triangle Three Buckets algorithm for
//!   single-valued line data (indicators, moving averages). Preserves visual
//!   shape by maximizing the triangle area with neighboring buckets.
//! - [`select_lod`]: Determines the target candle count given total candles
//!   and viewport width.

use crate::candle::CandleBuffer;

// ─── MinMax Bucketing ───────────────────────────────────────────────────

/// Downsample candles using MinMax bucketing to `target_count` super-candles.
///
/// Each bucket aggregates a contiguous group of input candles:
/// - `timestamp` = first candle's timestamp
/// - `open`      = first candle's open
/// - `high`      = maximum of all highs in the bucket
/// - `low`       = minimum of all lows in the bucket
/// - `close`     = last candle's close
/// - `volume`    = sum of all volumes (saturating at `u32::MAX`)
///
/// If `target_count >= candles.len()`, returns a clone (no downsampling).
/// If `target_count` is zero and `candles` is non-empty, it is clamped to 1.
pub fn downsample_minmax(candles: &CandleBuffer, target_count: usize) -> CandleBuffer {
    let n = candles.len();
    if n == 0 {
        return CandleBuffer::new();
    }
    if n <= target_count {
        return candles.clone();
    }

    let target = target_count.max(1);
    let bucket_size = n / target;
    let mut out = CandleBuffer::with_capacity(target);

    let mut i = 0;
    while i < n {
        // The last bucket absorbs the remainder to ensure all candles are covered.
        let remaining_buckets = target.saturating_sub(out.len());
        let end = if remaining_buckets <= 1 {
            n
        } else {
            (i + bucket_size).min(n)
        };

        let bucket_highs = &candles.highs[i..end];
        let bucket_lows = &candles.lows[i..end];

        // SIMD-friendly: contiguous f32 scans.
        let high = bucket_highs
            .iter()
            .copied()
            .fold(f32::NEG_INFINITY, f32::max);
        let low = bucket_lows.iter().copied().fold(f32::INFINITY, f32::min);
        let volume: u32 = candles.volumes[i..end]
            .iter()
            .copied()
            .fold(0u32, u32::saturating_add);

        out.push(
            candles.timestamps[i],   // first timestamp
            candles.opens[i],        // first open
            high,                    // max high
            low,                     // min low
            candles.closes[end - 1], // last close
            volume,                  // sum volume
        );

        i = end;
    }

    out
}

// ─── LTTB (Largest Triangle Three Buckets) ──────────────────────────────

/// Downsample single-valued line data using the LTTB algorithm.
///
/// Returns a `Vec<(timestamp, value)>` of length `target_count` (or fewer if
/// the input is shorter). The first and last points are always preserved.
///
/// LTTB selects one point per bucket that maximizes the triangle area formed
/// with the previously selected point and the average of the next bucket,
/// preserving the overall visual shape of the line.
///
/// If `target_count >= timestamps.len()`, returns all points unchanged.
/// Panics if `timestamps` and `values` have different lengths.
pub fn downsample_lttb(timestamps: &[i64], values: &[f32], target_count: usize) -> Vec<(i64, f32)> {
    assert_eq!(
        timestamps.len(),
        values.len(),
        "timestamps and values must have the same length"
    );

    let n = timestamps.len();
    if n <= target_count || target_count < 2 {
        return timestamps
            .iter()
            .copied()
            .zip(values.iter().copied())
            .collect();
    }

    let mut result = Vec::with_capacity(target_count);

    // Always keep the first point.
    result.push((timestamps[0], values[0]));

    let bucket_size = (n - 2) as f64 / (target_count - 2) as f64;
    let mut prev_selected = 0usize;

    for bucket_idx in 1..(target_count - 1) {
        // Current bucket range.
        let bucket_start = ((bucket_idx - 1) as f64 * bucket_size + 1.0) as usize;
        let bucket_end = ((bucket_idx as f64) * bucket_size + 1.0).min(n as f64) as usize;

        // Next bucket range (for computing the average "C" point).
        let next_start = bucket_end;
        let next_end = (((bucket_idx + 1) as f64) * bucket_size + 1.0).min(n as f64) as usize;

        // Average of the next bucket.
        let next_len = next_end - next_start;
        let avg_ts: f64 = timestamps[next_start..next_end]
            .iter()
            .map(|&t| t as f64)
            .sum::<f64>()
            / next_len as f64;
        let avg_val: f64 = values[next_start..next_end]
            .iter()
            .map(|&v| v as f64)
            .sum::<f64>()
            / next_len as f64;

        // Previous selected point ("A").
        let a_ts = timestamps[prev_selected] as f64;
        let a_val = values[prev_selected] as f64;

        // Find the point in the current bucket that maximizes triangle area.
        let mut max_area = -1.0f64;
        let mut max_idx = bucket_start;

        for j in bucket_start..bucket_end {
            let area = ((a_ts - avg_ts) * (values[j] as f64 - a_val)
                - (a_ts - timestamps[j] as f64) * (avg_val - a_val))
                .abs();
            if area > max_area {
                max_area = area;
                max_idx = j;
            }
        }

        result.push((timestamps[max_idx], values[max_idx]));
        prev_selected = max_idx;
    }

    // Always keep the last point.
    result.push((timestamps[n - 1], values[n - 1]));

    result
}

// ─── Auto-LOD Selection ─────────────────────────────────────────────────

/// Determine the target candle count for the given viewport width.
///
/// Rules:
/// - If `total_candles <= 2 * viewport_width`, return `total_candles`
///   (no downsampling needed).
/// - Otherwise, target = `2 * viewport_width` (2 candles per pixel gives
///   sub-pixel fidelity for anti-aliased wicks).
/// - Clamp to a minimum of 256 candles to prevent degenerate ultra-zoom-out.
///
/// Returns the target candle count. A return value equal to `total_candles`
/// means no downsampling is required.
pub fn select_lod(total_candles: usize, viewport_width: u32) -> usize {
    let max_useful = (viewport_width as usize) * 2;

    if total_candles <= max_useful {
        return total_candles;
    }

    max_useful.max(256)
}

#[cfg(test)]
mod tests;
