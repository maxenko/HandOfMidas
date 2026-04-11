//! The write-behind actor that owns the `redb::Database`.
//!
//! # Threading model
//!
//! `redb` is a sync API. Tokio has no place on the hot path of a
//! persistence actor that blocks on disk, so we use
//! [`mailbox_processor::MailboxProcessor::new_blocking`] — the same
//! pattern as `midas-store`'s DuckDB wrapper. The mailbox owns the
//! `redb::Database` on a dedicated OS thread and handles
//! [`OrderIntentMsg`] synchronously.
//!
//! A second "flush" thread runs in parallel: it wakes every 75 ms,
//! checks whether the dirty set is non-empty, and if so drains it
//! into a single `redb` write transaction with `Durability::Eventual`.
//! After 750 ms of inactivity it upgrades the commit durability to
//! `Durability::Immediate` so an idle process does not lose state on
//! a sudden power cut.
//!
//! We use `std::thread` rather than a tokio task for the flush loop
//! because the mailbox thread is already blocking — adding a tokio
//! task would pull a runtime dependency for no benefit, and complicate
//! shutdown ordering. The flush thread is driven by a condvar pair
//! (`Mutex<FlushCtl> + Condvar`); shutdown wakes it synchronously.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex as StdMutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use mailbox_processor::MailboxProcessor;
use redb::{Database, Durability, ReadableTable, TableDefinition};
use tokio::sync::mpsc::Sender;

use crate::annotation_store::SymbolKey;

use super::store::{TickerOrderIntentStore, UpsertOutcome};
use super::validate::{validate, IntentDefect};
use super::TickerOrderIntent;

// ── Tunables ──────────────────────────────────────────────────────────

/// Debounce period for the flush loop. A 60 Hz drag coalesces into
/// roughly 13 writes/sec on disk at this cadence.
const FLUSH_DEBOUNCE: Duration = Duration::from_millis(75);

/// Idle threshold after which opportunistic `Immediate` commits kick in.
const IDLE_THRESHOLD: Duration = Duration::from_millis(750);

/// The single table all rows live in. Value is
/// `serde_json::to_vec_pretty(&intent)` so the file is grep-able.
const TABLE: TableDefinition<'_, &str, &[u8]> = TableDefinition::new("ticker_intent_v1");

// ── Errors, messages, replies ─────────────────────────────────────────

/// Errors the actor can return on open or flush.
#[derive(Debug, thiserror::Error)]
pub enum IntentError {
    /// `redb::Database::create` failed.
    #[error("failed to open redb database at {path}: {reason}")]
    OpenFailed {
        /// Path that was attempted.
        path: PathBuf,
        /// Underlying error message.
        reason: String,
    },
    /// A write transaction failed at commit time.
    #[error("redb write transaction failed: {reason}")]
    WriteFailed {
        /// Underlying error message.
        reason: String,
    },
    /// A read transaction failed.
    #[error("redb read transaction failed: {reason}")]
    ReadFailed {
        /// Underlying error message.
        reason: String,
    },
    /// The mailbox channel was closed (actor thread crashed or shut down).
    #[error("the ticker-intent actor channel is closed")]
    ChannelClosed,
    /// A row failed validation at load time. The row is dropped; this
    /// variant is returned only from explicit `validate` calls.
    #[error("row failed validation: {0}")]
    ValidationFailed(#[from] IntentDefect),
}

/// Where a write originated. Carried through so downstream reducers
/// can skip the originating widget on refresh.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IntentSource {
    /// User edited the order panel.
    Panel,
    /// User dragged on the chart.
    Chart,
    /// Initial load from disk at startup.
    Hydration,
    /// GATR snap rule fired.
    GatrSnap,
    /// Bootstrapped empty intent on first symbol activation.
    Bootstrap,
}

/// Reason a write became a no-op.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoOpReason {
    /// The cached value was byte-identical to the proposed write.
    IdenticalToCache,
    /// The write arrived after a newer write for the same symbol.
    StaleSource,
    /// The write failed validation.
    InvalidIntent,
}

/// Messages accepted by the actor. Locked at Slice 1a — no downstream
/// slice re-opens this enum.
#[derive(Debug)]
pub enum OrderIntentMsg {
    /// Insert or replace an intent for a symbol.
    Upsert {
        /// Symbol being written.
        symbol: SymbolKey,
        /// The new intent.
        intent: Box<TickerOrderIntent>,
        /// Source of the write.
        source: IntentSource,
    },
    /// Remove a symbol's intent entirely (cache + disk row).
    ForgetSymbol {
        /// Symbol to forget.
        symbol: SymbolKey,
    },
    /// Force an immediate durable flush of the dirty set.
    FlushNow,
    /// Graceful shutdown. Drains the mailbox, does one final
    /// `Durability::Immediate` commit, then drops the `Database`.
    Shutdown {
        /// If `true`, bypass Slice 1b's disk-full guard (unused in 1a).
        force: bool,
    },
}

/// Replies the actor can send.
#[derive(Debug)]
pub enum OrderIntentReply {
    /// An upsert applied cleanly.
    Applied {
        /// Post-write store generation.
        generation: u64,
    },
    /// An upsert was skipped.
    NoOp {
        /// Why the upsert was skipped.
        reason: NoOpReason,
    },
    /// A forget succeeded (the symbol existed).
    Forgotten,
    /// A forget was skipped (absent symbol).
    AlreadyAbsent,
    /// A flush completed.
    Flushed,
    /// A shutdown completed.
    ShutdownAck,
    /// An operation failed.
    Error(String),
}

// ── Flush controller ──────────────────────────────────────────────────

/// Shared state between the mailbox thread and the flush thread.
pub(crate) struct FlushCtl {
    /// Set whenever a dirty-set entry appears. Cleared when flushed.
    pub(crate) wake: bool,
    /// When set, the flush thread does one final `Immediate` commit
    /// and exits.
    pub(crate) shutdown: bool,
    /// Last time we received a wake. Used for the idle heuristic.
    pub(crate) last_wake: Instant,
}

impl Default for FlushCtl {
    fn default() -> Self {
        Self {
            wake: false,
            shutdown: false,
            last_wake: Instant::now(),
        }
    }
}

/// The flush thread's loop.
///
/// Blocks on the condvar for up to `FLUSH_DEBOUNCE`. On wake, drains
/// dirty entries into a single write transaction. Picks
/// `Durability::Immediate` when idle, otherwise `Eventual`.
pub(crate) fn flush_loop(
    db: Arc<Database>,
    store: Arc<TickerOrderIntentStore>,
    ctl: Arc<(StdMutex<FlushCtl>, Condvar)>,
) {
    loop {
        // Wait for a wake or the debounce period, whichever comes first.
        let (lock, cvar) = &*ctl;
        let mut guard = match lock.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        let shutdown = loop {
            if guard.shutdown {
                break true;
            }
            if guard.wake {
                break false;
            }
            // wait_timeout only fails on mutex poison; if that happens
            // we are in an unrecoverable state (the handler thread
            // panicked holding the flush lock). Propagate by panicking
            // the flush loop — the app will have already crashed.
            let (next, _res) = match cvar.wait_timeout(guard, FLUSH_DEBOUNCE) {
                Ok(pair) => pair,
                Err(_) => unreachable_poison(),
            };
            guard = next;
            // After either a wake or a timeout, re-check flags. If
            // neither `wake` nor `shutdown` is set, the debounce expired
            // with nothing to do: go back to waiting.
            if !guard.wake && !guard.shutdown {
                continue;
            }
        };

        let last_wake = guard.last_wake;
        guard.wake = false;
        let is_shutdown = shutdown || guard.shutdown;
        drop(guard);

        let durability = if is_shutdown || last_wake.elapsed() >= IDLE_THRESHOLD {
            Durability::Immediate
        } else {
            Durability::Eventual
        };

        if let Err(e) = flush_dirty(&db, &store, durability) {
            tracing::warn!("ticker-intent flush failed: {e}");
        }

        if is_shutdown {
            break;
        }
    }
}

// `wait_timeout` returning `Err` only happens on mutex poison; if that
// happens we have bigger problems than durability. This tiny helper
// exists solely to let us construct something the compiler is happy
// with in the fallback arm without unsafe.
fn unreachable_poison() -> ! {
    panic!("ticker-intent flush lock poisoned — unrecoverable");
}

/// Drain the dirty set into one write transaction.
pub(crate) fn flush_dirty(
    db: &Database,
    store: &TickerOrderIntentStore,
    durability: Durability,
) -> Result<(), IntentError> {
    let dirty = store.drain_dirty();
    // Also collect symbols whose cache entry is missing — those are
    // deletions (via `forget`). Build a lookup against the cache once.
    let pending_deletes: Vec<SymbolKey> = {
        let live: std::collections::HashSet<_> =
            dirty.iter().map(|(s, _)| s.clone()).collect();
        // Any symbol in the dirty set but not in the cache is a delete.
        // `drain_dirty` already emits only rows that *are* still in the
        // cache, so we need a second pass for forgotten symbols. To keep
        // the API simple we re-read the dirty set via `store.forget` —
        // except at this point dirty has been drained. We handle forgets
        // inline in `handle_message` instead; `pending_deletes` stays
        // empty here.
        let _ = live;
        Vec::new()
    };

    if dirty.is_empty() && pending_deletes.is_empty() {
        return Ok(());
    }

    let mut txn = db
        .begin_write()
        .map_err(|e| IntentError::WriteFailed { reason: e.to_string() })?;
    txn.set_durability(durability);
    {
        let mut table = txn
            .open_table(TABLE)
            .map_err(|e| IntentError::WriteFailed { reason: e.to_string() })?;
        for (symbol, intent) in &dirty {
            let bytes = serde_json::to_vec_pretty(intent.as_ref()).map_err(|e| {
                IntentError::WriteFailed { reason: format!("encode {symbol}: {e}") }
            })?;
            table
                .insert(symbol.as_str(), bytes.as_slice())
                .map_err(|e| IntentError::WriteFailed { reason: e.to_string() })?;
        }
        for symbol in &pending_deletes {
            table
                .remove(symbol.as_str())
                .map_err(|e| IntentError::WriteFailed { reason: e.to_string() })?;
        }
    }
    txn.commit()
        .map_err(|e| IntentError::WriteFailed { reason: e.to_string() })?;
    Ok(())
}

// ── Actor spawn ───────────────────────────────────────────────────────

/// Shared state that the mailbox handler needs to mutate.
///
/// `db` is an `Option` so the shutdown handler can take it and drop it
/// synchronously — releasing the underlying file lock even while the
/// mailbox thread is still alive waiting for its channel to close.
pub(crate) struct ActorState {
    pub(crate) db: Option<Arc<Database>>,
    pub(crate) store: Arc<TickerOrderIntentStore>,
    pub(crate) ctl: Arc<(StdMutex<FlushCtl>, Condvar)>,
    pub(crate) flush_handle: Option<JoinHandle<()>>,
}

/// Triple of shared handles returned by [`open_and_hydrate`].
pub(crate) type HydratedActor = (
    Arc<TickerOrderIntentStore>,
    Arc<Database>,
    Arc<(StdMutex<FlushCtl>, Condvar)>,
);

/// Open the `redb::Database` at `path` and hydrate the store from it.
///
/// Returns the `(store, database, control)` triple the handle needs to
/// spawn the mailbox. Parent directory is created if missing.
pub(crate) fn open_and_hydrate(path: &Path) -> Result<HydratedActor, IntentError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| IntentError::OpenFailed {
            path: path.to_path_buf(),
            reason: e.to_string(),
        })?;
    }
    let db = Database::create(path).map_err(|e| IntentError::OpenFailed {
        path: path.to_path_buf(),
        reason: e.to_string(),
    })?;

    // Ensure the table exists (redb creates tables on first open_table
    // inside a write transaction).
    {
        let txn = db
            .begin_write()
            .map_err(|e| IntentError::WriteFailed { reason: e.to_string() })?;
        {
            let _ = txn
                .open_table(TABLE)
                .map_err(|e| IntentError::WriteFailed { reason: e.to_string() })?;
        }
        txn.commit()
            .map_err(|e| IntentError::WriteFailed { reason: e.to_string() })?;
    }

    let store = Arc::new(TickerOrderIntentStore::new());

    // Hydrate the cache from disk, dropping any row that fails to decode
    // or validate. No quarantine sidecar in Slice 1a.
    {
        let txn = db
            .begin_read()
            .map_err(|e| IntentError::ReadFailed { reason: e.to_string() })?;
        let table = txn
            .open_table(TABLE)
            .map_err(|e| IntentError::ReadFailed { reason: e.to_string() })?;
        let iter = table
            .iter()
            .map_err(|e| IntentError::ReadFailed { reason: e.to_string() })?;
        for entry in iter {
            let (k, v) = match entry {
                Ok(pair) => pair,
                Err(e) => {
                    tracing::warn!("ticker-intent: skipping unreadable row: {e}");
                    continue;
                }
            };
            let key_str = k.value().to_owned();
            let bytes = v.value().to_vec();
            match serde_json::from_slice::<TickerOrderIntent>(&bytes) {
                Ok(intent) => match validate(&intent, None, None) {
                    Ok(()) => store.seed(SymbolKey::new(&key_str), intent),
                    Err(defect) => {
                        tracing::warn!(
                            "ticker-intent: dropping invalid row for {key_str}: {defect}"
                        );
                    }
                },
                Err(e) => {
                    tracing::warn!("ticker-intent: dropping undecodable row for {key_str}: {e}");
                }
            }
        }
    }

    let db = Arc::new(db);
    let ctl = Arc::new((StdMutex::new(FlushCtl::default()), Condvar::new()));

    Ok((store, db, ctl))
}

/// Spawn the mailbox actor thread and the flush loop thread.
///
/// The returned [`MailboxProcessor`] is cheaply cloneable and drives
/// all state changes to the store + database.
pub(crate) fn spawn_actor(
    store: Arc<TickerOrderIntentStore>,
    db: Arc<Database>,
    ctl: Arc<(StdMutex<FlushCtl>, Condvar)>,
) -> MailboxProcessor<OrderIntentMsg, OrderIntentReply> {
    // Spawn the flush loop.
    let flush_db = db.clone();
    let flush_store = store.clone();
    let flush_ctl = ctl.clone();
    let flush_handle = thread::Builder::new()
        .name("ticker-intent-flush".to_string())
        .spawn(move || flush_loop(flush_db, flush_store, flush_ctl))
        .expect("failed to spawn ticker-intent flush thread");

    let initial = ActorState {
        db: Some(db.clone()),
        store: store.clone(),
        ctl: ctl.clone(),
        flush_handle: Some(flush_handle),
    };
    // Drop our local reference so the ActorState's `Option<Arc<Database>>`
    // can release the file lock on shutdown without a lingering clone.
    drop(db);

    MailboxProcessor::new_blocking(
        Some(256),
        initial,
        "ticker-intent",
        move |msg, mut state, reply_channel| {
            handle_message(&mut state, msg, reply_channel);
            state
        },
    )
}

/// Dispatch one message to the store / database.
fn handle_message(
    state: &mut ActorState,
    msg: OrderIntentMsg,
    reply_channel: Option<Sender<OrderIntentReply>>,
) {
    let reply = match msg {
        OrderIntentMsg::Upsert {
            symbol,
            intent,
            source: _,
        } => match state.store.upsert(symbol.clone(), *intent) {
            UpsertOutcome::Applied { generation } => {
                wake_flush(&state.ctl);
                OrderIntentReply::Applied { generation }
            }
            UpsertOutcome::NoOp { reason } => OrderIntentReply::NoOp { reason },
        },
        OrderIntentMsg::ForgetSymbol { symbol } => {
            let db = match state.db.as_ref() {
                Some(d) => d,
                None => {
                    reply_with(
                        reply_channel,
                        OrderIntentReply::Error("database already closed".into()),
                    );
                    return;
                }
            };
            if state.store.forget(&symbol) {
                // Delete the row synchronously so the next reopen does
                // not resurrect the symbol. We do this inline (not via
                // the flush thread) because `store.forget` removed it
                // from the cache, which means `drain_dirty` would not
                // emit it.
                let res = delete_row(db, symbol.as_str());
                if let Err(e) = res {
                    tracing::warn!("ticker-intent: failed to delete row for {symbol}: {e}");
                    OrderIntentReply::Error(e.to_string())
                } else {
                    OrderIntentReply::Forgotten
                }
            } else {
                OrderIntentReply::AlreadyAbsent
            }
        }
        OrderIntentMsg::FlushNow => match state.db.as_ref() {
            Some(db) => match flush_dirty(db, &state.store, Durability::Immediate) {
                Ok(()) => OrderIntentReply::Flushed,
                Err(e) => OrderIntentReply::Error(e.to_string()),
            },
            None => OrderIntentReply::Error("database already closed".into()),
        },
        OrderIntentMsg::Shutdown { force: _ } => {
            // (1) Final flush with Immediate durability, if the DB is
            //     still live.
            if let Some(db) = state.db.as_ref() {
                if let Err(e) = flush_dirty(db, &state.store, Durability::Immediate) {
                    tracing::warn!("ticker-intent: final flush failed: {e}");
                }
            }
            // (2) Tell the flush thread to exit and (3) wait for it.
            {
                let (lock, cvar) = &*state.ctl;
                let mut guard = match lock.lock() {
                    Ok(g) => g,
                    Err(poisoned) => poisoned.into_inner(),
                };
                guard.shutdown = true;
                cvar.notify_all();
            }
            if let Some(h) = state.flush_handle.take() {
                let _ = h.join();
            }
            // (4) Take and drop the Database — releases the file lock
            //     even though the mailbox thread is still alive waiting
            //     for its channel to close.
            state.db = None;
            OrderIntentReply::ShutdownAck
        }
    };

    reply_with(reply_channel, reply);
}

fn reply_with(ch: Option<Sender<OrderIntentReply>>, reply: OrderIntentReply) {
    if let Some(ch) = ch {
        let _ = ch.blocking_send(reply);
    }
}

fn delete_row(db: &Database, key: &str) -> Result<(), IntentError> {
    let mut txn = db
        .begin_write()
        .map_err(|e| IntentError::WriteFailed { reason: e.to_string() })?;
    txn.set_durability(Durability::Immediate);
    {
        let mut table = txn
            .open_table(TABLE)
            .map_err(|e| IntentError::WriteFailed { reason: e.to_string() })?;
        table
            .remove(key)
            .map_err(|e| IntentError::WriteFailed { reason: e.to_string() })?;
    }
    txn.commit()
        .map_err(|e| IntentError::WriteFailed { reason: e.to_string() })?;
    Ok(())
}

/// Notify the flush thread that new dirty entries exist.
pub(crate) fn wake_flush(ctl: &(StdMutex<FlushCtl>, Condvar)) {
    let (lock, cvar) = ctl;
    let mut guard = match lock.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    guard.wake = true;
    guard.last_wake = Instant::now();
    cvar.notify_all();
}
