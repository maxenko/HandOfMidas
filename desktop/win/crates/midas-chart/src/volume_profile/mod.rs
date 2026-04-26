//! Volume Profile computation.
//!
//! Distributes candle volume across price bins to build a horizontal
//! histogram overlay. Uses uniform distribution: each candle's volume
//! is spread equally across bins from its Low to its High.
//!
//! ## Slice 2 (VP-anchored, legacy stack)
//!
//! Adds [`candle_period_boundaries`], [`compute_anchored_volume_profiles`],
//! and [`anchored_profiles_to_instances`] to support per-period rendering
//! when `state.volume_profile.anchor != Viewport`. The kill-switch
//! (`experimental.disable_anchored_vp`) is enforced upstream — the
//! `compute::build_volume_profile` branch reads
//! `ChartInput::effective_vp_anchor`, never `state.volume_profile.anchor`
//! directly. See `plan/volume-profile-anchored/02-slice-2-legacy-render.md`.

use chrono::{DateTime, Datelike};
use chrono_tz::Tz;

use crate::camera::Camera2D;
use crate::instances::GridLineInstance;
use midas_core::{CandleData, VolumeProfileAnchor};

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

        for bin in bins.iter_mut().take(bin_hi + 1).skip(bin_lo) {
            if is_bull {
                bin.buy_volume += vol_per_bin;
            } else {
                bin.sell_volume += vol_per_bin;
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

// ─── Slice 2: per-period anchored Volume Profile ───────────────────────

/// Width clamps applied per anchored profile (legacy stack). Wider
/// periods clamp to `MAX_PROFILE_PX`; thinner ones clamp up to
/// `MIN_PROFILE_PX` so a single-candle period stays visible. Periods
/// narrower than `MIN_PERIOD_PX_TO_RENDER` degrade to a single 1-px
/// POC tick (S6 P2) when at least `MIN_POC_TICK_PX` pixels are
/// available; below that they're skipped entirely.
const MIN_PERIOD_PX_TO_RENDER: f32 = 12.0;
const MIN_PROFILE_PX: f32 = 24.0;
const MAX_PROFILE_PX: f32 = 240.0;
/// S6 P2 — narrow-period 1-pixel POC tick degradation. When a period
/// has between `MIN_POC_TICK_PX` and `MIN_PERIOD_PX_TO_RENDER` pixels
/// of horizontal span the renderer paints just the POC bin at the
/// period's left edge instead of dropping the period.
const MIN_POC_TICK_PX: f32 = 1.0;

/// Maximum profiles emitted per render. v1 hard-coded per Open
/// Question 4 (D10 of `00-index.md`); raised in S4/S6 if needed.
pub const MAX_PROFILES_PER_RENDER: usize = 100;

/// Bin count for an anchored per-period profile. Tighter than the
/// viewport-mode `((h*0.8)/3.0).clamp(20, 200)` formula because each
/// profile occupies only a fraction of the viewport width.
#[inline]
fn anchored_bin_count(viewport_height_px: f32) -> usize {
    ((viewport_height_px * 0.5) as usize).clamp(8, 24)
}

/// Group key for [`candle_period_boundaries`]. Two consecutive candles
/// share a period iff their keys compare equal.
#[derive(Copy, Clone, PartialEq, Eq)]
enum PeriodKey {
    Day(i32, u32, u32),
    /// ISO week — `(iso_year, iso_week)`. Mon-start.
    Week(i32, u32),
    Month(i32, u32),
    Year(i32),
}

impl PeriodKey {
    #[inline]
    fn from_dt(dt: &DateTime<Tz>, anchor: VolumeProfileAnchor) -> Self {
        match anchor {
            VolumeProfileAnchor::Daily => Self::Day(dt.year(), dt.month(), dt.day()),
            VolumeProfileAnchor::Weekly => {
                let iso = dt.iso_week();
                Self::Week(iso.year(), iso.week())
            }
            VolumeProfileAnchor::Monthly => Self::Month(dt.year(), dt.month()),
            VolumeProfileAnchor::Yearly => Self::Year(dt.year()),
            // `paint_per_period` paths filter Viewport / Unknown
            // upstream; this arm exists for forward-compat with
            // `#[serde(other)] Unknown` inputs.
            VolumeProfileAnchor::Viewport | VolumeProfileAnchor::Unknown => Self::Year(dt.year()),
        }
    }
}

/// Sane-range guard for epoch-millisecond timestamps. `0` (Unix epoch
/// origin) through 2100-01-01 — chrono-tz lookups can be unstable far
/// from now, and inputs outside this band are almost certainly unit-
/// confusion bugs (epoch seconds passed where ms expected).
const VALID_TS_MIN_MS: i64 = 0;
const VALID_TS_MAX_MS: i64 = 4_102_444_800_000;

/// Index of the first candle in each period over the supplied
/// timestamp slice. Returns the empty `Vec` for `Viewport`/`Unknown`
/// (caller falls back to the single-profile path) and for empty input.
///
/// # Critical: timestamps are epoch MILLISECONDS
///
/// `CandleData::timestamp(i) -> i64` is documented as epoch-ms in
/// `midas-core/src/candle_data/mod.rs`. This helper uses
/// `DateTime::from_timestamp_millis`, NOT `from_timestamp` (which
/// expects seconds). Mixing units silently breaks boundary detection
/// — every candle ends up in its own "period" and the whole anchored-
/// VP feature renders as one-bar-per-profile garbage. The
/// `boundaries_daily_three_nyse_days_ms_scale` regression test guards
/// against that unit-confusion bug.
pub fn candle_period_boundaries(
    timestamps_ms: &[i64],
    anchor: VolumeProfileAnchor,
    tz: Tz,
) -> Vec<usize> {
    if timestamps_ms.is_empty()
        || matches!(
            anchor,
            VolumeProfileAnchor::Viewport | VolumeProfileAnchor::Unknown
        )
    {
        return Vec::new();
    }

    let mut out = Vec::with_capacity(timestamps_ms.len() / 4);
    let mut prev_key: Option<PeriodKey> = None;
    for (i, &ts) in timestamps_ms.iter().enumerate() {
        if !(VALID_TS_MIN_MS..=VALID_TS_MAX_MS).contains(&ts) {
            tracing::warn!(target: "vp", ts, "candle ts out of safe range; skipped");
            continue;
        }
        let Some(dt_utc) = DateTime::from_timestamp_millis(ts) else {
            continue;
        };
        let dt = dt_utc.with_timezone(&tz);
        let key = PeriodKey::from_dt(&dt, anchor);
        if Some(key) != prev_key {
            out.push(i);
            prev_key = Some(key);
        }
    }
    out
}

/// True if the chart's bar-period (in seconds) is at least as coarse
/// as the configured anchor — per-period rendering would degenerate
/// to one bar per profile, so the caller falls back to Viewport.
///
/// `tf_secs` MUST be in seconds. Inferred via the median (or simply
/// the minimum positive) inter-candle delta in
/// `compute::build_volume_profile`; mirrors S3's `period_blocks_anchor`
/// on the new stack but avoids widening `ChartInput` with a
/// `Timeframe` field.
pub fn timeframe_blocks_anchor(tf_secs: u64, anchor: VolumeProfileAnchor) -> bool {
    use VolumeProfileAnchor::*;
    let anchor_min: u64 = match anchor {
        Daily => 86_400,
        Weekly => 7 * 86_400,
        Monthly => 30 * 86_400,
        Yearly => 365 * 86_400,
        Viewport | Unknown => return false,
    };
    tf_secs > 0 && tf_secs >= anchor_min
}

/// Public helper used by `compute::build_volume_profile`. Estimates
/// the chart's bar-period in seconds by walking adjacent timestamps
/// and returning the smallest positive delta — robust to session
/// boundary gaps that lengthen the gap between some pairs.
pub fn estimate_timeframe_secs_from_data(
    data: &dyn CandleData,
    vis_start: usize,
    vis_end: usize,
) -> u64 {
    if vis_end <= vis_start + 1 {
        return 0;
    }
    let last = vis_end.saturating_sub(1);
    let mut min_delta_ms: i64 = i64::MAX;
    for i in vis_start..last {
        let a = data.timestamp(i);
        let b = data.timestamp(i + 1);
        let d = b.saturating_sub(a);
        if d > 0 && d < min_delta_ms {
            min_delta_ms = d;
        }
    }
    if min_delta_ms == i64::MAX {
        return 0;
    }
    (min_delta_ms / 1000).max(0) as u64
}

/// Run the per-bin volume distribution over the candle range
/// `vis_start..vis_end`, but bin against the price range derived from
/// THAT slice's own `(min_low, max_high)` rather than the camera's
/// price bounds. Used by the per-period path; viewport-mode keeps
/// using [`compute_volume_profile`] which bins against the camera
/// price range.
fn compute_profile_for_range(
    data: &dyn CandleData,
    vis_start: usize,
    vis_end: usize,
    num_bins: usize,
) -> Option<VolumeProfile> {
    if vis_start >= vis_end || num_bins == 0 {
        return None;
    }
    let mut min_low = f32::INFINITY;
    let mut max_high = f32::NEG_INFINITY;
    for i in vis_start..vis_end {
        let l = data.low(i);
        let h = data.high(i);
        if l < min_low {
            min_low = l;
        }
        if h > max_high {
            max_high = h;
        }
    }
    if !min_low.is_finite() || !max_high.is_finite() || max_high <= min_low {
        return None;
    }
    compute_volume_profile(data, vis_start, vis_end, min_low, max_high, num_bins)
}

/// Compute one [`VolumeProfile`] per period, using `boundaries` (from
/// [`candle_period_boundaries`]) plus the visible-range end as the
/// terminating index. Caps to the most recent `max_profiles` periods
/// (drops oldest from view). Returns the profiles in chronological
/// order so the caller can pair each with its left/right pixel x.
pub fn compute_anchored_volume_profiles(
    data: &dyn CandleData,
    vis_start: usize,
    vis_end: usize,
    boundaries: &[usize],
    num_bins_per_profile: usize,
    max_profiles: usize,
) -> Vec<(usize, VolumeProfile)> {
    if boundaries.is_empty() || max_profiles == 0 || num_bins_per_profile == 0 {
        return Vec::new();
    }
    // Build (start, end) ranges:  boundaries[i]..boundaries[i+1], plus
    // the trailing  boundaries[last]..vis_end  range.
    let mut ranges: Vec<(usize, usize)> = Vec::with_capacity(boundaries.len());
    for w in boundaries.windows(2) {
        ranges.push((w[0], w[1]));
    }
    if let Some(&last) = boundaries.last() {
        if last < vis_end {
            ranges.push((last, vis_end));
        }
    }

    // Cap to the most recent N. Drains the front (oldest) so the
    // remaining ranges are contiguous on the right edge of the
    // visible window.
    if ranges.len() > max_profiles {
        let drop = ranges.len() - max_profiles;
        ranges.drain(0..drop);
    }

    let mut profiles = Vec::with_capacity(ranges.len());
    for (start, end) in ranges {
        if start < vis_start || end > vis_end || start >= end {
            continue;
        }
        if let Some(p) = compute_profile_for_range(data, start, end, num_bins_per_profile) {
            profiles.push((start, p));
        }
    }
    profiles
}

/// Convert a slice of `(start_idx, VolumeProfile)` pairs into GPU-
/// ready [`GridLineInstance`]s, one rectangle per non-empty bin per
/// profile. Each profile is positioned at its own `left_x` (from the
/// caller's `lefts_px`); bar widths are the per-bin volume scaled to
/// the per-profile `widths_px`.
///
/// `raw_period_widths_px` carries the unclamped per-period pixel span
/// so the renderer can degrade narrow periods (`width_px == 0.0` but
/// `raw_period_width >= MIN_POC_TICK_PX`) to a single 1-pixel POC tick
/// (S6 P2) instead of dropping them.
///
/// `lefts_px`, `widths_px`, and `raw_period_widths_px` MUST all have
/// the same length as `profiles`. Panics on a mismatched pair (the
/// call site is internal — a length invariant violation is a
/// programming bug, not user data).
pub fn anchored_profiles_to_instances(
    profiles: &[(usize, VolumeProfile)],
    lefts_px: &[f32],
    widths_px: &[f32],
    raw_period_widths_px: &[f32],
    camera: &Camera2D,
) -> Vec<GridLineInstance> {
    assert_eq!(profiles.len(), lefts_px.len());
    assert_eq!(profiles.len(), widths_px.len());
    assert_eq!(profiles.len(), raw_period_widths_px.len());

    let buy_color = [0.10, 0.55, 0.55, 0.30];
    let sell_color = [0.65, 0.20, 0.20, 0.30];
    let poc_color = [0.70, 0.58, 0.08, 0.45];
    let vp_height = camera.viewport_height as f32;

    let mut instances = Vec::with_capacity(profiles.len() * 24);

    for (((_start, profile), (&left_x, &width_px)), &raw_period_px) in profiles
        .iter()
        .zip(lefts_px.iter().zip(widths_px.iter()))
        .zip(raw_period_widths_px.iter())
    {
        if width_px <= 0.0 {
            // S6 P2 — narrow-period 1-pixel POC tick degradation.
            // When the period has at least MIN_POC_TICK_PX of pixels
            // available, paint the POC bin as a 1-px tick rather than
            // letting the period vanish entirely.
            if raw_period_px >= MIN_POC_TICK_PX && !profile.bins.is_empty() {
                let poc = &profile.bins[profile.poc_index];
                if poc.total() > 0 {
                    let y_top = camera.price_to_y(poc.price_high as f64);
                    let y_bottom = camera.price_to_y(poc.price_low as f64);
                    if y_top <= vp_height && y_bottom >= 0.0 {
                        instances.push(GridLineInstance {
                            rect: [left_x, y_top, left_x + MIN_POC_TICK_PX, y_bottom],
                            color: poc_color,
                        });
                    }
                }
            }
            continue;
        }
        let max_vol = profile
            .bins
            .iter()
            .map(|b| b.total())
            .max()
            .unwrap_or(1)
            .max(1);

        for bin in &profile.bins {
            let total = bin.total();
            if total == 0 {
                continue;
            }
            let y_top = camera.price_to_y(bin.price_high as f64);
            let y_bottom = camera.price_to_y(bin.price_low as f64);
            if y_top > vp_height || y_bottom < 0.0 {
                continue;
            }
            let bar_width = (total as f32 / max_vol as f32) * width_px;
            let buy_frac = bin.buy_volume as f32 / total as f32;
            let buy_width = bar_width * buy_frac;
            if buy_width > 0.5 {
                instances.push(GridLineInstance {
                    rect: [left_x, y_top, left_x + buy_width, y_bottom],
                    color: buy_color,
                });
            }
            let sell_width = bar_width - buy_width;
            if sell_width > 0.5 {
                instances.push(GridLineInstance {
                    rect: [left_x + buy_width, y_top, left_x + bar_width, y_bottom],
                    color: sell_color,
                });
            }
        }

        // POC dot — same scanline-circle approach as `profile_to_instances`,
        // anchored to the profile's left_x + bar width.
        if !profile.bins.is_empty() {
            let poc = &profile.bins[profile.poc_index];
            let poc_y_mid = {
                let yt = camera.price_to_y(poc.price_high as f64);
                let yb = camera.price_to_y(poc.price_low as f64);
                (yt + yb) / 2.0
            };
            let poc_bar_width = (poc.total() as f32 / max_vol as f32) * width_px;
            let radius: f32 = 2.0;
            let x_center = left_x + poc_bar_width + radius + 3.0;
            let slices = (radius * 2.0) as i32;
            for s in 0..slices {
                let y = poc_y_mid - radius + s as f32;
                let dist = ((s as f32 + 0.5) - radius).abs();
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
    }

    instances
}

/// Visible width clamps for an anchored profile, in chart pixels.
/// Wired by `compute::build_volume_profile`. Returns `None` to skip
/// the profile entirely (period too narrow at current zoom).
pub fn anchored_profile_width(raw_period_px: f32, width_fraction: f32) -> Option<f32> {
    if !raw_period_px.is_finite() || raw_period_px < MIN_PERIOD_PX_TO_RENDER {
        return None;
    }
    let clamp = raw_period_px.clamp(MIN_PROFILE_PX, MAX_PROFILE_PX);
    Some(clamp * width_fraction)
}

/// Per-period bin count exposed for the compute branch.
#[inline]
pub fn anchored_bin_count_for(viewport_height_px: f32) -> usize {
    anchored_bin_count(viewport_height_px)
}

#[cfg(test)]
mod tests;
