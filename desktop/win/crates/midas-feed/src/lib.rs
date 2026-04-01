//! midas-feed: Data import and market data ingest.
//!
//! Depends on: midas-core, midas-data
//!
//! Currently supports CSV import, deterministic test data generation,
//! and the TestProvider (DataProvider trait wrapper).

pub mod csv;
pub mod error;
pub mod test_provider;
pub mod testdata;

// ── Planned modules (uncomment as implemented) ──────────────────────
// pub mod aggregator; // Tick-to-candle aggregation
// pub mod replay;     // Historical data replay for testing

/// Re-export the primary entry point for ergonomic usage.
/// ```no_run
/// use midas_feed::import_csv;
/// let buf = import_csv(std::path::Path::new("data.csv")).unwrap();
/// ```
pub use csv::import_csv;
pub use error::CsvError;
pub use test_provider::TestProvider;
pub use testdata::TestDataProvider;
