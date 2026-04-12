//! Public facade for the ticker-intent store.
//!
//! `TickerOrderIntentHandle` is the type the rest of the app holds.
//! It owns a clone of the in-memory store (for sync reads), plus a
//! clone of the mailbox sender (for actor writes). Both are cheaply
//! cloneable — the handle itself is `Clone`.
//!
//! # Sync / async split
//!
//! iced's `update()` loop is synchronous. Anything the reducer needs
//! on the hot path must be callable from a sync context, which means:
//!
//! - [`TickerOrderIntentHandle::snapshot`]: sync, reads the cache
//!   directly. No message round-trip.
//! - [`TickerOrderIntentHandle::upsert`]: sync, mutates the cache
//!   synchronously **and** fires a notify to the actor to schedule the
//!   flush. The `Upsert` message is also sent on a best-effort basis
//!   so the actor can perform the inline deletion / source tagging
//!   logic — but the visible cache mutation has already happened.
//! - Flush and shutdown are async because they only make sense in an
//!   await point anyway (draining a mailbox).

use std::path::PathBuf;
use std::sync::Arc;

use mailbox_processor::MailboxProcessor;
use parking_lot::Mutex as PlMutex;

use crate::annotation_store::SymbolKey;

use super::actor::{
    open_and_hydrate, spawn_actor, wake_flush, FlushCtl, HydratedActor, IntentError,
    OrderIntentMsg, OrderIntentReply,
};
use super::store::{TickerOrderIntentStore, UpsertOutcome};
use super::TickerOrderIntent;

/// Abstraction over a ticker-intent handle, so reducer tests can inject
/// a mock in place of a real `redb`-backed handle.
///
/// Slice 3 fills in the reducer bodies; Slice 3's tests will implement
/// this trait on a dummy struct that tracks which methods were called.
/// Slice 2's bootstrap helper also takes this trait so the test suite
/// can drive it with a real handle-backed fixture.
pub trait TickerIntentAccess: Send + Sync {
    /// Sync read of the current intent for a symbol.
    fn snapshot(&self, symbol: &SymbolKey) -> Option<Arc<TickerOrderIntent>>;

    /// Sync upsert. Returns the outcome *of the cache mutation*; the
    /// actor-side persistence runs asynchronously in the background.
    fn upsert(&self, msg: OrderIntentMsg) -> UpsertOutcome;

    /// Force an immediate durable flush.
    #[allow(async_fn_in_trait, dead_code)]
    // Call sites (Slice 4's GATR snap, Slice 3's tests) land later.
    async fn flush_now(&self);

    /// Graceful shutdown: drain, commit, and drop the database.
    #[allow(async_fn_in_trait, dead_code)]
    // Sync `shutdown_blocking` is used from MidasApp; the async form
    // remains part of the trait for out-of-iced callers / tests.
    async fn shutdown(self);
}

/// Concrete handle backed by `redb` + the mailbox actor.
///
/// Cheaply cloneable — all clones share the same `Arc<Store>` and the
/// same mailbox channel.
#[derive(Clone)]
pub struct TickerOrderIntentHandle {
    store: Arc<TickerOrderIntentStore>,
    ctl: Arc<(std::sync::Mutex<FlushCtl>, std::sync::Condvar)>,
    mb: MailboxProcessor<OrderIntentMsg, OrderIntentReply>,
    /// Toasts to surface at app startup. Set at `open()` time by the
    /// corruption-recovery path in [`super::actor::open_and_hydrate`].
    /// Slice 4 will drain this on `MidasApp::new()` and pipe each
    /// entry into `Message::ShowToast` (the toast view layer itself
    /// is Slice 4 scope).
    #[allow(dead_code)] // drained by Slice 4's toast view layer
    pending_startup_toasts: Arc<PlMutex<Vec<String>>>,
    /// Modal message latched while the flush loop is stuck on
    /// `StorageFull` and a non-forced shutdown has been refused.
    /// Slice 4 reads this to decide whether to render a blocking
    /// modal at shutdown.
    #[allow(dead_code)] // drained by Slice 4's shutdown modal
    pending_modal_message: Arc<PlMutex<Option<String>>>,
}

impl TickerOrderIntentHandle {
    /// Open the store at `path`, hydrate from disk, and spawn the
    /// actor + flush threads.
    ///
    /// Returns synchronously, like `midas-store`'s `DbHandle::open`.
    ///
    /// Slice 1b failure-mode behavior:
    /// - Returns [`IntentError::AlreadyOpen`] if another instance
    ///   holds the file lock.
    /// - Silently recovers from a corrupt file by renaming it aside
    ///   (`<name>.corrupt.<unix_ts>`) and opening a fresh empty DB.
    ///   The caller should drain [`Self::take_pending_startup_toasts`]
    ///   after construction to surface the recovery toast.
    pub fn open(path: PathBuf) -> Result<Self, IntentError> {
        let HydratedActor {
            store,
            db,
            ctl,
            notifications,
        } = open_and_hydrate(&path)?;
        let pending_startup_toasts = notifications.pending_startup_toasts.clone();
        let pending_modal_message = notifications.pending_modal_message.clone();
        let mb = spawn_actor(store.clone(), db, ctl.clone(), notifications);
        Ok(Self {
            store,
            ctl,
            mb,
            pending_startup_toasts,
            pending_modal_message,
        })
    }

    /// Drain and return any startup toasts queued by the open path
    /// (currently only the corruption-recovery notice). Slice 4 calls
    /// this exactly once at construction time.
    ///
    /// Repeated calls after the first drain return an empty `Vec`.
    #[allow(dead_code)] // consumed by Slice 4's toast view layer
    pub fn take_pending_startup_toasts(&self) -> Vec<String> {
        std::mem::take(&mut *self.pending_startup_toasts.lock())
    }

    /// Read-only peek at the current pending modal message without
    /// clearing it. `Some` while the flush loop is stuck on
    /// `StorageFull` and a non-forced shutdown has been refused.
    /// Slice 4 uses this to decide whether to render the blocking
    /// "disk full" modal at shutdown time.
    #[allow(dead_code)] // consumed by Slice 4's shutdown modal
    pub fn pending_modal_message(&self) -> Option<String> {
        self.pending_modal_message.lock().clone()
    }

    /// Drain and return the pending modal message, if any. Slice 4
    /// calls this once it has surfaced the modal to the user and
    /// does not need the handle to keep it latched.
    #[allow(dead_code)] // consumed by Slice 4's shutdown modal
    pub fn take_pending_modal_message(&self) -> Option<String> {
        self.pending_modal_message.lock().take()
    }

    /// Read the current intent for a symbol. Lock-free after the clone.
    pub fn snapshot(&self, symbol: &SymbolKey) -> Option<Arc<TickerOrderIntent>> {
        self.store.snapshot(symbol)
    }

    /// Sync upsert.
    ///
    /// The in-memory cache is mutated *before* this call returns, so
    /// the very next [`Self::snapshot`] observes the new value. The
    /// persistence flush is scheduled on the background thread via the
    /// notify condvar.
    ///
    /// Returns the outcome of the cache mutation. The `msg` argument
    /// is consumed so the handler can take ownership of the intent.
    pub fn upsert(&self, msg: OrderIntentMsg) -> UpsertOutcome {
        match msg {
            OrderIntentMsg::Upsert {
                symbol,
                intent,
                source: _,
            } => {
                let outcome = self.store.upsert(symbol, *intent);
                if matches!(outcome, UpsertOutcome::Applied { .. }) {
                    wake_flush(&self.ctl);
                }
                outcome
            }
            // Non-upsert messages are forwarded to the actor but from a
            // sync context we can only fire-and-forget. Reducer tests
            // that need the reply should use the async API.
            other => {
                // Use try_send via the mailbox; if it fails the caller
                // can fall back to the async send.
                // `fire_and_forget` is async, but internally it only
                // awaits the channel send — from a blocking context we
                // spawn a lightweight tokio task when one exists.
                if let Ok(handle) = tokio::runtime::Handle::try_current() {
                    let mb = self.mb.clone();
                    handle.spawn(async move {
                        let _ = mb.fire_and_forget(other).await;
                    });
                } else {
                    tracing::warn!(
                        "ticker-intent: sync upsert of non-Upsert message dropped \
                         (no tokio runtime)"
                    );
                }
                UpsertOutcome::NoOp {
                    reason: super::NoOpReason::StaleSource,
                }
            }
        }
    }

    /// Async upsert that awaits the actor reply.
    ///
    /// Useful when the caller wants the store-side generation counter
    /// (e.g. in a test). Equivalent to [`Self::upsert`] on the happy
    /// path — both mutate the same cache.
    #[allow(dead_code)] // used by reducer tests in Slice 3
    pub async fn upsert_async(
        &self,
        msg: OrderIntentMsg,
    ) -> Result<OrderIntentReply, IntentError> {
        self.mb
            .send(msg)
            .await
            .map_err(|_| IntentError::ChannelClosed)
    }

    /// Remove a symbol from the store and delete its on-disk row.
    #[allow(dead_code)] // call site lands in Slice 5a (watchlist remove)
    pub async fn forget(&self, symbol: SymbolKey) -> Result<OrderIntentReply, IntentError> {
        self.mb
            .send(OrderIntentMsg::ForgetSymbol { symbol })
            .await
            .map_err(|_| IntentError::ChannelClosed)
    }

    /// Force a durable flush.
    #[allow(dead_code)] // used by Slice 4's GATR snap force-flush path
    pub async fn flush_now(&self) {
        let _ = self.mb.send(OrderIntentMsg::FlushNow).await;
    }

    /// Graceful shutdown: drain, commit, drop the database.
    #[allow(dead_code)] // async shutdown reserved for future non-iced callers
    pub async fn shutdown(self) {
        let _ = self.mb.send(OrderIntentMsg::Shutdown { force: false }).await;
    }

    /// Test-only: drive a non-forced shutdown without consuming
    /// `self`. Used by the disk-full shutdown-guard test to assert
    /// that the guard refuses the request and latches the modal
    /// message, then still allows a follow-up force-shutdown.
    #[cfg(test)]
    pub(crate) async fn shutdown_best_effort_non_forced(&self) {
        let _ = self.mb.send(OrderIntentMsg::Shutdown { force: false }).await;
    }

    /// Test-only: drive a force-shutdown without consuming `self`.
    /// After this returns the actor has dropped the database, so
    /// the handle is inert.
    #[cfg(test)]
    pub(crate) async fn shutdown_force(&self) {
        let _ = self.mb.send(OrderIntentMsg::Shutdown { force: true }).await;
    }

    /// Test-only: manually trip the disk-full backoff state. The
    /// real flush loop sets the same fields from the background
    /// thread; this hook lets the shutdown-guard test reach the
    /// same state without having to synchronize with a spawned
    /// failure fixture on a real file.
    #[cfg(test)]
    pub(crate) fn __test_force_disk_full_state(&self) {
        let (lock, cvar) = &*self.ctl;
        let mut guard = match lock.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard.disk_full_failures = 1;
        guard.next_retry_at =
            Some(std::time::Instant::now() + std::time::Duration::from_secs(30));
        cvar.notify_all();
    }

    /// Synchronous, best-effort shutdown usable from a non-async context
    /// (iced's sync `update()` loop).
    ///
    /// Signals the flush thread via the shared condvar so it performs a
    /// final `Immediate` commit and exits. Does **not** await the
    /// mailbox actor thread — when the handle is dropped, the sender
    /// side of the channel closes and the thread unwinds naturally.
    /// The flush thread's final commit is what matters for durability.
    pub fn shutdown_blocking(&self) {
        super::actor::signal_flush_shutdown(&self.ctl);
    }
}

impl TickerIntentAccess for TickerOrderIntentHandle {
    fn snapshot(&self, symbol: &SymbolKey) -> Option<Arc<TickerOrderIntent>> {
        TickerOrderIntentHandle::snapshot(self, symbol)
    }

    fn upsert(&self, msg: OrderIntentMsg) -> UpsertOutcome {
        TickerOrderIntentHandle::upsert(self, msg)
    }

    async fn flush_now(&self) {
        TickerOrderIntentHandle::flush_now(self).await
    }

    async fn shutdown(self) {
        TickerOrderIntentHandle::shutdown(self).await
    }
}
