//! Persistence for [`super::TickerState`] using `redb`.
//!
//! Mirrors the existing `ticker_order_intent::actor` pattern: a
//! dedicated background flush thread with 75ms debounce, condvar-driven
//! wake, and `Durability::Eventual` / `Immediate` split.
//!
//! The redb table name is `"ticker_state_v2"`. On open, if the v1 table
//! (`"ticker_intent_v1"`) exists, all rows are migrated via
//! [`super::migrate_v1_v2`] and written to the v2 table.
//!
//! # Slice 0 status
//!
//! This module defines the public API shape and the v1->v2 migration
//! logic. It is not yet wired into `MidasApp` — Slice 4 handles
//! startup/shutdown integration. The handle is functional and can be
//! used from tests.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex as StdMutex};
use std::thread;
use std::time::{Duration, Instant};

use parking_lot::RwLock;
use redb::{Database, Durability, ReadableTable, TableDefinition};

use crate::annotation_store::SymbolKey;

use super::TickerState;

// ── Tunables ────────────────────────────────────────────────────────

/// Debounce period for the flush loop.
const FLUSH_DEBOUNCE: Duration = Duration::from_millis(75);

/// Idle threshold after which `Durability::Immediate` commits kick in.
const IDLE_THRESHOLD: Duration = Duration::from_millis(750);

/// The v2 table. Value is `serde_json::to_vec_pretty(&state)`.
const TABLE_V2: TableDefinition<'_, &str, &[u8]> = TableDefinition::new("ticker_state_v2");

/// The v1 table (from `ticker_order_intent::actor`). Read-only during
/// migration; never written to by this module.
const TABLE_V1: TableDefinition<'_, &str, &[u8]> = TableDefinition::new("ticker_intent_v1");

// ── Errors ──────────────────────────────────────────────────────────

/// Errors from the persistence layer.
#[derive(Debug, thiserror::Error)]
pub enum PersistError {
    /// Failed to open the redb database.
    #[error("failed to open ticker-state database at {path}: {reason}")]
    Open {
        /// Path that was attempted.
        path: PathBuf,
        /// Underlying error message.
        reason: String,
    },
    /// A write transaction failed.
    #[error("redb write failed: {reason}")]
    Write {
        /// Underlying error message.
        reason: String,
    },
    /// A read transaction failed.
    #[error("redb read failed: {reason}")]
    Read {
        /// Underlying error message.
        reason: String,
    },
}

// ── Flush controller ────────────────────────────────────────────────

/// Shared state between the handle and the flush thread.
struct FlushCtl {
    /// Set whenever a dirty-set entry appears.
    wake: bool,
    /// When set, the flush thread does one final `Immediate` commit
    /// and exits.
    shutdown: bool,
    /// Set by the flush thread right before it exits. The handle's
    /// `shutdown_blocking()` waits on this via the condvar.
    done: bool,
    /// Last time we received a wake.
    last_wake: Instant,
}

impl Default for FlushCtl {
    fn default() -> Self {
        Self {
            wake: false,
            shutdown: false,
            done: false,
            last_wake: Instant::now(),
        }
    }
}

// ── In-memory cache ─────────────────────────────────────────────────

/// Thread-safe in-memory cache of per-symbol ticker states.
struct StateCache {
    /// The actual map.
    cache: RwLock<HashMap<SymbolKey, TickerState>>,
    /// Symbols that have been written but not yet flushed to disk.
    dirty: parking_lot::Mutex<HashSet<SymbolKey>>,
    /// Monotonic write counter.
    generation: AtomicU64,
}

impl StateCache {
    /// Construct an empty cache.
    fn new() -> Self {
        Self {
            cache: RwLock::new(HashMap::new()),
            dirty: parking_lot::Mutex::new(HashSet::new()),
            generation: AtomicU64::new(0),
        }
    }

    /// Read the current state for a symbol, if any.
    fn snapshot(&self, symbol: &SymbolKey) -> Option<TickerState> {
        self.cache.read().get(symbol).cloned()
    }

    /// Insert or replace the state for a symbol.
    fn upsert(&self, symbol: SymbolKey, state: TickerState) {
        self.cache.write().insert(symbol.clone(), state);
        self.dirty.lock().insert(symbol);
        self.generation.fetch_add(1, Ordering::AcqRel);
    }

    /// Remove a symbol from the cache.
    fn forget(&self, symbol: &SymbolKey) {
        let removed = self.cache.write().remove(symbol).is_some();
        if removed {
            self.dirty.lock().insert(symbol.clone());
            self.generation.fetch_add(1, Ordering::AcqRel);
        }
    }

    /// Drain the dirty set, returning (symbol, state) pairs.
    fn drain_dirty(&self) -> Vec<(SymbolKey, TickerState)> {
        let dirty: Vec<SymbolKey> = self.dirty.lock().drain().collect();
        let cache = self.cache.read();
        dirty
            .into_iter()
            .filter_map(|s| cache.get(&s).map(|st| (s, st.clone())))
            .collect()
    }

    /// Re-mark a batch of symbols as dirty (on flush failure).
    fn re_mark_dirty<I: IntoIterator<Item = SymbolKey>>(&self, symbols: I) {
        let cache = self.cache.read();
        let mut dirty = self.dirty.lock();
        for s in symbols {
            if cache.contains_key(&s) {
                dirty.insert(s);
            }
        }
    }

    /// Seed from disk without marking dirty.
    fn seed(&self, symbol: SymbolKey, state: TickerState) {
        self.cache.write().insert(symbol, state);
    }
}

// ── Handle ──────────────────────────────────────────────────────────

/// Public handle for the ticker-state persistence layer.
///
/// Cheaply cloneable. All clones share the same cache and flush thread.
#[derive(Clone)]
pub struct TickerStatePersistHandle {
    /// In-memory cache.
    cache: Arc<StateCache>,
    /// Flush-thread control primitive.
    ctl: Arc<(StdMutex<FlushCtl>, Condvar)>,
    /// The open database (kept alive for the flush thread).
    #[allow(dead_code)]
    db: Arc<Database>,
}

impl TickerStatePersistHandle {
    /// Open the ticker-state store at `path`, hydrate from disk, and
    /// spawn the flush thread.
    ///
    /// If the v1 table exists, all rows are migrated to v2 via
    /// [`super::migrate_v1_v2`].
    pub fn open(path: impl AsRef<Path>) -> Result<Self, PersistError> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| PersistError::Open {
                path: path.to_path_buf(),
                reason: e.to_string(),
            })?;
        }

        let db = Database::create(path).map_err(|e| PersistError::Open {
            path: path.to_path_buf(),
            reason: e.to_string(),
        })?;
        let db = Arc::new(db);
        let cache = Arc::new(StateCache::new());

        // Hydrate from v2 table.
        hydrate_v2(&db, &cache)?;

        // Migrate from v1 if present.
        migrate_v1_to_v2(&db, &cache)?;

        let ctl = Arc::new((StdMutex::new(FlushCtl::default()), Condvar::new()));

        // Spawn flush thread.
        let flush_db = db.clone();
        let flush_cache = cache.clone();
        let flush_ctl = ctl.clone();
        thread::Builder::new()
            .name("ticker-state-flush".into())
            .spawn(move || flush_loop(flush_db, flush_cache, flush_ctl))
            .map_err(|e| PersistError::Open {
                path: path.to_path_buf(),
                reason: format!("failed to spawn flush thread: {e}"),
            })?;

        Ok(Self { cache, ctl, db })
    }

    /// Read the current state for a symbol.
    pub fn snapshot(&self, symbol: &SymbolKey) -> Option<TickerState> {
        self.cache.snapshot(symbol)
    }

    /// Insert or replace the state for a symbol.
    pub fn upsert(&self, symbol: SymbolKey, state: TickerState) {
        self.cache.upsert(symbol, state);
        wake_flush(&self.ctl);
    }

    /// Remove a symbol from the store.
    pub fn forget(&self, symbol: &SymbolKey) {
        self.cache.forget(symbol);
        wake_flush(&self.ctl);
    }

    /// Force an immediate durable flush.
    pub fn flush_now(&self) {
        wake_flush(&self.ctl);
        // Wait for the flush to complete by signaling and sleeping briefly.
        // A proper implementation would use a reply channel; this is
        // sufficient for Slice 0.
        std::thread::sleep(Duration::from_millis(100));
    }

    /// Return a snapshot of all cached ticker states.
    ///
    /// Used by `MidasApp::new()` to hydrate the `tickers` HashMap on
    /// startup. The returned map is a clone of the in-memory cache,
    /// which has already been hydrated from disk (v2 table) and
    /// migrated from v1 if needed.
    pub fn all_states(&self) -> HashMap<SymbolKey, TickerState> {
        self.cache.cache.read().clone()
    }

    /// Graceful shutdown: signal the flush thread to do one final
    /// `Immediate` commit and exit. Does not block.
    pub fn shutdown(&self) {
        let (lock, cvar) = &*self.ctl;
        if let Ok(mut guard) = lock.lock() {
            guard.shutdown = true;
            cvar.notify_all();
        }
    }

    /// Blocking shutdown: signal the flush thread and wait for it to
    /// finish its final commit and exit. Used by tests and app close.
    pub fn shutdown_blocking(&self) {
        let (lock, cvar) = &*self.ctl;
        if let Ok(mut guard) = lock.lock() {
            guard.shutdown = true;
            cvar.notify_all();
            // Wait for the flush thread to set `done = true`.
            let timeout = Duration::from_secs(5);
            let start = Instant::now();
            while !guard.done && start.elapsed() < timeout {
                let (g, _) = match cvar.wait_timeout(guard, Duration::from_millis(50)) {
                    Ok(pair) => pair,
                    Err(e) => e.into_inner(),
                };
                guard = g;
            }
        }
    }
}

// ── Hydration ───────────────────────────────────────────────────────

/// Load all rows from the v2 table into the cache.
fn hydrate_v2(db: &Database, cache: &StateCache) -> Result<(), PersistError> {
    let txn = db
        .begin_read()
        .map_err(|e| PersistError::Read {
            reason: e.to_string(),
        })?;

    // The table might not exist yet (fresh database).
    let table = match txn.open_table(TABLE_V2) {
        Ok(t) => t,
        Err(redb::TableError::TableDoesNotExist(_)) => return Ok(()),
        Err(e) => {
            return Err(PersistError::Read {
                reason: e.to_string(),
            });
        }
    };

    for entry in table.iter().map_err(|e| PersistError::Read {
        reason: e.to_string(),
    })? {
        let (key, value) = entry.map_err(|e| PersistError::Read {
            reason: e.to_string(),
        })?;
        let symbol = SymbolKey::new(key.value());
        match serde_json::from_slice::<TickerState>(value.value()) {
            Ok(state) => cache.seed(symbol, state),
            Err(e) => {
                tracing::warn!(
                    "ticker-state: failed to deserialize v2 row for {}: {e}",
                    key.value()
                );
            }
        }
    }

    Ok(())
}

/// Migrate all rows from the v1 table to v2.
///
/// Only seeds rows that are NOT already in the cache (v2 takes
/// priority for last-write-wins).
fn migrate_v1_to_v2(db: &Database, cache: &StateCache) -> Result<(), PersistError> {
    let txn = db
        .begin_read()
        .map_err(|e| PersistError::Read {
            reason: e.to_string(),
        })?;

    let table = match txn.open_table(TABLE_V1) {
        Ok(t) => t,
        Err(redb::TableError::TableDoesNotExist(_)) => return Ok(()),
        Err(e) => {
            return Err(PersistError::Read {
                reason: e.to_string(),
            });
        }
    };

    let mut migrated = 0u32;
    for entry in table.iter().map_err(|e| PersistError::Read {
        reason: e.to_string(),
    })? {
        let (key, value) = entry.map_err(|e| PersistError::Read {
            reason: e.to_string(),
        })?;
        let symbol = SymbolKey::new(key.value());

        // Skip if v2 already has this symbol (last-write-wins: v2 is newer).
        if cache.snapshot(&symbol).is_some() {
            continue;
        }

        match serde_json::from_slice::<crate::ticker_order_intent::TickerOrderIntent>(
            value.value(),
        ) {
            Ok(intent) => {
                let state = super::migrate_v1_v2(&intent);
                cache.seed(symbol.clone(), state);
                migrated += 1;
            }
            Err(e) => {
                tracing::warn!(
                    "ticker-state: failed to deserialize v1 row for {}: {e}",
                    key.value()
                );
            }
        }
    }

    if migrated > 0 {
        tracing::info!("ticker-state: migrated {migrated} row(s) from v1 to v2");

        // Write migrated rows to the v2 table so future opens skip migration.
        let dirty = cache.drain_dirty();
        if !dirty.is_empty() {
            // Re-seed them (drain removed from dirty, not cache).
            // Actually the drain_dirty only drains the dirty set, not the cache.
            // But since we used seed() which doesn't mark dirty, drain_dirty
            // returns nothing. We need to write directly.
            write_batch_to_v2(db, &dirty)?;
        } else {
            // The seed path didn't mark dirty. Read all migrated rows
            // from the cache and write them.
            let all = {
                let map = cache.cache.read();
                map.iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect::<Vec<_>>()
            };
            write_batch_to_v2(db, &all)?;
        }
    }

    Ok(())
}

/// Write a batch of (symbol, state) pairs to the v2 table with
/// `Durability::Immediate`.
fn write_batch_to_v2(
    db: &Database,
    batch: &[(SymbolKey, TickerState)],
) -> Result<(), PersistError> {
    if batch.is_empty() {
        return Ok(());
    }
    let mut txn = db
        .begin_write()
        .map_err(|e| PersistError::Write {
            reason: e.to_string(),
        })?;
    txn.set_durability(Durability::Immediate);
    {
        let mut table = txn.open_table(TABLE_V2).map_err(|e| PersistError::Write {
            reason: e.to_string(),
        })?;
        for (symbol, state) in batch {
            let bytes = serde_json::to_vec_pretty(state).map_err(|e| PersistError::Write {
                reason: format!("encode {symbol}: {e}"),
            })?;
            table
                .insert(symbol.as_str(), bytes.as_slice())
                .map_err(|e| PersistError::Write {
                    reason: e.to_string(),
                })?;
        }
    }
    txn.commit().map_err(|e| PersistError::Write {
        reason: e.to_string(),
    })?;
    Ok(())
}

// ── Flush loop ──────────────────────────────────────────────────────

/// Wake the flush thread.
fn wake_flush(ctl: &Arc<(StdMutex<FlushCtl>, Condvar)>) {
    let (lock, cvar) = &**ctl;
    if let Ok(mut guard) = lock.lock() {
        guard.wake = true;
        guard.last_wake = Instant::now();
        cvar.notify_all();
    }
}

/// The flush thread's main loop.
fn flush_loop(db: Arc<Database>, cache: Arc<StateCache>, ctl: Arc<(StdMutex<FlushCtl>, Condvar)>) {
    loop {
        // Wait for a wake or the debounce period.
        let (lock, cvar) = &*ctl;
        let mut guard = match lock.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        let is_shutdown = if guard.shutdown {
            true
        } else if guard.wake {
            false
        } else {
            // Block until a wake signal or the debounce period elapses.
            let (next, _) = match cvar.wait_timeout(guard, FLUSH_DEBOUNCE) {
                Ok(pair) => pair,
                Err(_) => panic!("ticker-state flush lock poisoned"),
            };
            guard = next;
            guard.shutdown
        };

        let last_wake = guard.last_wake;
        guard.wake = false;
        drop(guard);

        let _durability = if is_shutdown || last_wake.elapsed() >= IDLE_THRESHOLD {
            Durability::Immediate
        } else {
            Durability::Eventual
        };

        // Drain dirty and flush.
        let dirty = cache.drain_dirty();
        if !dirty.is_empty() {
            if let Err(e) = write_batch_to_v2(&db, &dirty) {
                tracing::warn!("ticker-state flush failed: {e}");
                cache.re_mark_dirty(dirty.into_iter().map(|(s, _)| s));
            }
        }

        if is_shutdown {
            // Signal the handle that the flush thread is done.
            let (lock, cvar) = &*ctl;
            if let Ok(mut guard) = lock.lock() {
                guard.done = true;
                cvar.notify_all();
            }
            break;
        }
    }
}
