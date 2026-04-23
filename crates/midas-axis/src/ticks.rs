//! Shared tick-step selection logic for the axis implementations.
//!
//! Tick generation is a two-step routine:
//!
//! 1. **Pick a step.** Given a viewport span and a target pixel spacing,
//!    select the largest "nice" calendar step that yields at least the
//!    target spacing. Steps progress Year → Quarter → Month → Week →
//!    Day → 6h → Hour → 15min → 5min → Minute → 15s → 5s → Second.
//! 2. **Enumerate ticks.** Starting from the first aligned boundary at
//!    or after `start`, step through and emit a [`TimeTick`] for each.
//!
//! This module intentionally doesn't know about axis-compression gaps.
//! The compressed-axis generator calls [`pick_step`] per session range
//! then wraps `to_x` into the compressed mapping.

use std::borrow::Cow;

use chrono::{DateTime, Datelike, Duration, TimeZone, Timelike, Utc};
use midas_calendar::Timestamp;

use crate::{Importance, TickDensity, TickLabel, TimeTick};

/// Nice-round calendar step. Internally ordered by duration (coarsest at
/// the top).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum NiceStep {
    Year,
    Quarter, // 3 months
    Month,
    Week, // 7 days, anchored Monday
    Day,
    Hours(i64),
    Minutes(i64),
    Seconds(i64),
}

impl NiceStep {
    /// Nominal duration in seconds for step-size comparisons only. For
    /// calendar-sensitive steps (Year/Quarter/Month/Week), uses an
    /// average so the pick heuristic is monotonic.
    #[inline]
    fn nominal_secs(self) -> i64 {
        match self {
            NiceStep::Year => 365 * 86_400,
            NiceStep::Quarter => 91 * 86_400,
            NiceStep::Month => 30 * 86_400,
            NiceStep::Week => 7 * 86_400,
            NiceStep::Day => 86_400,
            NiceStep::Hours(h) => h * 3_600,
            NiceStep::Minutes(m) => m * 60,
            NiceStep::Seconds(s) => s,
        }
    }
}

/// Ordered list of nice steps from coarsest (Year) to finest (1s). Pick
/// the FIRST entry whose `nominal_secs` is <= `target_secs`, so the
/// number of ticks tends to fall near the `density` target.
const STEPS: &[NiceStep] = &[
    NiceStep::Year,
    NiceStep::Quarter,
    NiceStep::Month,
    NiceStep::Week,
    NiceStep::Day,
    NiceStep::Hours(6),
    NiceStep::Hours(3),
    NiceStep::Hours(1),
    NiceStep::Minutes(30),
    NiceStep::Minutes(15),
    NiceStep::Minutes(5),
    NiceStep::Minutes(1),
    NiceStep::Seconds(15),
    NiceStep::Seconds(5),
    NiceStep::Seconds(1),
];

/// Choose the step whose tick-count is closest (in log space) to the
/// number of ticks implied by `(width_px / target_px)` at the given
/// density. Aiming for ~11 ticks at Normal on a 1000px wide viewport.
pub(crate) fn pick_step(span_secs: i64, width_px: f32, density: TickDensity) -> NiceStep {
    let target_px = density.target_px();
    let desired_ticks = (width_px / target_px).max(2.0);

    let mut best: Option<(f32, NiceStep)> = None;
    for &step in STEPS {
        let step_secs = step.nominal_secs().max(1);
        let n = (span_secs as f32 / step_secs as f32).max(0.5);
        // Logarithmic distance — equally penalize 2× too many and 2× too
        // few. This avoids the "first step past threshold" pathology
        // where a 1-day span with desired=11 picks 1-hour (24 ticks)
        // when 3-hour (8 ticks) is the better match.
        let dist = (n / desired_ticks).log2().abs();
        match best {
            None => best = Some((dist, step)),
            Some((best_dist, _)) if dist < best_dist => best = Some((dist, step)),
            _ => {}
        }
    }
    best.map(|(_, s)| s).unwrap_or(NiceStep::Seconds(1))
}

/// First tick boundary at or after `ts`, aligned to `step`.
pub(crate) fn align_up(ts: Timestamp, step: NiceStep) -> Timestamp {
    match step {
        NiceStep::Year => {
            let y = ts.year();
            let jan1 = Utc
                .with_ymd_and_hms(y, 1, 1, 0, 0, 0)
                .single()
                .expect("jan1");
            if jan1 >= ts {
                jan1
            } else {
                Utc.with_ymd_and_hms(y + 1, 1, 1, 0, 0, 0)
                    .single()
                    .expect("jan1 next")
            }
        }
        NiceStep::Quarter => {
            let y = ts.year();
            let m = ts.month();
            let qstart_month = ((m - 1) / 3) * 3 + 1;
            let cur = Utc
                .with_ymd_and_hms(y, qstart_month, 1, 0, 0, 0)
                .single()
                .expect("q start");
            if cur >= ts {
                cur
            } else {
                let (ny, nm) = if qstart_month + 3 > 12 {
                    (y + 1, qstart_month + 3 - 12)
                } else {
                    (y, qstart_month + 3)
                };
                Utc.with_ymd_and_hms(ny, nm, 1, 0, 0, 0)
                    .single()
                    .expect("q next")
            }
        }
        NiceStep::Month => {
            let y = ts.year();
            let m = ts.month();
            let cur = Utc
                .with_ymd_and_hms(y, m, 1, 0, 0, 0)
                .single()
                .expect("m start");
            if cur >= ts {
                cur
            } else {
                let (ny, nm) = if m == 12 { (y + 1, 1) } else { (y, m + 1) };
                Utc.with_ymd_and_hms(ny, nm, 1, 0, 0, 0)
                    .single()
                    .expect("m next")
            }
        }
        NiceStep::Week => {
            // ISO week anchor: Monday 00:00 UTC.
            let date = ts.date_naive();
            // weekday: Monday = 1 ... Sunday = 7
            let weekday = date.weekday().number_from_monday() as i64;
            let days_back = weekday - 1;
            let monday = date - Duration::days(days_back);
            let monday_ts = Utc
                .with_ymd_and_hms(monday.year(), monday.month(), monday.day(), 0, 0, 0)
                .single()
                .expect("monday");
            if monday_ts >= ts {
                monday_ts
            } else {
                monday_ts + Duration::days(7)
            }
        }
        NiceStep::Day => {
            let date = ts.date_naive();
            let midnight = Utc
                .with_ymd_and_hms(date.year(), date.month(), date.day(), 0, 0, 0)
                .single()
                .expect("midnight");
            if midnight >= ts {
                midnight
            } else {
                midnight + Duration::days(1)
            }
        }
        NiceStep::Hours(h) => align_up_mod(ts, h * 3_600),
        NiceStep::Minutes(m) => align_up_mod(ts, m * 60),
        NiceStep::Seconds(s) => align_up_mod(ts, s),
    }
}

/// Round `ts` UP to the next multiple of `modulus_secs` relative to the
/// UTC epoch.
fn align_up_mod(ts: Timestamp, modulus_secs: i64) -> Timestamp {
    let epoch = ts.timestamp();
    let rem = epoch.rem_euclid(modulus_secs);
    if rem == 0 && ts.timestamp_subsec_nanos() == 0 {
        ts
    } else {
        let delta = modulus_secs - rem;
        ts + Duration::seconds(delta) - Duration::nanoseconds(ts.timestamp_subsec_nanos() as i64)
    }
}

/// Advance `ts` by one `step` (calendar-aware for Y/Q/M/W; modular for
/// clock intervals).
pub(crate) fn advance(ts: Timestamp, step: NiceStep) -> Timestamp {
    match step {
        NiceStep::Year => Utc
            .with_ymd_and_hms(ts.year() + 1, ts.month(), 1, 0, 0, 0)
            .single()
            .unwrap_or(ts + Duration::days(365)),
        NiceStep::Quarter => {
            let (y, m) = if ts.month() + 3 > 12 {
                (ts.year() + 1, ts.month() + 3 - 12)
            } else {
                (ts.year(), ts.month() + 3)
            };
            Utc.with_ymd_and_hms(y, m, 1, 0, 0, 0)
                .single()
                .unwrap_or(ts + Duration::days(91))
        }
        NiceStep::Month => {
            let (y, m) = if ts.month() == 12 {
                (ts.year() + 1, 1)
            } else {
                (ts.year(), ts.month() + 1)
            };
            Utc.with_ymd_and_hms(y, m, 1, 0, 0, 0)
                .single()
                .unwrap_or(ts + Duration::days(30))
        }
        NiceStep::Week => ts + Duration::days(7),
        NiceStep::Day => ts + Duration::days(1),
        NiceStep::Hours(h) => ts + Duration::hours(h),
        NiceStep::Minutes(m) => ts + Duration::minutes(m),
        NiceStep::Seconds(s) => ts + Duration::seconds(s),
    }
}

/// Format a [`TickLabel`] for the given `step` and `ts`. Primary label is
/// context-sensitive; a secondary (year) is emitted for Month/Quarter
/// steps so renderers can stack "Jan" over "2025".
pub(crate) fn label_for(ts: Timestamp, step: NiceStep) -> TickLabel {
    match step {
        NiceStep::Year => TickLabel::Primary(Cow::Owned(format!("{}", ts.year()))),
        NiceStep::Quarter => {
            let q = ((ts.month() - 1) / 3) + 1;
            TickLabel::WithSecondary {
                primary: Cow::Owned(format!("Q{q}")),
                secondary: Cow::Owned(format!("{}", ts.year())),
            }
        }
        NiceStep::Month => TickLabel::WithSecondary {
            primary: Cow::Borrowed(month_abbr(ts.month())),
            secondary: Cow::Owned(format!("{}", ts.year())),
        },
        NiceStep::Week | NiceStep::Day => TickLabel::Primary(Cow::Owned(format!(
            "{}-{:02}-{:02}",
            ts.year(),
            ts.month(),
            ts.day()
        ))),
        NiceStep::Hours(_) | NiceStep::Minutes(_) => {
            TickLabel::Primary(Cow::Owned(format!("{:02}:{:02}", ts.hour(), ts.minute())))
        }
        NiceStep::Seconds(_) => TickLabel::Primary(Cow::Owned(format!(
            "{:02}:{:02}:{:02}",
            ts.hour(),
            ts.minute(),
            ts.second()
        ))),
    }
}

/// Importance rule: Year / Quarter / Month / Week / Day / 6h are major;
/// finer steps are minor. Callers may override per-tick (e.g. the
/// compressed axis promotes session-boundary ticks to major regardless
/// of step).
pub(crate) fn importance_for(step: NiceStep) -> Importance {
    match step {
        NiceStep::Year
        | NiceStep::Quarter
        | NiceStep::Month
        | NiceStep::Week
        | NiceStep::Day
        | NiceStep::Hours(6) => Importance::Major,
        _ => Importance::Minor,
    }
}

#[inline]
fn month_abbr(month: u32) -> &'static str {
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

/// Produce a tick sequence between `[start, end]` at the chosen `step`.
/// `project` maps `Timestamp -> Option<f32>`; ticks whose timestamp falls
/// outside the axis' projection domain (e.g. inside a compressed gap) are
/// skipped.
pub(crate) fn enumerate_ticks<F>(
    start: Timestamp,
    end: Timestamp,
    step: NiceStep,
    mut project: F,
) -> Vec<TimeTick>
where
    F: FnMut(Timestamp) -> Option<f32>,
{
    let mut out = Vec::new();
    let mut cursor = align_up(start, step);
    let importance = importance_for(step);
    // Guard against pathological step sizes producing infinite loops
    // (shouldn't happen with STEPS, but belt-and-braces).
    let max_iters = 10_000_usize;
    let mut i = 0;
    while cursor <= end && i < max_iters {
        if let Some(x) = project(cursor) {
            out.push(TimeTick {
                x,
                ts: cursor,
                label: label_for(cursor, step),
                importance,
            });
        }
        let next = advance(cursor, step);
        if next <= cursor {
            break;
        }
        cursor = next;
        i += 1;
    }
    out
}

/// Utility used only by tests — stringify a `DateTime<Utc>` as an ISO-ish
/// debug form.
#[allow(dead_code)]
pub(crate) fn iso(ts: DateTime<Utc>) -> String {
    format!(
        "{}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        ts.year(),
        ts.month(),
        ts.day(),
        ts.hour(),
        ts.minute(),
        ts.second()
    )
}
