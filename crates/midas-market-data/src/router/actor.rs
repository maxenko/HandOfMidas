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
use std::time::Duration;

use midas_broker_core::market_data::{
    Bar, EndReason, MarketDataError, SecurityType, SymbolKey, Tick, WhatToShow,
};
use tokio::sync::{mpsc, oneshot};

use super::handle::{
    Guard, GuardCtrl, QuoteHandle, RtBarSubGuard, SubscriptionHandle, TickSubGuard, WatchGuard,
};
use super::publisher::{run_rt_bar_publisher, run_tick_publisher};
use super::state::{RouterState, SymbolHub};

/// Per-handler upstream-await budget. If a provider call
/// (`resolve_contract`, `subscribe_ticks`, `subscribe_realtime_bars`)
/// hangs past this deadline, the actor returns
/// [`MarketDataError::Other`] instead of wedging indefinitely. A wedged
/// actor would back up every subsequent control message (subscribe,
/// dec-ref, shutdown) — P0 wedge guard.
pub(crate) const ROUTER_ACTOR_OP_TIMEOUT: Duration = Duration::from_secs(10);

/// Backlog warn threshold on the control-plane mpsc. A healthy router
/// keeps pending messages << 100; if we see the counter cross this
/// line we log once at `warn` so the operator knows the actor is
/// struggling before tests / users start to observe latency.
pub(crate) const ROUTER_BACKLOG_WARN: usize = 1000;

/// Run a fallible upstream call under the per-handler deadline.
///
/// On `Elapsed` we synthesise a `MarketDataError::Other` carrying the
/// call label so logs distinguish `resolve_contract timeout` from
/// `subscribe_ticks timeout`.
async fn with_op_timeout<F, T>(label: &'static str, fut: F) -> Result<T, MarketDataError>
where
    F: std::future::Future<Output = Result<T, MarketDataError>>,
{
    match tokio::time::timeout(ROUTER_ACTOR_OP_TIMEOUT, fut).await {
        Ok(inner) => inner,
        Err(_) => Err(MarketDataError::Other(format!(
            "router: upstream {label} timed out after {:?}",
            ROUTER_ACTOR_OP_TIMEOUT
        ))),
    }
}

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
    /// Publisher observed `RecvError::Closed` on its upstream stream
    /// (slice B2). The actor flips the per-hub end-reason watch,
    /// emits a structured `warn!`, aborts any sibling publisher, and
    /// drops the hub from `per_symbol`.
    UpstreamClosed {
        symbol: SymbolKey,
        reason: EndReason,
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
    // Assemble the guard context once — it's what every
    // `SubscriptionHandle` will carry so DecRef sends stay balanced
    // with the actor's post-recv decrement.
    let guard_ctrl = GuardCtrl {
        control: control.clone(),
        backlog: Arc::clone(&state.backlog),
        backlog_warned: Arc::clone(&state.backlog_warned),
    };
    while let Some(msg) = rx.recv().await {
        // Keep the backlog counter balanced with the sender-side
        // bumps. Every `send_control` / `bump_backlog` paired on the
        // send path decrements here as the actor drains.
        state.backlog.fetch_sub(1, Ordering::Relaxed);
        match msg {
            RouterMsg::SubscribeTicks { symbol, reply } => {
                let result = handle_subscribe_ticks(&state, &guard_ctrl, &symbol).await;
                let _ = reply.send(result);
            }
            RouterMsg::SubscribeRtBars { symbol, reply } => {
                let result = handle_subscribe_rt_bars(&state, &guard_ctrl, &symbol).await;
                let _ = reply.send(result);
            }
            RouterMsg::OpenHubForWatch { symbol, reply } => {
                let result = handle_open_hub_for_watch(&state, &guard_ctrl, &symbol).await;
                let _ = reply.send(result);
            }
            RouterMsg::DecTickRef { symbol } => handle_dec_ref(&state, &symbol, DecKind::Tick),
            RouterMsg::DecRtBarRef { symbol } => handle_dec_ref(&state, &symbol, DecKind::RtBar),
            RouterMsg::DecWatchRef { symbol } => handle_dec_ref(&state, &symbol, DecKind::Watch),
            RouterMsg::DebugDump { reply } => {
                let snap = collect_debug_dump(&state);
                let _ = reply.send(snap);
            }
            RouterMsg::UpstreamClosed { symbol, reason } => {
                handle_upstream_closed(&state, &symbol, reason);
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
    let details = with_op_timeout(
        "resolve_contract",
        state
            .source
            .resolve_contract(sym, SecurityType::Stock, "SMART"),
    )
    .await?;
    let con_id = details.contract_id;
    state.contract_cache.insert(sym.clone(), details);
    Ok(con_id)
}

async fn handle_subscribe_ticks(
    state: &Arc<RouterState>,
    guard_ctrl: &GuardCtrl,
    symbol: &SymbolKey,
) -> Result<SubscriptionHandle<Tick>, MarketDataError> {
    let span = tracing::info_span!("router.subscribe_ticks", symbol = %symbol);
    let _enter = span.enter();

    // Reuse an existing hub if any.
    if let Some(hub_entry) = state.per_symbol.get(symbol) {
        let hub = hub_entry.clone();
        drop(hub_entry);
        // NB: we must not bump `tick_refcount` BEFORE
        // `ensure_tick_publisher.await?` — on an upstream error the
        // `?` returns without constructing a SubscriptionHandle, so
        // no TickSubGuard exists to fire `DecTickRef` on drop and
        // the refcount would leak forever. Order: fallible work
        // first, IncRef + handle construction last.
        // If the hub exists but no tick publisher is running (pure
        // rt-bar / watch-only hub), we still need one.
        ensure_tick_publisher(state, guard_ctrl, &hub).await?;
        hub.tick_refcount.fetch_add(1, Ordering::Relaxed);
        let rx = hub.ticks_tx.subscribe();
        let end_rx = hub.end_reason_tx.subscribe();
        return Ok(SubscriptionHandle::new(
            rx,
            tick_guard(symbol, guard_ctrl),
            end_rx,
        ));
    }

    // First subscribe for this symbol. NM-3: call source FIRST; on
    // Err, insert nothing.
    let con_id = resolve_con_id(state, symbol).await.inspect_err(
        |e| tracing::warn!(symbol = %symbol, error = %e, "resolve_contract failed; rolling back"),
    )?;
    let upstream = subscribe_ticks_upstream(state, symbol, con_id)
        .await
        .inspect_err(|e| tracing::warn!(symbol = %symbol, error = %e, "source.subscribe_ticks failed; rolling back"))?;

    let hub = SymbolHub::new(symbol.clone());
    hub.tick_refcount.store(1, Ordering::Relaxed);
    spawn_tick_publisher_on(&hub, upstream, guard_ctrl);
    state.per_symbol.insert(symbol.clone(), hub.clone());
    note_resubscribe_after_disconnect(state, symbol);

    let rx = hub.ticks_tx.subscribe();
    let end_rx = hub.end_reason_tx.subscribe();
    Ok(SubscriptionHandle::new(
        rx,
        tick_guard(symbol, guard_ctrl),
        end_rx,
    ))
}

/// Boxed `TickSubGuard` constructor — every tick-subscribe path needs
/// the same three-line `Box::new(TickSubGuard { … })` body.
fn tick_guard(symbol: &SymbolKey, guard_ctrl: &GuardCtrl) -> Box<dyn Guard> {
    Box::new(TickSubGuard {
        symbol: symbol.clone(),
        ctrl: guard_ctrl.clone(),
    })
}

/// Boxed `RtBarSubGuard` constructor.
fn rt_bar_guard(symbol: &SymbolKey, guard_ctrl: &GuardCtrl) -> Box<dyn Guard> {
    Box::new(RtBarSubGuard {
        symbol: symbol.clone(),
        ctrl: guard_ctrl.clone(),
    })
}

/// Run the upstream `subscribe_ticks` with default generic-ticks
/// under the actor's per-handler timeout.
async fn subscribe_ticks_upstream(
    state: &Arc<RouterState>,
    symbol: &SymbolKey,
    con_id: i32,
) -> Result<midas_broker_core::provider::TickStream, MarketDataError> {
    with_op_timeout(
        "subscribe_ticks",
        state
            .source
            .subscribe_ticks(symbol, con_id, Default::default()),
    )
    .await
}

/// Run the upstream `subscribe_realtime_bars` with `WhatToShow::Trades`
/// under the actor's per-handler timeout.
async fn subscribe_rt_bars_upstream(
    state: &Arc<RouterState>,
    symbol: &SymbolKey,
    con_id: i32,
) -> Result<midas_broker_core::provider::RealtimeBarStream, MarketDataError> {
    with_op_timeout(
        "subscribe_realtime_bars",
        state
            .source
            .subscribe_realtime_bars(symbol, con_id, WhatToShow::Trades),
    )
    .await
}

/// Spawn the tick publisher task against `hub` + `upstream` and stash
/// the join handle in `hub.tick_publisher_task`.
///
/// `guard_ctrl` is cloned into the publisher task so it can fire
/// `RouterMsg::UpstreamClosed` on a full upstream close (slice B2)
/// through the same balanced `bump_backlog` path the guards use.
fn spawn_tick_publisher_on(
    hub: &Arc<SymbolHub>,
    upstream: midas_broker_core::provider::TickStream,
    guard_ctrl: &GuardCtrl,
) {
    let hub_for_task = hub.clone();
    let ctrl_for_task = guard_ctrl.clone();
    let handle =
        tokio::spawn(
            async move { run_tick_publisher(hub_for_task, upstream, ctrl_for_task).await },
        );
    *hub.tick_publisher_task.lock() = Some(handle);
}

/// Spawn the RT-bar publisher task against `hub` + `upstream` and
/// stash the join handle in `hub.rt_bar_publisher_task`.
///
/// `guard_ctrl` is cloned into the publisher task so it can fire
/// `RouterMsg::UpstreamClosed` on a full upstream close (slice B2)
/// through the same balanced `bump_backlog` path the guards use.
fn spawn_rt_bar_publisher_on(
    hub: &Arc<SymbolHub>,
    upstream: midas_broker_core::provider::RealtimeBarStream,
    guard_ctrl: &GuardCtrl,
) {
    let hub_for_task = hub.clone();
    let ctrl_for_task = guard_ctrl.clone();
    let handle =
        tokio::spawn(
            async move { run_rt_bar_publisher(hub_for_task, upstream, ctrl_for_task).await },
        );
    *hub.rt_bar_publisher_task.lock() = Some(handle);
}

/// Ensure a tick publisher task is running on `hub`. Idempotent.
///
/// Used when a hub already exists (created by e.g. an RT-bar-only
/// path or a watch-only path) but no tick publisher is running yet.
async fn ensure_tick_publisher(
    state: &Arc<RouterState>,
    guard_ctrl: &GuardCtrl,
    hub: &Arc<SymbolHub>,
) -> Result<(), MarketDataError> {
    // Fast path: a publisher is already running.
    if hub
        .tick_publisher_task
        .lock()
        .as_ref()
        .is_some_and(|h| !h.is_finished())
    {
        return Ok(());
    }
    // Need to spawn. Resolve + subscribe + stash.
    let con_id = resolve_con_id(state, &hub.symbol).await?;
    let upstream = subscribe_ticks_upstream(state, &hub.symbol, con_id).await?;
    spawn_tick_publisher_on(hub, upstream, guard_ctrl);
    Ok(())
}

async fn handle_subscribe_rt_bars(
    state: &Arc<RouterState>,
    guard_ctrl: &GuardCtrl,
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
        let need_spawn = hub
            .rt_bar_publisher_task
            .lock()
            .as_ref()
            .is_none_or(|h| h.is_finished());
        if need_spawn {
            let con_id = resolve_con_id(state, symbol).await?;
            let upstream = subscribe_rt_bars_upstream(state, symbol, con_id).await?;
            hub.ensure_rt_bars_tx();
            spawn_rt_bar_publisher_on(&hub, upstream, guard_ctrl);
        }
        hub.rt_bar_refcount.fetch_add(1, Ordering::Relaxed);
        let rx = hub.ensure_rt_bars_tx().subscribe();
        let end_rx = hub.end_reason_tx.subscribe();
        return Ok(SubscriptionHandle::new(
            rx,
            rt_bar_guard(symbol, guard_ctrl),
            end_rx,
        ));
    }

    // First subscribe for this symbol. NM-3 rollback order.
    let con_id = resolve_con_id(state, symbol).await?;
    let upstream = subscribe_rt_bars_upstream(state, symbol, con_id).await?;

    let hub = SymbolHub::new(symbol.clone());
    hub.rt_bar_refcount.store(1, Ordering::Relaxed);
    let rx = hub.ensure_rt_bars_tx().subscribe();
    let end_rx = hub.end_reason_tx.subscribe();
    spawn_rt_bar_publisher_on(&hub, upstream, guard_ctrl);
    state.per_symbol.insert(symbol.clone(), hub.clone());
    note_resubscribe_after_disconnect(state, symbol);

    Ok(SubscriptionHandle::new(
        rx,
        rt_bar_guard(symbol, guard_ctrl),
        end_rx,
    ))
}

async fn handle_open_hub_for_watch(
    state: &Arc<RouterState>,
    guard_ctrl: &GuardCtrl,
    symbol: &SymbolKey,
) -> Result<QuoteHandle, MarketDataError> {
    // Reuse the existing hub if any.
    if let Some(hub_entry) = state.per_symbol.get(symbol) {
        let hub = hub_entry.clone();
        drop(hub_entry);
        // Same ordering constraint as `handle_subscribe_ticks`:
        // defer `fetch_add` until after the fallible publisher
        // spawn. On `Err(..)?` no `QuoteHandle` is returned to the
        // caller, so no `WatchGuard` exists to decrement on drop —
        // bumping the refcount before the await would leak it.
        // Make sure a tick publisher is running so the watch keeps
        // updating (NB-3 lazy-open).
        ensure_tick_publisher(state, guard_ctrl, &hub).await?;
        hub.watch_refcount.fetch_add(1, Ordering::Relaxed);
        let rx = hub.last_quote_tx.subscribe();
        return Ok(QuoteHandle::new(rx, watch_guard(symbol, guard_ctrl)));
    }

    // First watcher on this symbol — lazy-open a tick publisher.
    let con_id = resolve_con_id(state, symbol).await?;
    let upstream = subscribe_ticks_upstream(state, symbol, con_id).await?;

    let hub = SymbolHub::new(symbol.clone());
    hub.watch_refcount.store(1, Ordering::Relaxed);
    spawn_tick_publisher_on(&hub, upstream, guard_ctrl);
    let rx = hub.last_quote_tx.subscribe();
    state.per_symbol.insert(symbol.clone(), hub);
    note_resubscribe_after_disconnect(state, symbol);

    Ok(QuoteHandle::new(rx, watch_guard(symbol, guard_ctrl)))
}

/// `WatchGuard` constructor — QuoteHandle needs the concrete type (not
/// the boxed trait) so this keeps the call sites symmetric with
/// [`tick_guard`] / [`rt_bar_guard`].
fn watch_guard(symbol: &SymbolKey, guard_ctrl: &GuardCtrl) -> WatchGuard {
    WatchGuard {
        symbol: symbol.clone(),
        ctrl: guard_ctrl.clone(),
    }
}

/// Refcount field a `DecRef` message targets.
///
/// Each variant points at one of `SymbolHub`'s three `AtomicU32`
/// refcounts; [`handle_dec_ref`] dispatches the decrement-and-maybe-reap
/// logic uniformly so the three `DecRef` handlers share one body.
#[derive(Clone, Copy)]
enum DecKind {
    Tick,
    RtBar,
    Watch,
}

impl DecKind {
    fn refcount(self, hub: &SymbolHub) -> &std::sync::atomic::AtomicU32 {
        match self {
            Self::Tick => &hub.tick_refcount,
            Self::RtBar => &hub.rt_bar_refcount,
            Self::Watch => &hub.watch_refcount,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Tick => "tick",
            Self::RtBar => "rt-bar",
            Self::Watch => "watch",
        }
    }
}

/// Shared DecRef handler — decrements the chosen refcount, recovers
/// from underflow, and reaps the hub when the count hits zero.
fn handle_dec_ref(state: &RouterState, symbol: &SymbolKey, kind: DecKind) {
    let Some(hub_entry) = state.per_symbol.get(symbol) else {
        return;
    };
    let hub = hub_entry.clone();
    drop(hub_entry);
    let slot = kind.refcount(&hub);
    let prev = slot.fetch_sub(1, Ordering::Relaxed);
    if prev == 0 {
        tracing::warn!(symbol = %symbol, kind = kind.label(), "refcount underflow on dec");
        slot.store(0, Ordering::Relaxed);
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
        if let Some(h) = hub.tick_publisher_task.lock().take() {
            tracing::info!(symbol = %hub.symbol, "aborting tick publisher on last DecRef");
            h.abort();
        }
    }

    if rt_bars == 0 {
        if let Some(h) = hub.rt_bar_publisher_task.lock().take() {
            tracing::info!(symbol = %hub.symbol, "aborting rt-bar publisher on last DecRef");
            h.abort();
        }
    }

    if ticks == 0 && watches == 0 && rt_bars == 0 {
        state.per_symbol.remove(&hub.symbol);
        tracing::debug!(symbol = %hub.symbol, "hub removed after last DecRef");
    }
}

/// Slice B2 — handle a publisher's `UpstreamClosed` notification.
///
/// Steps, in order:
///
/// 1. Look up the hub. Idempotent — if it's already gone (e.g. both
///    publishers fired `UpstreamClosed` and the second arrived after
///    we removed the entry), do nothing. The first message has
///    already published the end-reason.
/// 2. Set the per-hub end-reason watch to `Some(reason)` BEFORE
///    removing the hub from `per_symbol`. Consumers that subscribed
///    via [`SubscriptionHandle::end_reason`] observe the reason as
///    the last lifecycle signal; subsequent `recv` on the broadcast
///    receivers will yield `Closed` once the hub `Arc` finally drops.
/// 3. Emit a structured `warn!` so the operator sees the symbol +
///    subscriber count + uptime + reason in logs.
/// 4. Abort the sibling publisher (if any). The publisher that fired
///    this message has already exited — its `JoinHandle` may still
///    be in the `Mutex`, but `is_finished()` is `true`. Aborting the
///    sibling drops its upstream and cancels the wire subscription.
/// 5. Remove the hub from `per_symbol`. Once the publisher tasks have
///    actually exited (drops their `Arc<SymbolHub>` clones), the
///    hub's broadcast senders drop and consumers see `Closed`.
/// 6. Mark the symbol "previously disconnected" so the next first-
///    subscribe path can emit a matching `info!` log for diagnostic
///    symmetry.
///
/// [`SubscriptionHandle::end_reason`]: super::handle::SubscriptionHandle::end_reason
fn handle_upstream_closed(state: &RouterState, symbol: &SymbolKey, reason: EndReason) {
    let Some(hub_entry) = state.per_symbol.get(symbol) else {
        // Already torn down by a sibling publisher's UpstreamClosed.
        return;
    };
    let hub = hub_entry.clone();
    drop(hub_entry);

    // Step 2: publish the end reason BEFORE the broadcast senders
    // drop. `watch::Sender::send` returns `Err` if there are no
    // receivers; that's fine.
    let _ = hub.end_reason_tx.send(Some(reason));

    // Step 3: structured warn for the operator.
    tracing::warn!(
        target: "midas_market_data::router",
        symbol = %hub.symbol,
        subscriber_count = hub.refcount(),
        hub_uptime_ms = hub.uptime().as_millis() as u64,
        reason = ?reason,
        "upstream closed; tearing down hub"
    );

    // Step 4: abort the sibling publisher (and the one that fired —
    // already exited, so this is a no-op for it).
    if let Some(h) = hub.tick_publisher_task.lock().take() {
        h.abort();
    }
    if let Some(h) = hub.rt_bar_publisher_task.lock().take() {
        h.abort();
    }

    // Step 5: drop from the map. Last `Arc<SymbolHub>` strong-count
    // drops once publisher tasks have observed the abort, after which
    // the broadcast senders close.
    state.per_symbol.remove(symbol);

    // Step 6: remember for the next first-subscribe info log.
    state.previously_disconnected.lock().insert(symbol.clone());
}

/// Slice B2 — emit `tracing::info!` when a first-subscribe path
/// re-spawns a hub for a symbol that was previously torn down by an
/// `UpstreamClosed`. Diagnostic symmetry with the tear-down warn.
fn note_resubscribe_after_disconnect(state: &RouterState, symbol: &SymbolKey) {
    let was_disconnected = state.previously_disconnected.lock().remove(symbol);
    if was_disconnected {
        tracing::info!(
            target: "midas_market_data::router",
            symbol = %symbol,
            "upstream reopened; new hub"
        );
    }
}

fn abort_all_publishers(state: &RouterState) {
    for entry in state.per_symbol.iter() {
        let hub = entry.value();
        if let Some(h) = hub.tick_publisher_task.lock().take() {
            h.abort();
        }
        if let Some(h) = hub.rt_bar_publisher_task.lock().take() {
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
            SymbolDebugInfo {
                symbol: hub.symbol.clone(),
                tick_refcount: hub.tick_refcount.load(Ordering::Relaxed),
                rt_bar_refcount: hub.rt_bar_refcount.load(Ordering::Relaxed),
                watch_refcount: hub.watch_refcount.load(Ordering::Relaxed),
                last_tick_ts_ms: hub.last_tick_ts.load(Ordering::Relaxed),
                tick_publisher_alive: hub
                    .tick_publisher_task
                    .lock()
                    .as_ref()
                    .is_some_and(|h| !h.is_finished()),
                rt_bar_publisher_alive: hub
                    .rt_bar_publisher_task
                    .lock()
                    .as_ref()
                    .is_some_and(|h| !h.is_finished()),
            }
        })
        .collect()
}
