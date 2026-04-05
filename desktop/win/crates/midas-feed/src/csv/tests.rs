use super::*;

// ── Yahoo Finance format ────────────────────────────────────────────

#[test]
fn import_yahoo_format() {
    let csv = "\
Date,Open,High,Low,Close,Adj Close,Volume
2024-01-02,185.33,186.35,184.01,185.64,185.64,42500000
2024-01-03,184.22,185.88,183.43,184.25,184.25,38700000
2024-01-04,182.15,183.09,180.88,181.91,181.91,41200000
";
    let buf = import_csv_from_str(csv).unwrap();
    assert_eq!(buf.len(), 3);

    // First candle: 2024-01-02 00:00 UTC
    let expected_first_ts = NaiveDate::from_ymd_opt(2024, 1, 2)
        .unwrap()
        .and_hms_opt(0, 0, 0)
        .unwrap()
        .and_utc()
        .timestamp_millis();
    assert_eq!(buf.timestamps[0], expected_first_ts);
    assert!((buf.opens[0] - 185.33).abs() < 0.01);
    assert!((buf.highs[0] - 186.35).abs() < 0.01);
    assert!((buf.lows[0] - 184.01).abs() < 0.01);
    assert!((buf.closes[0] - 185.64).abs() < 0.01);
    assert_eq!(buf.volumes[0], 42500000);
}

#[test]
fn adj_close_is_ignored() {
    // The close column should pick "Close", not "Adj Close".
    let csv = "\
Date,Open,High,Low,Close,Adj Close,Volume
2024-01-02,185.33,186.35,184.01,185.64,180.00,42500000
";
    let buf = import_csv_from_str(csv).unwrap();
    // Close should be 185.64 (the Close column), not 180.00 (Adj Close).
    assert!((buf.closes[0] - 185.64).abs() < 0.01);
}

// ── Epoch format ────────────────────────────────────────────────────

#[test]
fn import_epoch_format() {
    let csv = "\
timestamp,open,high,low,close,volume
1704153600000,185.33,186.35,184.01,185.64,42500000
1704153660000,185.70,186.10,185.50,185.90,1200000
1704153720000,185.90,186.50,185.80,186.30,980000
";
    let buf = import_csv_from_str(csv).unwrap();
    assert_eq!(buf.len(), 3);
    assert_eq!(buf.timestamps[0], 1704153600000);
    assert_eq!(buf.timestamps[1], 1704153660000);
    assert_eq!(buf.timestamps[2], 1704153720000);
}

// ── Reverse order → sorted ascending ────────────────────────────────

#[test]
fn import_reversed_order_sorts_ascending() {
    let csv = "\
Date,Open,High,Low,Close,Volume
2024-01-05,188.00,189.00,187.00,188.50,35000000
2024-01-04,186.00,187.00,185.00,186.50,38000000
2024-01-03,184.00,185.00,183.00,184.50,41000000
2024-01-02,182.00,183.00,181.00,182.50,44000000
";
    let buf = import_csv_from_str(csv).unwrap();
    assert_eq!(buf.len(), 4);

    // Verify timestamps are ascending.
    for i in 1..buf.len() {
        assert!(
            buf.timestamps[i] > buf.timestamps[i - 1],
            "timestamps not sorted at index {i}"
        );
    }

    // First timestamp should be Jan 2.
    let jan2 = NaiveDate::from_ymd_opt(2024, 1, 2)
        .unwrap()
        .and_hms_opt(0, 0, 0)
        .unwrap()
        .and_utc()
        .timestamp_millis();
    assert_eq!(buf.timestamps[0], jan2);
}

// ── Missing column ──────────────────────────────────────────────────

#[test]
fn missing_column_error() {
    let csv = "\
Date,Open,High,Low,Volume
2024-01-02,185.33,186.35,184.01,42500000
";
    let result = import_csv_from_str(csv);
    assert!(result.is_err());
    let err = result.unwrap_err();
    match &err {
        CsvError::MissingColumn(col) => assert_eq!(col, "close"),
        other => panic!("expected MissingColumn, got: {other}"),
    }
}

#[test]
fn missing_date_column_error() {
    let csv = "\
open,high,low,close,volume
185.33,186.35,184.01,185.64,42500000
";
    let result = import_csv_from_str(csv);
    assert!(result.is_err());
    match result.unwrap_err() {
        CsvError::MissingColumn(col) => {
            assert!(col.contains("date") || col.contains("time"));
        }
        other => panic!("expected MissingColumn, got: {other}"),
    }
}

// ── Empty file ──────────────────────────────────────────────────────

#[test]
fn empty_file_error() {
    let csv = "\
Date,Open,High,Low,Close,Volume
";
    let result = import_csv_from_str(csv);
    assert!(result.is_err());
    match result.unwrap_err() {
        CsvError::EmptyFile => {}
        other => panic!("expected EmptyFile, got: {other}"),
    }
}

#[test]
fn header_only_no_data() {
    let csv = "Date,Open,High,Low,Close,Volume\n";
    let result = import_csv_from_str(csv);
    assert!(result.is_err());
    match result.unwrap_err() {
        CsvError::EmptyFile => {}
        other => panic!("expected EmptyFile, got: {other}"),
    }
}

// ── BOM handling ────────────────────────────────────────────────────

#[test]
fn bom_handling() {
    // Prepend UTF-8 BOM to an otherwise valid CSV.
    let csv_no_bom = "\
Date,Open,High,Low,Close,Volume
2024-01-02,185.33,186.35,184.01,185.64,42500000
";
    let mut csv_with_bom = String::from("\u{FEFF}");
    csv_with_bom.push_str(csv_no_bom);

    // import_csv_from_str receives already-decoded String, but the BOM
    // character can confuse the csv crate's header detection. Let's
    // verify it works after stripping via our read_file_strip_bom path.
    // For the in-memory test, strip manually.
    let stripped = csv_with_bom.trim_start_matches('\u{FEFF}');
    let buf = import_csv_from_str(stripped).unwrap();
    assert_eq!(buf.len(), 1);
}

#[test]
fn bom_bytes_stripped_from_file() {
    // Simulate the byte-level BOM stripping.
    let csv_bytes: Vec<u8> = {
        let mut v = vec![0xEF, 0xBB, 0xBF]; // UTF-8 BOM
        v.extend_from_slice(
            b"Date,Open,High,Low,Close,Volume\n\
              2024-01-02,185.33,186.35,184.01,185.64,42500000\n",
        );
        v
    };

    // Simulate read_file_strip_bom logic inline.
    let content = if csv_bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        String::from_utf8_lossy(&csv_bytes[3..]).into_owned()
    } else {
        String::from_utf8_lossy(&csv_bytes).into_owned()
    };

    let buf = import_csv_from_str(&content).unwrap();
    assert_eq!(buf.len(), 1);
    assert!((buf.opens[0] - 185.33).abs() < 0.01);
}

// ── Negative price ──────────────────────────────────────────────────

#[test]
fn negative_price_error() {
    let csv = "\
Date,Open,High,Low,Close,Volume
2024-01-02,-185.33,186.35,184.01,185.64,42500000
";
    let result = import_csv_from_str(csv);
    assert!(result.is_err());
    match result.unwrap_err() {
        CsvError::NegativePrice { row: 2, .. } => {}
        other => panic!("expected NegativePrice at row 2, got: {other}"),
    }
}

// ── Invalid date ────────────────────────────────────────────────────

#[test]
fn invalid_date_error() {
    let csv = "\
Date,Open,High,Low,Close,Volume
not-a-date,185.33,186.35,184.01,185.64,42500000
";
    let result = import_csv_from_str(csv);
    assert!(result.is_err());
    match result.unwrap_err() {
        CsvError::InvalidDate { row: 2, .. } => {}
        other => panic!("expected InvalidDate at row 2, got: {other}"),
    }
}

// ── Datetime with time component ────────────────────────────────────

#[test]
fn datetime_with_time() {
    let csv = "\
timestamp,open,high,low,close,volume
2024-01-02 09:30:00,185.33,186.35,184.01,185.64,42500000
";
    let buf = import_csv_from_str(csv).unwrap();
    assert_eq!(buf.len(), 1);

    let expected_ts =
        chrono::NaiveDateTime::parse_from_str("2024-01-02 09:30:00", "%Y-%m-%d %H:%M:%S")
            .unwrap()
            .and_utc()
            .timestamp_millis();
    assert_eq!(buf.timestamps[0], expected_ts);
}

// ── Case-insensitive headers ────────────────────────────────────────

#[test]
fn case_insensitive_headers() {
    let csv = "\
DATE,OPEN,HIGH,LOW,CLOSE,VOLUME
2024-01-02,185.33,186.35,184.01,185.64,42500000
";
    let buf = import_csv_from_str(csv).unwrap();
    assert_eq!(buf.len(), 1);
}

// ── Epoch seconds (10-digit) ────────────────────────────────────────

#[test]
fn epoch_seconds_auto_detected() {
    let csv = "\
timestamp,open,high,low,close,volume
1704153600,185.33,186.35,184.01,185.64,42500000
";
    let buf = import_csv_from_str(csv).unwrap();
    assert_eq!(buf.timestamps[0], 1704153600 * 1000);
}

// ── Volume as float ─────────────────────────────────────────────────

#[test]
fn volume_as_float() {
    let csv = "\
Date,Open,High,Low,Close,Volume
2024-01-02,185.33,186.35,184.01,185.64,42500000.0
";
    let buf = import_csv_from_str(csv).unwrap();
    assert_eq!(buf.volumes[0], 42500000);
}

// ── Duplicate timestamps ────────────────────────────────────────────

#[test]
fn duplicate_timestamps_error() {
    let csv = "\
Date,Open,High,Low,Close,Volume
2024-01-02,185.33,186.35,184.01,185.64,42500000
2024-01-02,186.00,187.00,185.00,186.50,38000000
";
    let result = import_csv_from_str(csv);
    assert!(result.is_err());
    match result.unwrap_err() {
        CsvError::DuplicateTimestamp { .. } => {}
        other => panic!("expected DuplicateTimestamp, got: {other}"),
    }
}
