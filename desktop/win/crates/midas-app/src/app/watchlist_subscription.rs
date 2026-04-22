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

use std::sync::Arc;

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

    /// Non-blocking probe — whether the watch has a pending new
    /// value or, on `Err(Closed)`, whether the sender was dropped
    /// (S8 §F). Distinguishes "nothing new" (`Ok(false)`) from
    /// "watch is gone" (`Err(Closed)`) without consuming the
    /// pending-change flag — we still read the value via
    /// [`Self::snapshot`] afterwards.
    pub async fn has_changed(&self) -> Result<bool, tokio::sync::watch::error::RecvError> {
        let guard = self.inner.lock().await;
        guard.has_changed()
    }
}

/// Install a quote handle into the process-scoped
/// [`super::subscription_context::SubscriptionContext`]. No-op if the
/// context hasn't been installed yet.
pub fn install_quote_handle(sym: SymbolKey, handle: QuoteHandle) {
    if let Some(ctx) = super::subscription_context::current() {
        ctx.watchlist.insert(sym, Arc::new(QuoteEntry::new(handle)));
    }
}

pub fn remove_quote_handle(sym: &SymbolKey) {
    if let Some(ctx) = super::subscription_context::current() {
        ctx.watchlist.remove(sym);
    }
}

pub fn get_quote_handle(sym: &SymbolKey) -> Option<Arc<QuoteEntry>> {
    super::subscription_context::current().and_then(|ctx| ctx.watchlist.get(sym).map(|r| r.clone()))
}

/// Hashable key for `Subscription::run_with`. M-7: sort the
/// symbol list so the key is stable across re-renders.
#[derive(Clone, Debug, Hash, Eq, PartialEq)]
pub struct WatchlistSubKey {
    pub symbols: Vec<SymbolKey>,
}

/// Prune `entries` and the parallel `last` vec to drop every
/// `(sym, _)` whose sym is in `closed`. The two vecs stay
/// index-aligned: `last[i]` corresponds to `entries[i]`.
///
/// Extracted so the "storm-prevention" invariant is unit-testable
/// without having to stand up the full iced stream + router +
/// SimMarketData trio.
fn prune_closed(
    entries: &mut Vec<(SymbolKey, Arc<QuoteEntry>)>,
    last: &mut Vec<Option<Quote>>,
    closed: &[SymbolKey],
) {
    if closed.is_empty() {
        return;
    }
    let closed_set: std::collections::HashSet<&SymbolKey> = closed.iter().collect();
    let mut new_entries: Vec<(SymbolKey, Arc<QuoteEntry>)> = Vec::with_capacity(entries.len());
    let mut new_last: Vec<Option<Quote>> = Vec::with_capacity(last.len());
    for (idx, (sym, entry)) in entries.drain(..).enumerate() {
        if closed_set.contains(&sym) {
            continue;
        }
        new_entries.push((sym, entry));
        new_last.push(last[idx].clone());
    }
    *entries = new_entries;
    *last = new_last;
}

pub fn watchlist_stream_builder(
    key: &WatchlistSubKey,
) -> impl iced::futures::Stream<Item = Message> {
    let key = key.clone();
    iced::stream::channel(256, async move |mut output| {
        // Lazy-subscribe missing handles via the process-scoped
        // router slot, same pattern as `chart_stream_builder`.
        let Some(router) = super::subscription_registry::router() else {
            return;
        };
        let mut entries: Vec<(SymbolKey, Arc<QuoteEntry>)> = Vec::with_capacity(key.symbols.len());
        for sym in &key.symbols {
            if let Some(e) = get_quote_handle(sym) {
                entries.push((sym.clone(), e));
                continue;
            }
            match router.last_quote(sym.clone()).await {
                Ok(handle) => {
                    install_quote_handle(sym.clone(), handle);
                    if let Some(e) = get_quote_handle(sym) {
                        entries.push((sym.clone(), e));
                    }
                }
                Err(e) => {
                    tracing::warn!(symbol = %sym.symbol, "last_quote failed: {e}");
                }
            }
        }
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
            let mut closed: Vec<SymbolKey> = Vec::new();
            for (idx, (sym, entry)) in entries.iter().enumerate() {
                // S8 §F: probe `has_changed()` before reading the
                // snapshot so we can detect the rare case where the
                // router dropped the watch sender (e.g. every
                // consumer DecRef'd and the publisher was torn
                // down). On `Err(Closed)` emit `QuoteResync` so the
                // handler can re-open via `last_quote`.
                match entry.has_changed().await {
                    Ok(_) => {}
                    Err(_) => {
                        closed.push(sym.clone());
                        continue;
                    }
                }
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
            if !closed.is_empty() {
                // Prune the closed entries from BOTH `entries` and
                // the parallel `last` vec so the next 50 ms tick
                // doesn't re-detect them and emit another
                // `QuoteResync`. Without this prune the loop storms
                // the handler with resync messages at the poll
                // cadence.
                prune_closed(&mut entries, &mut last, &closed);

                for sym in closed {
                    // Drop the stale registry entry so the next
                    // iced-diff pass doesn't keep handing out the
                    // closed receiver.
                    remove_quote_handle(&sym);
                    if output
                        .send(Message::QuoteResync {
                            symbol: sym.clone(),
                        })
                        .await
                        .is_err()
                    {
                        return;
                    }
                }

                // Nothing left to poll — exit the subscription task.
                // An iced re-diff will rebuild us if the caller
                // re-issues `last_quote` for the symbol.
                if entries.is_empty() {
                    return;
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    //! Pruning-invariant coverage for Bug 2 (QuoteResync storm).
    use super::*;
    use midas_market_data::MarketDataRouter;
    use tokio::runtime::Runtime;

    fn build_router() -> Arc<MarketDataRouter> {
        use midas_broker::sim::{SimMarketData, SimMarketDataConfig};
        let sim = SimMarketData::new(SimMarketDataConfig {
            farm_up_delay_ms: 1,
            burst_enabled: false,
            tick_drift_bps: 0.0,
            tick_cadence_ms: 60_000,
            ..SimMarketDataConfig::default()
        });
        let src: Arc<dyn midas_broker::MarketDataSource> = sim.clone();
        MarketDataRouter::new(src)
    }

    async fn make_entry(router: &Arc<MarketDataRouter>, sym: &str) -> Arc<QuoteEntry> {
        let key = SymbolKey {
            contract_id: 0,
            symbol: sym.to_string(),
        };
        let handle = router.last_quote(key).await.expect("last_quote");
        Arc::new(QuoteEntry::new(handle))
    }

    #[test]
    fn prune_closed_drops_matching_entries_and_keeps_others_index_aligned() {
        let rt = Runtime::new().expect("tokio rt");
        rt.block_on(async {
            let router = build_router();
            let aapl = SymbolKey {
                contract_id: 0,
                symbol: "AAPL_WL_T1".to_string(),
            };
            let msft = SymbolKey {
                contract_id: 0,
                symbol: "MSFT_WL_T1".to_string(),
            };
            let tsla = SymbolKey {
                contract_id: 0,
                symbol: "TSLA_WL_T1".to_string(),
            };
            let mut entries = vec![
                (aapl.clone(), make_entry(&router, "AAPL_WL_T1").await),
                (msft.clone(), make_entry(&router, "MSFT_WL_T1").await),
                (tsla.clone(), make_entry(&router, "TSLA_WL_T1").await),
            ];
            // Populate `last` with a unique tag per slot so we can
            // verify the parallel vec stays index-aligned with
            // `entries` after prune.
            let mut last: Vec<Option<Quote>> = vec![
                Some(Quote {
                    bid: Some(1.0),
                    ..Quote::default()
                }),
                Some(Quote {
                    bid: Some(2.0),
                    ..Quote::default()
                }),
                Some(Quote {
                    bid: Some(3.0),
                    ..Quote::default()
                }),
            ];

            prune_closed(&mut entries, &mut last, std::slice::from_ref(&msft));

            assert_eq!(entries.len(), 2);
            assert_eq!(entries[0].0, aapl);
            assert_eq!(entries[1].0, tsla);
            // Parallel vec must stay in lock-step with `entries`.
            assert_eq!(last.len(), 2);
            assert_eq!(last[0].as_ref().and_then(|q| q.bid), Some(1.0));
            assert_eq!(last[1].as_ref().and_then(|q| q.bid), Some(3.0));
        });
    }

    #[test]
    fn prune_closed_is_noop_when_closed_list_is_empty() {
        let rt = Runtime::new().expect("tokio rt");
        rt.block_on(async {
            let router = build_router();
            let aapl = SymbolKey {
                contract_id: 0,
                symbol: "AAPL_WL_T2".to_string(),
            };
            let mut entries = vec![(aapl.clone(), make_entry(&router, "AAPL_WL_T2").await)];
            let mut last: Vec<Option<Quote>> = vec![None];
            prune_closed(&mut entries, &mut last, &[]);
            assert_eq!(entries.len(), 1);
            assert_eq!(last.len(), 1);
        });
    }

    #[test]
    fn prune_closed_can_empty_entries_for_all_match() {
        let rt = Runtime::new().expect("tokio rt");
        rt.block_on(async {
            let router = build_router();
            let aapl = SymbolKey {
                contract_id: 0,
                symbol: "AAPL_WL_T3".to_string(),
            };
            let mut entries = vec![(aapl.clone(), make_entry(&router, "AAPL_WL_T3").await)];
            let mut last: Vec<Option<Quote>> = vec![None];
            prune_closed(&mut entries, &mut last, std::slice::from_ref(&aapl));
            assert!(entries.is_empty());
            assert!(last.is_empty());
        });
    }
}
