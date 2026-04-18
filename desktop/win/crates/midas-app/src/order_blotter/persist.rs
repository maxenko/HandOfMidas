//! redb persistence for [`super::OrderRow`]s, keyed by broker-assigned
//! order UUID.
//!
//! Mirrors the structure of `ticker_state/persist.rs`:
//! - Native flush thread, 75ms debounce.
//! - `parking_lot::Mutex` dirty set, `Condvar`-driven wake.
//! - `Durability::Eventual` during bursts; escalated to `Immediate`
//!   after 750ms of quiet or on shutdown.
//! - `shutdown_blocking()` waits up to 5s for the final commit.
//!
//! Table schema: `order_history_v1` with `&[u8]` key (16 raw uuid bytes,
//! little-endian) and `&[u8]` value (pretty-printed JSON `OrderRow`).
//!
//! Retention: unbounded v1 (plan's "Retention policy for v1"). Future
//! growth: prune by age, emit `tracing::warn!` at 10k rows.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex as StdMutex};
use std::thread;
use std::time::{Duration, Instant};

use parking_lot::RwLock;
use redb::{Database, Durability, ReadableTable, TableDefinition};
use uuid::Uuid;

use super::OrderRow;

// ── Tunables ────────────────────────────────────────────────────────

const FLUSH_DEBOUNCE: Duration = Duration::from_millis(75);
const IDLE_THRESHOLD: Duration = Duration::from_millis(750);

/// redb value: pretty-printed JSON `OrderRow`.
const TABLE_V1: TableDefinition<'_, &[u8], &[u8]> = TableDefinition::new("order_history_v1");

/// Threshold above which retention is loud in logs. Plan-mandated.
const RETENTION_WARN_THRESHOLD: usize = 10_000;

// ── Errors ──────────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum PersistError {
    #[error("failed to open order-history database at {path}: {reason}")]
    Open { path: PathBuf, reason: String },
    #[error("redb write failed: {reason}")]
    Write { reason: String },
    #[error("redb read failed: {reason}")]
    Read { reason: String },
}

// ── Flush controller ────────────────────────────────────────────────

struct FlushCtl {
    wake: bool,
    shutdown: bool,
    done: bool,
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

struct RowCache {
    rows: RwLock<HashMap<Uuid, OrderRow>>,
    dirty: parking_lot::Mutex<HashSet<Uuid>>,
    generation: AtomicU64,
    /// Set `true` after the retention threshold has been logged once,
    /// so the warning doesn't spam every append.
    warned_retention: parking_lot::Mutex<bool>,
}

impl RowCache {
    fn new() -> Self {
        Self {
            rows: RwLock::new(HashMap::new()),
            dirty: parking_lot::Mutex::new(HashSet::new()),
            generation: AtomicU64::new(0),
            warned_retention: parking_lot::Mutex::new(false),
        }
    }

    fn upsert(&self, id: Uuid, row: OrderRow) {
        let mut map = self.rows.write();
        map.insert(id, row);
        let len = map.len();
        drop(map);
        self.dirty.lock().insert(id);
        self.generation.fetch_add(1, Ordering::AcqRel);
        if len >= RETENTION_WARN_THRESHOLD {
            let mut warned = self.warned_retention.lock();
            if !*warned {
                tracing::warn!(
                    "order history at {len} rows — over {RETENTION_WARN_THRESHOLD} \
                     retention threshold; future growth: prune old rows"
                );
                *warned = true;
            }
        }
    }

    fn drain_dirty(&self) -> Vec<(Uuid, OrderRow)> {
        let dirty: Vec<Uuid> = self.dirty.lock().drain().collect();
        let rows = self.rows.read();
        dirty
            .into_iter()
            .filter_map(|id| rows.get(&id).map(|r| (id, r.clone())))
            .collect()
    }

    fn re_mark_dirty<I: IntoIterator<Item = Uuid>>(&self, ids: I) {
        let rows = self.rows.read();
        let mut dirty = self.dirty.lock();
        for id in ids {
            if rows.contains_key(&id) {
                dirty.insert(id);
            }
        }
    }

    fn seed(&self, id: Uuid, row: OrderRow) {
        self.rows.write().insert(id, row);
    }

    fn all_rows(&self) -> Vec<OrderRow> {
        self.rows.read().values().cloned().collect()
    }
}

// ── Handle ──────────────────────────────────────────────────────────

/// Public handle for the order-history persistence layer. Cheaply
/// cloneable; all clones share the cache and flush thread.
#[derive(Clone)]
pub struct OrderHistoryPersistHandle {
    cache: Arc<RowCache>,
    ctl: Arc<(StdMutex<FlushCtl>, Condvar)>,
    #[allow(dead_code)]
    db: Arc<Database>,
}

impl OrderHistoryPersistHandle {
    /// Open the order-history store at `path`, hydrate from disk, and
    /// spawn the flush thread.
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
        let cache = Arc::new(RowCache::new());

        hydrate(&db, &cache)?;

        let ctl = Arc::new((StdMutex::new(FlushCtl::default()), Condvar::new()));

        let flush_db = db.clone();
        let flush_cache = cache.clone();
        let flush_ctl = ctl.clone();
        thread::Builder::new()
            .name("order-history-flush".into())
            .spawn(move || flush_loop(flush_db, flush_cache, flush_ctl))
            .map_err(|e| PersistError::Open {
                path: path.to_path_buf(),
                reason: format!("failed to spawn flush thread: {e}"),
            })?;

        Ok(Self { cache, ctl, db })
    }

    /// Insert or replace a row and wake the flush thread.
    pub fn upsert(&self, id: Uuid, row: OrderRow) {
        self.cache.upsert(id, row);
        wake_flush(&self.ctl);
    }

    /// Return every row currently in the cache. Used by `MidasApp::new`
    /// to hydrate the live blotter on startup.
    pub fn all_rows(&self) -> Vec<OrderRow> {
        self.cache.all_rows()
    }

    /// Graceful shutdown without blocking.
    #[allow(dead_code)]
    pub fn shutdown(&self) {
        let (lock, cvar) = &*self.ctl;
        if let Ok(mut guard) = lock.lock() {
            guard.shutdown = true;
            cvar.notify_all();
        }
    }

    /// Blocking shutdown: signal the flush thread and wait up to 5s.
    pub fn shutdown_blocking(&self) {
        let (lock, cvar) = &*self.ctl;
        if let Ok(mut guard) = lock.lock() {
            guard.shutdown = true;
            cvar.notify_all();
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

fn hydrate(db: &Database, cache: &RowCache) -> Result<(), PersistError> {
    let txn = db.begin_read().map_err(|e| PersistError::Read {
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

    let mut loaded = 0u32;
    for entry in table.iter().map_err(|e| PersistError::Read {
        reason: e.to_string(),
    })? {
        let (key, value) = entry.map_err(|e| PersistError::Read {
            reason: e.to_string(),
        })?;
        let key_bytes = key.value();
        if key_bytes.len() != 16 {
            tracing::warn!(
                "order-history: skipping row with non-16-byte key (len={})",
                key_bytes.len()
            );
            continue;
        }
        let mut bytes = [0u8; 16];
        bytes.copy_from_slice(key_bytes);
        let id = Uuid::from_bytes(bytes);
        match serde_json::from_slice::<OrderRow>(value.value()) {
            Ok(row) => {
                cache.seed(id, row);
                loaded += 1;
            }
            Err(e) => {
                tracing::warn!("order-history: failed to deserialise row {id}: {e}");
            }
        }
    }

    if loaded > 0 {
        tracing::info!("order-history: hydrated {loaded} rows from redb");
    }

    Ok(())
}

// ── Batch write ─────────────────────────────────────────────────────

fn write_batch(db: &Database, batch: &[(Uuid, OrderRow)]) -> Result<(), PersistError> {
    if batch.is_empty() {
        return Ok(());
    }
    let mut txn = db.begin_write().map_err(|e| PersistError::Write {
        reason: e.to_string(),
    })?;
    txn.set_durability(Durability::Immediate);
    {
        let mut table = txn.open_table(TABLE_V1).map_err(|e| PersistError::Write {
            reason: e.to_string(),
        })?;
        for (id, row) in batch {
            let bytes = serde_json::to_vec_pretty(row).map_err(|e| PersistError::Write {
                reason: format!("encode row {id}: {e}"),
            })?;
            table
                .insert(id.as_bytes().as_slice(), bytes.as_slice())
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

fn wake_flush(ctl: &Arc<(StdMutex<FlushCtl>, Condvar)>) {
    let (lock, cvar) = &**ctl;
    if let Ok(mut guard) = lock.lock() {
        guard.wake = true;
        guard.last_wake = Instant::now();
        cvar.notify_all();
    }
}

fn flush_loop(db: Arc<Database>, cache: Arc<RowCache>, ctl: Arc<(StdMutex<FlushCtl>, Condvar)>) {
    loop {
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
            let (next, _) = match cvar.wait_timeout(guard, FLUSH_DEBOUNCE) {
                Ok(pair) => pair,
                Err(_) => panic!("order-history flush lock poisoned"),
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

        let dirty = cache.drain_dirty();
        if !dirty.is_empty() {
            if let Err(e) = write_batch(&db, &dirty) {
                tracing::warn!("order-history flush failed: {e}");
                cache.re_mark_dirty(dirty.into_iter().map(|(id, _)| id));
            }
        }

        if is_shutdown {
            let (lock, cvar) = &*ctl;
            if let Ok(mut guard) = lock.lock() {
                guard.done = true;
                cvar.notify_all();
            }
            break;
        }
    }
}
