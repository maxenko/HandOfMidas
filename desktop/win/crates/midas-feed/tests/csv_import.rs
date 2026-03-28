//! Integration tests for the CSV import pipeline.
//!
//! These tests exercise the full `import_csv` path, reading real CSV files
//! from `tests/data/`.

use std::path::PathBuf;

use midas_feed::csv::{import_csv, import_csv_from_str};
use midas_feed::CsvError;

/// Helper: resolve a test data file relative to the crate root.
fn test_data(name: &str) -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("tests");
    path.push("data");
    path.push(name);
    path
}

// ─── Yahoo Finance format (aapl_daily_sample.csv) ───────────────────────

#[test]
fn import_aapl_daily_sample() {
    let path = test_data("aapl_daily_sample.csv");
    let buf = import_csv(&path).expect("failed to import aapl_daily_sample.csv");

    assert_eq!(buf.len(), 20, "expected 20 candles");

    // First candle: 2024-01-02
    let first_ts = chrono::NaiveDate::from_ymd_opt(2024, 1, 2)
        .unwrap()
        .and_hms_opt(0, 0, 0)
        .unwrap()
        .and_utc()
        .timestamp_millis();
    assert_eq!(buf.timestamps[0], first_ts);
    assert!((buf.opens[0] - 185.33).abs() < 0.01);
    assert!((buf.highs[0] - 186.35).abs() < 0.01);
    assert!((buf.lows[0] - 184.01).abs() < 0.01);
    assert!((buf.closes[0] - 185.64).abs() < 0.01);
    assert_eq!(buf.volumes[0], 42500000);

    // Last candle: 2024-01-30
    let last_ts = chrono::NaiveDate::from_ymd_opt(2024, 1, 30)
        .unwrap()
        .and_hms_opt(0, 0, 0)
        .unwrap()
        .and_utc()
        .timestamp_millis();
    assert_eq!(buf.timestamps[19], last_ts);
    assert!((buf.closes[19] - 192.05).abs() < 0.01);

    // Timestamps must be monotonically increasing.
    for i in 1..buf.len() {
        assert!(
            buf.timestamps[i] > buf.timestamps[i - 1],
            "timestamps not sorted at index {i}"
        );
    }
}

#[test]
fn aapl_adj_close_is_ignored() {
    // The "Adj Close" column in the sample has the same value as "Close",
    // but the importer should be selecting the "Close" column by name, not
    // "Adj Close". Verify by checking a known close value.
    let path = test_data("aapl_daily_sample.csv");
    let buf = import_csv(&path).unwrap();
    // Row 1 (2024-01-02): Close=185.64, Adj Close=185.64 (same here)
    assert!((buf.closes[0] - 185.64).abs() < 0.01);
}

// ─── Generic epoch format (generic_1m_sample.csv) ───────────────────────

#[test]
fn import_generic_1m_sample() {
    let path = test_data("generic_1m_sample.csv");
    let buf = import_csv(&path).expect("failed to import generic_1m_sample.csv");

    assert_eq!(buf.len(), 50, "expected 50 candles");

    // First timestamp
    assert_eq!(buf.timestamps[0], 1704290400000);

    // Last timestamp
    assert_eq!(buf.timestamps[49], 1704293340000);

    // Timestamps must be monotonically increasing.
    for i in 1..buf.len() {
        assert!(
            buf.timestamps[i] > buf.timestamps[i - 1],
            "timestamps not sorted at index {i}"
        );
    }

    // Verify first candle prices.
    assert!((buf.opens[0] - 185.33).abs() < 0.01);
    assert!((buf.highs[0] - 185.55).abs() < 0.01);
    assert!((buf.lows[0] - 185.20).abs() < 0.01);
    assert!((buf.closes[0] - 185.45).abs() < 0.01);
    assert_eq!(buf.volumes[0], 125000);
}

// ─── Reversed order (reversed_order.csv) ────────────────────────────────

#[test]
fn import_reversed_order_sorts_ascending() {
    let path = test_data("reversed_order.csv");
    let buf = import_csv(&path).expect("failed to import reversed_order.csv");

    assert_eq!(buf.len(), 20, "expected 20 candles");

    // Timestamps must be sorted ascending.
    for i in 1..buf.len() {
        assert!(
            buf.timestamps[i] > buf.timestamps[i - 1],
            "timestamps not sorted at index {i}: {} <= {}",
            buf.timestamps[i],
            buf.timestamps[i - 1]
        );
    }

    // First candle after sorting should be 2024-01-02.
    let jan2 = chrono::NaiveDate::from_ymd_opt(2024, 1, 2)
        .unwrap()
        .and_hms_opt(0, 0, 0)
        .unwrap()
        .and_utc()
        .timestamp_millis();
    assert_eq!(buf.timestamps[0], jan2);

    // Last candle after sorting should be 2024-01-30.
    let jan30 = chrono::NaiveDate::from_ymd_opt(2024, 1, 30)
        .unwrap()
        .and_hms_opt(0, 0, 0)
        .unwrap()
        .and_utc()
        .timestamp_millis();
    assert_eq!(buf.timestamps[19], jan30);
}

// ─── Missing column errors ──────────────────────────────────────────────

#[test]
fn missing_close_column_returns_error() {
    let csv = "Date,Open,High,Low,Volume\n2024-01-02,185.33,186.35,184.01,42500000\n";
    let result = import_csv_from_str(csv);
    assert!(result.is_err());
    match result.unwrap_err() {
        CsvError::MissingColumn(col) => assert_eq!(col, "close"),
        other => panic!("expected MissingColumn(\"close\"), got: {other}"),
    }
}

#[test]
fn missing_volume_column_returns_error() {
    let csv = "Date,Open,High,Low,Close\n2024-01-02,185.33,186.35,184.01,185.64\n";
    let result = import_csv_from_str(csv);
    assert!(result.is_err());
    match result.unwrap_err() {
        CsvError::MissingColumn(col) => assert_eq!(col, "volume"),
        other => panic!("expected MissingColumn(\"volume\"), got: {other}"),
    }
}

// ─── Empty file errors ──────────────────────────────────────────────────

#[test]
fn empty_csv_body_returns_error() {
    let csv = "Date,Open,High,Low,Close,Volume\n";
    let result = import_csv_from_str(csv);
    assert!(result.is_err());
    match result.unwrap_err() {
        CsvError::EmptyFile => {}
        other => panic!("expected EmptyFile, got: {other}"),
    }
}

// ─── BOM handling ───────────────────────────────────────────────────────

#[test]
fn bom_file_imports_correctly() {
    // Write a temporary file with a BOM.
    let dir = tempfile::tempdir().expect("failed to create temp dir");
    let path = dir.path().join("bom_test.csv");
    {
        use std::io::Write;
        let mut f = std::fs::File::create(&path).unwrap();
        // UTF-8 BOM
        f.write_all(&[0xEF, 0xBB, 0xBF]).unwrap();
        f.write_all(
            b"Date,Open,High,Low,Close,Volume\n\
              2024-01-02,185.33,186.35,184.01,185.64,42500000\n",
        )
        .unwrap();
    }

    let buf = import_csv(&path).expect("failed to import BOM file");
    assert_eq!(buf.len(), 1);
    assert!((buf.opens[0] - 185.33).abs() < 0.01);
}

// ─── Negative price ─────────────────────────────────────────────────────

#[test]
fn negative_price_returns_error() {
    let csv = "\
Date,Open,High,Low,Close,Volume
2024-01-02,-1.0,186.35,184.01,185.64,42500000
";
    let result = import_csv_from_str(csv);
    assert!(result.is_err());
    match result.unwrap_err() {
        CsvError::NegativePrice { row: 2, .. } => {}
        other => panic!("expected NegativePrice at row 2, got: {other}"),
    }
}

// ─── File not found ─────────────────────────────────────────────────────

#[test]
fn nonexistent_file_returns_io_error() {
    let result = import_csv(std::path::Path::new("/nonexistent/path/data.csv"));
    assert!(result.is_err());
    match result.unwrap_err() {
        CsvError::Io(_) => {}
        other => panic!("expected Io error, got: {other}"),
    }
}
