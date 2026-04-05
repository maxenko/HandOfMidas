//! Parsers for IB-style duration and bar-size strings.
//!
//! Converts the string formats used in `BrokerCommand::RequestHistoricalData`
//! into typed `Timeframe` values and epoch timestamps.

use chrono::{DateTime, Months, Utc};
use midas_core::Timeframe;

use crate::error::BrokerError;

/// Parse an IB `bar_size` string into a [`Timeframe`].
///
/// Supported formats match the IB API historical data bar sizes.
pub fn parse_bar_size(bar_size: &str) -> Result<Timeframe, BrokerError> {
    match bar_size {
        "1 secs" => Ok(Timeframe::S1),
        "5 secs" => Ok(Timeframe::S5),
        "15 secs" => Ok(Timeframe::S15),
        "30 secs" => Ok(Timeframe::S30),
        "1 min" => Ok(Timeframe::M1),
        "5 mins" => Ok(Timeframe::M5),
        "15 mins" => Ok(Timeframe::M15),
        "30 mins" => Ok(Timeframe::M30),
        "1 hour" => Ok(Timeframe::H1),
        "4 hours" => Ok(Timeframe::H4),
        "1 day" => Ok(Timeframe::D1),
        "1 week" => Ok(Timeframe::W1),
        "1 month" => Ok(Timeframe::MN1),
        other => Err(BrokerError::Config(format!(
            "unknown bar_size: \"{other}\""
        ))),
    }
}

/// Parse an IB `duration` string and compute the start timestamp.
///
/// Given `end` (UTC epoch seconds) and a duration like `"30 D"`, returns
/// the start timestamp. Supported units: `S` (seconds), `D` (days),
/// `W` (weeks), `M` (months), `Y` (years).
pub fn duration_to_start(end: i64, duration: &str) -> Result<i64, BrokerError> {
    let parts: Vec<&str> = duration.split_whitespace().collect();
    if parts.len() != 2 {
        return Err(BrokerError::Config(format!(
            "invalid duration format: \"{duration}\" (expected \"<n> <unit>\")"
        )));
    }

    let n: u32 = parts[0].parse().map_err(|_| {
        BrokerError::Config(format!(
            "invalid duration number: \"{}\" in \"{duration}\"",
            parts[0]
        ))
    })?;

    match parts[1] {
        "S" => Ok(end - n as i64),
        "D" => Ok(end - n as i64 * 86400),
        "W" => Ok(end - n as i64 * 7 * 86400),
        "M" => subtract_calendar_months(end, n, duration),
        "Y" => subtract_calendar_months(end, n * 12, duration),
        other => Err(BrokerError::Config(format!(
            "unknown duration unit: \"{other}\" in \"{duration}\""
        ))),
    }
}

/// Convert a [`Timeframe`] to the IB bar size string.
///
/// This is the inverse of [`parse_bar_size`].
pub fn timeframe_to_bar_size(tf: Timeframe) -> String {
    match tf {
        Timeframe::S1 => "1 secs",
        Timeframe::S5 => "5 secs",
        Timeframe::S15 => "15 secs",
        Timeframe::S30 => "30 secs",
        Timeframe::M1 => "1 min",
        Timeframe::M5 => "5 mins",
        Timeframe::M15 => "15 mins",
        Timeframe::M30 => "30 mins",
        Timeframe::H1 => "1 hour",
        Timeframe::H4 => "4 hours",
        Timeframe::D1 => "1 day",
        Timeframe::W1 => "1 week",
        Timeframe::MN1 => "1 month",
    }
    .to_string()
}

fn subtract_calendar_months(end: i64, months: u32, duration: &str) -> Result<i64, BrokerError> {
    let dt = DateTime::<Utc>::from_timestamp(end, 0)
        .ok_or_else(|| BrokerError::Config(format!("invalid end timestamp: {end}")))?;
    let start_dt = dt.checked_sub_months(Months::new(months)).ok_or_else(|| {
        BrokerError::Config(format!("month subtraction overflow for \"{duration}\""))
    })?;
    Ok(start_dt.timestamp())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── parse_bar_size ────────────────────────────────────────────────

    #[test]
    fn parse_all_bar_sizes() {
        let cases = [
            ("1 secs", Timeframe::S1),
            ("5 secs", Timeframe::S5),
            ("15 secs", Timeframe::S15),
            ("30 secs", Timeframe::S30),
            ("1 min", Timeframe::M1),
            ("5 mins", Timeframe::M5),
            ("15 mins", Timeframe::M15),
            ("30 mins", Timeframe::M30),
            ("1 hour", Timeframe::H1),
            ("4 hours", Timeframe::H4),
            ("1 day", Timeframe::D1),
            ("1 week", Timeframe::W1),
            ("1 month", Timeframe::MN1),
        ];
        for (s, expected) in cases {
            assert_eq!(parse_bar_size(s).unwrap(), expected, "failed for \"{s}\"");
        }
    }

    #[test]
    fn parse_bar_size_unknown() {
        assert!(parse_bar_size("2 mins").is_err());
        assert!(parse_bar_size("").is_err());
        assert!(parse_bar_size("daily").is_err());
    }

    // ── duration_to_start ─────────────────────────────────────────────

    #[test]
    fn duration_seconds() {
        let end = 1_000_000;
        assert_eq!(duration_to_start(end, "3600 S").unwrap(), end - 3600);
    }

    #[test]
    fn duration_days() {
        let end = 1_700_000_000;
        assert_eq!(duration_to_start(end, "30 D").unwrap(), end - 30 * 86400);
    }

    #[test]
    fn duration_weeks() {
        let end = 1_700_000_000;
        assert_eq!(duration_to_start(end, "2 W").unwrap(), end - 14 * 86400);
    }

    #[test]
    fn duration_months() {
        // 2024-01-15 → subtract 3 months → 2023-10-15
        let end = 1705276800; // 2024-01-15 00:00 UTC
        let start = duration_to_start(end, "3 M").unwrap();
        let dt = DateTime::<Utc>::from_timestamp(start, 0).unwrap();
        assert_eq!(dt.format("%Y-%m-%d").to_string(), "2023-10-15");
    }

    #[test]
    fn duration_years() {
        let end = 1705276800; // 2024-01-15 00:00 UTC
        let start = duration_to_start(end, "1 Y").unwrap();
        let dt = DateTime::<Utc>::from_timestamp(start, 0).unwrap();
        assert_eq!(dt.format("%Y-%m-%d").to_string(), "2023-01-15");
    }

    #[test]
    fn duration_invalid_format() {
        assert!(duration_to_start(1000, "30").is_err());
        assert!(duration_to_start(1000, "D 30").is_err());
        assert!(duration_to_start(1000, "abc D").is_err());
        assert!(duration_to_start(1000, "30 X").is_err());
    }
}
