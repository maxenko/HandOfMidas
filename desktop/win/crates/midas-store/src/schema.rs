use duckdb::Connection;

use crate::error::StoreError;

const CURRENT_VERSION: i32 = 1;

/// Apply all pending schema migrations. Idempotent — safe to call on every startup.
pub fn run_migrations(conn: &Connection) -> Result<(), StoreError> {
    // Version table lives in default schema, survives DROP SCHEMA on data schemas.
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_version (
             version    INTEGER NOT NULL PRIMARY KEY,
             applied_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
         );",
    )
    .map_err(|e| StoreError::Migration(format!("schema_version table: {e}")))?;

    let current: i32 = conn
        .query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_version",
            [],
            |row| row.get(0),
        )
        .map_err(|e| StoreError::Migration(format!("read version: {e}")))?;

    if current >= CURRENT_VERSION {
        tracing::debug!(current_version = current, "schema is up to date");
        return Ok(());
    }

    tracing::info!(from = current, to = CURRENT_VERSION, "applying schema migrations");

    if current < 1 {
        migrate_v1(conn)?;
    }

    Ok(())
}

fn migrate_v1(conn: &Connection) -> Result<(), StoreError> {
    conn.execute_batch(
        "CREATE SCHEMA IF NOT EXISTS market;
         CREATE SCHEMA IF NOT EXISTS meta;
         CREATE SCHEMA IF NOT EXISTS cache;

         CREATE TABLE IF NOT EXISTS market.candles (
             symbol         VARCHAR    NOT NULL,
             timeframe_secs INTEGER    NOT NULL,
             timestamp_ms   BIGINT     NOT NULL,
             open           FLOAT      NOT NULL,
             high           FLOAT      NOT NULL,
             low            FLOAT      NOT NULL,
             close          FLOAT      NOT NULL,
             volume         UINTEGER   NOT NULL,
             PRIMARY KEY (symbol, timeframe_secs, timestamp_ms)
         );

         CREATE TABLE IF NOT EXISTS meta.data_ranges (
             symbol         VARCHAR    NOT NULL,
             timeframe_secs INTEGER    NOT NULL,
             candle_count   INTEGER    NOT NULL DEFAULT 0,
             first_ts       BIGINT     NOT NULL DEFAULT 0,
             last_ts        BIGINT     NOT NULL DEFAULT 0,
             source         VARCHAR    NOT NULL DEFAULT 'csv',
             updated_at     TIMESTAMP  DEFAULT CURRENT_TIMESTAMP,
             PRIMARY KEY (symbol, timeframe_secs)
         );

         CREATE TABLE IF NOT EXISTS meta.symbols (
             symbol     VARCHAR PRIMARY KEY,
             name       VARCHAR,
             sec_type   VARCHAR NOT NULL DEFAULT 'STK',
             exchange   VARCHAR NOT NULL DEFAULT 'SMART',
             currency   VARCHAR NOT NULL DEFAULT 'USD',
             con_id     INTEGER,
             min_tick   DOUBLE,
             updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
         );

         INSERT INTO schema_version (version) VALUES (1);",
    )
    .map_err(|e| StoreError::Migration(format!("v1: {e}")))?;

    tracing::info!("migration v1 applied");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_conn() -> Connection {
        Connection::open_in_memory().unwrap()
    }

    #[test]
    fn test_migration_idempotent() {
        let conn = test_conn();
        run_migrations(&conn).unwrap();
        run_migrations(&conn).unwrap(); // second call must not error
    }

    #[test]
    fn test_schema_version_tracked() {
        let conn = test_conn();
        run_migrations(&conn).unwrap();
        let version: i32 = conn
            .query_row("SELECT MAX(version) FROM schema_version", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert!(version >= 1);
    }

    #[test]
    fn test_all_tables_exist() {
        let conn = test_conn();
        run_migrations(&conn).unwrap();

        let expected = [
            ("market", "candles"),
            ("meta", "data_ranges"),
            ("meta", "symbols"),
        ];

        for (schema, table) in expected {
            let exists: bool = conn
                .query_row(
                    "SELECT COUNT(*) > 0 FROM information_schema.tables
                     WHERE table_schema = ? AND table_name = ?",
                    [schema, table],
                    |row| row.get(0),
                )
                .unwrap();
            assert!(exists, "table {schema}.{table} should exist");
        }
    }
}
