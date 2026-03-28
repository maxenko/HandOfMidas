//! Error types for the CSV import pipeline.

/// Errors that can occur during CSV import.
#[derive(Debug, thiserror::Error)]
pub enum CsvError {
    /// Underlying I/O error (file not found, permission denied, etc.).
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// A row in the CSV could not be parsed.
    #[error("CSV parse error at row {row}: {message}")]
    ParseError {
        /// 1-based row number in the CSV file (including header).
        row: usize,
        /// Human-readable description of the parse failure.
        message: String,
    },

    /// A required column (open, high, low, close, volume, or a date/time
    /// column) was not found in the CSV header.
    #[error("missing required column: {0}")]
    MissingColumn(String),

    /// A date/time value could not be parsed.
    #[error("invalid date format at row {row}: {value}")]
    InvalidDate {
        /// 1-based row number in the CSV file.
        row: usize,
        /// The raw date string that failed to parse.
        value: String,
    },

    /// The CSV file contained a header but zero data rows.
    #[error("no data rows found")]
    EmptyFile,

    /// A row from the `csv` crate reader could not be deserialized.
    #[error("CSV reader error: {0}")]
    CsvReader(#[from] csv::Error),

    /// A price value was negative.
    #[error("negative price at row {row}: {value}")]
    NegativePrice {
        /// 1-based row number.
        row: usize,
        /// The offending value.
        value: f32,
    },

    /// A timestamp is in the future (beyond current wall-clock time).
    #[error("future timestamp at row {row}: {ts_ms}")]
    FutureTimestamp {
        /// 1-based row number.
        row: usize,
        /// The epoch-millisecond timestamp.
        ts_ms: i64,
    },

    /// Duplicate timestamps found after sorting.
    #[error("duplicate timestamp at row {row}: {ts_ms}")]
    DuplicateTimestamp {
        /// 1-based row number (after sort).
        row: usize,
        /// The duplicated epoch-millisecond timestamp.
        ts_ms: i64,
    },
}
