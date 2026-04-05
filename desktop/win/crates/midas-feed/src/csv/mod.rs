//! CSV import pipeline for OHLCV candle data.
//!
//! Supports auto-detection of common CSV formats including Yahoo Finance,
//! generic epoch-millisecond, and other OHLCV exports. Produces a sorted,
//! validated [`CandleBuffer`] ready for rendering or binary serialization.
//!
//! # Supported formats
//!
//! | Source          | Date column              | Extra columns handled |
//! |-----------------|--------------------------|----------------------|
//! | Yahoo Finance   | `Date` (YYYY-MM-DD)      | `Adj Close` skipped  |
//! | Generic epoch   | `timestamp` (epoch ms)   | —                    |
//! | Alpha Vantage   | `timestamp` (YYYY-MM-DD) | —                    |
//!
//! # Example
//!
//! ```no_run
//! use std::path::Path;
//! use midas_feed::csv::import_csv;
//!
//! let buffer = import_csv(Path::new("data/AAPL.csv")).unwrap();
//! assert!(buffer.len() > 0);
//! ```

use std::io::Read;
use std::path::Path;

use chrono::NaiveDate;
use midas_data::candle::CandleBuffer;

use crate::error::CsvError;

// ─── Column mapping ─────────────────────────────────────────────────────

/// Indices of the required OHLCV columns within the CSV header row.
struct ColumnMap {
    date: usize,
    open: usize,
    high: usize,
    low: usize,
    close: usize,
    volume: usize,
}

/// Detect column indices from a CSV header row (case-insensitive).
///
/// Returns a `ColumnMap` or `CsvError::MissingColumn` if a required column
/// is absent.
fn detect_columns(headers: &csv::StringRecord) -> Result<ColumnMap, CsvError> {
    let fields: Vec<String> = headers.iter().map(|h| h.trim().to_lowercase()).collect();

    let date = find_col(&fields, &["date", "time", "timestamp", "datetime"])
        .ok_or_else(|| CsvError::MissingColumn("date/time/timestamp".into()))?;

    let open =
        find_col(&fields, &["open", "o"]).ok_or_else(|| CsvError::MissingColumn("open".into()))?;

    let high =
        find_col(&fields, &["high", "h"]).ok_or_else(|| CsvError::MissingColumn("high".into()))?;

    let low =
        find_col(&fields, &["low", "l"]).ok_or_else(|| CsvError::MissingColumn("low".into()))?;

    // "close" but NOT "adj close"
    let close = find_close_col(&fields).ok_or_else(|| CsvError::MissingColumn("close".into()))?;

    let volume = find_col(&fields, &["volume", "vol", "v"])
        .ok_or_else(|| CsvError::MissingColumn("volume".into()))?;

    Ok(ColumnMap {
        date,
        open,
        high,
        low,
        close,
        volume,
    })
}

/// Find the first column whose lowercased name matches one of `candidates`.
fn find_col(fields: &[String], candidates: &[&str]) -> Option<usize> {
    fields
        .iter()
        .position(|f| candidates.iter().any(|&c| f == c))
}

/// Find the "close" column, preferring an exact match for "close" or "c"
/// over "adj close". This ensures we pick the unadjusted close when both
/// "Close" and "Adj Close" are present (Yahoo Finance format).
fn find_close_col(fields: &[String]) -> Option<usize> {
    // First try exact matches that are NOT "adj close".
    for (i, f) in fields.iter().enumerate() {
        if (f == "close" || f == "c") && f != "adj close" {
            return Some(i);
        }
    }
    None
}

// ─── Date / timestamp parsing ───────────────────────────────────────────

/// Parse a date/time value from a CSV cell into epoch milliseconds (UTC).
///
/// Handles:
/// - Epoch milliseconds (large integer, > 1_000_000_000_000)
/// - Epoch seconds (integer < 1_000_000_000_000 but > 1_000_000_000)
/// - `YYYY-MM-DD` date strings (treated as midnight UTC)
/// - `YYYY-MM-DD HH:MM:SS` datetime strings (treated as UTC)
fn parse_timestamp(value: &str, row: usize) -> Result<i64, CsvError> {
    let trimmed = value.trim();

    // Try parsing as an integer (epoch ms or epoch seconds).
    if let Ok(n) = trimmed.parse::<i64>() {
        if n > 1_000_000_000_000 {
            // Epoch milliseconds
            return Ok(n);
        } else if n > 1_000_000_000 {
            // Epoch seconds — convert to ms
            return Ok(n * 1000);
        }
        // Very small number — probably not a valid timestamp, fall through.
    }

    // Try YYYY-MM-DD
    if let Ok(date) = NaiveDate::parse_from_str(trimmed, "%Y-%m-%d") {
        let dt = date.and_hms_opt(0, 0, 0).expect("midnight is always valid");
        return Ok(dt.and_utc().timestamp_millis());
    }

    // Try YYYY-MM-DD HH:MM:SS
    if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(trimmed, "%Y-%m-%d %H:%M:%S") {
        return Ok(dt.and_utc().timestamp_millis());
    }

    // Try ISO 8601 with T separator: YYYY-MM-DDTHH:MM:SS
    if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(trimmed, "%Y-%m-%dT%H:%M:%S") {
        return Ok(dt.and_utc().timestamp_millis());
    }

    Err(CsvError::InvalidDate {
        row,
        value: trimmed.to_string(),
    })
}

// ─── Row parsing ────────────────────────────────────────────────────────

/// A single parsed candle row before validation.
struct RawCandle {
    ts: i64,
    open: f32,
    high: f32,
    low: f32,
    close: f32,
    volume: u32,
}

/// Parse one CSV record into a `RawCandle` using the detected column map.
fn parse_row(
    record: &csv::StringRecord,
    cols: &ColumnMap,
    row: usize,
) -> Result<RawCandle, CsvError> {
    let get = |idx: usize, name: &str| -> Result<&str, CsvError> {
        record.get(idx).ok_or_else(|| CsvError::ParseError {
            row,
            message: format!("missing column `{name}` (index {idx})"),
        })
    };

    let ts_str = get(cols.date, "date/timestamp")?;
    let ts = parse_timestamp(ts_str, row)?;

    let open = parse_f32(get(cols.open, "open")?, row, "open")?;
    let high = parse_f32(get(cols.high, "high")?, row, "high")?;
    let low = parse_f32(get(cols.low, "low")?, row, "low")?;
    let close = parse_f32(get(cols.close, "close")?, row, "close")?;
    let volume = parse_volume(get(cols.volume, "volume")?, row)?;

    Ok(RawCandle {
        ts,
        open,
        high,
        low,
        close,
        volume,
    })
}

/// Parse a string as `f32`, returning a descriptive error on failure.
fn parse_f32(s: &str, row: usize, field: &str) -> Result<f32, CsvError> {
    s.trim().parse::<f32>().map_err(|_| CsvError::ParseError {
        row,
        message: format!("cannot parse `{field}` as f32: \"{s}\""),
    })
}

/// Parse volume, which may be a float in some CSV exports (e.g., "42500000.0").
fn parse_volume(s: &str, row: usize) -> Result<u32, CsvError> {
    let trimmed = s.trim();

    // Try direct u32 first.
    if let Ok(v) = trimmed.parse::<u32>() {
        return Ok(v);
    }

    // Some exports write volume as float (e.g., "42500000.0").
    if let Ok(v) = trimmed.parse::<f64>() {
        if v >= 0.0 && v <= u32::MAX as f64 {
            return Ok(v as u32);
        }
    }

    Err(CsvError::ParseError {
        row,
        message: format!("cannot parse volume: \"{trimmed}\""),
    })
}

// ─── Validation ─────────────────────────────────────────────────────────

/// Validate a single candle's prices. Returns an error for negative prices.
fn validate_prices(candle: &RawCandle, row: usize) -> Result<(), CsvError> {
    for (value, name) in [
        (candle.open, "open"),
        (candle.high, "high"),
        (candle.low, "low"),
        (candle.close, "close"),
    ] {
        if value < 0.0 {
            return Err(CsvError::NegativePrice { row, value });
        }
        if !value.is_finite() {
            return Err(CsvError::ParseError {
                row,
                message: format!("`{name}` is not finite: {value}"),
            });
        }
    }
    Ok(())
}

/// Check that a timestamp is not in the future (with a small grace margin).
fn validate_timestamp_not_future(ts_ms: i64, row: usize) -> Result<(), CsvError> {
    // Allow 48 hours of grace for timezone differences and data lag.
    let now_ms = chrono::Utc::now().timestamp_millis();
    let grace_ms = 48 * 60 * 60 * 1000;
    if ts_ms > now_ms + grace_ms {
        return Err(CsvError::FutureTimestamp { row, ts_ms });
    }
    Ok(())
}

// ─── Sorting helpers ────────────────────────────────────────────────────

/// An intermediate candle with its original row number, used for sorting
/// and deduplication before writing into a `CandleBuffer`.
struct IndexedCandle {
    ts: i64,
    open: f32,
    high: f32,
    low: f32,
    close: f32,
    volume: u32,
}

// ─── Public API ─────────────────────────────────────────────────────────

/// Import an OHLCV CSV file and return a validated, sorted [`CandleBuffer`].
///
/// The importer auto-detects column layout from the header row
/// (case-insensitive). It handles:
///
/// - Yahoo Finance format (`Date, Open, High, Low, Close, Adj Close, Volume`)
/// - Generic epoch format (`timestamp, open, high, low, close, volume`)
/// - Date strings (`YYYY-MM-DD`, `YYYY-MM-DD HH:MM:SS`) and epoch
///   milliseconds
/// - Files with a UTF-8 BOM at the start
/// - Reverse-chronological order (sorted ascending on output)
///
/// # Errors
///
/// Returns [`CsvError`] on I/O failure, missing columns, unparseable rows,
/// negative prices, future timestamps, or empty files.
pub fn import_csv(path: &Path) -> Result<CandleBuffer, CsvError> {
    // Read the file, stripping a BOM if present.
    let content = read_file_strip_bom(path)?;
    import_csv_from_str(&content)
}

/// Import OHLCV data from an in-memory CSV string. This is the core
/// implementation shared by [`import_csv`] and unit tests.
pub fn import_csv_from_str(content: &str) -> Result<CandleBuffer, CsvError> {
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(true)
        .flexible(true)
        .trim(csv::Trim::All)
        .from_reader(content.as_bytes());

    let headers = reader.headers()?.clone();
    let cols = detect_columns(&headers)?;

    // Parse all data rows.
    let mut candles: Vec<IndexedCandle> = Vec::new();
    for (idx, result) in reader.records().enumerate() {
        let row_num = idx + 2; // 1-based, header is row 1
        let record = result.map_err(|e| CsvError::ParseError {
            row: row_num,
            message: e.to_string(),
        })?;

        let raw = parse_row(&record, &cols, row_num)?;
        validate_prices(&raw, row_num)?;
        validate_timestamp_not_future(raw.ts, row_num)?;

        candles.push(IndexedCandle {
            ts: raw.ts,
            open: raw.open,
            high: raw.high,
            low: raw.low,
            close: raw.close,
            volume: raw.volume,
        });
    }

    if candles.is_empty() {
        return Err(CsvError::EmptyFile);
    }

    // Sort by timestamp ascending.
    candles.sort_by_key(|c| c.ts);

    // Validate monotonically increasing timestamps (no duplicates after sort).
    for i in 1..candles.len() {
        if candles[i].ts == candles[i - 1].ts {
            return Err(CsvError::DuplicateTimestamp {
                row: i + 1,
                ts_ms: candles[i].ts,
            });
        }
    }

    // Build the CandleBuffer.
    let mut buffer = CandleBuffer::with_capacity(candles.len());
    for c in &candles {
        buffer.push(c.ts, c.open, c.high, c.low, c.close, c.volume);
    }

    Ok(buffer)
}

/// Read a file to a `String`, stripping a leading UTF-8 BOM if present.
fn read_file_strip_bom(path: &Path) -> Result<String, CsvError> {
    let mut file = std::fs::File::open(path)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;

    // UTF-8 BOM: EF BB BF
    let content = if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        String::from_utf8_lossy(&bytes[3..]).into_owned()
    } else {
        String::from_utf8_lossy(&bytes).into_owned()
    };

    Ok(content)
}

#[cfg(test)]
mod tests;
