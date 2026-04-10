use duckdb::Connection;
use mailbox_processor::MailboxProcessor;
use midas_data::CandleBuffer;
use tokio::sync::mpsc::Sender;

use crate::actor::{DbCommand, DbReply};
use crate::error::StoreError;
use crate::queries;
use crate::schema::run_migrations;
use crate::types::{CacheInfo, DataKey, StoreConfig};

/// Actor state: tracks whether connection has permanently failed.
enum ConnState {
    /// Not yet initialized (lazy open on first command).
    Uninit,
    /// Connection is open and healthy.
    Open(Connection),
    /// Connection failed permanently — all subsequent commands get this error.
    Failed(String),
}

/// Async-safe handle to the DuckDB data store.
///
/// Wraps a `MailboxProcessor` that owns a DuckDB `Connection` on a dedicated
/// OS thread. All database operations are serialized through the mailbox.
///
/// `DbHandle` is cheaply cloneable — each clone shares the same channel.
#[derive(Clone)]
pub struct DbHandle {
    mb: MailboxProcessor<DbCommand, DbReply>,
}

impl DbHandle {
    /// Open a DuckDB database. Synchronous — spawns the actor thread and
    /// returns immediately. Connection opens lazily on the first command.
    pub fn open(config: StoreConfig) -> Self {
        let path = config.path.clone();
        let memory_limit_mb = config.memory_limit_mb;
        let threads = config.threads;

        let mb = MailboxProcessor::new_blocking(
            Some(256),
            ConnState::Uninit,
            "duckdb-store",
            move |cmd, state, reply_channel| {
                let conn = match state {
                    ConnState::Open(c) => c,
                    ConnState::Failed(ref msg) => {
                        // Permanent failure — don't retry.
                        send_reply(
                            reply_channel,
                            DbReply::Error(StoreError::ConnectionFailed(msg.clone())),
                        );
                        return ConnState::Failed(msg.clone());
                    }
                    ConnState::Uninit => match init_connection(&path, memory_limit_mb, threads) {
                        Ok(c) => {
                            tracing::info!("DuckDB store ready");
                            c
                        }
                        Err(e) => {
                            let msg = e.to_string();
                            tracing::error!("DuckDB init failed: {msg}");
                            send_reply(reply_channel, DbReply::Error(e));
                            return ConnState::Failed(msg);
                        }
                    },
                };

                // Dispatch command.
                dispatch(&conn, cmd, reply_channel);
                ConnState::Open(conn)
            },
        );

        DbHandle { mb }
    }

    /// Open an in-memory DuckDB database (for tests).
    pub fn open_memory() -> Self {
        Self::open(StoreConfig::memory())
    }

    /// Insert candles into the store with a source tag.
    pub async fn insert_candles(
        &self,
        key: DataKey,
        buffer: CandleBuffer,
        source: &str,
    ) -> Result<usize, StoreError> {
        let reply = self
            .mb
            .send(DbCommand::InsertCandles {
                key,
                buffer,
                source: source.into(),
            })
            .await
            .map_err(|_| StoreError::ChannelClosed)?;
        match reply {
            DbReply::Inserted(r) => r,
            DbReply::Error(e) => Err(e),
            _ => Err(StoreError::UnexpectedReply),
        }
    }

    /// Fire-and-forget insert (write-behind cache pattern).
    pub async fn fire_and_forget_insert(
        &self,
        key: DataKey,
        buffer: CandleBuffer,
        source: &str,
    ) -> Result<(), StoreError> {
        self.mb
            .fire_and_forget(DbCommand::InsertCandles {
                key,
                buffer,
                source: source.into(),
            })
            .await
            .map_err(|_| StoreError::ChannelClosed)
    }

    /// Query all candles for a given key.
    pub async fn query_candles(&self, key: DataKey) -> Result<CandleBuffer, StoreError> {
        let reply = self
            .mb
            .send(DbCommand::QueryCandles { key })
            .await
            .map_err(|_| StoreError::ChannelClosed)?;
        match reply {
            DbReply::Candles(r) => r,
            DbReply::Error(e) => Err(e),
            _ => Err(StoreError::UnexpectedReply),
        }
    }

    /// Query candles within a time range.
    pub async fn query_candles_range(
        &self,
        key: DataKey,
        start: i64,
        end: i64,
    ) -> Result<CandleBuffer, StoreError> {
        let reply = self
            .mb
            .send(DbCommand::QueryCandlesRange { key, start, end })
            .await
            .map_err(|_| StoreError::ChannelClosed)?;
        match reply {
            DbReply::Candles(r) => r,
            DbReply::Error(e) => Err(e),
            _ => Err(StoreError::UnexpectedReply),
        }
    }

    /// List all cached symbol/timeframe pairs.
    pub async fn list_cached(&self) -> Result<Vec<CacheInfo>, StoreError> {
        let reply = self
            .mb
            .send(DbCommand::ListCached)
            .await
            .map_err(|_| StoreError::ChannelClosed)?;
        match reply {
            DbReply::CacheList(r) => r,
            DbReply::Error(e) => Err(e),
            _ => Err(StoreError::UnexpectedReply),
        }
    }

    /// Graceful shutdown: checkpoint WAL.
    pub async fn shutdown(&self) -> Result<(), StoreError> {
        let reply = self
            .mb
            .send(DbCommand::Shutdown)
            .await
            .map_err(|_| StoreError::ChannelClosed)?;
        match reply {
            DbReply::ShutdownAck => Ok(()),
            DbReply::Error(e) => Err(e),
            _ => Err(StoreError::UnexpectedReply),
        }
    }
}

/// Initialize the DuckDB connection, configure it, and run migrations.
fn init_connection(
    path: &Option<std::path::PathBuf>,
    memory_limit_mb: u32,
    threads: u8,
) -> Result<Connection, StoreError> {
    let conn = match path {
        Some(p) => Connection::open(p).map_err(|e| StoreError::ConnectionFailed(e.to_string()))?,
        None => {
            Connection::open_in_memory().map_err(|e| StoreError::ConnectionFailed(e.to_string()))?
        }
    };

    // Configure — propagate errors instead of silently ignoring them.
    conn.execute_batch(&format!(
        "SET memory_limit = '{}MB'; \
         SET threads = {}; \
         SET enable_progress_bar = false; \
         SET enable_object_cache = true;",
        memory_limit_mb.max(1), // Clamp to at least 1MB
        threads.max(1),         // Clamp to at least 1 thread
    ))
    .map_err(|e| StoreError::ConfigFailed(e.to_string()))?;

    run_migrations(&conn)?;

    // Reconcile metadata on startup (self-healing after crash).
    if let Err(e) = queries::reconcile_data_ranges(&conn) {
        tracing::warn!("data_ranges reconciliation failed: {e}");
        // Non-fatal: metadata may be stale but data is intact.
    }

    Ok(conn)
}

fn send_reply(ch: Option<Sender<DbReply>>, reply: DbReply) {
    if let Some(ch) = ch {
        let _ = ch.blocking_send(reply);
    }
}

fn dispatch(conn: &Connection, cmd: DbCommand, reply_channel: Option<Sender<DbReply>>) {
    let reply = match cmd {
        DbCommand::InsertCandles {
            key,
            buffer,
            source,
        } => DbReply::Inserted(queries::bulk_insert(conn, &key, &buffer, &source)),
        DbCommand::QueryCandles { key } => DbReply::Candles(queries::query_all(conn, &key)),
        DbCommand::QueryCandlesRange { key, start, end } => {
            DbReply::Candles(queries::query_range(conn, &key, start, end))
        }
        DbCommand::ListCached => DbReply::CacheList(queries::list_cached(conn)),
        DbCommand::Shutdown => {
            let _ = conn.execute_batch("CHECKPOINT");
            DbReply::ShutdownAck
        }
    };

    send_reply(reply_channel, reply);
}

#[cfg(test)]
mod tests;
