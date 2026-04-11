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

use crate::annotation_store::SymbolKey;

use super::actor::{
    open_and_hydrate, spawn_actor, wake_flush, FlushCtl, IntentError, OrderIntentMsg,
    OrderIntentReply,
};
use super::store::{TickerOrderIntentStore, UpsertOutcome};
use super::TickerOrderIntent;

/// Abstraction over a ticker-intent handle, so reducer tests can inject
/// a mock in place of a real `redb`-backed handle.
///
/// Slice 3 fills in the reducer bodies; Slice 3's tests will implement
/// this trait on a dummy struct that tracks which methods were called.
pub trait TickerIntentAccess: Send + Sync {
    /// Sync read of the current intent for a symbol.
    fn snapshot(&self, symbol: &SymbolKey) -> Option<Arc<TickerOrderIntent>>;

    /// Sync upsert. Returns the outcome *of the cache mutation*; the
    /// actor-side persistence runs asynchronously in the background.
    fn upsert(&self, msg: OrderIntentMsg) -> UpsertOutcome;

    /// Force an immediate durable flush.
    #[allow(async_fn_in_trait)]
    async fn flush_now(&self);

    /// Graceful shutdown: drain, commit, and drop the database.
    #[allow(async_fn_in_trait)]
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
}

impl TickerOrderIntentHandle {
    /// Open the store at `path`, hydrate from disk, and spawn the
    /// actor + flush threads.
    ///
    /// Returns synchronously, like `midas-store`'s `DbHandle::open`.
    pub fn open(path: PathBuf) -> Result<Self, IntentError> {
        let (store, db, ctl) = open_and_hydrate(&path)?;
        let mb = spawn_actor(store.clone(), db, ctl.clone());
        Ok(Self { store, ctl, mb })
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
    pub async fn forget(&self, symbol: SymbolKey) -> Result<OrderIntentReply, IntentError> {
        self.mb
            .send(OrderIntentMsg::ForgetSymbol { symbol })
            .await
            .map_err(|_| IntentError::ChannelClosed)
    }

    /// Force a durable flush.
    pub async fn flush_now(&self) {
        let _ = self.mb.send(OrderIntentMsg::FlushNow).await;
    }

    /// Graceful shutdown: drain, commit, drop the database.
    pub async fn shutdown(self) {
        let _ = self.mb.send(OrderIntentMsg::Shutdown { force: false }).await;
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
