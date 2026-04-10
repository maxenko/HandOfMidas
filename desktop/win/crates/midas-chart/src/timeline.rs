//! TC2000-style adaptive timeline labels for the chart time axis.
//!
//! This module is the **single authoritative source** for all timeline label
//! logic: tier selection, formatting, boundary detection, and label
//! placement for both normal (timestamp) and collapsed (index) modes.
//!
//! # Design
//!
//! TC2000 labels at **time boundary crossings** — the moment the hour,
//! day, or month changes between consecutive candles. The label tier
//! (what granularity to show) is chosen from the candle duration and
//! zoom level:
//!
//! | Candle duration | Zoomed in | Zoomed out |
//! |-----------------|-----------|------------|
//! | 1-min / 5-min   | "12:45 p" (minute) | "10 a" (hour) → "19" (day) |
//! | 30-min          | "1 p" (hour) + day boundary | "19", "20" (day) |
//! | Daily           | "5", "12" (day) + month boundary | "Apr", "May" (month) |
//! | Weekly          | "Feb", "Mar" (month) | "Feb", "Mar" (month) |

use crate::camera::Camera2D;
use midas_core::CandleData;

/// Maximum number of labels on the time axis.
const MAX_LABELS: usize = 50;

/// Minimum pixel spacing between consecutive labels.
const MIN_LABEL_SPACING_PX: f32 = 60.0;

// ── Public types ────────────────────────────────────────────────────

/// Display tier: which date component is shown as the primary label.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tier {
    /// "12:45 p", "1:00 p" — minute-level labels.
    Minute,
    /// "10 a", "1 p" — hour-level labels.
    Hour,
    /// "5", "12", "29" — day-of-month numbers.
    Day,
    /// "Jan", "Feb", "Mar" — month names.
    Month,
}

/// A single positioned timeline label for the time axis.
#[derive(Clone, Debug)]
pub struct TimelineLabel {
    /// Primary label text (e.g. "10 a", "5", "Jan").
    pub text: String,
    /// Boundary (secondary) text shown at tier transitions
    /// (e.g. "Mar 26, 2026" at a day boundary). `None` for regular labels.
    pub secondary: Option<String>,
    /// Screen X position in logical pixels.
    pub screen_x: f32,
    /// Whether this label sits at a higher-order boundary.
    pub is_boundary: bool,
    /// The display tier this label belongs to (Minute, Hour, Day, Month).
    pub tier: Tier,
}

// ── Public API ──────────────────────────────────────────────────────

/// Unified entry point: compute timeline labels for the current mode.
///
/// Callers pass `collapse_gaps` and the candle data; this function
/// dispatches to the correct algorithm (timestamp-aligned for normal
/// mode, boundary-scanning for collapsed mode).
pub fn compute(
    camera: &Camera2D,
    data: &dyn CandleData,
    candle_duration: f64,
    collapse_gaps: bool,
) -> Vec<TimelineLabel> {
    if data.is_empty() {
        return Vec::new();
    }

    if collapse_gaps {
        // Index-space camera: vis range from fractional indices.
        let len = data.len();
        let vis_start = (camera.time_start.floor() as isize).max(0) as usize;
        let vis_end = (camera.time_end.ceil() as usize).min(len);
        let vis_start = vis_start.min(vis_end);

        // index_to_x: center of candle slot in pixel space.
        let index_to_x = |local_idx: usize| -> f32 {
            let global = vis_start + local_idx;
            camera.snap_to_pixel(camera.time_to_x(global as f64 + 0.5))
        };

        for_collapsed_mode(
            camera,
            data,
            vis_start,
            vis_end,
            candle_duration,
            &index_to_x,
        )
    } else {
        for_normal_mode(camera, candle_duration)
    }
}

/// Compute timeline labels for **normal mode** (timestamp-space camera).
///
/// The camera's `time_start`/`time_end` are epoch milliseconds.
/// Labels are placed at regular time-aligned intervals.
pub fn for_normal_mode(camera: &Camera2D, _candle_duration: f64) -> Vec<TimelineLabel> {
    let time_range = camera.time_end - camera.time_start;
    if time_range <= 0.0 {
        return Vec::new();
    }
    let desired = (camera.viewport_width as f64 / 150.0).max(1.0);
    let time_step = nice_time_step(time_range, desired);
    if time_step <= 0.0 {
        return Vec::new();
    }

    let tier = tier_from_step(time_step);
    let mut labels = Vec::new();
    let first = (camera.time_start / time_step).ceil() * time_step;
    let mut t = first;
    let mut prev_ts: Option<i64> = None;

    while t < camera.time_end && labels.len() < MAX_LABELS {
        let x = camera.snap_to_pixel(camera.time_to_x(t));
        let ts = t as i64;
        let (text, is_boundary, secondary) = format_for_tier(ts, prev_ts, tier);
        labels.push(TimelineLabel {
            text,
            secondary,
            screen_x: x,
            is_boundary,
            tier,
        });
        prev_ts = Some(ts);
        t += time_step;
    }
    labels
}

/// Compute timeline labels for **collapsed mode** (index-space camera).
///
/// Walks through actual candle timestamps and detects meaningful
/// time-boundary crossings (hour/day/month changes). This produces
/// labels at semantically correct positions regardless of index spacing.
pub fn for_collapsed_mode(
    camera: &Camera2D,
    data: &dyn CandleData,
    vis_start: usize,
    vis_end: usize,
    candle_duration: f64,
    index_to_x: &dyn Fn(usize) -> f32,
) -> Vec<TimelineLabel> {
    let visible_count = vis_end.saturating_sub(vis_start);
    if visible_count == 0 {
        return Vec::new();
    }

    // Choose tier from candle duration + visible time span.
    let first_ts = data.timestamp(vis_start) as f64;
    let last_ts = data.timestamp(vis_end.saturating_sub(1)) as f64;
    let time_span = (last_ts - first_ts).max(candle_duration);
    let desired_labels = (camera.viewport_width as f64 / 150.0).max(1.0);
    let time_per_label = time_span / desired_labels;
    let tier = tier_from_data(candle_duration, time_per_label);

    // Scan visible candles and find boundary crossings.
    let mut candidates: Vec<(usize, i64, bool)> = Vec::new(); // (local_idx, ts, is_boundary)
    let mut prev_ts: Option<i64> = None;

    for local_idx in 0..visible_count {
        let data_idx = vis_start + local_idx;
        let ts = data.timestamp(data_idx);

        if let Some(prev) = prev_ts {
            if is_tier_boundary(prev, ts, tier) {
                let higher = is_higher_boundary(prev, ts, tier);
                candidates.push((local_idx, ts, higher));
            }
        } else {
            // Always include the first visible candle.
            candidates.push((local_idx, ts, false));
        }
        prev_ts = Some(ts);
    }

    // Thin candidates to respect minimum spacing.
    thin_by_spacing(&candidates, camera, index_to_x, tier)
}

// ── Tier selection ──────────────────────────────────────────────────

/// Choose tier from a grid time step (used in normal mode).
fn tier_from_step(step_ms: f64) -> Tier {
    const HOUR: f64 = 3_600_000.0;
    const DAY: f64 = 86_400_000.0;
    const WEEK: f64 = 604_800_000.0;

    if step_ms < HOUR {
        Tier::Minute
    } else if step_ms < DAY {
        Tier::Hour
    } else if step_ms < WEEK * 2.0 {
        Tier::Day
    } else {
        Tier::Month
    }
}

/// Choose tier from candle duration and visible span per label.
/// Used in collapsed mode where we pick from data characteristics.
fn tier_from_data(candle_duration_ms: f64, time_per_label_ms: f64) -> Tier {
    const HOUR: f64 = 3_600_000.0;
    const DAY: f64 = 86_400_000.0;
    const WEEK: f64 = 604_800_000.0;

    if candle_duration_ms < DAY {
        // Intraday data.
        if time_per_label_ms < HOUR * 2.0 {
            Tier::Minute
        } else if time_per_label_ms < DAY * 2.0 {
            Tier::Hour
        } else if time_per_label_ms < WEEK * 8.0 {
            Tier::Day
        } else {
            Tier::Month
        }
    } else {
        // Daily / weekly data.
        if time_per_label_ms < WEEK * 2.0 {
            Tier::Day
        } else {
            Tier::Month
        }
    }
}

// ── Boundary detection ──────────────────────────────────────────────

/// Does the transition from `prev_ts` to `ts` cross a tier boundary?
fn is_tier_boundary(prev_ts: i64, ts: i64, tier: Tier) -> bool {
    use chrono::{Datelike, Timelike};

    let prev = ts_to_dt(prev_ts);
    let curr = ts_to_dt(ts);

    match tier {
        Tier::Minute => {
            // Boundary every 15 minutes (aligned: :00, :15, :30, :45)
            // or when the hour changes.
            let prev_slot = prev.hour() * 4 + prev.minute() / 15;
            let curr_slot = curr.hour() * 4 + curr.minute() / 15;
            prev_slot != curr_slot || prev.ordinal0() != curr.ordinal0()
        }
        Tier::Hour => {
            prev.hour() != curr.hour()
                || prev.ordinal0() != curr.ordinal0()
                || prev.year() != curr.year()
        }
        Tier::Day => prev.ordinal0() != curr.ordinal0() || prev.year() != curr.year(),
        Tier::Month => prev.month() != curr.month() || prev.year() != curr.year(),
    }
}

/// Does the transition cross a *higher-order* boundary?
/// (e.g. day change for Hour tier, month change for Day tier)
fn is_higher_boundary(prev_ts: i64, ts: i64, tier: Tier) -> bool {
    let prev = ts_to_dt(prev_ts);
    let curr = ts_to_dt(ts);
    use chrono::Datelike;

    match tier {
        Tier::Minute => {
            // Higher = day changed
            prev.ordinal0() != curr.ordinal0() || prev.year() != curr.year()
        }
        Tier::Hour => {
            // Higher = day changed
            prev.ordinal0() != curr.ordinal0() || prev.year() != curr.year()
        }
        Tier::Day => {
            // Higher = month changed
            prev.month() != curr.month() || prev.year() != curr.year()
        }
        Tier::Month => {
            // Higher = year changed
            prev.year() != curr.year()
        }
    }
}

// ── Thinning ────────────────────────────────────────────────────────

/// Filter candidates to maintain minimum pixel spacing, then format.
fn thin_by_spacing(
    candidates: &[(usize, i64, bool)],
    _camera: &Camera2D,
    index_to_x: &dyn Fn(usize) -> f32,
    tier: Tier,
) -> Vec<TimelineLabel> {
    if candidates.is_empty() {
        return Vec::new();
    }

    let mut labels = Vec::new();
    let mut last_x: f32 = f32::NEG_INFINITY;
    let mut prev_emitted_ts: Option<i64> = None;

    for &(local_idx, ts, is_higher) in candidates {
        let x = index_to_x(local_idx);

        // Always emit higher-order boundaries; otherwise enforce spacing.
        if !is_higher && (x - last_x) < MIN_LABEL_SPACING_PX {
            continue;
        }

        let (text, _, secondary) = format_for_tier(ts, prev_emitted_ts, tier);
        // Override is_boundary from the higher flag (more accurate than
        // format_for_tier's prev comparison since we skipped some labels).
        let is_boundary = is_higher;
        let secondary = if is_higher {
            secondary.or_else(|| Some(format_secondary_for_tier(ts, tier)))
        } else {
            None
        };

        labels.push(TimelineLabel {
            text,
            secondary,
            screen_x: x,
            is_boundary,
            tier,
        });
        last_x = x;
        prev_emitted_ts = Some(ts);

        if labels.len() >= MAX_LABELS {
            break;
        }
    }
    labels
}

/// Generate secondary text for a higher-order boundary.
fn format_secondary_for_tier(ts: i64, tier: Tier) -> String {
    let dt = ts_to_dt(ts);
    use chrono::Datelike;
    match tier {
        Tier::Minute | Tier::Hour => format_month_day_year(&dt),
        Tier::Day => format!("{} {}", month_abbrev(dt.month()), dt.year()),
        Tier::Month => format!("{}", dt.year()),
    }
}

// ── Formatting ──────────────────────────────────────────────────────

/// Format a timestamp according to the tier.
/// Returns `(primary_text, is_boundary, secondary_text)`.
fn format_for_tier(
    ts_ms: i64,
    prev_ts_ms: Option<i64>,
    tier: Tier,
) -> (String, bool, Option<String>) {
    use chrono::{Datelike, Timelike};

    let dt = ts_to_dt(ts_ms);
    let prev_dt = prev_ts_ms.map(ts_to_dt);

    match tier {
        Tier::Minute => {
            let (suffix, h12) = to_12h(dt.hour());
            let primary = format!("{}:{:02} {}", h12, dt.minute(), suffix);
            let is_boundary = prev_dt
                .map(|p| p.ordinal0() != dt.ordinal0() || p.year() != dt.year())
                .unwrap_or(false);
            let secondary = if is_boundary {
                Some(format_month_day_year(&dt))
            } else {
                None
            };
            (primary, is_boundary, secondary)
        }
        Tier::Hour => {
            let (suffix, h12) = to_12h(dt.hour());
            let primary = format!("{} {}", h12, suffix);
            let is_boundary = prev_dt
                .map(|p| p.ordinal0() != dt.ordinal0() || p.year() != dt.year())
                .unwrap_or(false);
            let secondary = if is_boundary {
                Some(format_month_day_year(&dt))
            } else {
                None
            };
            (primary, is_boundary, secondary)
        }
        Tier::Day => {
            let primary = format!("{}", dt.day());
            let is_boundary = prev_dt
                .map(|p| p.month() != dt.month() || p.year() != dt.year())
                .unwrap_or(false);
            let secondary = if is_boundary {
                Some(format!("{} {}", month_abbrev(dt.month()), dt.year()))
            } else {
                None
            };
            (primary, is_boundary, secondary)
        }
        Tier::Month => {
            let primary = month_abbrev(dt.month()).to_string();
            let is_boundary = prev_dt.map(|p| p.year() != dt.year()).unwrap_or(false);
            let secondary = if is_boundary {
                Some(format!("{}", dt.year()))
            } else {
                None
            };
            (primary, is_boundary, secondary)
        }
    }
}

// ── Time step computation ───────────────────────────────────────────

/// Standard time intervals (milliseconds), used for normal-mode grid alignment.
const TIME_STEPS_MS: &[f64] = &[
    1_000.0,           // 1 second
    5_000.0,           // 5 seconds
    10_000.0,          // 10 seconds
    30_000.0,          // 30 seconds
    60_000.0,          // 1 minute
    300_000.0,         // 5 minutes
    600_000.0,         // 10 minutes
    900_000.0,         // 15 minutes
    1_800_000.0,       // 30 minutes
    3_600_000.0,       // 1 hour
    14_400_000.0,      // 4 hours
    86_400_000.0,      // 1 day
    604_800_000.0,     // 1 week
    2_592_000_000.0,   // ~30 days
    5_184_000_000.0,   // ~60 days
    7_776_000_000.0,   // ~90 days
    15_552_000_000.0,  // ~180 days
    31_536_000_000.0,  // ~1 year
    63_072_000_000.0,  // ~2 years
    157_680_000_000.0, // ~5 years
];

/// Choose a time step from the standard intervals.
fn nice_time_step(time_range: f64, desired_divisions: f64) -> f64 {
    if desired_divisions <= 0.0 || time_range <= 0.0 {
        return 0.0;
    }
    let target = time_range / desired_divisions;
    for &step in TIME_STEPS_MS {
        if step >= target {
            return step;
        }
    }
    // Fallback: round up to nice multiple of years.
    let year_ms = 31_536_000_000.0_f64;
    let years = target / year_ms;
    let magnitude = 10_f64.powf(years.log10().floor());
    let normalized = years / magnitude;
    let nice = if normalized <= 1.5 {
        1.0
    } else if normalized <= 3.5 {
        2.0
    } else if normalized <= 7.5 {
        5.0
    } else {
        10.0
    };
    nice * magnitude * year_ms
}

// ── Helpers ─────────────────────────────────────────────────────────

fn ts_to_dt(ts_ms: i64) -> chrono::DateTime<chrono::Local> {
    let utc = chrono::DateTime::from_timestamp_millis(ts_ms)
        .unwrap_or_else(|| chrono::DateTime::from_timestamp(0, 0).unwrap());
    utc.with_timezone(&chrono::Local)
}

fn to_12h(hour24: u32) -> (&'static str, u32) {
    match hour24 {
        0 => ("a", 12),
        1..=11 => ("a", hour24),
        12 => ("p", 12),
        _ => ("p", hour24 - 12),
    }
}

fn format_month_day_year(dt: &chrono::DateTime<chrono::Local>) -> String {
    use chrono::Datelike;
    format!("{} {}, {}", month_abbrev(dt.month()), dt.day(), dt.year())
}

fn month_abbrev(month: u32) -> &'static str {
    match month {
        1 => "Jan",
        2 => "Feb",
        3 => "Mar",
        4 => "Apr",
        5 => "May",
        6 => "Jun",
        7 => "Jul",
        8 => "Aug",
        9 => "Sep",
        10 => "Oct",
        11 => "Nov",
        12 => "Dec",
        _ => "???",
    }
}
