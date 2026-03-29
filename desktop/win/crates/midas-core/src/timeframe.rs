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
mod tests {
    use super::*;

    // ── as_secs tests ───────────────────────────────────────────────────

    #[test]
    fn as_secs_sub_minute() {
        assert_eq!(Timeframe::S1.as_secs(), 1);
        assert_eq!(Timeframe::S5.as_secs(), 5);
        assert_eq!(Timeframe::S15.as_secs(), 15);
        assert_eq!(Timeframe::S30.as_secs(), 30);
    }

    #[test]
    fn as_secs_minutes() {
        assert_eq!(Timeframe::M1.as_secs(), 60);
        assert_eq!(Timeframe::M5.as_secs(), 300);
        assert_eq!(Timeframe::M15.as_secs(), 900);
        assert_eq!(Timeframe::M30.as_secs(), 1800);
    }

    #[test]
    fn as_secs_hours() {
        assert_eq!(Timeframe::H1.as_secs(), 3600);
        assert_eq!(Timeframe::H4.as_secs(), 14400);
    }

    #[test]
    fn as_secs_daily_and_above() {
        assert_eq!(Timeframe::D1.as_secs(), 86400);
        assert_eq!(Timeframe::W1.as_secs(), 604800);
        assert_eq!(Timeframe::MN1.as_secs(), 2592000);
    }

    // ── floor_timestamp tests ───────────────────────────────────────────

    #[test]
    fn floor_m5_aligns_to_5_min_boundary() {
        // 2024-01-15T00:00:00Z = 1705276800000
        // 2024-01-15T09:47:30Z = +35250000    = 1705312050000
        // Floor to 5min: 1705312050000 % 300000 = 150000
        //   -> 1705312050000 - 150000           = 1705311900000  (09:45:00Z)
        let ts = 1705312050000i64;
        let floored = Timeframe::M5.floor_timestamp(ts);
        let expected = 1705311900000i64;
        assert_eq!(floored, expected);
    }

    #[test]
    fn floor_h1_aligns_to_hour_boundary() {
        // 2024-01-15T00:00:00Z = 1705276800000
        // 2024-01-15T10:00:00Z = 1705276800000 + 10*3_600_000 = 1705312800000
        // 2024-01-15T10:30:00Z = 1705312800000 + 1_800_000    = 1705314600000
        let ts = 1705314600000i64; // 2024-01-15T10:30:00Z
        let floored = Timeframe::H1.floor_timestamp(ts);
        let expected = 1705312800000i64; // 2024-01-15T10:00:00Z
        assert_eq!(floored, expected);
    }

    #[test]
    fn floor_d1_aligns_to_midnight_utc() {
        // 2024-01-15 14:30:00 UTC -> should floor to 2024-01-15 00:00:00 UTC
        let ts = 1705326600000i64; // 2024-01-15T14:30:00Z
        let floored = Timeframe::D1.floor_timestamp(ts);
        let expected = 1705276800000i64; // 2024-01-15T00:00:00Z
        assert_eq!(floored, expected);
    }

    #[test]
    fn floor_w1_aligns_to_monday() {
        // 2024-01-17 is a Wednesday
        // Should floor to Monday 2024-01-15
        let ts = 1705507200000i64; // 2024-01-17T12:00:00Z
        let floored = Timeframe::W1.floor_timestamp(ts);
        let expected = 1705276800000i64; // 2024-01-15T00:00:00Z (Monday)
        assert_eq!(floored, expected);
    }

    #[test]
    fn floor_mn1_aligns_to_first_of_month() {
        // 2024-01-15 -> should floor to 2024-01-01 00:00:00 UTC
        let ts = 1705326600000i64; // 2024-01-15T14:30:00Z
        let floored = Timeframe::MN1.floor_timestamp(ts);
        let expected = 1704067200000i64; // 2024-01-01T00:00:00Z
        assert_eq!(floored, expected);
    }

    #[test]
    fn floor_already_aligned_is_identity() {
        // Exactly on a 5-minute boundary (1705311900000 = the M5 floor from above)
        let ts = 1705311900000i64;
        assert_eq!(Timeframe::M5.floor_timestamp(ts), ts);
    }

    #[test]
    fn floor_s1_is_millisecond_truncation() {
        // Any timestamp with sub-second milliseconds should floor to whole second
        let ts = 1705312050123i64; // has 123ms fractional
        let floored = Timeframe::S1.floor_timestamp(ts);
        assert_eq!(floored, 1705312050000i64);
    }

    #[test]
    fn floor_h4_aligns_to_4h_boundary() {
        // 2024-01-15T00:00:00Z = 1705276800000
        // 2024-01-15T10:30:00Z = +37800000    = 1705314600000
        // H4 period = 14400000ms. 1705314600000 % 14400000 = ?
        // 1705314600000 / 14400000 = 118424.625 -> floor * 14400000 = 1705315200000?
        // Actually: epoch 0 mod H4 boundary: 1705314600000 rem 14400000
        //   1705314600000 / 14400000 = 118424.625
        //   118424 * 14400000 = 1705305600000  (08:00 UTC)
        //   remainder = 1705314600000 - 1705305600000 = 9000000  (2.5 hours)
        let ts = 1705314600000i64; // 2024-01-15T10:30:00Z
        let floored = Timeframe::H4.floor_timestamp(ts);
        let expected = 1705305600000i64; // 2024-01-15T08:00:00Z
        assert_eq!(floored, expected);
    }

    // ── Display tests ───────────────────────────────────────────────────

    #[test]
    fn display_shows_human_readable() {
        assert_eq!(Timeframe::M1.to_string(), "1m");
        assert_eq!(Timeframe::H4.to_string(), "4H");
        assert_eq!(Timeframe::D1.to_string(), "1D");
        assert_eq!(Timeframe::MN1.to_string(), "1M");
    }

    // ── file_suffix / from_suffix roundtrip ─────────────────────────────

    #[test]
    fn suffix_roundtrip() {
        let all = [
            Timeframe::S1,
            Timeframe::S5,
            Timeframe::S15,
            Timeframe::S30,
            Timeframe::M1,
            Timeframe::M5,
            Timeframe::M15,
            Timeframe::M30,
            Timeframe::H1,
            Timeframe::H4,
            Timeframe::D1,
            Timeframe::W1,
            Timeframe::MN1,
        ];
        for tf in all {
            let suffix = tf.file_suffix();
            let parsed = Timeframe::from_suffix(suffix);
            assert_eq!(parsed, Some(tf), "roundtrip failed for {tf:?} -> {suffix}");
        }
    }

    #[test]
    fn from_suffix_unknown_returns_none() {
        assert_eq!(Timeframe::from_suffix("2m"), None);
        assert_eq!(Timeframe::from_suffix(""), None);
    }

    // ── serde roundtrip ─────────────────────────────────────────────────

    #[test]
    fn serde_roundtrip() {
        let tf = Timeframe::H4;
        let json = serde_json::to_string(&tf).unwrap();
        let back: Timeframe = serde_json::from_str(&json).unwrap();
        assert_eq!(tf, back);
    }

    // ── is_calendar ─────────────────────────────────────────────────────

    #[test]
    fn is_calendar_weekly_monthly() {
        assert!(Timeframe::W1.is_calendar());
        assert!(Timeframe::MN1.is_calendar());
        assert!(!Timeframe::D1.is_calendar());
        assert!(!Timeframe::M5.is_calendar());
    }

    // ── next_boundary ───────────────────────────────────────────────────

    #[test]
    fn next_boundary_m5() {
        // Input: 1705312050000 (09:47:30Z), floor to M5 = 1705311900000 (09:45:00Z)
        // Next boundary = 1705311900000 + 300000 = 1705312200000 (09:50:00Z)
        let ts = 1705312050000i64;
        let next = Timeframe::M5.next_boundary(ts);
        let expected = 1705312200000i64;
        assert_eq!(next, expected);
    }

    #[test]
    fn next_boundary_mn1() {
        // 2024-01-15 -> next boundary is 2024-02-01
        let ts = 1705326600000i64; // 2024-01-15T14:30:00Z
        let next = Timeframe::MN1.next_boundary(ts);
        let expected = 1706745600000i64; // 2024-02-01T00:00:00Z
        assert_eq!(next, expected);
    }
}
