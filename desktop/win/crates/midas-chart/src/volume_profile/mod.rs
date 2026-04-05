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
mod tests;
