//! Timeframe enum representing candle periods from 1-second to 1-month.
//!
//! Each variant carries a fixed nominal duration in seconds, a human-readable
//! display label, and boundary-alignment logic for timestamp flooring.

use chrono::Datelike;
use std::fmt;

/// Supported candle timeframes, from sub-second to monthly.
#[derive(
    Copy, Clone, Eq, PartialEq, Hash, Debug, Ord, PartialOrd, serde::Serialize, serde::Deserialize,
)]
pub enum Timeframe {
    /// 1 second
    S1,
    /// 5 seconds
    S5,
    /// 15 seconds
    S15,
    /// 30 seconds
    S30,
    /// 1 minute
    M1,
    /// 5 minutes
    M5,
    /// 15 minutes
    M15,
    /// 30 minutes
    M30,
    /// 1 hour
    H1,
    /// 4 hours
    H4,
    /// 1 day
    D1,
    /// 1 week
    W1,
    /// 1 month (30 days nominal)
    MN1,
}

impl Timeframe {
    /// Duration in seconds. For `D1`/`W1`/`MN1` these are nominal values
    /// (actual calendar duration varies for months).
    pub const fn as_secs(&self) -> u32 {
        match self {
            Self::S1 => 1,
            Self::S5 => 5,
            Self::S15 => 15,
            Self::S30 => 30,
            Self::M1 => 60,
            Self::M5 => 300,
            Self::M15 => 900,
            Self::M30 => 1800,
            Self::H1 => 3600,
            Self::H4 => 14400,
            Self::D1 => 86400,
            Self::W1 => 604800,
            Self::MN1 => 2592000, // 30 days nominal
        }
    }

    /// File suffix for binary file naming (e.g., `"5m"` for `M5`).
    pub const fn file_suffix(&self) -> &'static str {
        match self {
            Self::S1 => "1s",
            Self::S5 => "5s",
            Self::S15 => "15s",
            Self::S30 => "30s",
            Self::M1 => "1m",
            Self::M5 => "5m",
            Self::M15 => "15m",
            Self::M30 => "30m",
            Self::H1 => "1H",
            Self::H4 => "4H",
            Self::D1 => "1D",
            Self::W1 => "1W",
            Self::MN1 => "1M",
        }
    }

    /// Human-readable display name for UI labels.
    pub const fn display_name(&self) -> &'static str {
        match self {
            Self::S1 => "1s",
            Self::S5 => "5s",
            Self::S15 => "15s",
            Self::S30 => "30s",
            Self::M1 => "1m",
            Self::M5 => "5m",
            Self::M15 => "15m",
            Self::M30 => "30m",
            Self::H1 => "1H",
            Self::H4 => "4H",
            Self::D1 => "1D",
            Self::W1 => "1W",
            Self::MN1 => "1M",
        }
    }

    /// Parse from file suffix string (e.g., `"1m"` -> `M1`).
    pub fn from_suffix(s: &str) -> Option<Self> {
        match s {
            "1s" => Some(Self::S1),
            "5s" => Some(Self::S5),
            "15s" => Some(Self::S15),
            "30s" => Some(Self::S30),
            "1m" => Some(Self::M1),
            "5m" => Some(Self::M5),
            "15m" => Some(Self::M15),
            "30m" => Some(Self::M30),
            "1H" => Some(Self::H1),
            "4H" => Some(Self::H4),
            "1D" => Some(Self::D1),
            "1W" => Some(Self::W1),
            "1M" => Some(Self::MN1),
            _ => None,
        }
    }

    /// Whether this timeframe is calendar-aligned (boundary depends on
    /// calendar, not just modular arithmetic).
    pub const fn is_calendar(&self) -> bool {
        matches!(self, Self::W1 | Self::MN1)
    }

    /// Align a timestamp (epoch milliseconds) to the start of its candle period.
    ///
    /// For sub-daily timeframes: pure modular arithmetic on UTC.
    /// For `D1`: floor to midnight UTC.
    /// For `W1`: floor to Monday 00:00 UTC.
    /// For `MN1`: floor to 1st of the month 00:00 UTC.
    pub fn floor_timestamp(&self, ts_ms: i64) -> i64 {
        match self {
            // Sub-daily and daily: modular arithmetic
            Self::S1
            | Self::S5
            | Self::S15
            | Self::S30
            | Self::M1
            | Self::M5
            | Self::M15
            | Self::M30
            | Self::H1
            | Self::H4 => {
                let period_ms = self.as_secs() as i64 * 1000;
                ts_ms - (ts_ms.rem_euclid(period_ms))
            }

            // Daily: floor to midnight UTC
            Self::D1 => {
                let day_ms = 86_400_000i64;
                ts_ms - (ts_ms.rem_euclid(day_ms))
            }

            // Weekly: floor to Monday 00:00 UTC
            // Epoch (1970-01-01) was a Thursday. Monday is day 4 of that week.
            // Days since epoch: ts / 86400000
            // Day of week: (days + 3) % 7  (0=Monday, 6=Sunday)
            Self::W1 => {
                let day_ms = 86_400_000i64;
                let days = ts_ms.div_euclid(day_ms);
                let dow = (days + 3).rem_euclid(7); // 0=Mon
                (days - dow) * day_ms
            }

            // Monthly: floor to 1st of the month 00:00 UTC
            Self::MN1 => {
                let dt = chrono::DateTime::from_timestamp_millis(ts_ms)
                    .expect("timestamp out of range for DateTime");
                let floored = dt
                    .date_naive()
                    .with_day(1)
                    .expect("day 1 is always valid")
                    .and_hms_opt(0, 0, 0)
                    .expect("midnight is always valid");
                floored.and_utc().timestamp_millis()
            }
        }
    }

    /// Compute the timestamp of the next candle boundary after `ts_ms`.
    pub fn next_boundary(&self, ts_ms: i64) -> i64 {
        match self {
            Self::MN1 => {
                let dt = chrono::DateTime::from_timestamp_millis(ts_ms)
                    .expect("timestamp out of range for DateTime");
                let d = dt.date_naive();
                let next_month = if d.month() == 12 {
                    chrono::NaiveDate::from_ymd_opt(d.year() + 1, 1, 1).expect("valid date")
                } else {
                    chrono::NaiveDate::from_ymd_opt(d.year(), d.month() + 1, 1).expect("valid date")
                };
                next_month
                    .and_hms_opt(0, 0, 0)
                    .expect("midnight is always valid")
                    .and_utc()
                    .timestamp_millis()
            }
            _ => {
                let floored = self.floor_timestamp(ts_ms);
                floored + self.as_secs() as i64 * 1000
            }
        }
    }
}

impl fmt::Display for Timeframe {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.display_name())
    }
}

#[cfg(test)]
mod tests;
