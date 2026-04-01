/// Errors produced by midas-store operations.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// DuckDB returned an error.
    #[error("DuckDB error: {0}")]
    DuckDb(#[from] duckdb::Error),

    /// The database file could not be opened.
    #[error("connection failed: {0}")]
    ConnectionFailed(String),

    /// DuckDB configuration (memory limit, threads) failed to apply.
    #[error("configuration failed: {0}")]
    ConfigFailed(String),

    /// Schema migration failed.
    #[error("migration failed: {0}")]
    Migration(String),

    /// The actor thread has exited or the channel is closed.
    #[error("actor channel closed")]
    ChannelClosed,

    /// A `timeframe_secs` value does not map to any known Timeframe variant.
    #[error("invalid timeframe_secs: {0}")]
    InvalidTimeframe(u32),

    /// Actor returned an unexpected reply variant.
    #[error("unexpected reply from actor")]
    UnexpectedReply,
}
