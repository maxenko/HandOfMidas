//! SQLite database layer for midas-broker.
//!
//! Provides [`BrokerDb`] which manages a single SQLite connection with WAL mode,
//! applies embedded migrations on open, and exposes the connection for use by
//! repository functions.

use std::path::Path;
use std::sync::{Arc, Mutex};
use thiserror::Error;

/// Errors produced by the database layer.
#[derive(Debug, Error)]
pub enum BrokerDbError {
    #[error("rusqlite error: {0}")]
    Rusqlite(#[from] rusqlite::Error),

    #[error("migration error: {0}")]
    Migration(String),
}

/// Embedded migration scripts, applied in order.
/// Each entry is `(version, sql)` where version starts at 1.
const MIGRATIONS: &[(u32, &str)] = &[
    (1, include_str!("../migrations/001_initial.sql")),
];

/// Handle to the broker SQLite database.
///
/// The connection is wrapped in `Arc<Mutex<..>>` so it can be shared across
/// sync repository calls (typically invoked via `spawn_blocking`).
pub struct BrokerDb {
    conn: Arc<Mutex<rusqlite::Connection>>,
}

impl BrokerDb {
    /// Open (or create) a database file at `path`, apply pragmas and
    /// run any pending migrations.
    pub fn open(path: &Path) -> Result<Self, BrokerDbError> {
        let conn = rusqlite::Connection::open(path)?;
        let db = Self {
            conn: Arc::new(Mutex::new(conn)),
        };
        db.apply_pragmas()?;
        db.run_migrations()?;
        Ok(db)
    }

    /// Open an in-memory database. Useful for tests.
    pub fn open_in_memory() -> Result<Self, BrokerDbError> {
        let conn = rusqlite::Connection::open_in_memory()?;
        let db = Self {
            conn: Arc::new(Mutex::new(conn)),
        };
        db.apply_pragmas()?;
        db.run_migrations()?;
        Ok(db)
    }

    /// Return a clone of the `Arc<Mutex<Connection>>` for use by repository
    /// functions.
    pub fn conn(&self) -> &Arc<Mutex<rusqlite::Connection>> {
        &self.conn
    }

    // ── private helpers ──────────────────────────────────────────────

    fn apply_pragmas(&self) -> Result<(), BrokerDbError> {
        let conn = self.conn.lock().expect("db mutex poisoned");
        // WAL mode for concurrent readers.
        conn.pragma_update(None, "journal_mode", "WAL")?;
        // NORMAL sync is safe with WAL and much faster than FULL.
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        // ~8 MB page cache.
        conn.pragma_update(None, "cache_size", -8000)?;
        // Enforce foreign keys.
        conn.pragma_update(None, "foreign_keys", "ON")?;
        // Wait up to 5 s when the DB is locked by another writer.
        conn.pragma_update(None, "busy_timeout", 5000)?;
        Ok(())
    }

    fn run_migrations(&self) -> Result<(), BrokerDbError> {
        let conn = self.conn.lock().expect("db mutex poisoned");

        let current_version: u32 =
            conn.pragma_query_value(None, "user_version", |row| row.get(0))?;

        for &(version, sql) in MIGRATIONS {
            if version <= current_version {
                continue;
            }

            // Use unchecked_transaction so we can embed PRAGMA inside.
            let tx = conn.unchecked_transaction().map_err(BrokerDbError::Rusqlite)?;

            tx.execute_batch(sql).map_err(|e| {
                BrokerDbError::Migration(format!(
                    "migration v{version} failed: {e}"
                ))
            })?;

            tx.pragma_update(None, "user_version", version)
                .map_err(|e| {
                    BrokerDbError::Migration(format!(
                        "failed to update user_version to {version}: {e}"
                    ))
                })?;

            tx.commit().map_err(|e| {
                BrokerDbError::Migration(format!(
                    "failed to commit migration v{version}: {e}"
                ))
            })?;
        }

        Ok(())
    }
}

// ── tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_migrations_create_all_tables() {
        let db = BrokerDb::open_in_memory().expect("open in-memory db");
        let conn = db.conn().lock().expect("lock");

        let expected_tables = [
            "orders",
            "order_audit",
            "fills",
            "positions",
            "account_values",
            "contracts",
        ];

        for table in &expected_tables {
            let exists: bool = conn
                .prepare(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                )
                .unwrap()
                .query_row(rusqlite::params![table], |row| row.get::<_, i64>(0))
                .map(|c| c == 1)
                .unwrap();

            assert!(exists, "table '{table}' should exist after migration");
        }

        // Verify user_version was bumped.
        let version: u32 = conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, 1);
    }

    #[test]
    fn test_idempotent_migration() {
        // Running open_in_memory twice on the same connection is not possible,
        // but we can verify that calling run_migrations a second time is a no-op
        // by opening a file-based DB, closing it, and re-opening.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.db");

        {
            let _db = BrokerDb::open(&path).expect("first open");
        }
        {
            let db = BrokerDb::open(&path).expect("second open");
            let conn = db.conn().lock().unwrap();
            let version: u32 = conn
                .pragma_query_value(None, "user_version", |row| row.get(0))
                .unwrap();
            assert_eq!(version, 1);
        }
    }
}
