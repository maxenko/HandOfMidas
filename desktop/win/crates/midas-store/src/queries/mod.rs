use duckdb::{params, Connection};
use midas_data::CandleBuffer;

use crate::convert::timeframe_from_secs;
use crate::error::StoreError;
use crate::types::{timeframe_to_i32, CacheInfo, DataKey};

/// Bulk insert a CandleBuffer using DELETE-before-INSERT + Appender.
///
/// The entire operation (delete + insert + metadata update) runs inside
/// a transaction. If any step fails, the transaction is rolled back and
/// existing data is preserved.
///
/// Returns the number of rows inserted.
pub fn bulk_insert(
    conn: &Connection,
    key: &DataKey,
    buf: &CandleBuffer,
    source: &str,
) -> Result<usize, StoreError> {
    if buf.is_empty() {
        return Ok(0);
    }

    let tf_secs = timeframe_to_i32(key.timeframe)?;
    let first_ts = buf.timestamps[0];
    let last_ts = buf.timestamps[buf.len() - 1];

    conn.execute_batch("BEGIN TRANSACTION")?;

    let result = bulk_insert_inner(conn, key, buf, source, tf_secs, first_ts, last_ts);

    match &result {
        Ok(_) => conn.execute_batch("COMMIT")?,
        Err(_) => {
            let _ = conn.execute_batch("ROLLBACK");
        }
    }

    result
}

fn bulk_insert_inner(
    conn: &Connection,
    key: &DataKey,
    buf: &CandleBuffer,
    source: &str,
    tf_secs: i32,
    first_ts: i64,
    last_ts: i64,
) -> Result<usize, StoreError> {
    // Delete existing data — Appender does NOT support conflict resolution.
    conn.execute(
        "DELETE FROM market.candles WHERE symbol = ? AND timeframe_secs = ?",
        params![&key.symbol, tf_secs],
    )?;

    // Bulk insert via Appender.
    {
        let mut appender = conn.appender_to_db("candles", "market")?;
        for i in 0..buf.len() {
            appender.append_row(params![
                &key.symbol,
                tf_secs,
                buf.timestamps[i],
                buf.opens[i],
                buf.highs[i],
                buf.lows[i],
                buf.closes[i],
                buf.volumes[i],
            ])?;
        }
        appender.flush()?;
    }

    // Update metadata.
    conn.execute(
        "DELETE FROM meta.data_ranges WHERE symbol = ? AND timeframe_secs = ?",
        params![&key.symbol, tf_secs],
    )?;
    conn.execute(
        "INSERT INTO meta.data_ranges
         (symbol, timeframe_secs, candle_count, first_ts, last_ts, source)
         VALUES (?, ?, ?, ?, ?, ?)",
        params![
            &key.symbol,
            tf_secs,
            i32::try_from(buf.len()).unwrap_or(i32::MAX),
            first_ts,
            last_ts,
            source
        ],
    )?;

    Ok(buf.len())
}

/// Query all candles for a given DataKey, ordered by timestamp ascending.
pub fn query_all(conn: &Connection, key: &DataKey) -> Result<CandleBuffer, StoreError> {
    let tf_secs = timeframe_to_i32(key.timeframe)?;

    // Use metadata for capacity hint when available.
    let capacity = conn
        .query_row(
            "SELECT candle_count FROM meta.data_ranges
             WHERE symbol = ? AND timeframe_secs = ?",
            params![&key.symbol, tf_secs],
            |row| row.get::<_, i32>(0),
        )
        .unwrap_or(5000) as usize;

    let mut stmt = conn.prepare_cached(
        "SELECT timestamp_ms, open, high, low, close, volume
         FROM market.candles
         WHERE symbol = ? AND timeframe_secs = ?
         ORDER BY timestamp_ms ASC",
    )?;

    let mut buf = CandleBuffer::with_capacity(capacity);
    let mut rows = stmt.query(params![&key.symbol, tf_secs])?;
    while let Some(row) = rows.next()? {
        buf.push(
            row.get(0)?,
            row.get(1)?,
            row.get(2)?,
            row.get(3)?,
            row.get(4)?,
            row.get(5)?,
        );
    }
    Ok(buf)
}

/// Query candles within a time range [start_ts, end_ts] inclusive.
pub fn query_range(
    conn: &Connection,
    key: &DataKey,
    start_ts: i64,
    end_ts: i64,
) -> Result<CandleBuffer, StoreError> {
    let tf_secs = timeframe_to_i32(key.timeframe)?;

    let mut stmt = conn.prepare_cached(
        "SELECT timestamp_ms, open, high, low, close, volume
         FROM market.candles
         WHERE symbol = ? AND timeframe_secs = ?
           AND timestamp_ms >= ? AND timestamp_ms <= ?
         ORDER BY timestamp_ms ASC",
    )?;

    let mut buf = CandleBuffer::with_capacity(5000);
    let mut rows = stmt.query(params![&key.symbol, tf_secs, start_ts, end_ts])?;
    while let Some(row) = rows.next()? {
        buf.push(
            row.get(0)?,
            row.get(1)?,
            row.get(2)?,
            row.get(3)?,
            row.get(4)?,
            row.get(5)?,
        );
    }
    Ok(buf)
}

/// List all cached symbol/timeframe pairs with metadata.
pub fn list_cached(conn: &Connection) -> Result<Vec<CacheInfo>, StoreError> {
    let mut stmt = conn.prepare_cached(
        "SELECT symbol, timeframe_secs, candle_count, first_ts, last_ts, source
         FROM meta.data_ranges
         ORDER BY symbol, timeframe_secs",
    )?;

    let mut result = Vec::new();
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let tf_secs: i32 = row.get(1)?;
        match u32::try_from(tf_secs) {
            Ok(tf_u32) => {
                if let Some(timeframe) = timeframe_from_secs(tf_u32) {
                    result.push(CacheInfo {
                        key: DataKey {
                            symbol: row.get(0)?,
                            timeframe,
                        },
                        candle_count: row.get::<_, i32>(2)? as usize,
                        first_ts: row.get(3)?,
                        last_ts: row.get(4)?,
                        source: row.get(5)?,
                    });
                } else {
                    tracing::warn!(timeframe_secs = tf_secs, "skipping unknown timeframe");
                }
            }
            Err(_) => {
                tracing::warn!(
                    timeframe_secs = tf_secs,
                    "skipping negative timeframe_secs (data corruption?)"
                );
            }
        }
    }
    Ok(result)
}

/// Delete all data for a given symbol/timeframe (transactional).
#[allow(dead_code)] // Used in tests; will be wired to UI in future phase.
pub fn delete_symbol(conn: &Connection, key: &DataKey) -> Result<(), StoreError> {
    let tf_secs = timeframe_to_i32(key.timeframe)?;
    conn.execute_batch("BEGIN TRANSACTION")?;
    let result = (|| -> Result<(), StoreError> {
        conn.execute(
            "DELETE FROM market.candles WHERE symbol = ? AND timeframe_secs = ?",
            params![&key.symbol, tf_secs],
        )?;
        conn.execute(
            "DELETE FROM meta.data_ranges WHERE symbol = ? AND timeframe_secs = ?",
            params![&key.symbol, tf_secs],
        )?;
        Ok(())
    })();
    match &result {
        Ok(()) => conn.execute_batch("COMMIT")?,
        Err(_) => {
            let _ = conn.execute_batch("ROLLBACK");
        }
    }
    result
}

/// Reconcile `meta.data_ranges` from actual `market.candles` data.
///
/// Only runs a full reconciliation if metadata is out of sync (different
/// number of distinct symbol/timeframe pairs). This avoids a full table
/// scan on every startup for well-maintained databases.
pub fn reconcile_data_ranges(conn: &Connection) -> Result<(), StoreError> {
    // Quick check: does metadata count match candle data distinct keys?
    let meta_count: i64 = conn.query_row("SELECT COUNT(*) FROM meta.data_ranges", [], |row| {
        row.get(0)
    })?;
    let candle_keys: i64 = conn.query_row(
        "SELECT COUNT(DISTINCT (symbol, timeframe_secs)) FROM market.candles",
        [],
        |row| row.get(0),
    )?;

    if meta_count == candle_keys && candle_keys > 0 {
        tracing::debug!("data_ranges in sync ({meta_count} entries), skipping reconciliation");
        return Ok(());
    }

    if candle_keys == 0 && meta_count == 0 {
        return Ok(()); // Empty database, nothing to reconcile.
    }

    tracing::info!(meta_count, candle_keys, "reconciling data_ranges metadata");

    // Full reconciliation: query aggregates then rewrite metadata row-by-row.
    // (DuckDB has a debug-mode assertion bug in INSERT INTO ... SELECT
    //  with string literals in the SELECT list.)
    let mut stmt = conn.prepare(
        "SELECT symbol, timeframe_secs, COUNT(*) as cnt,
                MIN(timestamp_ms) as first_ts, MAX(timestamp_ms) as last_ts
         FROM market.candles
         GROUP BY symbol, timeframe_secs",
    )?;

    let mut rows_data: Vec<(String, i32, i64, i64, i64)> = Vec::new();
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        rows_data.push((
            row.get(0)?,
            row.get(1)?,
            row.get(2)?,
            row.get(3)?,
            row.get(4)?,
        ));
    }
    drop(rows);
    drop(stmt);

    conn.execute_batch("BEGIN TRANSACTION")?;
    let result = (|| -> Result<(), StoreError> {
        conn.execute("DELETE FROM meta.data_ranges", [])?;
        let mut insert = conn.prepare(
            "INSERT INTO meta.data_ranges
                 (symbol, timeframe_secs, candle_count, first_ts, last_ts, source)
             VALUES (?, ?, ?, ?, ?, 'reconciled')",
        )?;
        for (symbol, tf_secs, count, first_ts, last_ts) in &rows_data {
            insert.execute(params![symbol, tf_secs, count, first_ts, last_ts])?;
        }
        Ok(())
    })();
    match &result {
        Ok(()) => conn.execute_batch("COMMIT")?,
        Err(_) => {
            let _ = conn.execute_batch("ROLLBACK");
        }
    }
    result
}

#[cfg(test)]
mod tests;
