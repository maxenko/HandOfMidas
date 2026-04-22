//! Control-plane actor for [`MarketDataRouter`].
//!
//! The actor serialises every subscribe / unsubscribe / decref /
//! shutdown request (BR-4). Handle construction is done by the actor
//! itself so the caller cannot observe a refcount-without-guard state
//! (see §C of `06-slice-5-router.md`).
//!
//! Source-failure rollback (NM-3): when the first subscribe for a
//! symbol fails the upstream call, the actor replies `Err(...)` and
//! inserts NOTHING into `per_symbol`. No half-registered state
//! survives a source failure.
//!
//! [`MarketDataRouter`]: crate::router::MarketDataRouter

use std::sync::atomic::Ordering;
use std::sync::Arc;

use midas_broker_core::market_data::{
    Bar, MarketDataError, SecurityType, SymbolKey, Tick, WhatToShow,
};
use tokio::sync::{mpsc, oneshot};

use super::handle::{
    Guard, QuoteHandle, RtBarSubGuard, SubscriptionHandle, TickSubGuard, WatchGuard,
};
use super::publisher::{run_rt_bar_publisher, run_tick_publisher};
use super::state::{RouterState, SymbolHub};

/// Single debug-dump snapshot entry (M-28).
#[derive(Debug, Clone)]
pub struct SymbolDebugInfo {
    /// Symbol this row describes.
    pub symbol: SymbolKey,
    /// Live `SubscriptionHandle<Tick>` count.
    pub tick_refcount: u32,
    /// Live `SubscriptionHandle<Bar>` count (RT bars).
    pub rt_bar_refcount: u32,
    /// Live `QuoteHandle` count.
    pub watch_refcount: u32,
    /// Wall-clock-ms timestamp of the most recent tick, or 0 if none.
    pub last_tick_ts_ms: i64,
    /// Whether the tick publisher task is still spawned.
    pub tick_publisher_alive: bool,
    /// Whether the rt-bar publisher task is still spawned.
    pub rt_bar_publisher_alive: bool,
}

/// Messages the control actor consumes.
///
/// `SubscribeTicks` / `SubscribeRtBars` carry a `oneshot::Sender` that
/// ships the fully-formed [`SubscriptionHandle`] back to the caller,
/// so the refcount increment and the guard construction happen inside
/// the actor with no visible window between them.
pub(crate) enum RouterMsg {
    /// First / subsequent subscribe to tick fan-out.
    SubscribeTicks {
        symbol: SymbolKey,
        reply: oneshot::Sender<Result<SubscriptionHandle<Tick>, MarketDataError>>,
    },
    /// First / subsequent subscribe to realtime-bar fan-out.
    SubscribeRtBars {
        symbol: SymbolKey,
        reply: oneshot::Sender<Result<SubscriptionHandle<Bar>, MarketDataError>>,
    },
    /// Open (or reuse) the quote watch for a symbol.
    ///
    /// Replies with a pre-wrapped [`QuoteHandle`] on success.
    OpenHubForWatch {
        symbol: SymbolKey,
        reply: oneshot::Sender<Result<QuoteHandle, MarketDataError>>,
    },
    DecTickRef {
        symbol: SymbolKey,
    },
    DecRtBarRef {
        symbol: SymbolKey,
    },
    DecWatchRef {
        symbol: SymbolKey,
    },
    DebugDump {
        reply: oneshot::Sender<Vec<SymbolDebugInfo>>,
    },
    Shutdown,
}

/// Run the actor loop. Terminates on `Shutdown` or when the mpsc is
/// closed (all router handles dropped).
pub(crate) async fn run_control_actor(
    mut rx: mpsc::UnboundedReceiver<RouterMsg>,
    state: Arc<RouterState>,
    control: mpsc::UnboundedSender<RouterMsg>,
) {
    while let Some(msg) = rx.recv().await {
        match msg {
            RouterMsg::SubscribeTicks { symbol, reply } => {
                let result = handle_subscribe_ticks(&state, &control, &symbol).await;
                let _ = reply.send(result);
            }
            RouterMsg::SubscribeRtBars { symbol, reply } => {
                let result = handle_subscribe_rt_bars(&state, &control, &symbol).await;
                let _ = reply.send(result);
            }
            RouterMsg::OpenHubForWatch { symbol, reply } => {
                let result = handle_open_hub_for_watch(&state, &control, &symbol).await;
                let _ = reply.send(result);
            }
            RouterMsg::DecTickRef { symbol } => {
                handle_dec_tick(&state, &symbol);
            }
            RouterMsg::DecRtBarRef { symbol } => {
                handle_dec_rt_bar(&state, &symbol);
            }
            RouterMsg::DecWatchRef { symbol } => {
                handle_dec_watch(&state, &symbol);
            }
            RouterMsg::DebugDump { reply } => {
                let snap = collect_debug_dump(&state);
                let _ = reply.send(snap);
            }
            RouterMsg::Shutdown => {
                tracing::info!("router actor shutdown received; aborting publishers");
                abort_all_publishers(&state);
                break;
            }
        }
    }
    // Final cleanup on channel close.
    abort_all_publishers(&state);
    tracing::debug!("router actor exited");
}

/// Resolve the symbol's contract id, first against the cache, then
/// the upstream source (NM-1). Caches the result on success.
async fn resolve_con_id(state: &RouterState, sym: &SymbolKey) -> Result<i32, MarketDataError> {
    if let Some(c) = state.contract_cache.get(sym) {
        return Ok(c.contract_id);
    }
    let details = state
        .source
        .resolve_contract(sym, SecurityType::Stock, "SMART")
        .await?;
    let con_id = details.contract_id;
    state.contract_cache.insert(sym.clone(), details);
    Ok(con_id)
}

async fn handle_subscribe_ticks(
    state: &Arc<RouterState>,
    control: &mpsc::UnboundedSender<RouterMsg>,
    symbol: &SymbolKey,
) -> Result<SubscriptionHandle<Tick>, MarketDataError> {
    let span = tracing::info_span!("router.subscribe_ticks", symbol = %symbol);
    let _enter = span.enter();

    // Reuse an existing hub if any.
    if let Some(hub_entry) = state.per_symbol.get(symbol) {
        let hub = hub_entry.clone();
        drop(hub_entry);
        hub.tick_refcount.fetch_add(1, Ordering::Relaxed);
        // If the hub exists but no tick publisher is running (pure
        // rt-bar / watch-only hub), we still need one.
        ensure_tick_publisher(state, &hub).await?;
        let rx = hub.ticks_tx.subscribe();
        let guard: Box<dyn Guard> = Box::new(TickSubGuard {
            symbol: symbol.clone(),
            control: control.clone(),
        });
        return Ok(SubscriptionHandle::new(rx, guard));
    }

    // First subscribe for this symbol. NM-3: call source FIRST; on
    // Err, insert nothing.
    let con_id = match resolve_con_id(state, symbol).await {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(symbol = %symbol, error = %e, "resolve_contract failed; rolling back");
            return Err(e);
        }
    };

    let upstream = match state
        .source
        .subscribe_ticks(symbol, con_id, Default::default())
        .await
    {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(symbol = %symbol, error = %e, "source.subscribe_ticks failed; rolling back");
            return Err(e);
        }
    };

    let hub = SymbolHub::new(symbol.clone());
    hub.tick_refcount.store(1, Ordering::Relaxed);
    let hub_for_task = hub.clone();
    let handle = tokio::spawn(async move { run_tick_publisher(hub_for_task, upstream).await });
    *hub.tick_publisher_task.lock().unwrap() = Some(handle);
    state.per_symbol.insert(symbol.clone(), hub.clone());

    let rx = hub.ticks_tx.subscribe();
    let guard: Box<dyn Guard> = Box::new(TickSubGuard {
        symbol: symbol.clone(),
        control: control.clone(),
    });
    Ok(SubscriptionHandle::new(rx, guard))
}

/// Ensure a tick publisher task is running on `hub`. Idempotent.
///
/// Used when a hub already exists (created by e.g. an RT-bar-only
/// path or a watch-only path) but no tick publisher is running yet.
async fn ensure_tick_publisher(
    state: &Arc<RouterState>,
    hub: &Arc<SymbolHub>,
) -> Result<(), MarketDataError> {
    // Fast path: a publisher is already running.
    {
        let slot = hub.tick_publisher_task.lock().unwrap();
        if let Some(h) = slot.as_ref() {
            if !h.is_finished() {
                return Ok(());
            }
        }
    }
    // Need to spawn. Resolve + subscribe + stash.
    let con_id = resolve_con_id(state, &hub.symbol).await?;
    let upstream = state
        .source
        .subscribe_ticks(&hub.symbol, con_id, Default::default())
        .await?;
    let hub_for_task = hub.clone();
    let handle = tokio::spawn(async move { run_tick_publisher(hub_for_task, upstream).await });
    *hub.tick_publisher_task.lock().unwrap() = Some(handle);
    Ok(())
}

async fn handle_subscribe_rt_bars(
    state: &Arc<RouterState>,
    control: &mpsc::UnboundedSender<RouterMsg>,
    symbol: &SymbolKey,
) -> Result<SubscriptionHandle<Bar>, MarketDataError> {
    let span = tracing::info_span!("router.subscribe_rt_bars", symbol = %symbol);
    let _enter = span.enter();

    // If the hub already exists but no RT-bar publisher, we need to
    // lazily initialise the RT path.
    if let Some(hub_entry) = state.per_symbol.get(symbol) {
        let hub = hub_entry.clone();
        drop(hub_entry);
        // Is the rt-bar publisher already running?
        let need_spawn = {
            let slot = hub.rt_bar_publisher_task.lock().unwrap();
            slot.as_ref().map(|h| h.is_finished()).unwrap_or(true)
        };
        if need_spawn {
            let con_id = resolve_con_id(state, symbol).await?;
            let upstream = state
                .source
                .subscribe_realtime_bars(symbol, con_id, WhatToShow::Trades)
                .await?;
            hub.ensure_rt_bars_tx();
            let hub_for_task = hub.clone();
            let handle =
                tokio::spawn(async move { run_rt_bar_publisher(hub_for_task, upstream).await });
            *hub.rt_bar_publisher_task.lock().unwrap() = Some(handle);
        }
        hub.rt_bar_refcount.fetch_add(1, Ordering::Relaxed);
        let tx = hub.ensure_rt_bars_tx();
        let rx = tx.subscribe();
        let guard: Box<dyn Guard> = Box::new(RtBarSubGuard {
            symbol: symbol.clone(),
            control: control.clone(),
        });
        return Ok(SubscriptionHandle::new(rx, guard));
    }

    // First subscribe for this symbol. NM-3 rollback order.
    let con_id = resolve_con_id(state, symbol).await?;
    let upstream = state
        .source
        .subscribe_realtime_bars(symbol, con_id, WhatToShow::Trades)
        .await?;

    let hub = SymbolHub::new(symbol.clone());
    hub.rt_bar_refcount.store(1, Ordering::Relaxed);
    let tx = hub.ensure_rt_bars_tx();
    let rx = tx.subscribe();
    let hub_for_task = hub.clone();
    let handle = tokio::spawn(async move { run_rt_bar_publisher(hub_for_task, upstream).await });
    *hub.rt_bar_publisher_task.lock().unwrap() = Some(handle);
    state.per_symbol.insert(symbol.clone(), hub.clone());

    let guard: Box<dyn Guard> = Box::new(RtBarSubGuard {
        symbol: symbol.clone(),
        control: control.clone(),
    });
    Ok(SubscriptionHandle::new(rx, guard))
}

async fn handle_open_hub_for_watch(
    state: &Arc<RouterState>,
    control: &mpsc::UnboundedSender<RouterMsg>,
    symbol: &SymbolKey,
) -> Result<QuoteHandle, MarketDataError> {
    // Reuse the existing hub if any.
    if let Some(hub_entry) = state.per_symbol.get(symbol) {
        let hub = hub_entry.clone();
        drop(hub_entry);
        hub.watch_refcount.fetch_add(1, Ordering::Relaxed);
        // Make sure a tick publisher is running so the watch keeps
        // updating (NB-3 lazy-open).
        ensure_tick_publisher(state, &hub).await?;
        let rx = hub.last_quote_tx.subscribe();
        let guard = WatchGuard {
            symbol: symbol.clone(),
            control: control.clone(),
        };
        return Ok(QuoteHandle::new(rx, guard));
    }

    // First watcher on this symbol — lazy-open a tick publisher.
    let con_id = resolve_con_id(state, symbol).await?;
    let upstream = state
        .source
        .subscribe_ticks(symbol, con_id, Default::default())
        .await?;

    let hub = SymbolHub::new(symbol.clone());
    hub.watch_refcount.store(1, Ordering::Relaxed);
    let hub_for_task = hub.clone();
    let handle = tokio::spawn(async move { run_tick_publisher(hub_for_task, upstream).await });
    *hub.tick_publisher_task.lock().unwrap() = Some(handle);
    let rx = hub.last_quote_tx.subscribe();
    state.per_symbol.insert(symbol.clone(), hub);
    let guard = WatchGuard {
        symbol: symbol.clone(),
        control: control.clone(),
    };
    Ok(QuoteHandle::new(rx, guard))
}

fn handle_dec_tick(state: &RouterState, symbol: &SymbolKey) {
    let Some(hub_entry) = state.per_symbol.get(symbol) else {
        return;
    };
    let hub = hub_entry.clone();
    drop(hub_entry);
    let prev = hub.tick_refcount.fetch_sub(1, Ordering::Relaxed);
    if prev == 0 {
        // Underflow — log and carry on.
        tracing::warn!(symbol = %symbol, "tick refcount underflow on dec");
        hub.tick_refcount.store(0, Ordering::Relaxed);
        return;
    }
    if prev == 1 {
        // Hit zero ticks. If there is also no watcher, kill the tick
        // publisher. If rt-bars are also idle, drop the hub entirely.
        maybe_reap(state, &hub);
    }
}

fn handle_dec_rt_bar(state: &RouterState, symbol: &SymbolKey) {
    let Some(hub_entry) = state.per_symbol.get(symbol) else {
        return;
    };
    let hub = hub_entry.clone();
    drop(hub_entry);
    let prev = hub.rt_bar_refcount.fetch_sub(1, Ordering::Relaxed);
    if prev == 0 {
        tracing::warn!(symbol = %symbol, "rt-bar refcount underflow on dec");
        hub.rt_bar_refcount.store(0, Ordering::Relaxed);
        return;
    }
    if prev == 1 {
        maybe_reap(state, &hub);
    }
}

fn handle_dec_watch(state: &RouterState, symbol: &SymbolKey) {
    let Some(hub_entry) = state.per_symbol.get(symbol) else {
        return;
    };
    let hub = hub_entry.clone();
    drop(hub_entry);
    let prev = hub.watch_refcount.fetch_sub(1, Ordering::Relaxed);
    if prev == 0 {
        tracing::warn!(symbol = %symbol, "watch refcount underflow on dec");
        hub.watch_refcount.store(0, Ordering::Relaxed);
        return;
    }
    if prev == 1 {
        maybe_reap(state, &hub);
    }
}

/// If every refcount is zero, abort publishers and drop the hub from
/// the map. Partial states are fine — if e.g. tick_refcount==0 but
/// rt_bar_refcount>0, we leave the tick publisher alone (it will
/// auto-exit on its own streak) but do NOT remove the hub from the
/// map.
fn maybe_reap(state: &RouterState, hub: &Arc<SymbolHub>) {
    let ticks = hub.tick_refcount.load(Ordering::Relaxed);
    let watches = hub.watch_refcount.load(Ordering::Relaxed);
    let rt_bars = hub.rt_bar_refcount.load(Ordering::Relaxed);

    // Abort the tick publisher once both broadcast AND watch are idle
    // (NB-3). If rt-bars are still live, the hub stays in the map.
    if ticks == 0 && watches == 0 {
        if let Some(h) = hub.tick_publisher_task.lock().unwrap().take() {
            tracing::info!(symbol = %hub.symbol, "aborting tick publisher on last DecRef");
            h.abort();
        }
    }

    if rt_bars == 0 {
        if let Some(h) = hub.rt_bar_publisher_task.lock().unwrap().take() {
            tracing::info!(symbol = %hub.symbol, "aborting rt-bar publisher on last DecRef");
            h.abort();
        }
    }

    if ticks == 0 && watches == 0 && rt_bars == 0 {
        state.per_symbol.remove(&hub.symbol);
        tracing::debug!(symbol = %hub.symbol, "hub removed after last DecRef");
    }
}

fn abort_all_publishers(state: &RouterState) {
    for entry in state.per_symbol.iter() {
        let hub = entry.value();
        if let Some(h) = hub.tick_publisher_task.lock().unwrap().take() {
            h.abort();
        }
        if let Some(h) = hub.rt_bar_publisher_task.lock().unwrap().take() {
            h.abort();
        }
    }
    state.per_symbol.clear();
}

fn collect_debug_dump(state: &RouterState) -> Vec<SymbolDebugInfo> {
    state
        .per_symbol
        .iter()
        .map(|entry| {
            let hub = entry.value();
            let tick_alive = hub
                .tick_publisher_task
                .lock()
                .unwrap()
                .as_ref()
                .map(|h| !h.is_finished())
                .unwrap_or(false);
            let rt_alive = hub
                .rt_bar_publisher_task
                .lock()
                .unwrap()
                .as_ref()
                .map(|h| !h.is_finished())
                .unwrap_or(false);
            SymbolDebugInfo {
                symbol: hub.symbol.clone(),
                tick_refcount: hub.tick_refcount.load(Ordering::Relaxed),
                rt_bar_refcount: hub.rt_bar_refcount.load(Ordering::Relaxed),
                watch_refcount: hub.watch_refcount.load(Ordering::Relaxed),
                last_tick_ts_ms: hub.last_tick_ts.load(Ordering::Relaxed),
                tick_publisher_alive: tick_alive,
                rt_bar_publisher_alive: rt_alive,
            }
        })
        .collect()
}
