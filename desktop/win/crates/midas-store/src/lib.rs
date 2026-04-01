//! midas-store: DuckDB-backed persistent cache for historical candle data.
//!
//! All DuckDB operations run on a dedicated OS thread via a mailbox actor.
//! The public [`DbHandle`] communicates with this thread through async
//! message passing, keeping blocking C++ FFI calls off the tokio threadpool.

mod actor;
mod convert;
mod error;
mod handle;
mod queries;
mod schema;
mod types;

pub use error::StoreError;
pub use handle::DbHandle;
pub use types::{CacheInfo, DataKey, StoreConfig};
