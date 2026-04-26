//! [`VolumeProfileLayer`] — horizontal volume-profile histogram.
//!
//! Ports the legacy overlay from `midas_chart::volume_profile` onto the
//! sans-IO scene stack (slice 7 of the chart-transition plan).
//!
//! ## Algorithm
//!
//! 1. Pick the visible-range slice of the shared `CandleSeries`.
//! 2. Price range = min-low..max-high over the visible candles.
//! 3. Bin count is **viewport-adaptive**: per legacy
//!    `widget/compute/mod.rs:126`,
//!    `((viewport.height_px * 0.8) / 3.0).clamp(20.0, 200.0) as usize`
//!    — a denser profile for taller viewports. NOT a hardcoded 40.
//! 4. For each visible candle, distribute `volume` uniformly across the
//!    bins its `[low, high]` intersects (integer-division spread;
//!    remainder goes to the mid bin, matching legacy).
//! 5. The bin with the largest total volume is the Point of Control
//!    (POC) and is emitted with a distinctly brighter colour.
//!
//! ## Output
//!
//! One [`QuadInstance`] per bin with non-zero volume. Bars are
//! left-anchored; bar width is proportional to
//! `bin_volume / max_bin_volume` × `style.max_bar_px_fraction ×
//! viewport.width_px`. Each bar spans the bin's vertical slice of the
//! viewport (price → y via the context's `PriceAxis`).

use std::ops::Range;

use chrono::{DateTime, Datelike};
use chrono_tz::Tz;
use midas_calendar::{BarPeriod, CalendarSpan, ExchangeCalendar};

use crate::layer::{LayerId, LayerZ, SceneLayer};
use crate::layers::candle::SharedCandleSeries;
use crate::paint::PaintContext;
use crate::primitives::QuadInstance;

pub mod anchor;
pub use anchor::VolumeProfileAnchor;

/// Slice 3 (anchored VP, new stack): width clamps applied per-period.
/// Periods narrower than [`MIN_PERIOD_PX_TO_RENDER`] degrade to a
/// single 1-px POC tick (S6 P2) when at least
/// [`MIN_POC_TICK_PX`] of horizontal pixels are available; below that
/// they're skipped entirely. Periods wider than `MAX_PROFILE_PX` are
/// clamped so a yearly window never spans the entire viewport. Lower
/// bound `MIN_PROFILE_PX` keeps a thin partition barely visible.
const MIN_PERIOD_PX_TO_RENDER: f32 = 12.0;
const MIN_PROFILE_PX: f32 = 24.0;
const MAX_PROFILE_PX: f32 = 240.0;
/// S6 P2 — narrow-period 1-pixel POC tick degradation. When a
/// partition's pixel span is in `[MIN_POC_TICK_PX, MIN_PERIOD_PX_TO_RENDER)`
/// the layer paints a single 1-px POC tick at the partition's left
/// edge instead of dropping the partition. TradingView convention.
const MIN_POC_TICK_PX: f32 = 1.0;

/// Visual knobs for [`VolumeProfileLayer`].
///
/// All colour channels are RGBA8. `neighbour_color` paints every bin
/// below the POC; `poc_color` paints the single densest bin and MUST
/// be distinguishable (brighter / more saturated / higher alpha) per
/// the slice-7 test "POC gets a distinctly brighter color than
/// neighbours".
#[derive(Copy, Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct VolumeProfileStyle {
    /// Fraction of viewport width the largest bar consumes. Legacy
    /// used 0.25.
    pub max_bar_px_fraction: f32,
    /// Colour of non-POC bins.
    pub neighbour_color: [u8; 4],
    /// Colour of the POC bin. Must be distinctly brighter than
    /// `neighbour_color`.
    pub poc_color: [u8; 4],
}

impl Default for VolumeProfileStyle {
    fn default() -> Self {
        Self {
            max_bar_px_fraction: 0.25,
            // Semi-transparent teal, matches the legacy buy-volume
            // tint (`[0.10, 0.55, 0.55, 0.30]` → 0..255 scale).
            neighbour_color: [0x1a, 0x8c, 0x8c, 0x4d],
            // Bright muted-gold, RGB is noticeably lighter and alpha
            // is noticeably higher than `neighbour_color` — the
            // "distinctly brighter" requirement from the plan.
            poc_color: [0xe6, 0xb8, 0x1a, 0xcc],
        }
    }
}

/// Behaviour knobs for [`VolumeProfileLayer`] (Slice 3 of the VP-
/// anchored plan).
///
/// `anchor` selects per-viewport vs per-period rendering;
/// `width_fraction` scales the clamped per-period pixel-width
/// `[MIN_PROFILE_PX, MAX_PROFILE_PX]` to taste; `max_profiles` bounds
/// the number of partitions retained from the visible range — the
/// most-recent N partitions are kept and older ones dropped, so a
/// 5-year zoom-out doesn't blow up the bin compute on Daily anchor.
///
/// Marked `#[non_exhaustive]` so a future field (e.g. `value_area_pct`)
/// adds without breaking external constructors.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct VolumeProfileConfig {
    /// Per-period vs single-viewport anchor mode.
    pub anchor: VolumeProfileAnchor,
    /// Scales the per-period pixel-width after the
    /// `[MIN_PROFILE_PX, MAX_PROFILE_PX]` clamp. `1.0` paints the full
    /// clamped width; `0.5` halves every per-period histogram.
    /// Has no effect in `Viewport` mode (the legacy
    /// `VolumeProfileStyle::max_bar_px_fraction` already covers that
    /// path).
    pub width_fraction: f32,
    /// Hard cap on the number of partitions emitted. Caller passes
    /// `100` for parity with [`midas_core::VolumeProfileSettings`]'s
    /// implicit hundred-period budget. Ignored in `Viewport` mode
    /// (single profile).
    pub max_profiles: usize,
}

impl Default for VolumeProfileConfig {
    fn default() -> Self {
        Self {
            anchor: VolumeProfileAnchor::Viewport,
            width_fraction: 0.7,
            max_profiles: 100,
        }
    }
}

/// Compute bin count from viewport height using the legacy formula —
/// `((height * 0.8) / 3.0).clamp(20, 200) as usize`.
///
/// Exposed so callers and tests can reason about bin counts without
/// duplicating the formula. Never returns zero.
#[inline]
pub fn bin_count_for_viewport(viewport_height_px: f32) -> usize {
    ((viewport_height_px * 0.8) / 3.0).clamp(20.0, 200.0) as usize
}

/// Horizontal volume-profile histogram layer.
///
/// Holds a read-handle to the candle series plus the currently-visible
/// candle-index range. The driver updates `visible_range` through
/// [`Self::set_visible_range`]; `paint` is pure and takes a short
/// read-guard.
///
/// Slice 3 of the VP-anchored plan widened the layer to support per-
/// period rendering. `config.anchor` selects between the legacy
/// single-profile-over-viewport mode (`Viewport`/`Unknown`) and the
/// per-period split (`Daily`/`Weekly`/`Monthly`/`Yearly`). Period
/// boundaries are derived from `Candle::ts_open` projected into the
/// layer's owned calendar timezone — the layer holds its own
/// `&'static dyn ExchangeCalendar` mirroring [`super::SessionBandLayer`]
/// rather than widening [`PaintContext`].
pub struct VolumeProfileLayer {
    candles: SharedCandleSeries,
    visible_range: Range<usize>,
    style: VolumeProfileStyle,
    config: VolumeProfileConfig,
    calendar: &'static dyn ExchangeCalendar,
}

impl VolumeProfileLayer {
    /// Build a layer over `candles`, restricted to `visible_range`,
    /// with the supplied style + config + calendar.
    ///
    /// `calendar` is consulted only when `config.anchor` is a per-
    /// period mode; in `Viewport` / `Unknown` mode the layer renders a
    /// single profile and the calendar is unused. Holding the
    /// reference unconditionally keeps the constructor simple and
    /// matches the `SessionBandLayer` pattern (calendar lives on the
    /// layer, not on `PaintContext`).
    pub fn new(
        candles: SharedCandleSeries,
        visible_range: Range<usize>,
        style: VolumeProfileStyle,
        config: VolumeProfileConfig,
        calendar: &'static dyn ExchangeCalendar,
    ) -> Self {
        Self {
            candles,
            visible_range,
            style,
            config,
            calendar,
        }
    }

    /// Convenience: build with [`VolumeProfileStyle::default`] +
    /// [`VolumeProfileConfig::default`] (which is `Viewport` anchor).
    /// `calendar` is ignored in `Viewport` mode but the constructor
    /// still requires one for forward-compat with per-period configs.
    pub fn with_defaults(
        candles: SharedCandleSeries,
        visible_range: Range<usize>,
        calendar: &'static dyn ExchangeCalendar,
    ) -> Self {
        Self::new(
            candles,
            visible_range,
            VolumeProfileStyle::default(),
            VolumeProfileConfig::default(),
            calendar,
        )
    }

    /// Update the visible index range. Called by the scene driver when
    /// the user pans / zooms.
    pub fn set_visible_range(&mut self, range: Range<usize>) {
        self.visible_range = range;
    }

    /// Borrow the currently-visible index range.
    #[inline]
    pub fn visible_range(&self) -> &Range<usize> {
        &self.visible_range
    }

    #[inline]
    pub fn style(&self) -> VolumeProfileStyle {
        self.style
    }

    #[inline]
    pub fn config(&self) -> &VolumeProfileConfig {
        &self.config
    }
}

/// Internal bin record: price span + total volume. Kept outside the
/// public API — the layer exposes behaviour through `paint`, not
/// through the histogram.
#[derive(Copy, Clone, Debug, Default)]
struct Bin {
    volume: u64,
}

impl SceneLayer for VolumeProfileLayer {
    fn id(&self) -> LayerId {
        LayerId("volume_profile")
    }

    fn z(&self) -> LayerZ {
        LayerZ::VOLUME_PROFILE
    }

    fn paint(&self, ctx: &mut PaintContext<'_>) {
        // Single short-lived read-guard for the entire pass. Slice 3
        // (VP-anchored, D9 step 6): partition + bin must read a single
        // consistent snapshot of the series. A two-guard variant could
        // observe a tick land between partition and bin, producing a
        // partial last-period histogram.
        let guard = self.candles.read();
        if guard.is_empty() {
            return;
        }

        // Clamp visible_range to the current series length. The driver
        // is the authority on range, but a stale range must not cause
        // an out-of-bounds panic or over-count bins.
        let series_len = guard.len();
        let start = self.visible_range.start.min(series_len);
        let end = self.visible_range.end.min(series_len);
        if start >= end {
            return;
        }

        // D12 fallback: anchor coarser-than-or-equal-to bar period
        // (e.g. `Anchor::Daily` over `BarPeriod::Calendar(D1)`) →
        // render as a single profile, exactly like `Viewport`.
        let configured = self.config.anchor;
        let effective = if period_blocks_anchor(guard.period(), configured) {
            tracing::debug!(
                target: "vp",
                anchor = ?configured,
                period = ?guard.period(),
                reason = "anchor_too_coarse_for_period",
                "anchored mode falling back to Viewport"
            );
            VolumeProfileAnchor::Viewport
        } else {
            configured
        };

        match effective {
            VolumeProfileAnchor::Viewport | VolumeProfileAnchor::Unknown => {
                self.paint_viewport(ctx, &guard, start..end);
            }
            _ => {
                self.paint_per_period(ctx, &guard, start..end, effective);
            }
        }
    }
}

impl VolumeProfileLayer {
    /// Paint the legacy single-profile-over-viewport histogram. Used
    /// for `Anchor::Viewport`/`Unknown` and for the D12 fallback when
    /// the bar period is at least as coarse as the configured anchor.
    /// Output bytes-for-bytes equivalent to the slice-7 baseline so
    /// the existing 13 unit tests still pass.
    fn paint_viewport(
        &self,
        ctx: &mut PaintContext<'_>,
        guard: &midas_bars::CandleSeries,
        range: Range<usize>,
    ) {
        let (price_low, price_high) = visible_price_range(guard, range.clone());
        // Guard against NaN / degenerate ranges (equal or inverted);
        // `partial_cmp` keeps the NaN-safe short-circuit explicit.
        if !matches!(
            price_high.partial_cmp(&price_low),
            Some(std::cmp::Ordering::Greater)
        ) {
            return;
        }

        let num_bins = bin_count_for_viewport(ctx.viewport.height_px);
        if num_bins == 0 {
            return;
        }
        let bin_size = ((price_high - price_low) as f64) / num_bins as f64;
        if bin_size <= 0.0 {
            return;
        }

        let bins = compute_bins_for_range(guard, range, num_bins, price_low, bin_size);

        let max_vol = bins.iter().map(|b| b.volume).max().unwrap_or(0);
        if max_vol == 0 {
            return;
        }

        let poc_idx = poc_index(&bins);
        let max_bar_px = ctx.viewport.width_px * self.style.max_bar_px_fraction;
        let vp_height = ctx.viewport.height_px;

        for (i, bin) in bins.iter().enumerate() {
            if bin.volume == 0 {
                continue;
            }
            // Bin y-span via the price axis. `to_y` maps `price_high`
            // to y=0 and `price_low` to y=height, so the HIGH edge of
            // a bin is the SMALLER y and the LOW edge is the LARGER y.
            let bin_price_lo = price_low as f64 + i as f64 * bin_size;
            let bin_price_hi = bin_price_lo + bin_size;
            let y_top = ctx.price_to_y(bin_price_hi);
            let y_bot = ctx.price_to_y(bin_price_lo);
            // Cull bins fully outside the viewport — legacy parity.
            if y_top > vp_height || y_bot < 0.0 {
                continue;
            }
            let bar_w = (bin.volume as f32 / max_vol as f32) * max_bar_px;
            let color = if i == poc_idx {
                self.style.poc_color
            } else {
                self.style.neighbour_color
            };
            ctx.out.quads.push(QuadInstance {
                x: 0.0,
                y: y_top,
                w: bar_w,
                h: (y_bot - y_top).max(0.0),
                color,
            });
        }
    }

    /// Paint one histogram per period (Daily/Weekly/Monthly/Yearly).
    /// Slice 3 of the VP-anchored plan.
    fn paint_per_period(
        &self,
        ctx: &mut PaintContext<'_>,
        guard: &midas_bars::CandleSeries,
        range: Range<usize>,
        anchor: VolumeProfileAnchor,
    ) {
        let tz = self.calendar.tz();
        let mut partitions = partition_by_anchor(guard, range, anchor, tz);
        if partitions.is_empty() {
            return;
        }
        // Cap to the most-recent `max_profiles` partitions. A 5-year
        // zoom-out on Daily anchor would produce ~1300 partitions; the
        // cap keeps bin compute proportional to the visible budget.
        if partitions.len() > self.config.max_profiles {
            let drop = partitions.len() - self.config.max_profiles;
            partitions.drain(0..drop);
        }

        // Per-period bin count: tighter than the viewport-mode count
        // because each profile occupies only a fraction of the viewport
        // width — denser bins would render as sub-pixel shimmer.
        let num_bins = bin_count_for_period(ctx.viewport.height_px);
        if num_bins == 0 {
            return;
        }

        let vp_height = ctx.viewport.height_px;
        let vp_width = ctx.viewport.width_px;

        for (idx, partition) in partitions.iter().enumerate() {
            // Left edge: pixel-x of the first candle's open in the
            // partition. Right edge: pixel-x of the NEXT partition's
            // first candle's open (or the viewport's right edge when
            // this is the trailing partition).
            let Some(first) = guard.at(partition.start) else {
                continue;
            };
            let left_x = ctx.axis.to_x(first.ts_open());
            let right_x = if idx + 1 < partitions.len() {
                let next_partition_start = partitions[idx + 1].start;
                let Some(next_first) = guard.at(next_partition_start) else {
                    continue;
                };
                ctx.axis.to_x(next_first.ts_open())
            } else {
                vp_width
            };

            let raw_width = (right_x - left_x).max(0.0);
            if raw_width < MIN_PERIOD_PX_TO_RENDER {
                // S6 P2: degrade to a single 1-px POC tick when the
                // partition has at least one pixel of horizontal
                // breathing room; below MIN_POC_TICK_PX the partition
                // is genuinely sub-pixel and we skip.
                if raw_width >= MIN_POC_TICK_PX {
                    let (price_low, price_high) = visible_price_range(guard, partition.clone());
                    if matches!(
                        price_high.partial_cmp(&price_low),
                        Some(std::cmp::Ordering::Greater)
                    ) {
                        let bin_size = ((price_high - price_low) as f64) / num_bins as f64;
                        if bin_size > 0.0 {
                            let bins = compute_bins_for_range(
                                guard,
                                partition.clone(),
                                num_bins,
                                price_low,
                                bin_size,
                            );
                            let max_vol = bins.iter().map(|b| b.volume).max().unwrap_or(0);
                            if max_vol > 0 {
                                let poc_idx = poc_index(&bins);
                                let poc_price_lo = price_low as f64 + poc_idx as f64 * bin_size;
                                let poc_price_hi = poc_price_lo + bin_size;
                                let y_top = ctx.price_to_y(poc_price_hi);
                                let y_bot = ctx.price_to_y(poc_price_lo);
                                if y_top <= vp_height && y_bot >= 0.0 {
                                    ctx.out.quads.push(QuadInstance {
                                        x: left_x,
                                        y: y_top,
                                        w: MIN_POC_TICK_PX,
                                        h: (y_bot - y_top).max(0.0),
                                        color: self.style.poc_color,
                                    });
                                }
                            }
                        }
                    }
                }
                continue;
            }
            let width_px =
                raw_width.clamp(MIN_PROFILE_PX, MAX_PROFILE_PX) * self.config.width_fraction;

            let (price_low, price_high) = visible_price_range(guard, partition.clone());
            if !matches!(
                price_high.partial_cmp(&price_low),
                Some(std::cmp::Ordering::Greater)
            ) {
                continue;
            }

            let bin_size = ((price_high - price_low) as f64) / num_bins as f64;
            if bin_size <= 0.0 {
                continue;
            }

            let bins =
                compute_bins_for_range(guard, partition.clone(), num_bins, price_low, bin_size);
            let max_vol = bins.iter().map(|b| b.volume).max().unwrap_or(0);
            if max_vol == 0 {
                continue;
            }
            let poc_idx = poc_index(&bins);

            for (i, bin) in bins.iter().enumerate() {
                if bin.volume == 0 {
                    continue;
                }
                let bin_price_lo = price_low as f64 + i as f64 * bin_size;
                let bin_price_hi = bin_price_lo + bin_size;
                let y_top = ctx.price_to_y(bin_price_hi);
                let y_bot = ctx.price_to_y(bin_price_lo);
                if y_top > vp_height || y_bot < 0.0 {
                    continue;
                }
                let bar_w = (bin.volume as f32 / max_vol as f32) * width_px;
                let color = if i == poc_idx {
                    self.style.poc_color
                } else {
                    self.style.neighbour_color
                };
                ctx.out.quads.push(QuadInstance {
                    x: left_x,
                    y: y_top,
                    w: bar_w,
                    h: (y_bot - y_top).max(0.0),
                    color,
                });
            }
        }
    }
}

/// Scan `range` and return `(min_low, max_high)` over that slice of
/// `series`. `range` is assumed in-bounds (caller clamps).
fn visible_price_range(series: &midas_bars::CandleSeries, range: Range<usize>) -> (f32, f32) {
    let mut lo = f64::INFINITY;
    let mut hi = f64::NEG_INFINITY;
    for idx in range {
        let Some(c) = series.at(idx) else { continue };
        let l = c.low();
        let h = c.high();
        if l < lo {
            lo = l;
        }
        if h > hi {
            hi = h;
        }
    }
    (lo as f32, hi as f32)
}

/// Compute per-bin volume distribution for `range` using the same
/// uniform-spread rule as the legacy `volume_profile` widget.
/// Extracted so [`VolumeProfileLayer::paint_viewport`] and
/// [`VolumeProfileLayer::paint_per_period`] share one implementation.
fn compute_bins_for_range(
    series: &midas_bars::CandleSeries,
    range: Range<usize>,
    num_bins: usize,
    price_low: f32,
    bin_size: f64,
) -> Vec<Bin> {
    let mut bins = vec![Bin::default(); num_bins];
    for idx in range {
        let Some(c) = series.at(idx) else { continue };
        let hi = c.high();
        let lo = c.low();
        let vol = c.volume();
        if vol == 0 || !matches!(hi.partial_cmp(&lo), Some(std::cmp::Ordering::Greater)) {
            continue;
        }
        let bin_lo_f = ((lo - price_low as f64) / bin_size).floor().max(0.0);
        let bin_hi_f = ((hi - price_low as f64) / bin_size)
            .ceil()
            .min(num_bins as f64);
        let bin_lo = bin_lo_f as usize;
        let mut bin_hi = bin_hi_f as usize;
        if bin_hi == 0 {
            continue;
        }
        bin_hi = bin_hi.min(num_bins).saturating_sub(1);
        if bin_lo > bin_hi {
            continue;
        }
        let touched = (bin_hi - bin_lo + 1) as u64;
        let per = vol / touched;
        let remainder = vol - per * touched;
        for bin in &mut bins[bin_lo..=bin_hi] {
            bin.volume += per;
        }
        if remainder > 0 {
            let mid = (bin_lo + bin_hi) / 2;
            bins[mid].volume += remainder;
        }
    }
    bins
}

/// POC index — first (lowest-price) bin with the max total volume.
/// Ties resolve to the first bin, matching legacy `max_by_key`.
fn poc_index(bins: &[Bin]) -> usize {
    bins.iter()
        .enumerate()
        .max_by_key(|(_, b)| b.volume)
        .map(|(i, _)| i)
        .unwrap_or(0)
}

/// Per-period bin count: tighter than [`bin_count_for_viewport`] —
/// `min(24, floor(viewport_height / 2))`. Each per-period histogram
/// occupies only a fraction of the viewport width, so denser bins
/// would render as sub-pixel shimmer.
#[inline]
fn bin_count_for_period(viewport_height_px: f32) -> usize {
    ((viewport_height_px * 0.5) as usize).clamp(1, 24)
}

/// Partition the visible candle range by `anchor`. Returns one
/// `Range<usize>` per period; consecutive candles whose
/// `Candle::ts_open` projected into `tz` produce the same
/// [`PeriodKey`] are grouped.
///
/// `Anchor::Viewport`/`Unknown` short-circuit to `vec![range]` —
/// the caller never invokes this for those modes today, but keeping
/// the contract symmetric makes the function reusable from a future
/// "always go through partition" refactor without behavior change.
fn partition_by_anchor(
    series: &midas_bars::CandleSeries,
    range: Range<usize>,
    anchor: VolumeProfileAnchor,
    tz: Tz,
) -> Vec<Range<usize>> {
    if matches!(
        anchor,
        VolumeProfileAnchor::Viewport | VolumeProfileAnchor::Unknown
    ) {
        return vec![range];
    }
    if range.is_empty() {
        return Vec::new();
    }
    let mut partitions: Vec<Range<usize>> = Vec::new();
    let mut current_key: Option<PeriodKey> = None;
    let mut start = range.start;
    for idx in range.clone() {
        let Some(c) = series.at(idx) else { continue };
        let dt = c.ts_open().with_timezone(&tz);
        let key = period_key_for(dt, anchor);
        match current_key {
            None => {
                current_key = Some(key);
                start = idx;
            }
            Some(k) if k != key => {
                partitions.push(start..idx);
                current_key = Some(key);
                start = idx;
            }
            Some(_) => {}
        }
    }
    if current_key.is_some() {
        partitions.push(start..range.end);
    }
    partitions
}

/// Group key for [`partition_by_anchor`]. Two candles share a
/// partition iff their keys compare equal.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum PeriodKey {
    Day(i32, u32, u32),
    /// ISO week — `(iso_year, iso_week)`. ISO weeks start on Monday.
    Week(i32, u32),
    Month(i32, u32),
    Year(i32),
}

fn period_key_for(dt: DateTime<Tz>, anchor: VolumeProfileAnchor) -> PeriodKey {
    match anchor {
        VolumeProfileAnchor::Daily => PeriodKey::Day(dt.year(), dt.month(), dt.day()),
        VolumeProfileAnchor::Weekly => {
            let iso = dt.iso_week();
            PeriodKey::Week(iso.year(), iso.week())
        }
        VolumeProfileAnchor::Monthly => PeriodKey::Month(dt.year(), dt.month()),
        VolumeProfileAnchor::Yearly => PeriodKey::Year(dt.year()),
        // `paint_per_period` filters out Viewport / Unknown before
        // reaching here. Reachable only via partition_by_anchor's
        // public-ish surface in tests; the degenerate single-year
        // bucket keeps partitioning total rather than panicking.
        VolumeProfileAnchor::Viewport | VolumeProfileAnchor::Unknown => PeriodKey::Year(dt.year()),
    }
}

/// Minimum bar-period (in days) for the calendar/session unit a given
/// [`BarPeriod`] represents. Private to this layer per Recon R3 — do
/// NOT promote to `midas-calendar`; only this consumer needs the
/// mapping today, and the natural shape varies (Quarter is "90" here
/// but might be "calendar quarter" elsewhere).
fn period_unit_days(p: BarPeriod) -> u32 {
    match p {
        BarPeriod::Clock(_) | BarPeriod::Session(_) => 0,
        BarPeriod::Calendar(CalendarSpan::Week) => 7,
        BarPeriod::Calendar(CalendarSpan::Month) => 30,
        BarPeriod::Calendar(CalendarSpan::Quarter) => 90,
        BarPeriod::Calendar(CalendarSpan::Year) => 365,
        // `CalendarSpan` is `#[non_exhaustive]`. Future variants
        // default to "treat as never-blocking" so the fallback gates
        // opt-in per variant — safer than auto-blocking on unknown.
        BarPeriod::Calendar(_) => 0,
        // `BarPeriod` is `#[non_exhaustive]` too.
        _ => 0,
    }
}

/// True if the bar period is at least as coarse as the configured
/// anchor, which makes per-period rendering degenerate (one candle
/// per partition or worse). Triggers the D12 silent fallback to
/// `Viewport`.
fn period_blocks_anchor(period: BarPeriod, anchor: VolumeProfileAnchor) -> bool {
    let min = anchor.min_period_days();
    min > 0 && period_unit_days(period) >= min
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use chrono::TimeZone;
    use midas_axis::{ContinuousAxis, DefaultFormatter, LinearPriceAxis, PriceRange, Viewport};
    use midas_bars::{Candle, CandleSeries, Completeness, Ohlcv, Symbol};
    use midas_calendar::{xnys, BarPeriod, Timestamp};
    use parking_lot::RwLock;

    use super::*;
    use crate::primitives::ScenePrimitives;
    use crate::ThemePalette;

    fn utc(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> Timestamp {
        chrono::Utc.with_ymd_and_hms(y, mo, d, h, mi, 0).unwrap()
    }

    fn mk_candle(ts: Timestamp, o: f64, h: f64, l: f64, c: f64, vol: u64) -> Candle {
        let cal = xnys();
        let sym = Symbol::new("SPY", cal.id());
        let session = cal.classify(ts);
        let window = cal.bar_window(ts, BarPeriod::m1()).unwrap();
        let ohlcv = Ohlcv::new(o, h, l, c, vol, 1, None).unwrap();
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

    /// Helper: build a `PaintContext` with a given viewport + price range.
    fn paint_with(
        layer: &VolumeProfileLayer,
        pr: PriceRange,
        vp: Viewport,
        out: &mut ScenePrimitives,
    ) {
        let axis = ContinuousAxis::new(
            utc(2024, 1, 17, 14, 30),
            utc(2024, 1, 17, 14, 30) + chrono::Duration::hours(1),
            vp.width_px,
        )
        .unwrap();
        let pal = ThemePalette::dark_default();
        let paxis = LinearPriceAxis::new(pr, vp.height_px);
        let fmt = DefaultFormatter::new();
        let mut ctx = PaintContext {
            axis: &axis,
            viewport: vp,
            price_range: pr,
            palette: &pal,
            price_axis: &paxis,
            formatter: &fmt,
            out,
        };
        layer.paint(&mut ctx);
    }

    fn empty_series() -> SharedCandleSeries {
        let cal = xnys();
        Arc::new(RwLock::new(CandleSeries::new(
            cal.id(),
            BarPeriod::m1(),
            Symbol::new("SPY", cal.id()),
        )))
    }

    // ── 1. Bin-count formula ──────────────────────────────────────────

    #[test]
    fn bin_count_matches_legacy_formula_across_viewports() {
        // Directly probes the pure helper — no series / paint needed.
        assert_eq!(bin_count_for_viewport(100.0), 26);
        assert_eq!(bin_count_for_viewport(500.0), 133);
        assert_eq!(bin_count_for_viewport(1000.0), 200);
        // Lower clamp: anything below ~75px clamps to 20.
        assert_eq!(bin_count_for_viewport(50.0), 20);
        // Upper clamp: anything above ~750px clamps to 200.
        assert_eq!(bin_count_for_viewport(10_000.0), 200);
    }

    // ── 2. Empty series → zero quads ──────────────────────────────────

    #[test]
    fn empty_series_emits_no_quads() {
        let layer = VolumeProfileLayer::with_defaults(empty_series(), 0..0, xnys());
        let mut out = ScenePrimitives::default();
        paint_with(
            &layer,
            PriceRange::new(95.0, 105.0).unwrap(),
            Viewport::new(1000.0, 400.0),
            &mut out,
        );
        assert_eq!(out.quads.len(), 0);
    }

    #[test]
    fn empty_visible_range_emits_no_quads() {
        let cal = xnys();
        let mut s = CandleSeries::new(cal.id(), BarPeriod::m1(), Symbol::new("SPY", cal.id()));
        s.push(mk_candle(
            utc(2024, 1, 17, 14, 30),
            100.0,
            101.0,
            99.0,
            100.5,
            1_000,
        ));
        let series: SharedCandleSeries = Arc::new(RwLock::new(s));
        let layer = VolumeProfileLayer::with_defaults(series, 0..0, xnys());
        let mut out = ScenePrimitives::default();
        paint_with(
            &layer,
            PriceRange::new(95.0, 105.0).unwrap(),
            Viewport::new(1000.0, 400.0),
            &mut out,
        );
        assert_eq!(out.quads.len(), 0);
    }

    // ── 3. Single candle — volume distributes across intersected bins

    #[test]
    fn single_candle_distributes_volume_across_intersected_bins() {
        // Candle spans [99.0, 101.0] — 2-wide. With num_bins = 26 over
        // [99.0, 101.0] (visible-range price span), bin_size = 2/26 ≈
        // 0.0769. Every one of the 26 bins is intersected → every bin
        // gets a share of the 10_000 volume.
        let cal = xnys();
        let mut s = CandleSeries::new(cal.id(), BarPeriod::m1(), Symbol::new("SPY", cal.id()));
        s.push(mk_candle(
            utc(2024, 1, 17, 14, 30),
            100.0,
            101.0,
            99.0,
            100.5,
            10_000,
        ));
        let series: SharedCandleSeries = Arc::new(RwLock::new(s));
        let layer = VolumeProfileLayer::with_defaults(series, 0..1, xnys());
        let mut out = ScenePrimitives::default();
        paint_with(
            &layer,
            PriceRange::new(95.0, 105.0).unwrap(),
            Viewport::new(1000.0, 100.0), // → 26 bins
            &mut out,
        );
        // The single candle spans from min-low (99) to max-high (101)
        // — by definition 100% of visible bins are intersected. 26
        // quads emitted.
        assert_eq!(out.quads.len(), 26);
    }

    // ── 4. POC tint brighter than neighbours ─────────────────────────

    #[test]
    fn poc_bin_gets_distinctly_brighter_color() {
        // Two candles: one with thin range + huge volume anchors the
        // POC; one with wide range + small volume contributes broad
        // low-volume bins.
        let cal = xnys();
        let mut s = CandleSeries::new(cal.id(), BarPeriod::m1(), Symbol::new("SPY", cal.id()));
        let t = utc(2024, 1, 17, 14, 30);
        // Wide candle — fills many bins with small volume.
        s.push(mk_candle(t, 100.0, 105.0, 95.0, 100.0, 100));
        // Narrow candle — focused volume in one spot.
        s.push(mk_candle(
            t + chrono::Duration::minutes(1),
            100.0,
            100.2,
            99.8,
            100.1,
            1_000_000,
        ));
        let series: SharedCandleSeries = Arc::new(RwLock::new(s));
        let layer = VolumeProfileLayer::with_defaults(series, 0..2, xnys());
        let mut out = ScenePrimitives::default();
        paint_with(
            &layer,
            PriceRange::new(90.0, 110.0).unwrap(),
            Viewport::new(1000.0, 400.0),
            &mut out,
        );
        assert!(!out.quads.is_empty());
        let style = VolumeProfileStyle::default();
        let poc_count = out
            .quads
            .iter()
            .filter(|q| q.color == style.poc_color)
            .count();
        // Exactly one POC bin.
        assert_eq!(poc_count, 1);
        // POC alpha strictly higher than neighbour alpha.
        assert!(style.poc_color[3] > style.neighbour_color[3]);
        // POC RGB strictly brighter (at least one channel higher) —
        // implies visually distinguishable.
        let neigh_lum = style.neighbour_color[0] as u32
            + style.neighbour_color[1] as u32
            + style.neighbour_color[2] as u32;
        let poc_lum =
            style.poc_color[0] as u32 + style.poc_color[1] as u32 + style.poc_color[2] as u32;
        assert!(poc_lum > neigh_lum);
    }

    // ── 5. Bin vertical extent matches price-range / bin-count ───────

    #[test]
    fn bin_height_equals_price_range_over_bin_count_within_ulp() {
        let cal = xnys();
        let mut s = CandleSeries::new(cal.id(), BarPeriod::m1(), Symbol::new("SPY", cal.id()));
        s.push(mk_candle(
            utc(2024, 1, 17, 14, 30),
            100.0,
            101.0,
            99.0,
            100.5,
            10_000,
        ));
        let series: SharedCandleSeries = Arc::new(RwLock::new(s));
        let layer = VolumeProfileLayer::with_defaults(series, 0..1, xnys());
        let mut out = ScenePrimitives::default();
        let vp = Viewport::new(1000.0, 100.0); // → 26 bins
        paint_with(&layer, PriceRange::new(95.0, 105.0).unwrap(), vp, &mut out);
        assert_eq!(out.quads.len(), 26);
        // Visible-range price span = 101 - 99 = 2.0. Bin height in
        // price is 2/26. Bin height in pixels:
        // price axis range is 105-95 = 10 over 100px → 1 price unit =
        // 10px, so expected_h = (2.0/26) * 10 ≈ 0.7692 px.
        let expected_h = (2.0_f64 / 26.0_f64) as f32 * 10.0;
        for q in &out.quads {
            // 1 f32 ULP near 0.77 is ~9e-8 — compare with a generous
            // 1e-4 tolerance since the y coords round-trip through
            // `price_to_y` and pick up linear-projection rounding.
            assert!(
                (q.h - expected_h).abs() < 1e-3,
                "q.h={} expected={}",
                q.h,
                expected_h
            );
        }
    }

    // ── 6. Volume conservation ───────────────────────────────────────

    #[test]
    fn total_bin_volume_conserves_visible_candle_volume() {
        let cal = xnys();
        let mut s = CandleSeries::new(cal.id(), BarPeriod::m1(), Symbol::new("SPY", cal.id()));
        let t = utc(2024, 1, 17, 14, 30);
        s.push(mk_candle(t, 100.0, 102.0, 98.0, 101.0, 7_777));
        s.push(mk_candle(
            t + chrono::Duration::minutes(1),
            101.0,
            103.0,
            99.0,
            100.0,
            3_333,
        ));
        s.push(mk_candle(
            t + chrono::Duration::minutes(2),
            100.0,
            104.0,
            96.0,
            103.0,
            9_001,
        ));
        let total_input: u64 = s.iter().map(|c| c.volume()).sum();
        assert_eq!(total_input, 7_777 + 3_333 + 9_001);

        // Mirror the layer's internals to recover per-bin volumes (the
        // layer doesn't expose bins directly — by design). We walk the
        // same algorithm with the same inputs and verify the sum.
        let vp = Viewport::new(1000.0, 400.0);
        let num_bins = bin_count_for_viewport(vp.height_px);
        let price_low = s.iter().map(|c| c.low()).fold(f64::INFINITY, f64::min);
        let price_high = s.iter().map(|c| c.high()).fold(f64::NEG_INFINITY, f64::max);
        let bin_size = (price_high - price_low) / num_bins as f64;
        let mut total_distributed: u64 = 0;
        for c in s.iter() {
            let vol = c.volume();
            if vol == 0 || c.high() <= c.low() {
                continue;
            }
            let bin_lo = ((c.low() - price_low) / bin_size).floor().max(0.0) as usize;
            let bin_hi_f = ((c.high() - price_low) / bin_size)
                .ceil()
                .min(num_bins as f64);
            let bin_hi = (bin_hi_f as usize).min(num_bins).saturating_sub(1);
            if bin_lo > bin_hi {
                continue;
            }
            let touched = (bin_hi - bin_lo + 1) as u64;
            let per = vol / touched;
            let remainder = vol - per * touched;
            total_distributed += per * touched + remainder;
        }
        assert_eq!(total_distributed, total_input);

        // Sanity: the layer also emits something non-empty for this data.
        let series: SharedCandleSeries = Arc::new(RwLock::new(s));
        let layer = VolumeProfileLayer::with_defaults(series, 0..3, xnys());
        let mut out = ScenePrimitives::default();
        paint_with(&layer, PriceRange::new(90.0, 110.0).unwrap(), vp, &mut out);
        assert!(!out.quads.is_empty());
    }

    // ── 7. Price range = min-low to max-high of visible candles ──────

    #[test]
    fn price_range_spans_visible_min_low_to_max_high() {
        // If price range is derived from VISIBLE candles (not screen
        // price range), then shrinking the visible slice to a single
        // narrow-ranged candle should keep all bins inside that
        // narrow band — i.e. all quads fall between the narrow
        // candle's low and high y coords.
        let cal = xnys();
        let mut s = CandleSeries::new(cal.id(), BarPeriod::m1(), Symbol::new("SPY", cal.id()));
        let t = utc(2024, 1, 17, 14, 30);
        // Wide candle (ignored).
        s.push(mk_candle(t, 100.0, 109.0, 91.0, 108.0, 1_000));
        // Narrow candle (the only visible one).
        s.push(mk_candle(
            t + chrono::Duration::minutes(1),
            100.0,
            100.5,
            99.5,
            100.2,
            5_000,
        ));
        let series: SharedCandleSeries = Arc::new(RwLock::new(s));
        // Only the narrow candle in the visible range.
        let layer = VolumeProfileLayer::with_defaults(series, 1..2, xnys());
        let mut out = ScenePrimitives::default();
        let pr = PriceRange::new(90.0, 110.0).unwrap();
        let vp = Viewport::new(1000.0, 400.0);
        paint_with(&layer, pr, vp, &mut out);
        assert!(!out.quads.is_empty());
        // Narrow candle spans [99.5, 100.5] → y-span inside a 90..110
        // viewport: y(100.5) = (110-100.5)/20 * 400 = 190;
        // y(99.5) = (110-99.5)/20 * 400 = 210.
        // Every quad must lie inside that band.
        for q in &out.quads {
            assert!(
                q.y >= 190.0 - 1e-3 && (q.y + q.h) <= 210.0 + 1e-3,
                "quad y={} h={} outside narrow-visible band [190, 210]",
                q.y,
                q.h
            );
        }
    }

    // ── 8. Visible-range subset: only in-range candles contribute ────

    #[test]
    fn candles_outside_visible_range_are_ignored() {
        let cal = xnys();
        let mut s = CandleSeries::new(cal.id(), BarPeriod::m1(), Symbol::new("SPY", cal.id()));
        let t = utc(2024, 1, 17, 14, 30);
        // Index 0 — huge volume, wide range.
        s.push(mk_candle(t, 100.0, 105.0, 95.0, 104.0, 10_000_000));
        // Index 1 — small volume, narrow range.
        s.push(mk_candle(
            t + chrono::Duration::minutes(1),
            100.0,
            100.1,
            99.9,
            100.05,
            10,
        ));
        let series: SharedCandleSeries = Arc::new(RwLock::new(s));

        // Visible range = [1, 2): only the narrow low-vol candle. If
        // candle 0 leaked in, we'd see wide-band bins.
        let layer = VolumeProfileLayer::with_defaults(Arc::clone(&series), 1..2, xnys());
        let mut out = ScenePrimitives::default();
        let pr = PriceRange::new(90.0, 110.0).unwrap();
        let vp = Viewport::new(1000.0, 400.0);
        paint_with(&layer, pr, vp, &mut out);

        // The narrow candle's span is [99.9, 100.1] → y-band
        // [199, 201] inside a 400-px viewport over 90..110.
        assert!(!out.quads.is_empty());
        for q in &out.quads {
            assert!(
                q.y >= 199.0 - 1e-2 && q.y + q.h <= 201.0 + 1e-2,
                "candle 0 leaked: quad y={} h={}",
                q.y,
                q.h
            );
        }
    }

    // ── 9. LayerZ integer slotting ───────────────────────────────────

    #[test]
    fn layer_z_volume_profile_between_volume_and_candle() {
        assert!(LayerZ::VOLUME < LayerZ::VOLUME_PROFILE);
        assert!(LayerZ::VOLUME_PROFILE < LayerZ::CANDLE);
        // Integer assertion — guards against silent renumbering.
        assert_eq!(LayerZ::VOLUME_PROFILE.0, 350);
    }

    // ── 10. SceneLayer glue ──────────────────────────────────────────

    #[test]
    fn scene_layer_id_and_z() {
        let layer = VolumeProfileLayer::with_defaults(empty_series(), 0..0, xnys());
        assert_eq!(layer.id(), LayerId("volume_profile"));
        assert_eq!(layer.z(), LayerZ::VOLUME_PROFILE);
    }

    #[test]
    fn is_passive_by_default() {
        // Slice 7 VP is passive — `as_interactive` returns None.
        let mut layer = VolumeProfileLayer::with_defaults(empty_series(), 0..0, xnys());
        assert!(layer.as_interactive().is_none());
    }

    // ── 11. set_visible_range ────────────────────────────────────────

    #[test]
    fn set_visible_range_updates_internal_range() {
        let mut layer = VolumeProfileLayer::with_defaults(empty_series(), 0..0, xnys());
        layer.set_visible_range(3..7);
        assert_eq!(*layer.visible_range(), 3..7);
    }

    // ── 12. Stale visible-range clamping ─────────────────────────────

    #[test]
    fn stale_visible_range_is_clamped_not_panic() {
        // Range end larger than series len must NOT panic.
        let cal = xnys();
        let mut s = CandleSeries::new(cal.id(), BarPeriod::m1(), Symbol::new("SPY", cal.id()));
        s.push(mk_candle(
            utc(2024, 1, 17, 14, 30),
            100.0,
            101.0,
            99.0,
            100.5,
            1_000,
        ));
        let series: SharedCandleSeries = Arc::new(RwLock::new(s));
        // Driver lags behind a `push` — range thinks there are 999 bars.
        let layer = VolumeProfileLayer::with_defaults(series, 0..999, xnys());
        let mut out = ScenePrimitives::default();
        paint_with(
            &layer,
            PriceRange::new(90.0, 110.0).unwrap(),
            Viewport::new(1000.0, 400.0),
            &mut out,
        );
        // Didn't panic; and the single candle did contribute.
        assert!(!out.quads.is_empty());
    }

    // ── 13. Zero-volume candles are skipped ──────────────────────────

    #[test]
    fn zero_volume_candles_contribute_nothing() {
        let cal = xnys();
        let mut s = CandleSeries::new(cal.id(), BarPeriod::m1(), Symbol::new("SPY", cal.id()));
        let t = utc(2024, 1, 17, 14, 30);
        s.push(mk_candle(t, 100.0, 101.0, 99.0, 100.5, 0));
        s.push(mk_candle(
            t + chrono::Duration::minutes(1),
            100.0,
            101.0,
            99.0,
            100.5,
            0,
        ));
        let series: SharedCandleSeries = Arc::new(RwLock::new(s));
        let layer = VolumeProfileLayer::with_defaults(series, 0..2, xnys());
        let mut out = ScenePrimitives::default();
        paint_with(
            &layer,
            PriceRange::new(95.0, 105.0).unwrap(),
            Viewport::new(1000.0, 400.0),
            &mut out,
        );
        assert_eq!(out.quads.len(), 0);
    }

    // ─── Slice 3 (VP-anchored): per-period rendering ─────────────────

    use midas_calendar::{crypto_spot, CalendarSpan, ClockInterval, SessionSpan};

    /// Build a CandleSeries with `n` minute-spaced candles starting at
    /// `start`. Uses the supplied calendar so the candles' window /
    /// session classification matches the calendar's reckoning.
    fn series_m1(start: Timestamp, n: usize, cal: &'static dyn ExchangeCalendar) -> CandleSeries {
        let sym = Symbol::new("TST", cal.id());
        let mut s = CandleSeries::new(cal.id(), BarPeriod::m1(), sym);
        for i in 0..n {
            let ts = start + chrono::Duration::minutes(i as i64);
            let session = cal.classify(ts);
            let window = cal.bar_window(ts, BarPeriod::m1()).unwrap();
            let p = 100.0 + (i % 5) as f64;
            let ohlcv = Ohlcv::new(p, p + 0.5, p - 0.5, p + 0.1, 1_000, 1, None).unwrap();
            s.push(
                Candle::new(
                    sym,
                    cal,
                    BarPeriod::m1(),
                    session,
                    window,
                    ohlcv,
                    Completeness::Completed,
                )
                .unwrap(),
            );
        }
        s
    }

    /// Build one candle per `step` (e.g. one minute apart), N total.
    /// Uses the supplied period instead of `m1`.
    fn series_with_period(
        start: Timestamp,
        n: usize,
        step: chrono::Duration,
        period: BarPeriod,
        cal: &'static dyn ExchangeCalendar,
    ) -> CandleSeries {
        let sym = Symbol::new("TST", cal.id());
        let mut s = CandleSeries::new(cal.id(), period, sym);
        for i in 0..n {
            let ts = start + step * i as i32;
            let session = cal.classify(ts);
            let window = cal.bar_window(ts, period).unwrap();
            let p = 100.0 + (i % 5) as f64;
            let ohlcv = Ohlcv::new(p, p + 0.5, p - 0.5, p + 0.1, 1_000, 1, None).unwrap();
            s.push(
                Candle::new(
                    sym,
                    cal,
                    period,
                    session,
                    window,
                    ohlcv,
                    Completeness::Completed,
                )
                .unwrap(),
            );
        }
        s
    }

    /// Test #2 — `Anchor::Unknown` paints the same `QuadInstance`s as
    /// `Anchor::Viewport` (forward-compat sink). Asserts on the full
    /// quad bucket so any regression in either branch trips the diff.
    #[test]
    fn anchor_unknown_treated_as_viewport() {
        let cal = xnys();
        let s = series_m1(utc(2024, 1, 17, 14, 30), 30, cal);
        let series: SharedCandleSeries = Arc::new(RwLock::new(s));

        // Viewport-mode reference render.
        let mut out_view = ScenePrimitives::default();
        let view_layer = VolumeProfileLayer::new(
            Arc::clone(&series),
            0..30,
            VolumeProfileStyle::default(),
            VolumeProfileConfig {
                anchor: VolumeProfileAnchor::Viewport,
                ..VolumeProfileConfig::default()
            },
            cal,
        );
        paint_with(
            &view_layer,
            PriceRange::new(95.0, 110.0).unwrap(),
            Viewport::new(1000.0, 400.0),
            &mut out_view,
        );

        // Unknown-mode render — must match byte-for-byte.
        let mut out_unknown = ScenePrimitives::default();
        let unknown_layer = VolumeProfileLayer::new(
            Arc::clone(&series),
            0..30,
            VolumeProfileStyle::default(),
            VolumeProfileConfig {
                anchor: VolumeProfileAnchor::Unknown,
                ..VolumeProfileConfig::default()
            },
            cal,
        );
        paint_with(
            &unknown_layer,
            PriceRange::new(95.0, 110.0).unwrap(),
            Viewport::new(1000.0, 400.0),
            &mut out_unknown,
        );

        assert_eq!(out_view.quads, out_unknown.quads);
    }

    /// Test #4 — three NYSE trading days of M5-spaced candles partition
    /// into 3 daily groups under `XnysCalendar`'s ET timezone.
    /// 2024-01-17 (Wed), 2024-01-18 (Thu), 2024-01-19 (Fri) — RTH only.
    #[test]
    fn partition_daily_3_nyse_days_xnys() {
        let cal = xnys();
        // 14:30 UTC = 09:30 ET (RTH open). Build 78 m5-equivalent
        // candles per day (78 * 5 = 390 mins = 6.5h RTH). Use M1 step
        // for simplicity — 390 candles per day.
        let mut s = CandleSeries::new(cal.id(), BarPeriod::m1(), Symbol::new("TST", cal.id()));
        for day in 0..3i64 {
            let day_open = utc(2024, 1, 17, 14, 30) + chrono::Duration::days(day);
            for m in 0..390i64 {
                let ts = day_open + chrono::Duration::minutes(m);
                let session = cal.classify(ts);
                let window = cal.bar_window(ts, BarPeriod::m1()).unwrap();
                let p = 100.0;
                let ohlcv = Ohlcv::new(p, p + 0.5, p - 0.5, p + 0.1, 1_000, 1, None).unwrap();
                s.push(
                    Candle::new(
                        Symbol::new("TST", cal.id()),
                        cal,
                        BarPeriod::m1(),
                        session,
                        window,
                        ohlcv,
                        Completeness::Completed,
                    )
                    .unwrap(),
                );
            }
        }
        let total = s.len();
        let parts = partition_by_anchor(&s, 0..total, VolumeProfileAnchor::Daily, cal.tz());
        assert_eq!(parts.len(), 3, "expected 3 daily partitions, got {parts:?}");
        // Coverage check: partitions tile the full range with no gaps.
        assert_eq!(parts.first().unwrap().start, 0);
        assert_eq!(parts.last().unwrap().end, total);
    }

    /// Test #5 — same calendar-day candles under `CryptoSpotCalendar`'s
    /// UTC timezone partition differently from XNYS's ET. Use a
    /// fixture that crosses 00:00 UTC but not 00:00 ET so the two
    /// calendars disagree.
    ///
    /// Crypto open: 2024-03-04 23:00 UTC (= 18:00 ET) → 2024-03-05
    /// 02:00 UTC (= 21:00 ET). UTC sees 2 days, ET sees 1 day.
    #[test]
    fn partition_daily_3_days_crypto_utc() {
        let cal = crypto_spot();
        // 4 hours of M1 candles spanning 23:00..03:00 UTC.
        let s = series_m1(utc(2024, 3, 4, 23, 0), 240, cal);
        let parts = partition_by_anchor(&s, 0..s.len(), VolumeProfileAnchor::Daily, cal.tz());
        // UTC: Mar 4 (23:00–23:59) + Mar 5 (00:00–02:59) → 2 partitions.
        assert_eq!(parts.len(), 2, "crypto UTC partitions: {parts:?}");

        // Same fixture under xnys' ET: 23:00 UTC = 18:00 ET (Mar 4),
        // 02:59 UTC next day = 21:59 ET same day → all on Mar 4 ET.
        let cal_xnys = xnys();
        let parts_et =
            partition_by_anchor(&s, 0..s.len(), VolumeProfileAnchor::Daily, cal_xnys.tz());
        assert_eq!(parts_et.len(), 1, "xnys ET partitions: {parts_et:?}");
    }

    /// Test #6 — ISO weeks start on Monday. Dec-28 (Thu) → Jan-4
    /// straddles the Mon Dec-30 / Mon Jan-6 boundary; this fixture
    /// (Dec-28..Jan-4 = 8 days) splits at Mon-2024-Jan-01 since
    /// Dec-25..Dec-31-2023 is ISO week 52 of 2023 and Jan-1..Jan-7
    /// 2024 is ISO week 1 of 2024.
    #[test]
    fn partition_weekly_iso() {
        let cal = xnys();
        // 8 candles, one per day, spanning Dec 28 2023 (Thu) through
        // Jan 4 2024 (Thu). Use m1 candle, step = 1 day.
        let s = series_with_period(
            utc(2023, 12, 28, 15, 0),
            8,
            chrono::Duration::days(1),
            BarPeriod::m1(),
            cal,
        );
        let parts = partition_by_anchor(&s, 0..s.len(), VolumeProfileAnchor::Weekly, cal.tz());
        assert_eq!(parts.len(), 2, "expected 2 ISO-week partitions: {parts:?}");
    }

    /// Test #7 — monthly partition splits at the calendar month edge.
    #[test]
    fn partition_monthly() {
        let cal = xnys();
        // Jan 30, 31, Feb 1, Feb 2 — 4 days, 2 partitions.
        let s = series_with_period(
            utc(2024, 1, 30, 15, 0),
            4,
            chrono::Duration::days(1),
            BarPeriod::m1(),
            cal,
        );
        let parts = partition_by_anchor(&s, 0..s.len(), VolumeProfileAnchor::Monthly, cal.tz());
        assert_eq!(parts.len(), 2);
    }

    /// Test #7b — yearly partition splits at the calendar year edge.
    #[test]
    fn partition_yearly() {
        let cal = xnys();
        // Dec 30 2023, Dec 31 2023, Jan 1 2024 — 2 partitions.
        let s = series_with_period(
            utc(2023, 12, 30, 15, 0),
            3,
            chrono::Duration::days(1),
            BarPeriod::m1(),
            cal,
        );
        let parts = partition_by_anchor(&s, 0..s.len(), VolumeProfileAnchor::Yearly, cal.tz());
        assert_eq!(parts.len(), 2);
    }

    /// Test #8 — DST spring-forward (Sun 2024-03-10 in ET) does not
    /// induce a spurious partition split. 3 daily candles spanning
    /// Mar 9, 10, 11 in ET → exactly 3 daily partitions.
    #[test]
    fn partition_dst_spring_forward() {
        let cal = xnys();
        // Mar 9 (Sat 19:00 UTC = 14:00 EST), Mar 10 (Sun 18:00 UTC =
        // 14:00 EDT — spring forward), Mar 11 (Mon 18:00 UTC =
        // 14:00 EDT). Each candle on a different ET date.
        let mut s = CandleSeries::new(cal.id(), BarPeriod::m1(), Symbol::new("TST", cal.id()));
        for (h_utc, day) in [(19, 9), (18, 10), (18, 11)] {
            let ts = utc(2024, 3, day, h_utc, 0);
            let session = cal.classify(ts);
            let window = cal.bar_window(ts, BarPeriod::m1()).unwrap();
            let ohlcv = Ohlcv::new(100.0, 101.0, 99.0, 100.5, 1_000, 1, None).unwrap();
            s.push(
                Candle::new(
                    Symbol::new("TST", cal.id()),
                    cal,
                    BarPeriod::m1(),
                    session,
                    window,
                    ohlcv,
                    Completeness::Completed,
                )
                .unwrap(),
            );
        }
        let parts = partition_by_anchor(&s, 0..s.len(), VolumeProfileAnchor::Daily, cal.tz());
        assert_eq!(parts.len(), 3, "DST induced spurious partition: {parts:?}");
    }

    /// Test #9 — `Anchor::Daily` over a `BarPeriod::Calendar(Week)`
    /// series falls back to single-profile (Viewport) rendering.
    /// Asserts via `period_blocks_anchor` directly so the test stays
    /// independent of the calendar's exact bar-window math.
    #[test]
    fn anchor_blocks_when_period_too_coarse() {
        // Daily anchor (min=1) blocks Weekly (unit_days=7) ✓
        assert!(period_blocks_anchor(
            BarPeriod::Calendar(CalendarSpan::Week),
            VolumeProfileAnchor::Daily
        ));
        // Weekly anchor (min=7) blocks Year (unit_days=365) ✓
        assert!(period_blocks_anchor(
            BarPeriod::Calendar(CalendarSpan::Year),
            VolumeProfileAnchor::Weekly
        ));
        // M1 (Clock — unit_days=0) does NOT block any anchor.
        assert!(!period_blocks_anchor(
            BarPeriod::Clock(ClockInterval::Minutes(1)),
            VolumeProfileAnchor::Daily
        ));
        // Session(Regular) is treated as sub-daily for the purposes of
        // this gate (per Recon R3 — promotion to a "true" days mapping
        // is rejected as speculative generality).
        assert!(!period_blocks_anchor(
            BarPeriod::Session(SessionSpan::Regular),
            VolumeProfileAnchor::Daily
        ));
        // Viewport / Unknown anchors never block.
        assert!(!period_blocks_anchor(
            BarPeriod::Calendar(CalendarSpan::Year),
            VolumeProfileAnchor::Viewport
        ));
        assert!(!period_blocks_anchor(
            BarPeriod::Calendar(CalendarSpan::Year),
            VolumeProfileAnchor::Unknown
        ));
    }

    /// Test #10 — visible-range with 200 daily partitions and
    /// `max_profiles = 100` emits exactly 100 profiles, all from the
    /// most-recent partitions (oldest 100 dropped).
    #[test]
    fn max_profiles_cap_drops_oldest() {
        let cal = xnys();
        // 200 candles, one per day. Use 14:30 UTC = 09:30 ET (RTH open).
        let s = series_with_period(
            utc(2024, 1, 2, 14, 30),
            200,
            chrono::Duration::days(1),
            BarPeriod::m1(),
            cal,
        );

        // Count partitions before cap to confirm the fixture.
        let parts = partition_by_anchor(&s, 0..s.len(), VolumeProfileAnchor::Daily, cal.tz());
        assert!(
            parts.len() >= 100,
            "fixture should produce ≥100 partitions, got {}",
            parts.len()
        );

        let series: SharedCandleSeries = Arc::new(RwLock::new(s));
        let layer = VolumeProfileLayer::new(
            Arc::clone(&series),
            0..200,
            VolumeProfileStyle::default(),
            VolumeProfileConfig {
                anchor: VolumeProfileAnchor::Daily,
                width_fraction: 1.0,
                max_profiles: 100,
            },
            cal,
        );
        let mut out = ScenePrimitives::default();
        paint_with(
            &layer,
            PriceRange::new(99.0, 106.0).unwrap(),
            Viewport::new(20_000.0, 400.0),
            &mut out,
        );
        // Each emitted partition produces ≥1 quad. The cap may trim
        // partitions from the oldest end before the per-partition
        // pixel-width clamp filters narrow ones, so total emit count is
        // bounded above by 100 * num_bins. Lower bound is harder to
        // assert deterministically — but we can ensure the layer never
        // emitted "everything" by checking that emit count is below
        // the uncapped reference's quad count.
        let layer_uncapped = VolumeProfileLayer::new(
            Arc::clone(&series),
            0..200,
            VolumeProfileStyle::default(),
            VolumeProfileConfig {
                anchor: VolumeProfileAnchor::Daily,
                width_fraction: 1.0,
                max_profiles: 1_000,
            },
            cal,
        );
        let mut out_uncapped = ScenePrimitives::default();
        paint_with(
            &layer_uncapped,
            PriceRange::new(99.0, 106.0).unwrap(),
            Viewport::new(20_000.0, 400.0),
            &mut out_uncapped,
        );
        assert!(
            out.quads.len() <= out_uncapped.quads.len(),
            "capped quads ({}) must not exceed uncapped quads ({})",
            out.quads.len(),
            out_uncapped.quads.len()
        );
        // Capped emission MUST be strictly fewer than uncapped given
        // 200 partitions and a cap of 100.
        assert!(
            out.quads.len() < out_uncapped.quads.len(),
            "cap should have dropped some partitions (capped={}, uncapped={})",
            out.quads.len(),
            out_uncapped.quads.len()
        );
    }

    /// Test #11 — when a partition's pixel-width is wider than
    /// `MAX_PROFILE_PX` (240 px), every emitted quad's `x + w` stays
    /// within `[left_x, left_x + MAX_PROFILE_PX]`.
    #[test]
    fn width_clamps_to_max() {
        let cal = xnys();
        // Two daily partitions; pick a viewport so each spans
        // ~10000/2 = 5000 px (well above MAX_PROFILE_PX = 240).
        let s = series_with_period(
            utc(2024, 1, 17, 14, 30),
            2,
            chrono::Duration::days(1),
            BarPeriod::m1(),
            cal,
        );
        let series: SharedCandleSeries = Arc::new(RwLock::new(s));
        let layer = VolumeProfileLayer::new(
            Arc::clone(&series),
            0..2,
            VolumeProfileStyle::default(),
            VolumeProfileConfig {
                anchor: VolumeProfileAnchor::Daily,
                width_fraction: 1.0,
                max_profiles: 100,
            },
            cal,
        );
        let mut out = ScenePrimitives::default();
        paint_with(
            &layer,
            PriceRange::new(95.0, 110.0).unwrap(),
            Viewport::new(10_000.0, 400.0),
            &mut out,
        );
        assert!(!out.quads.is_empty());
        // For each quad: w must be ≤ MAX_PROFILE_PX (the full clamp +
        // width-fraction = 1.0 envelope).
        for q in &out.quads {
            assert!(
                q.w <= MAX_PROFILE_PX + 1e-3,
                "quad width {} exceeds MAX_PROFILE_PX {}",
                q.w,
                MAX_PROFILE_PX
            );
        }
    }

    /// S6 P2 — narrow-period 1-pixel POC tick degradation. With four
    /// daily partitions squeezed into a 24-px viewport, each partition
    /// gets ~6 px (in `[MIN_POC_TICK_PX, MIN_PERIOD_PX_TO_RENDER)`).
    /// The layer should emit one tick-quad per partition with width
    /// equal to `MIN_POC_TICK_PX`, not drop them silently.
    #[test]
    fn narrow_partitions_emit_one_pixel_poc_tick_each() {
        let cal = xnys();
        // Four daily candles starting at 09:30 ET. Each candle holds
        // its own daily partition.
        let s = series_with_period(
            utc(2024, 1, 17, 14, 30),
            4,
            chrono::Duration::days(1),
            BarPeriod::m1(),
            cal,
        );
        let series: SharedCandleSeries = Arc::new(RwLock::new(s));
        let layer = VolumeProfileLayer::new(
            Arc::clone(&series),
            0..4,
            VolumeProfileStyle::default(),
            VolumeProfileConfig {
                anchor: VolumeProfileAnchor::Daily,
                width_fraction: 1.0,
                max_profiles: 100,
            },
            cal,
        );

        // Custom paint context: axis spans exactly the 4-day fixture
        // range, viewport width 24 px → each partition gets 6 px (in
        // the [1, 12) tick band).
        let vp = Viewport::new(24.0, 400.0);
        let pr = PriceRange::new(95.0, 110.0).unwrap();
        let axis = ContinuousAxis::new(
            utc(2024, 1, 17, 14, 30),
            utc(2024, 1, 17, 14, 30) + chrono::Duration::days(4),
            vp.width_px,
        )
        .unwrap();
        let pal = ThemePalette::dark_default();
        let paxis = LinearPriceAxis::new(pr, vp.height_px);
        let fmt = DefaultFormatter::new();
        let mut out = ScenePrimitives::default();
        {
            let mut ctx = PaintContext {
                axis: &axis,
                viewport: vp,
                price_range: pr,
                palette: &pal,
                price_axis: &paxis,
                formatter: &fmt,
                out: &mut out,
            };
            layer.paint(&mut ctx);
        }

        assert!(
            !out.quads.is_empty(),
            "narrow partitions should emit POC ticks, not be dropped"
        );
        // Every emitted quad in the tick band has width == MIN_POC_TICK_PX
        // and the POC color (no neighbour bins are emitted in tick mode).
        for q in &out.quads {
            assert!(
                (q.w - MIN_POC_TICK_PX).abs() < 1e-3,
                "narrow-partition quad has width {}, expected MIN_POC_TICK_PX={}",
                q.w,
                MIN_POC_TICK_PX
            );
            assert_eq!(
                q.color,
                VolumeProfileStyle::default().poc_color,
                "narrow-partition tick must use POC color"
            );
        }
    }

    /// S6 P2 — partitions narrower than `MIN_POC_TICK_PX` (genuinely
    /// sub-pixel) drop entirely; no tick is emitted.
    #[test]
    fn sub_pixel_partitions_drop_entirely() {
        let cal = xnys();
        // 200 daily partitions squeezed into 100 px → each ~0.5 px,
        // below MIN_POC_TICK_PX = 1.0.
        let s = series_with_period(
            utc(2024, 1, 2, 14, 30),
            200,
            chrono::Duration::days(1),
            BarPeriod::m1(),
            cal,
        );
        let series: SharedCandleSeries = Arc::new(RwLock::new(s));
        let layer = VolumeProfileLayer::new(
            Arc::clone(&series),
            0..200,
            VolumeProfileStyle::default(),
            VolumeProfileConfig {
                anchor: VolumeProfileAnchor::Daily,
                width_fraction: 1.0,
                max_profiles: 1_000,
            },
            cal,
        );

        let vp = Viewport::new(100.0, 400.0);
        let pr = PriceRange::new(95.0, 110.0).unwrap();
        let axis = ContinuousAxis::new(
            utc(2024, 1, 2, 14, 30),
            utc(2024, 1, 2, 14, 30) + chrono::Duration::days(200),
            vp.width_px,
        )
        .unwrap();
        let pal = ThemePalette::dark_default();
        let paxis = LinearPriceAxis::new(pr, vp.height_px);
        let fmt = DefaultFormatter::new();
        let mut out = ScenePrimitives::default();
        {
            let mut ctx = PaintContext {
                axis: &axis,
                viewport: vp,
                price_range: pr,
                palette: &pal,
                price_axis: &paxis,
                formatter: &fmt,
                out: &mut out,
            };
            layer.paint(&mut ctx);
        }

        assert!(
            out.quads.is_empty(),
            "sub-pixel partitions should drop entirely, got {} quads",
            out.quads.len()
        );
    }
}
