use midas_core::Timeframe;

use crate::error::StoreError;

/// Composite key identifying a cached dataset: symbol + timeframe.
#[derive(Clone, Debug, Hash, Eq, PartialEq)]
pub struct DataKey {
    pub symbol: String,
    pub timeframe: Timeframe,
}

/// Metadata about a cached candle series.
#[derive(Clone, Debug)]
pub struct CacheInfo {
    pub key: DataKey,
    pub candle_count: usize,
    pub first_ts: i64,
    pub last_ts: i64,
    pub source: String,
}

/// Configuration for opening a DuckDB store.
#[derive(Clone, Debug)]
pub struct StoreConfig {
    /// Path to the DuckDB database file. None = in-memory.
    pub path: Option<std::path::PathBuf>,
    /// Maximum memory DuckDB may use (MB).
    pub memory_limit_mb: u32,
    /// Number of DuckDB internal threads.
    pub threads: u8,
}

impl Default for StoreConfig {
    fn default() -> Self {
        Self {
            path: Some(std::path::PathBuf::from("cache.duckdb")),
            memory_limit_mb: 256,
            threads: 2,
        }
    }
}

impl StoreConfig {
    /// In-memory config for tests.
    pub fn memory() -> Self {
        Self {
            path: None,
            memory_limit_mb: 64,
            threads: 1,
        }
    }
}

/// Convert `Timeframe::as_secs()` (u32) to i32 safely for DuckDB INTEGER columns.
/// All current timeframe values fit in i32 (max is MN1 = 2,592,000).
pub(crate) fn timeframe_to_i32(tf: Timeframe) -> Result<i32, StoreError> {
    let secs = tf.as_secs();
    i32::try_from(secs).map_err(|_| StoreError::InvalidTimeframe(secs))
}
