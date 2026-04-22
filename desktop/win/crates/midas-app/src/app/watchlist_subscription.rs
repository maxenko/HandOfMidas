// S7c: watchlist quote subscription stream.
//
// One aggregated iced subscription over the union of every
// watchlist's tickers. Inside the stream we open a `QuoteHandle`
// per symbol (via a static registry populated at subscribe time)
// and poll `has_changed()` every 50 ms, batching changed quotes
// into `Message::QuoteBatch`.
//
// Unlike charts/tickers, the watchlist registry stores the full
// `QuoteHandle` rather than a handle-to-handle wrapper — the
// `watch::Receiver<Quote>` is already clonable by design.
//
// Legacy coexistence: the existing `BrokerEvent::Tick` arm still
// updates `market_cache` through the old broker bridge. The
// router-era path (`QuoteBatch`) walks the same cache; last-write
// wins per cell.

#![allow(dead_code)]

use std::sync::{Arc, LazyLock};

use dashmap::DashMap;
use iced::futures::SinkExt;
use midas_broker_core::market_data::Quote;
use midas_broker_core::SymbolKey;
use midas_market_data::QuoteHandle;
use tokio::sync::Mutex;

use super::subscription_helpers::WATCHLIST_POLL_MS;
use crate::app::Message;

/// Registry entry for a watchlist quote subscription. Holds the
/// `QuoteHandle` behind a mutex so multiple subscription restarts
/// (iced re-diff) can all borrow the same underlying watch via
/// `watch::Receiver::clone()`.
pub struct QuoteEntry {
    inner: Mutex<QuoteHandle>,
}

impl QuoteEntry {
    pub fn new(handle: QuoteHandle) -> Self {
        Self {
            inner: Mutex::new(handle),
        }
    }

    /// Read the current snapshot under the mutex.
    pub async fn snapshot(&self) -> Quote {
        let guard = self.inner.lock().await;
        let q = guard.borrow().clone();
        q
    }

    /// Wait for the next change on the underlying watch. Drops the
    /// mutex while waiting so peer subscribers can also poll.
    pub async fn wait_changed(&self) -> Result<(), tokio::sync::watch::error::RecvError> {
        let mut guard = self.inner.lock().await;
        guard.changed().await
    }
}

/// Global watchlist quote registry. Keyed on broker-core
/// `SymbolKey` (wire form). Entries are inserted when the app first
/// binds the router + a watchlist symbol; removed when the last
/// watchlist panel drops the symbol.
pub static WATCHLIST_REGISTRY: LazyLock<DashMap<SymbolKey, Arc<QuoteEntry>>> =
    LazyLock::new(DashMap::new);

pub fn install_quote_handle(sym: SymbolKey, handle: QuoteHandle) {
    WATCHLIST_REGISTRY.insert(sym, Arc::new(QuoteEntry::new(handle)));
}

pub fn remove_quote_handle(sym: &SymbolKey) {
    WATCHLIST_REGISTRY.remove(sym);
}

pub fn get_quote_handle(sym: &SymbolKey) -> Option<Arc<QuoteEntry>> {
    WATCHLIST_REGISTRY.get(sym).map(|r| r.clone())
}

/// Hashable key for `Subscription::run_with`. M-7: sort the
/// symbol list so the key is stable across re-renders.
#[derive(Clone, Debug, Hash, Eq, PartialEq)]
pub struct WatchlistSubKey {
    pub symbols: Vec<SymbolKey>,
}

pub fn watchlist_stream_builder(
    key: &WatchlistSubKey,
) -> impl iced::futures::Stream<Item = Message> {
    let key = key.clone();
    iced::stream::channel(256, async move |mut output| {
        // Snapshot entries up front; symbols missing from the
        // registry are skipped, not failed — the bind path will
        // install them and iced will re-diff on the next frame.
        let entries: Vec<(SymbolKey, Arc<QuoteEntry>)> = key
            .symbols
            .iter()
            .filter_map(|sym| get_quote_handle(sym).map(|e| (sym.clone(), e)))
            .collect();
        if entries.is_empty() {
            return;
        }
        let mut interval = tokio::time::interval(WATCHLIST_POLL_MS);
        // Track the last quote we emitted per symbol so we only
        // ship a (symbol, quote) pair when it has actually changed.
        let mut last: Vec<Option<Quote>> = vec![None; entries.len()];
        loop {
            interval.tick().await;
            let mut batch: Vec<(SymbolKey, Quote)> = Vec::new();
            for (idx, (sym, entry)) in entries.iter().enumerate() {
                let q = entry.snapshot().await;
                let should_emit = match &last[idx] {
                    None => true,
                    Some(prev) => prev != &q,
                };
                if should_emit {
                    last[idx] = Some(q.clone());
                    batch.push((sym.clone(), q));
                }
            }
            if !batch.is_empty() && output.send(Message::QuoteBatch(batch)).await.is_err() {
                break;
            }
        }
    })
}
