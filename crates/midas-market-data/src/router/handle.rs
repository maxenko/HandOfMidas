//! RAII subscription handles returned by [`MarketDataRouter`].
//!
//! Handles own two pieces of state: a private `broadcast::Receiver` and
//! a `Box<dyn Guard>`. The guard sends a `DecRef` message on `Drop`
//! (BR-3 / NB-1). Handles are deliberately `!Clone`: cloning would not
//! `IncRef` upstream and would destabilise the refcount.
//!
//! Consumers have three entry points, in order of preference:
//!
//! * [`SubscriptionHandle::recv`] — borrow-based, retains ownership
//!   across calls.
//! * [`SubscriptionHandle::into_stream`] — move-based, folds rx+guard
//!   into a single [`futures::Stream`] so drop of the stream drops the
//!   guard. This is the "can't forget to hold the guard" shape and is
//!   what `history_then_live` builds on.
//! * [`SubscriptionHandle::into_parts`] — escape hatch for tests or
//!   composition; caller must keep the guard alive alongside the
//!   receiver.
//!
//! [`MarketDataRouter`]: crate::router::MarketDataRouter

use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use futures::Stream;
use midas_broker_core::market_data::{Quote, SymbolKey};
use tokio::sync::{broadcast, mpsc, watch};
use tokio_stream::wrappers::BroadcastStream;

use super::actor::RouterMsg;

/// Shared marker for every RAII guard issued by the router.
///
/// The `Send + Sync` bound is what lets [`SubscriptionHandle`] hold the
/// guard behind `Box<dyn Guard>` and still be sent between tasks.
pub trait Guard: Send + Sync {}

/// Guard for a tick subscription — sends `DecTickRef` on drop.
pub(crate) struct TickSubGuard {
    pub(crate) symbol: SymbolKey,
    pub(crate) control: mpsc::UnboundedSender<RouterMsg>,
}

impl Drop for TickSubGuard {
    fn drop(&mut self) {
        // Send is fire-and-forget; if the control channel is already
        // closed the actor is being torn down and the upstream is
        // already being cancelled — nothing to do.
        let _ = self.control.send(RouterMsg::DecTickRef {
            symbol: self.symbol.clone(),
        });
    }
}
impl Guard for TickSubGuard {}

/// Guard for a realtime-bar subscription — sends `DecRtBarRef` on drop.
pub(crate) struct RtBarSubGuard {
    pub(crate) symbol: SymbolKey,
    pub(crate) control: mpsc::UnboundedSender<RouterMsg>,
}

impl Drop for RtBarSubGuard {
    fn drop(&mut self) {
        let _ = self.control.send(RouterMsg::DecRtBarRef {
            symbol: self.symbol.clone(),
        });
    }
}
impl Guard for RtBarSubGuard {}

/// Guard for a quote-watch subscription — sends `DecWatchRef` on drop.
pub(crate) struct WatchGuard {
    pub(crate) symbol: SymbolKey,
    pub(crate) control: mpsc::UnboundedSender<RouterMsg>,
}

impl Drop for WatchGuard {
    fn drop(&mut self) {
        let _ = self.control.send(RouterMsg::DecWatchRef {
            symbol: self.symbol.clone(),
        });
    }
}
impl Guard for WatchGuard {}

/// Fan-out handle for a per-symbol broadcast.
///
/// Generic over `T = Tick | Bar`. Explicitly `!Clone` (no derive) —
/// clones would not increment the refcount and would break cleanup.
/// Use [`SubscriptionHandle::resubscribe`] if you need a secondary
/// receiver that piggybacks on the same guard.
///
/// The inner broadcast receiver is private; consumers must go through
/// [`recv`], [`into_stream`], or [`into_parts`].
///
/// [`recv`]: SubscriptionHandle::recv
/// [`into_stream`]: SubscriptionHandle::into_stream
/// [`into_parts`]: SubscriptionHandle::into_parts
pub struct SubscriptionHandle<T> {
    rx: broadcast::Receiver<Arc<T>>,
    /// `Option` so [`into_parts`] can `.take()` without Clone; always
    /// `Some` while the handle is live. Holds the RAII guard whose
    /// drop fires `DecRef` on the router.
    _guard: Box<dyn Guard>,
}

impl<T: Clone + Send + Sync + 'static> SubscriptionHandle<T> {
    /// Build a handle from its parts.
    ///
    /// Pub-crate only — the router actor is the sole constructor.
    pub(crate) fn new(rx: broadcast::Receiver<Arc<T>>, guard: Box<dyn Guard>) -> Self {
        Self { rx, _guard: guard }
    }

    /// Borrow-based receive. Handle retains ownership of rx + guard,
    /// so refcounting stays intact across recv() calls.
    pub async fn recv(&mut self) -> Result<Arc<T>, broadcast::error::RecvError> {
        self.rx.recv().await
    }

    /// Re-subscribe: fresh broadcast receiver on the same channel.
    ///
    /// Does NOT bump the refcount — the original handle's guard still
    /// drives cancellation. Only safe while `self` is alive.
    pub fn resubscribe(&self) -> broadcast::Receiver<Arc<T>> {
        self.rx.resubscribe()
    }

    /// Consume into (receiver, guard). Caller is responsible for
    /// keeping the guard alive alongside the receiver — dropping it
    /// early will cascade a `DecRef`. Prefer [`into_stream`] unless
    /// you need the raw receiver.
    ///
    /// [`into_stream`]: SubscriptionHandle::into_stream
    pub fn into_parts(self) -> (broadcast::Receiver<Arc<T>>, Box<dyn Guard>) {
        (self.rx, self._guard)
    }

    /// Consume into a [`Stream`] that owns both rx AND guard. Dropping
    /// the returned stream drops the guard, which `DecRef`s upstream —
    /// this is the preferred consumer API (NB-1 / NB-2).
    ///
    /// `Lagged` items are silently skipped (consumer got behind and
    /// the broadcast ring wrapped); `Closed` ends the stream.
    pub fn into_stream(self) -> GuardedStream<T> {
        let (rx, guard) = self.into_parts();
        GuardedStream {
            inner: BroadcastStream::new(rx),
            _guard: guard,
        }
    }
}

impl<T> std::fmt::Debug for SubscriptionHandle<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SubscriptionHandle")
            .field("rx", &"<broadcast::Receiver>")
            .field("_guard", &"<Box<dyn Guard>>")
            .finish()
    }
}

/// Watchlist-friendly wrapper around `watch::Receiver<Quote>`.
///
/// Wraps the [`WatchGuard`] alongside the receiver so dropping this
/// struct triggers `DecWatchRef`. Derefs to `watch::Receiver<Quote>`
/// for ergonomic calls.
pub struct QuoteHandle {
    rx: watch::Receiver<Quote>,
    _guard: WatchGuard,
}

impl QuoteHandle {
    pub(crate) fn new(rx: watch::Receiver<Quote>, guard: WatchGuard) -> Self {
        Self { rx, _guard: guard }
    }

    /// Borrow the current [`Quote`] (snapshot-style).
    pub fn borrow(&self) -> watch::Ref<'_, Quote> {
        self.rx.borrow()
    }

    /// Wait for the next change to the underlying quote.
    pub async fn changed(&mut self) -> Result<(), watch::error::RecvError> {
        self.rx.changed().await
    }

    /// Non-blocking probe — `Ok(true)` if a new value is pending,
    /// `Ok(false)` if nothing changed since the last observation,
    /// `Err(RecvError)` if the sender was dropped (S8 §F — watchlist
    /// resync signal).
    pub fn has_changed(&self) -> Result<bool, watch::error::RecvError> {
        self.rx.has_changed()
    }

    /// Borrow mutable access to the underlying `watch::Receiver`.
    ///
    /// Pre-existing callers that need to pass the receiver to
    /// `select!` or clone it can reach through here; the returned
    /// reference borrows `self` so the guard is still held.
    pub fn inner_mut(&mut self) -> &mut watch::Receiver<Quote> {
        &mut self.rx
    }
}

impl std::fmt::Debug for QuoteHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QuoteHandle").finish()
    }
}

/// Stream wrapper that owns both the broadcast receiver and the RAII
/// guard of a [`SubscriptionHandle`].
///
/// Returned by [`SubscriptionHandle::into_stream`]. Skips
/// `RecvError::Lagged` items silently and terminates on
/// `RecvError::Closed` — dropping the stream drops the guard and
/// `DecRef`s upstream.
pub struct GuardedStream<T> {
    inner: BroadcastStream<Arc<T>>,
    _guard: Box<dyn Guard>,
}

impl<T: Clone + Send + Sync + 'static> Stream for GuardedStream<T> {
    type Item = Arc<T>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            match Pin::new(&mut self.inner).poll_next(cx) {
                Poll::Ready(Some(Ok(v))) => return Poll::Ready(Some(v)),
                Poll::Ready(Some(Err(_lagged))) => continue, // skip Lagged
                Poll::Ready(None) => return Poll::Ready(None),
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

impl<T> std::fmt::Debug for GuardedStream<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GuardedStream").finish()
    }
}
