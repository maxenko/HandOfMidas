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
//!
//! # Dead-code allowance
//!
//! This module defines the full Slice 1a + 1b message / reply surface
//! even though Slice 2 only wires the synchronous `Upsert` and sync
//! shutdown paths. Several enum variants, reply fields, and helper
//! methods (`IntentSource::Panel`/`Chart`, `InvalidIntent`, reply
//! payload fields, etc.) are consumed by Slices 3, 4, and 5. Suppress
//! dead-code at the file level rather than sprinkling per-item
//! attributes — this module is the frozen Slice 1a/1b API surface.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex as StdMutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use mailbox_processor::MailboxProcessor;
use parking_lot::Mutex as PlMutex;
use redb::{
    CommitError, Database, DatabaseError, Durability, ReadableTable, StorageError,
    TableDefinition, TransactionError,
};
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

/// Disk-full backoff schedule. Index = number of *prior* failures.
/// Once we reach the last entry we stay at that value forever.
const DISK_FULL_BACKOFF: [Duration; 5] = [
    Duration::from_secs(1),
    Duration::from_secs(2),
    Duration::from_secs(5),
    Duration::from_secs(10),
    Duration::from_secs(30),
];

/// The modal message surfaced when a plain `Shutdown { force: false }`
/// arrives while the flush loop is stuck on `StorageFull`. Also the
/// exact string Slice 2 will pipe into `Message::ShowToast` once the
/// handle is wired into `MidasApp`.
pub const DISK_FULL_MODAL_MESSAGE: &str =
    "Cannot save order memory — disk full. Free space to exit cleanly.";

/// The startup toast surfaced by Slice 1b's corruption-recovery path.
/// Slice 2 drains `TickerOrderIntentHandle::pending_startup_toasts`
/// into `Message::ShowToast` when the handle is constructed.
pub const CORRUPTION_RECOVERY_TOAST: &str =
    "Order memory reset — previous file was corrupt";

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
    /// Another instance of the app already holds the file lock on this
    /// database. Slice 1b surfaces this as a graceful exit at app
    /// startup; `MidasApp::new()` (Slice 2) will convert it to a
    /// user-facing "Another Hand of Midas instance is running" dialog.
    #[error("ticker-intent database at {path} is already open by another instance")]
    AlreadyOpen {
        /// Path that was attempted.
        path: PathBuf,
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
    /// Number of *consecutive* disk-full commit failures. Zero when
    /// the store is healthy. Used to index [`DISK_FULL_BACKOFF`].
    pub(crate) disk_full_failures: u32,
    /// When the flush loop is allowed to try again after a disk-full
    /// hit. `None` while the store is healthy.
    pub(crate) next_retry_at: Option<Instant>,
}

impl FlushCtl {
    /// Whether the store is currently in the disk-full backoff state.
    pub(crate) fn in_disk_full_backoff(&self) -> bool {
        self.disk_full_failures > 0
    }

    /// The next backoff delay to use for the given failure count.
    pub(crate) fn backoff_delay(failures: u32) -> Duration {
        let idx = (failures.saturating_sub(1) as usize).min(DISK_FULL_BACKOFF.len() - 1);
        DISK_FULL_BACKOFF[idx]
    }
}

impl Default for FlushCtl {
    fn default() -> Self {
        Self {
            wake: false,
            shutdown: false,
            last_wake: Instant::now(),
            disk_full_failures: 0,
            next_retry_at: None,
        }
    }
}

/// Handles the handle needs to expose to Slice 2 so that any pending
/// startup toast or modal message can be drained into the iced view
/// layer after construction. Populated from inside
/// [`open_and_hydrate`] (for corruption recovery) and from the flush
/// loop (for disk-full shutdown guards).
#[derive(Clone, Default)]
pub(crate) struct HandleNotifications {
    /// Toasts to fire `Message::ShowToast` with at app startup.
    /// Currently only ever holds the corruption-recovery toast.
    pub(crate) pending_startup_toasts: Arc<PlMutex<Vec<String>>>,
    /// A modal message that blocks shutdown. Set by the flush loop
    /// when `Shutdown { force: false }` arrives while disk-full
    /// backoff is active. Slice 2 will render this as a modal dialog
    /// and reissue `Shutdown { force: true }` when the user has freed
    /// space (or accepted data loss).
    pub(crate) pending_modal_message: Arc<PlMutex<Option<String>>>,
}

/// Classification of a [`flush_dirty`] failure. The flush loop needs
/// to distinguish "disk full — retry later" from "something else went
/// wrong" so the backoff schedule only kicks in for the former.
#[derive(Debug)]
pub(crate) enum FlushError {
    /// The filesystem rejected the write with
    /// `std::io::ErrorKind::StorageFull`. Dirty symbols have already
    /// been re-marked so the next flush attempt will see them.
    DiskFull(IntentError),
    /// Any other [`IntentError`] (encode failures, lock poison,
    /// transaction errors). Treated as a transient warning — the
    /// dirty symbols are re-marked so a later user edit does not
    /// silently drop state.
    Other(IntentError),
}

impl std::fmt::Display for FlushError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FlushError::DiskFull(e) => write!(f, "disk full: {e}"),
            FlushError::Other(e) => write!(f, "{e}"),
        }
    }
}

/// Inspect a [`redb::DatabaseError`] and decide whether the underlying
/// cause is `std::io::ErrorKind::StorageFull`.
fn is_storage_full_database_error(e: &DatabaseError) -> bool {
    match e {
        DatabaseError::Storage(s) => is_storage_full_storage_error(s),
        _ => false,
    }
}

/// Inspect a [`redb::StorageError`] and decide whether the underlying
/// cause is `std::io::ErrorKind::StorageFull`.
fn is_storage_full_storage_error(e: &StorageError) -> bool {
    matches!(e, StorageError::Io(io) if io.kind() == std::io::ErrorKind::StorageFull)
}

/// Inspect a [`redb::TransactionError`] (returned from
/// `Database::begin_write`) and decide whether the underlying cause is
/// `std::io::ErrorKind::StorageFull`.
fn is_storage_full_transaction_error(e: &TransactionError) -> bool {
    match e {
        TransactionError::Storage(s) => is_storage_full_storage_error(s),
        _ => false,
    }
}

/// Inspect a [`redb::CommitError`] and decide whether the underlying
/// cause is `std::io::ErrorKind::StorageFull`. `CommitError` is
/// `#[non_exhaustive]`, so we add a wildcard arm.
fn is_storage_full_commit_error(e: &CommitError) -> bool {
    match e {
        CommitError::Storage(s) => is_storage_full_storage_error(s),
        _ => false,
    }
}

/// The flush thread's loop.
///
/// Blocks on the condvar for up to `FLUSH_DEBOUNCE`. On wake, drains
/// dirty entries into a single write transaction. Picks
/// `Durability::Immediate` when idle, otherwise `Eventual`.
///
/// On `std::io::ErrorKind::StorageFull` during commit, the drained
/// symbols are re-marked dirty and the loop schedules a retry per
/// [`DISK_FULL_BACKOFF`]. While in that state, ordinary wakes are
/// ignored until the retry deadline passes.
pub(crate) fn flush_loop(
    db: Arc<Database>,
    store: Arc<TickerOrderIntentStore>,
    ctl: Arc<(StdMutex<FlushCtl>, Condvar)>,
) {
    loop {
        // Compute the wait budget up front so the backoff deadline
        // caps how long we sleep.
        let (wait_budget, is_backoff_ready) = {
            let guard = match ctl.0.lock() {
                Ok(g) => g,
                Err(poisoned) => poisoned.into_inner(),
            };
            if guard.shutdown {
                // Final drain happens below via the main match arm;
                // fall through.
                (Duration::ZERO, false)
            } else {
                match guard.next_retry_at {
                    Some(deadline) => {
                        let remaining = deadline.saturating_duration_since(Instant::now());
                        if remaining.is_zero() {
                            (Duration::ZERO, true)
                        } else {
                            (remaining.min(FLUSH_DEBOUNCE), false)
                        }
                    }
                    None => (FLUSH_DEBOUNCE, false),
                }
            }
        };

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
            if is_backoff_ready {
                break false;
            }
            // While in backoff, ignore ordinary wakes until the retry
            // deadline elapses. The wait below still caps at
            // `wait_budget` so the loop re-enters on the deadline.
            let has_fired_deadline = guard.next_retry_at.is_some_and(|d| Instant::now() >= d);
            if guard.in_disk_full_backoff() {
                if has_fired_deadline {
                    break false;
                }
                guard.wake = false;
            } else if guard.wake {
                break false;
            }
            if wait_budget.is_zero() {
                break false;
            }
            // wait_timeout only fails on mutex poison; if that happens
            // we are in an unrecoverable state (the handler thread
            // panicked holding the flush lock). Propagate by panicking
            // the flush loop — the app will have already crashed.
            let (next, _res) = match cvar.wait_timeout(guard, wait_budget) {
                Ok(pair) => pair,
                Err(_) => unreachable_poison(),
            };
            guard = next;
            if guard.shutdown {
                break true;
            }
            let has_fired_deadline = guard.next_retry_at.is_some_and(|d| Instant::now() >= d);
            if guard.in_disk_full_backoff() {
                if has_fired_deadline {
                    break false;
                }
                continue;
            }
            if !guard.wake {
                continue;
            }
            break false;
        };

        let last_wake = guard.last_wake;
        guard.wake = false;
        let is_shutdown = shutdown || guard.shutdown;
        let in_backoff = guard.in_disk_full_backoff();
        drop(guard);

        let durability = if is_shutdown || in_backoff || last_wake.elapsed() >= IDLE_THRESHOLD {
            Durability::Immediate
        } else {
            Durability::Eventual
        };

        match flush_dirty(&db, &store, durability) {
            Ok(()) => {
                let mut guard = match ctl.0.lock() {
                    Ok(g) => g,
                    Err(poisoned) => poisoned.into_inner(),
                };
                if guard.disk_full_failures > 0 {
                    tracing::info!(
                        "ticker-intent: disk-full recovered after {} failure(s)",
                        guard.disk_full_failures
                    );
                }
                guard.disk_full_failures = 0;
                guard.next_retry_at = None;
            }
            Err(FlushError::DiskFull(e)) => {
                let mut guard = match ctl.0.lock() {
                    Ok(g) => g,
                    Err(poisoned) => poisoned.into_inner(),
                };
                guard.disk_full_failures = guard.disk_full_failures.saturating_add(1);
                let delay = FlushCtl::backoff_delay(guard.disk_full_failures);
                guard.next_retry_at = Some(Instant::now() + delay);
                tracing::error!(
                    "ticker-intent: disk full during commit (failure #{}): {e}. \
                     Retrying in {:?}; {} dirty symbol(s) preserved.",
                    guard.disk_full_failures,
                    delay,
                    store.dirty_len(),
                );
            }
            Err(FlushError::Other(e)) => {
                tracing::warn!("ticker-intent flush failed: {e}");
            }
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
///
/// On [`FlushError::DiskFull`] or [`FlushError::Other`], every symbol
/// that was drained by this call is re-marked dirty so the next flush
/// attempt will see it. This is what lets the disk-full backoff state
/// keep the un-persisted set alive until the filesystem recovers.
pub(crate) fn flush_dirty(
    db: &Database,
    store: &TickerOrderIntentStore,
    durability: Durability,
) -> Result<(), FlushError> {
    let dirty = store.drain_dirty();

    if dirty.is_empty() {
        return Ok(());
    }

    // Helper: re-insert the just-drained symbols so a failed commit
    // does not silently drop the user's writes.
    let requeue = |store: &TickerOrderIntentStore| {
        store.re_mark_dirty(dirty.iter().map(|(s, _)| s.clone()));
    };

    let mut txn = match db.begin_write() {
        Ok(t) => t,
        Err(e) => {
            let is_full = is_storage_full_transaction_error(&e);
            requeue(store);
            let err = IntentError::WriteFailed { reason: e.to_string() };
            return Err(if is_full {
                FlushError::DiskFull(err)
            } else {
                FlushError::Other(err)
            });
        }
    };
    txn.set_durability(durability);
    {
        let mut table = match txn.open_table(TABLE) {
            Ok(t) => t,
            Err(e) => {
                requeue(store);
                return Err(FlushError::Other(IntentError::WriteFailed {
                    reason: e.to_string(),
                }));
            }
        };
        for (symbol, intent) in &dirty {
            let bytes = match serde_json::to_vec_pretty(intent.as_ref()) {
                Ok(b) => b,
                Err(e) => {
                    requeue(store);
                    return Err(FlushError::Other(IntentError::WriteFailed {
                        reason: format!("encode {symbol}: {e}"),
                    }));
                }
            };
            if let Err(e) = table.insert(symbol.as_str(), bytes.as_slice()) {
                let is_full = is_storage_full_storage_error(&e);
                requeue(store);
                let err = IntentError::WriteFailed { reason: e.to_string() };
                return Err(if is_full {
                    FlushError::DiskFull(err)
                } else {
                    FlushError::Other(err)
                });
            }
        }
    }
    if let Err(e) = txn.commit() {
        let is_full = is_storage_full_commit_error(&e);
        requeue(store);
        let err = IntentError::WriteFailed { reason: e.to_string() };
        return Err(if is_full {
            FlushError::DiskFull(err)
        } else {
            FlushError::Other(err)
        });
    }
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
    pub(crate) notifications: HandleNotifications,
}

/// Everything [`open_and_hydrate`] hands back to
/// [`super::handle::TickerOrderIntentHandle::open`]. A struct rather
/// than a tuple so Slice 1b could add `notifications` without rippling
/// the signature through every test.
pub(crate) struct HydratedActor {
    /// The in-memory cache, seeded from disk.
    pub(crate) store: Arc<TickerOrderIntentStore>,
    /// The open `redb::Database`.
    pub(crate) db: Arc<Database>,
    /// The flush-thread control primitive.
    pub(crate) ctl: Arc<(StdMutex<FlushCtl>, Condvar)>,
    /// Pending startup toasts and modal messages the handle should
    /// expose to the view layer. Slice 1b pre-populates the startup
    /// toast from the corruption-recovery path.
    pub(crate) notifications: HandleNotifications,
}

/// Try to open a `redb::Database` at `path`, classifying the error
/// into (multi-instance lock | corruption | other-open-failure). Kept
/// separate from [`open_and_hydrate`] so the corruption recovery path
/// can call it twice without duplicating error classification.
fn try_open_database(path: &Path) -> Result<Database, DatabaseError> {
    Database::create(path)
}

/// Determine whether an `Err(DatabaseError)` should trigger the
/// whole-file corruption recovery path: move-aside + fresh create.
///
/// `DatabaseError::Storage(StorageError::Corrupted(_))` is the
/// first-class signal. We also recover on `UpgradeRequired` (the file
/// is on an older format than this build understands — from the
/// user's point of view indistinguishable from corruption), and on
/// `Storage(StorageError::Io)` variants that do **not** look like
/// disk-full (a ragged-header `read` returns `UnexpectedEof`, which
/// is corruption, but `StorageFull` is a separate failure mode that
/// must not be mistaken for a corrupt file).
fn is_corruption_like(e: &DatabaseError) -> bool {
    match e {
        DatabaseError::Storage(StorageError::Corrupted(_)) => true,
        DatabaseError::UpgradeRequired(_) => true,
        DatabaseError::Storage(StorageError::Io(io)) => {
            let kind = io.kind();
            // A well-formed redb file that is too short to parse
            // surfaces as `UnexpectedEof` or `InvalidData` — treat as
            // corruption. `StorageFull` and `PermissionDenied` stay
            // as hard open-failures.
            !matches!(
                kind,
                std::io::ErrorKind::StorageFull
                    | std::io::ErrorKind::PermissionDenied
                    | std::io::ErrorKind::NotFound
            )
        }
        _ => false,
    }
}

/// Rename a corrupt file aside with a `.corrupt.<unix_ts>` suffix.
/// On filesystem races (the aside-name already exists — extremely
/// unlikely) we fall through to appending a nanosecond counter.
fn move_aside_corrupt(path: &Path) -> std::io::Result<PathBuf> {
    let ts = chrono::Utc::now().timestamp();
    let filename = path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "ticker_state.redb".to_string());
    let mut aside = path.with_file_name(format!("{filename}.corrupt.{ts}"));
    if aside.exists() {
        let nanos = chrono::Utc::now().timestamp_subsec_nanos();
        aside = path.with_file_name(format!("{filename}.corrupt.{ts}.{nanos}"));
    }
    std::fs::rename(path, &aside)?;
    Ok(aside)
}

/// Open the `redb::Database` at `path` and hydrate the store from it.
///
/// Returns a [`HydratedActor`] the handle uses to spawn the mailbox.
/// Parent directory is created if missing.
///
/// Slice 1b failure-mode behavior:
/// - `DatabaseError::DatabaseAlreadyOpen` → [`IntentError::AlreadyOpen`]
///   (the caller surfaces this as a graceful exit / dialog).
/// - `DatabaseError::Storage(StorageError::Corrupted(_))`,
///   `DatabaseError::UpgradeRequired(_)`, or a corruption-like IO
///   error → rename the file to `<name>.corrupt.<unix_ts>`, log at
///   `error!` level, open a fresh empty database at the original
///   path, and push the recovery message onto
///   `notifications.pending_startup_toasts` so Slice 2 can surface
///   it with `Message::ShowToast`.
pub(crate) fn open_and_hydrate(path: &Path) -> Result<HydratedActor, IntentError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| IntentError::OpenFailed {
            path: path.to_path_buf(),
            reason: e.to_string(),
        })?;
    }

    let notifications = HandleNotifications::default();

    let db = match try_open_database(path) {
        Ok(db) => db,
        Err(DatabaseError::DatabaseAlreadyOpen) => {
            return Err(IntentError::AlreadyOpen {
                path: path.to_path_buf(),
            });
        }
        Err(e) if is_corruption_like(&e) => {
            // Double-check this is not a disk-full misclassification.
            if is_storage_full_database_error(&e) {
                return Err(IntentError::OpenFailed {
                    path: path.to_path_buf(),
                    reason: e.to_string(),
                });
            }
            tracing::error!(
                "ticker-intent: database at {} is unreadable ({e}); \
                 moving aside and starting fresh",
                path.display()
            );
            let aside = move_aside_corrupt(path).map_err(|io_err| IntentError::OpenFailed {
                path: path.to_path_buf(),
                reason: format!("failed to rename corrupt file: {io_err}"),
            })?;
            tracing::error!(
                "ticker-intent: renamed corrupt file to {}",
                aside.display()
            );
            notifications
                .pending_startup_toasts
                .lock()
                .push(CORRUPTION_RECOVERY_TOAST.to_string());
            try_open_database(path).map_err(|e2| IntentError::OpenFailed {
                path: path.to_path_buf(),
                reason: format!("fresh create after corruption: {e2}"),
            })?
        }
        Err(e) => {
            return Err(IntentError::OpenFailed {
                path: path.to_path_buf(),
                reason: e.to_string(),
            });
        }
    };

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

    Ok(HydratedActor {
        store,
        db,
        ctl,
        notifications,
    })
}

/// Test-only variant of [`open_and_hydrate`] that opens the database
/// with a caller-supplied [`redb::StorageBackend`]. Used by the
/// disk-full backoff test to inject a backend that returns
/// `std::io::ErrorKind::StorageFull` on demand.
#[cfg(test)]
pub(crate) fn open_and_hydrate_with_backend<B>(
    backend: B,
) -> Result<HydratedActor, IntentError>
where
    B: redb::StorageBackend,
{
    let db = redb::Builder::new()
        .create_with_backend(backend)
        .map_err(|e| IntentError::OpenFailed {
            path: PathBuf::from("<in-memory>"),
            reason: e.to_string(),
        })?;

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
    let db = Arc::new(db);
    let ctl = Arc::new((StdMutex::new(FlushCtl::default()), Condvar::new()));
    Ok(HydratedActor {
        store,
        db,
        ctl,
        notifications: HandleNotifications::default(),
    })
}

/// Spawn the mailbox actor thread and the flush loop thread.
///
/// The returned [`MailboxProcessor`] is cheaply cloneable and drives
/// all state changes to the store + database.
pub(crate) fn spawn_actor(
    store: Arc<TickerOrderIntentStore>,
    db: Arc<Database>,
    ctl: Arc<(StdMutex<FlushCtl>, Condvar)>,
    notifications: HandleNotifications,
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
        notifications,
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
                Err(FlushError::DiskFull(e)) => {
                    // Trip the backoff state so the shutdown guard sees it.
                    let mut guard = match state.ctl.0.lock() {
                        Ok(g) => g,
                        Err(poisoned) => poisoned.into_inner(),
                    };
                    if guard.disk_full_failures == 0 {
                        guard.disk_full_failures = 1;
                        guard.next_retry_at =
                            Some(Instant::now() + FlushCtl::backoff_delay(1));
                    }
                    tracing::error!("ticker-intent: FlushNow hit disk full: {e}");
                    OrderIntentReply::Error(e.to_string())
                }
                Err(FlushError::Other(e)) => OrderIntentReply::Error(e.to_string()),
            },
            None => OrderIntentReply::Error("database already closed".into()),
        },
        OrderIntentMsg::Shutdown { force } => {
            // (0) If we are in disk-full backoff and the caller did
            //     not force, surface the modal message and refuse to
            //     drop the actor. Slice 2 will render the modal and
            //     re-issue `Shutdown { force: true }` if the user
            //     accepts data loss.
            let in_backoff = {
                let guard = match state.ctl.0.lock() {
                    Ok(g) => g,
                    Err(poisoned) => poisoned.into_inner(),
                };
                guard.in_disk_full_backoff()
            };
            if in_backoff && !force {
                *state.notifications.pending_modal_message.lock() =
                    Some(DISK_FULL_MODAL_MESSAGE.to_string());
                tracing::warn!(
                    "ticker-intent: refusing non-forced shutdown while disk full; \
                     modal message latched"
                );
                reply_with(
                    reply_channel,
                    OrderIntentReply::Error(DISK_FULL_MODAL_MESSAGE.to_string()),
                );
                return;
            }
            if in_backoff && force {
                tracing::error!(
                    "ticker-intent: force-shutdown while disk full; {} symbol(s) \
                     of un-persisted order memory will be dropped",
                    state.store.dirty_len()
                );
            }
            // (1) Final flush with Immediate durability, if the DB is
            //     still live. Swallow the error — shutdown is terminal.
            if let Some(db) = state.db.as_ref() {
                match flush_dirty(db, &state.store, Durability::Immediate) {
                    Ok(()) => {}
                    Err(e) => tracing::warn!("ticker-intent: final flush failed: {e}"),
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

/// Signal the flush thread to perform a final `Immediate` commit and
/// exit. Used by [`super::handle::TickerOrderIntentHandle::shutdown_blocking`]
/// so iced's sync `update()` path can trigger durable shutdown without
/// awaiting the mailbox actor.
pub(crate) fn signal_flush_shutdown(ctl: &(StdMutex<FlushCtl>, Condvar)) {
    let (lock, cvar) = ctl;
    let mut guard = match lock.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    guard.shutdown = true;
    cvar.notify_all();
}
