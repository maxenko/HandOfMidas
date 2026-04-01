use midas_data::CandleBuffer;

use crate::error::StoreError;
use crate::types::{CacheInfo, DataKey};

/// Commands sent to the DuckDB actor thread.
pub(crate) enum DbCommand {
    InsertCandles {
        key: DataKey,
        buffer: CandleBuffer,
        source: String,
    },
    QueryCandles {
        key: DataKey,
    },
    QueryCandlesRange {
        key: DataKey,
        start: i64,
        end: i64,
    },
    ListCached,
    Shutdown,
}

/// Replies from the DuckDB actor thread.
pub(crate) enum DbReply {
    Inserted(Result<usize, StoreError>),
    Candles(Result<CandleBuffer, StoreError>),
    CacheList(Result<Vec<CacheInfo>, StoreError>),
    ShutdownAck,
    /// Connection or migration failure, independent of command type.
    Error(StoreError),
}
