# Architecture Review — Broker / Market-Data Router / Sim Layer

**Scope.** Root-workspace crates between the IB wire protocol and the desktop
app — `midas-broker-core`, `midas-broker`, `midas-market-data`,
`midas-ib-sim`, `mailbox_processor`. Reviewed at commit `b7a7485` (router
refactor landed `4795651..db5c871`).

**Method.** Cargo dependency graphs, trait signatures,
`handle::SubscriptionHandle` drop chains, control-actor message flow,
config-validation surface, and the fuzz harness seam. Code was read, not
modified.

---

## Summary

The router refactor delivered the architecturally strongest layer in the
repo. `midas-broker-core` is a true leaf with zero intra-workspace deps;
`midas-market-data` holds an `Arc<dyn MarketDataSource>` and never names a
concrete backend; `SubscriptionHandle<T>` enforces RAII cleanup with
compile-time guarantees (`!Clone`, `!Copy`, private fields); and the live
guard sits in config validation before any socket is opened. The P1 audit
that moved the provider trait from `midas-broker` into `midas-broker-core`
was the right call — it severed the last structural back-edge and made the
router's backend-agnosticism real rather than aspirational.

Four non-trivial issues emerged: (1) `IbMarketData::connect` transitions to
`Ready` before farm-up arrives, contradicting the trait-doc contract and
the plan's M-23 invariant; (2) the router observes `Disconnected` upstream
but has no defined behaviour for mid-stream transitions — publisher tasks
will fight the retry loop; (3) the two-faucet (ticks + RT-bars) design is
defensible as IB fidelity but creates a parallel ownership path that R21
unification will have to dismantle carefully; (4) `MarketDataError::Other`
is a 37-site escape hatch and hides missing domain variants.

None of the issues are blocking for the session-aware-charts foundation,
but (1) and (2) are P1 for the Phase-1 IB paper connection.

---

## Lens 1 — Coupling & Boundaries

**Rule 1 (no ibapi past broker)** and **Rule 6 (traits as sole seam)** are
structurally enforced, not rule-by-convention. Verified via Cargo graph:

- `midas-broker-core/Cargo.toml` — zero workspace deps. Only serde,
  chrono, thiserror, async-trait, tokio/sync.
- `midas-market-data/Cargo.toml:24` — `midas-broker-core` is the only
  runtime workspace dep. `midas-broker` is dev-only. `cargo check -p
  midas-market-data` cannot reach `ibapi` even transitively.
- `midas-ib-sim/Cargo.toml:19` — depends on `midas-broker-core` only.
  The sim is a wire-protocol speaker, not a `MarketDataSource` impl —
  that seam is `midas-broker/src/ib/*` talking to the simulator over TCP.
- `midas-broker/Cargo.toml` — sole crate holding `ibapi = "=2.10"`.

Object-safety is enforced by a compile-only contract test
(`crates/midas-broker/tests/trait_object_safety.rs:28-36`):
`fn _boxable(_: Arc<dyn MarketDataSource>) {}` plus mock impls driving
every method through the trait object. A method that takes `self` by
value or returns `impl Trait` would fail this file.

**Post-P1 audit cleanup.** The provider trait now lives in
`midas-broker-core::provider` and is re-exported by both
`midas-market-data/src/router/mod.rs:53` and
`midas-broker/src/market_data_source.rs:14`. Pre-P1 the router depended
on `midas-broker` to reach the trait — that back-edge is gone.

**One leaky seam:** `OrderType`, `Tif`, `OcaType`, `TriggerMethod`,
`AlgoStrategy`, `OrderCondition`, and `OrderEvent` in `order_client.rs`
are modelled directly on IB's order form. Deliberate (module docstring
at `order_client.rs:1-11`) and correct — the trait is the router-era IB
surface — but it means `midas-broker-core` carries IB's domain model,
not a generic broker's. A future OANDA adapter translates to IB-shaped
enums, not the reverse. Reasonable; worth acknowledging.

---

## Lens 2 — The Two-Faucet Question

`MarketDataSource` exposes `subscribe_ticks` (~250 ms sampled) AND
`subscribe_realtime_bars` (5 s) as independent wire subscriptions. The
router mirrors both with parallel broadcast lanes, publisher tasks,
refcounts, and cancel paths (`SymbolHub` at `state.rs:89-130`).
`history_then_live` and the aggregator both consume only RT-bars — the
aggregator module comment at `task.rs:10-12` explicitly rules out
tick-based aggregation because "volume would drift versus IB's own 5 s
bars".

**Is the current two-faucet design correct?** Yes, for now:

1. **IB-fidelity.** IB bills these as two separate wire subscriptions
   with distinct pacing limits. Collapsing pre-IB-connection would force
   the adapter to fake one by decimating the other.
2. **Volume correctness.** IB's 5 s bars include hidden / iceberg fills
   that don't surface as L1 ticks. A tick-built aggregator under-counts
   on illiquid names.
3. **Session-aware-charts uses both independently.** Watchlist cells
   want the coalesced `Quote` (tick-driven via `update_last_quote` in
   `publisher.rs:149`); charts want completed bars (RT-bar driven).

**R21 unification target.** Saves one wire subscription per symbol but
introduces volume-drift risk. Post-unification the right shape is a
*selectable* aggregator source — sim uses ticks (it controls truth), IB
keeps RT-bars (it doesn't), selection lives in the `MarketDataSource`
impl, not the router. Keep the router's dual-lane design; make the
aggregator source a provider concern. Removing the RT-bar faucet
naively is a 300-LOC change in `aggregator/task.rs` plus a new
volume-reconciliation path — not a refactor.

---

## Lens 3 — RAII Subscription Handles

`SubscriptionHandle<T>` (`handle.rs:125`) is the pattern's keystone.

**Structural enforcement.** `!Clone` by omission (no derive, no manual
impl), `!Copy` via `Box<dyn Guard>`, private `rx` and `_guard` fields.
`into_parts()` exists as an explicit opt-out.

**Drop chain.** `SubscriptionHandle` drop → `Box<dyn Guard>` drop →
`TickSubGuard::drop` → `ctrl.send(RouterMsg::DecTickRef)` (unbounded
mpsc, fire-and-forget). Actor decrements `hub.tick_refcount`; if zero
and no other refcount holds, `maybe_reap` at `actor.rs:478` aborts the
publisher task, which drops the upstream `TickStream`, which fires the
provider's `cancel` closure. Every hop is unconditional.

**Send+Sync.** `Guard: Send + Sync` is explicit (`handle.rs:62`).
`GuardCtrl` captures atomics + `mpsc::UnboundedSender` only — all
Send+Sync. Broadcast receiver is `Send + !Sync`, fine for ownership
moves across tasks.

**Lock ordering.** The `parking_lot::Mutex` around the publisher
`JoinHandle` (`state.rs:123-125`) is the only lock on the DecRef path.
`per_symbol` DashMap entries are cloned and immediately dropped
(`actor.rs:199-201`) so no DashMap shard is held across the mutex.
`h.abort()` is non-blocking.

**Drop-from-runtime.** Unbounded mpsc send doesn't call
`Handle::current()`, so guard drops are safe post-runtime-shutdown.

**Dropped mid-`recv().await`.** `recv()` takes `&mut self.rx`; the
borrow checker forbids dropping the handle while the borrow is live.
If the polling task is aborted, the future drops first, then the
handle at scope exit — correct ordering by construction.

**Nuance.** `GuardedStream::poll_next` closes on `Lagged`
(`handle.rs:281-288`), surfacing ring-wrap loss as end-of-stream. But
borrow-style `recv()` still hands Lagged to the caller. Mixed
semantics across the two entry points is a paper cut — R5 below.

---

## Lens 4 — Order Flow vs Market-Data Flow

The split is correct. Order events are low-volume / high-value
(broadcast cap 8192, every message matters); market data is
high-volume / lossy-OK (cap 4096, Lagged survivable via resubscribe).
Merging would force one channel-sizing policy on both domains.

`SimOrderClient` and `SimMarketData` are separate structs, as are
`IbOrderClient` and `IbMarketData` — but they share underlying state
through `Arc`-held primitives (e.g. `ordering_ready_tx` at
`ib/market_data.rs:81`, `client_handle()` at `:176`). Right factoring:
trait-level separation for consumers, backend-level composition for
shared resources like the rust-ibapi `Client`.

**Coupling at the router level.** None — the router holds only
`Arc<dyn MarketDataSource>`. `OrderClient` subscribers deal directly
with backend broadcasts. Operationally asymmetric (market-data gets
RAII + pacing + contract cache + history seam; orders get a raw
broadcast), but the asymmetry is justified by different consumption
patterns. If a second "router" lands for orders, lift the
refcounted-handle pattern into `midas-broker-core`.

---

## Specific Concerns

### C1. `Ready` state race — IB adapter marks Ready before farm-up

`IbMarketData::connect` at `ib/market_data.rs:109-143` transitions
`Connecting` → `Connected` → `Ready` in a single method call, passing
only `nextValidId` before firing `Ready`. The comment at line 138-140
offloads farm-gating to "the router policy", but
`router/mod.rs:298` passes `connection_state()` straight through.
Meanwhile `ConnectionState::Ready`'s doc in
`market_data/connection.rs:65` promises "Connected AND all farms up
AND nextValidId received". Contradiction.

Consequence: consumer who waits on `Ready` before placing an order can
hit an IB session where MKT farm hasn't reported HMDS-available —
order held / rejected.

**Severity: P1.** Lands the moment real IB is wired in. Fix: gate
`Ready` inside `IbMarketData::connect` on first `FarmStatus::Ok(MKT)`,
or split into `OrderingReady` + `MarketDataReady` + `Ready`.

### C2. Router has no `Disconnected` handling

Grep for `Disconnected`/`Reconnecting` in `midas-market-data/src`
returns zero. The router republishes `connection_state()` but takes no
action on flips. Publisher tasks block on `upstream.next().await`; on
`RecvError::Closed` they exit (`publisher.rs:76-82`), but the
`SymbolHub` is NOT torn down — refcounts remain, DecRef sends still
arrive. The hub lingers with zero publishers until the last consumer
drops. On upstream reconnect, no fresh `subscribe_ticks` is fired.

**Severity: P1.** Design question, not a bug yet — sim doesn't
disconnect in tests. IB reconnect semantics need explicit policy: does
the router re-subscribe on reconnect, or treat disconnect as
teardown-with-error? Plan doc is silent.

### C3. `MarketDataError::Other(String)` escape hatch

37 occurrences across 11 files. Most are defensible (IB error
passthrough, timeout synthesis), but notable misses:

- `router/actor.rs:55` — timeout should be typed
  `MarketDataError::Timeout { call: &'static str }`.
- `ib/market_data.rs:116` — `"ib connect: {e}"` could be
  `ConnectFailed(..)`.
- `ib/market_data.rs:133` — `next_valid_order_id` handshake error,
  distinct from runtime errors.

**Severity: P3.** Taxonomising would let the retry loop distinguish
"retry" from "give up".

### C4. Live-trading guard coverage

`BrokerConfig::validate()` at `config.rs:250-259` rejects port 4001
without `allow_live`. Coverage gaps:

- `load_from_file` — covered (calls `validate()?`).
- Direct mutation (`cfg.connection.port = 4001`) — NOT covered.
  `validate()` is only invoked from `load_from_file`.
- `IbMarketData::new` + `.connect()` takes `IbMarketDataConfig` (in
  `ib/config.rs`), not `BrokerConfig`. Guard does NOT apply to direct
  adapter construction.

**Severity: P1 money-critical.** Guard works for TOML but is
bypassable. Fix: assert `port != 4001 || allow_live` inside
`IbMarketData::connect` as defence-in-depth.

### C5. Test-only injection leak-safety

`inject_for_test` is triple-gated: feature `test_inject` default-off at
`midas-broker-core/Cargo.toml:11-15`, trait cfg at `provider.rs:169`,
sim impl cfg at `sim/market_data.rs:1077`. Router accessor
`source_for_test()` at `router/mod.rs:346-349` matches. A release build
of the app without the feature cannot see the method. **Verdict: safe.**

### C6. Fuzz surface — `decode_incoming` only

The fuzz target at `midas-ib-sim/fuzz/fuzz_targets/decode_incoming.rs`
feeds arbitrary bytes into `TwsCodec::decode`. Correct primary target —
wire codec is the trust boundary. But the sim's own *output encoder*
is not fuzzed; a round-trip `MarketEmission → encode → decode →
MarketEmission` target would catch asymmetries. **Severity: P3.**

---

## Strengths

1. **Trait seam enforced structurally.** Core crate is leaf; router
   cannot name concrete backends even if someone tried.

2. **Object-safety contract tests.** `trait_object_safety.rs` exercises
   every method through `Arc<dyn _>`. Future `fn x(self)` or
   `-> impl Trait` fails CI.

3. **RAII discipline is airtight.** `SubscriptionHandle<T>` is `!Clone`,
   private fields, guard is `Box<dyn Guard + Send + Sync>`, DecRef is
   unconditional on drop. Actor stores refcount *after* fallible work
   (`actor.rs:211`) so error paths cannot leak.

4. **Source-failure rollback (NM-3).** `handle_subscribe_ticks` calls
   source FIRST and only inserts into `per_symbol` on success
   (`actor.rs:217-232`). Failed subscribe leaves zero state.

5. **Backlog observability.** `AtomicUsize` counter
   (`router/mod.rs:67`) with sticky `ROUTER_BACKLOG_WARN = 1000`
   warn-once gives operators a trip-wire. Fire-and-forget sends keep
   counter balanced via explicit rollback on send error.

6. **Per-handler timeout.** `ROUTER_ACTOR_OP_TIMEOUT = 10s` wraps every
   fallible upstream call (`actor.rs:49-60`). Wedged provider cannot
   stall the control mpsc indefinitely.

7. **`GuardedStream` closes on Lagged.** Deliberate choice
   (`handle.rs:281-288`) preventing the silent-gap bug in
   `history_then_live`; the code comment names the vulnerability.

8. **Sim/IB isomorphism test.** `tests/ib_sim_isomorphism.rs` runs both
   backends through the same trait surface — right place to catch sim
   drift from IB.

---

## Recommendations

### P1 (before Phase-1 IB paper connection)

**R1. Fix `Ready` semantics.** Gate `IbMarketData::connect` on first
`FarmStatus::Ok(MKT)` before transitioning to `Ready`, or split into
`OrderingReady` + `MarketDataReady` + `Ready`. Update trait doc.
File: `ib/market_data.rs:138-141`.

**R2. Define router policy for mid-stream disconnects.** Add explicit
`handle_upstream_disconnect` in the control actor: either tear down
all `SymbolHub`s and surface typed errors, or retain hubs and
re-subscribe on reconnect. Document the choice. File:
`router/actor.rs`.

**R3. Harden live-trading guard.** Add `assert!(port != 4001 ||
allow_live)` inside `IbMarketData::connect` as defence-in-depth
against programmatic `BrokerConfig` mutation bypassing TOML
validation. File: `ib/market_data.rs:109`.

### P2 (next major refactor window)

**R4. Taxonomise `MarketDataError`.** Split `Other(String)` into
`Timeout { call }`, `Handshake(String)`, `ConnectFailed(String)`.
Retry logic distinguishes transient from terminal without string-match.
File: `midas-broker-core/src/market_data/error.rs`.

**R5. Unify `Lagged` policy.** Either make `SubscriptionHandle::recv`
close on first Lagged (mirroring `GuardedStream`) or make
`GuardedStream` forward Lagged. Mixed semantics is a paper cut.
File: `router/handle.rs`.

**R6. Design R21 unification.** Tick-only aggregator as per-provider
opt-in (sim enables, IB keeps RT-bars), parallel `SessionedBarAggregator`
alongside the existing `BarAggregator`. Do NOT remove the RT-bar faucet.

### P3 (nice-to-have)

**R7. Outgoing-encoder fuzz target.** Round-trip
`MarketEmission → encode → decode → MarketEmission` in
`midas-ib-sim/fuzz/fuzz_targets/`.

**R8. Extract RAII subscription pattern.** If a second router
materialises (orders, account-events), lift `Guard`, `GuardCtrl`, and
refcount-on-construction into `midas-broker-core::refcount`.
Premature today; track as a future note.

**R9. Evolution signals.** New backend (OANDA, Alpaca): implement both
traits + translate to IB-shaped enums — ~800 LOC seam. Multi-account IB
(same backend, different clientIDs): two `IbMarketData` + two
`MarketDataRouter` instances, architecture supports this unchanged.
Multi-exchange disambiguation works via `SymbolKey.contract_id`
(`lib.rs:120-124`) already. Abstraction cost looks healthy.

---

## Closing

The router/broker/sim layer is the foundation this codebase stands on,
and it stands firm. P1 items (Ready semantics, disconnect policy,
live-guard hardening) are concentrated in the IB adapter and router
actor — small surfaces, single-commit fixes. Nothing in the core trait
surface needs to move. The two-faucet architecture is correct for IB
fidelity; R21 unification should preserve both faucets and let the
aggregator select its source at the provider level.

Primary evidence:
- `crates/midas-broker-core/src/provider.rs:62-171`
- `crates/midas-market-data/src/router/mod.rs:53-390`
- `crates/midas-market-data/src/router/actor.rs:122-170`
- `crates/midas-market-data/src/router/handle.rs:125-188`
- `crates/midas-broker/src/ib/market_data.rs:109-143`
- `crates/midas-broker/src/config.rs:250-259`
- `crates/midas-broker/tests/trait_object_safety.rs:28-36`
