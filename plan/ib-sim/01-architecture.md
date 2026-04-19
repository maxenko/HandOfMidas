# Stage 01 — Architecture

*Crate layout, process model, module boundaries, public API surface. This is the load-bearing stage — every other stage depends on the seams defined here.*

**Depends on**: research phase (complete)
**Blocks**: every other stage

## Process model

**One Rust binary**: `midas-ib-sim-server`

```
┌───────────────────────────────────────────────────────────────┐
│  midas-ib-sim-server (binary)                                 │
│                                                                │
│  ┌─────────────┐   ┌──────────────────┐   ┌────────────────┐ │
│  │ TCP Listener│──▶│ Connection actor │──▶│ Per-session    │ │
│  │ port 7497   │   │ (handshake, mux) │   │ Tokio task     │ │
│  └─────────────┘   └──────────────────┘   └────────────────┘ │
│                                                    │           │
│                                                    ▼           │
│                                          ┌─────────────────┐   │
│                                          │ Engine actor    │   │
│                                          │ (single-thread) │   │
│                                          └─────────────────┘   │
│                                                    │           │
│                        ┌───────────────────────────┼────────┐  │
│                        ▼                   ▼               ▼  │
│                ┌───────────────┐  ┌──────────────┐  ┌──────┐ │
│                │ Market data   │  │ Order book   │  │ Clock│ │
│                │ engine        │  │ (state mach) │  │      │ │
│                └───────────────┘  └──────────────┘  └──────┘ │
└───────────────────────────────────────────────────────────────┘

    ▲                                           ▲
    │                                           │
    │ TCP (TWS wire protocol)                   │ Control plane
    │                                           │ (HTTP/JSON or stdin commands)
    │                                           │
┌─────────────────┐                  ┌──────────────────────┐
│ rust-ibapi      │                  │ devloop / CLI / tests│
│ client          │                  │ (inject faults)      │
│ (in midas-broker)│                  └──────────────────────┘
└─────────────────┘
```

Key invariants:

- **One engine actor**, single-threaded over all sessions. All state lives behind it. This is the same actor shape as `midas-broker::BrokerEngine` — we reuse the pattern.
- **Per-session Tokio tasks** handle the wire protocol (decode inbound frames, encode outbound frames). They send typed messages to the engine and subscribe to an outbound broadcast channel.
- **Control plane is separate** from the TWS port. Faults are injected over a side channel so test code can coordinate scenarios without spoofing the wire protocol. Default: HTTP/JSON on a second port (e.g., 9497) or stdin JSONL for CI.

## Crate layout

New workspace member in the **root workspace** (broker side, shares `midas-broker-core`):

```
crates/
  midas-ib-sim/
    Cargo.toml
    src/
      lib.rs                    # re-exports public API
      server.rs                 # TCP listener, connection accept loop
      engine/
        mod.rs                  # Engine actor, command dispatch
        state.rs                # Session + order state
        clock.rs                # Clock trait + RealClock + VirtualClock
        scheduler.rs            # Event scheduler (priority queue on virtual time)
      protocol/
        mod.rs                  # Public codec API
        framing.rs              # Length-prefixed frame reader/writer
        handshake.rs            # API\0 + version-range handshake
        messages/
          mod.rs                # Message enum
          incoming.rs           # Client → sim message types
          outgoing.rs           # Sim → client message types
          fields.rs             # Field codec (NUL-delimited, sentinels)
      market_data/
        mod.rs                  # MarketDataEngine trait + dispatcher
        generator/
          mod.rs                # Synthetic generator top-level
          garch.rs              # GARCH(1,1) volatility process
          hawkes.rs             # Hawkes-lite arrival process
          roll.rs               # Bid-ask bounce + spread
          u_shape.rs            # Intraday intensity table
        replay/
          mod.rs                # Replay engine (reads .dbn + .scenario)
          dbn_reader.rs         # Databento dbn format reader
          recorder.rs           # Captures real IB sessions → .dbn + .tws.pcap
      orders/
        mod.rs                  # Order simulator
        state_machine.rs        # Order + bracket state transitions
        fill_model.rs           # Synthetic fills (pessimistic default)
        brackets.rs             # Parent-child bracket semantics
      quirks/
        mod.rs                  # Quirk orchestrator
        msg_rate.rs             # 50 msg/sec limiter → error 100 + disconnect
        line_limit.rs           # 100 L1 line cap + overflow error
        historical_pacing.rs    # 60/10min + 6-in-2s + 15s cooldown
        farm_status.rs          # 1100/1101/1102/2103-2108/2158 emitter
      scenario/
        mod.rs                  # YAML scenario loader + runner
        script.rs               # Scenario script types
        injector.rs             # Translates scenario events → engine actions
      control/
        mod.rs                  # Control-plane HTTP API
        api.rs                  # Endpoints: inject/disconnect/lag/status
      bin/
        server.rs               # binary entry point
    tests/
      handshake_e2e.rs
      tick_stream_e2e.rs
      order_lifecycle_e2e.rs
      scenario_replay_e2e.rs
    fixtures/
      wire/                     # Captured wire-byte fixtures (reused from rust-ibapi + ours)
      scenarios/                # Canonical YAML scenarios
      sessions/                 # Recorded real-IB sessions (.dbn + .tws.pcap)
```

### Dependencies (Cargo.toml)

```toml
[dependencies]
midas-broker-core = { path = "../midas-broker-core" }
tokio = { version = "1", features = ["rt-multi-thread", "macros", "net", "io-util", "sync", "time", "signal"] }
bytes = "1"
tokio-util = { version = "0.7", features = ["codec"] }
thiserror = "2"
tracing = "0.1"
tracing-subscriber = "0.3"
serde = { version = "1", features = ["derive"] }
serde_yaml = "0.9"
rand = "0.8"
rand_distr = "0.4"   # Student-t, log-normal, Poisson
statrs = "0.17"      # statistical distributions
chrono = "0.4"
uuid = { version = "1", features = ["v4", "serde"] }
dbn = "0.19"         # Databento format
axum = "0.7"         # control plane HTTP
clap = { version = "4", features = ["derive"] }

[dev-dependencies]
proptest = "1"
tokio-test = "0.4"
rstest = "0.21"
```

### Public API surface

```rust
// lib.rs exports — kept minimal so the library can embed into CI tests
pub use crate::server::{Sim, SimConfig, start_sim};
pub use crate::engine::clock::{Clock, RealClock, VirtualClock};
pub use crate::market_data::MarketDataMode;
pub use crate::scenario::Scenario;
pub use crate::control::ControlApi;
```

## Central types frozen in Stage 01 (to prevent Wave-2 merge conflicts)

Wave 2 puts 17 agents concurrently working on stages 03/04/05/07. Every stage wants to extend shared central enums (`EngineCmd`, `EngineEvent`, `MarketEmission`, `OrderEmission`, `QuirkViolation`, the `MarketSnapshot` tick-push contract). Each extension is a central-file edit → merge conflicts.

**Mitigation**: Stage 01 is the sole owner of the central enum surface. It defines **all** variants up front — one per concern Wave 2 will implement — as stubs returning `todo!()` or `Vec::new()`. Wave 2 agents fill in the *bodies*, never add new variants.

Central types frozen in Stage 01:

```rust
// Every Wave 2 concern has a named variant here.
pub enum EngineCmd {
    // From sessions
    StartApi { session: SessionId, client_id: i32 },
    PlaceOrder { session: SessionId, req: PlaceOrderReq },
    CancelOrder { session: SessionId, order_id: OrderId },
    SubscribeMarketData { session: SessionId, req_id: ReqId, contract: ContractSpec, mode: SubMode },
    UnsubscribeMarketData { session: SessionId, req_id: ReqId },
    ReqContractData { session: SessionId, req_id: ReqId, contract: ContractSpec },
    ReqHistoricalData { session: SessionId, req_id: ReqId, req: HistoricalReq },
    ReqRealTimeBars { session: SessionId, req_id: ReqId, req: RealTimeBarsReq },
    ReqPositions { session: SessionId },
    ReqAccountSummary { session: SessionId, req_id: ReqId, group: String, tags: String },
    ReqAccountData { session: SessionId, subscribe: bool, acct_code: String },
    ReqExecutions { session: SessionId, req_id: ReqId, filter: ExecutionFilter },
    ReqGlobalCancel { session: SessionId },
    ReqCurrentTime { session: SessionId },
    ReqIds { session: SessionId, num_ids: i32 },
    ReqMarketDataType { session: SessionId, data_type: MarketDataType },

    // From control plane — all fault injection variants enumerated now
    InjectDisconnect { session: SessionId, reason: String },
    InjectLag { session: SessionId, duration: Duration },
    InjectPacingViolation { session: SessionId },
    InjectFarmOutage { code: i32, farms: Vec<String> },
    InjectFarmRestore { code: i32, farms: Vec<String> },
    InjectPriceJump { symbol: SymbolKey, magnitude_pct: f64 },
    InjectGap { symbol: SymbolKey, from: f64, to: f64 },
    InjectHalt { symbol: SymbolKey, duration: Duration },
    InjectBurst { symbols: Vec<SymbolKey>, multiplier: f64, duration: Duration },
    InjectDailyRestart,
    LoadScenario(Scenario),
    DumpState { reply: oneshot::Sender<EngineSnapshot> },

    // From scheduler
    Tick(VirtualInstant),
}
```

**The market-data → orders push contract is a function, not a trait method**. Naming two `Emission` enums the same would be confusing; instead:

```rust
/// Market data events the engine emits outbound (and relays internally to orders).
pub enum MarketEmission {
    TickPrice { key: SubKey, tick: TickType, price: f64, size: Option<i64>, attribs: TickAttribs },
    TickSize  { key: SubKey, tick: TickType, size: i64 },
    TickString{ key: SubKey, tick: TickType, value: String },
    TickGeneric { key: SubKey, tick: TickType, value: f64 },
    Bar       { key: SubKey, bar: Bar5s },
    HistoricalBatch { key: SubKey, bars: Vec<Bar>, is_complete: bool },
}

/// Order events the engine emits outbound.
pub enum OrderEmission {
    OpenOrder(OpenOrder),
    OrderStatus(OrderStatus),
    Execution(Execution),
    Commission(CommissionReport),
    Reject { order_id: OrderId, code: i32, message: String },
    Position(PositionUpdate),
    PortfolioValue(PortfolioValueUpdate),
    AcctValue(AcctValueUpdate),
    AcctDownloadEnd(String),
    PositionEnd,
}

/// Engine-internal message from market_data to orders at each mid-price update.
/// The orders simulator reads this to evaluate fills on resting orders.
pub struct MarketSnapshot {
    pub symbol: SymbolKey,
    pub mid: f64,
    pub bid: f64,
    pub ask: f64,
    pub last: f64,      // optional — set if the update carried a trade
    pub volume: Option<i64>,
    pub ts: VirtualInstant,
}

/// The MarketDataEngine trait is PULL-based (step() -> Vec<MarketEmission>).
/// Its step() is called from the engine loop. Each emission is:
///   1. Translated to outbound wire messages (via protocol codec)
///   2. Projected to a MarketSnapshot and passed to OrderSimulator::on_market_snapshot()
///      for fill evaluation.
pub trait MarketDataEngine: Send {
    fn step(&mut self, now: VirtualInstant) -> Vec<MarketEmission>;
    // ... subscribe/unsubscribe/snapshot as before
}

pub trait OrderSimulator: Send {
    fn on_market_snapshot(&mut self, snap: &MarketSnapshot) -> Vec<OrderEmission>;
    // ... place/cancel/etc.
}
```

This resolves the inconsistency: Stage 03 owns `MarketEmission` (pull via `step()`), Stage 04 owns `OrderEmission` + consumes `MarketSnapshot`, and the engine loop glues them — calling `step()`, projecting the emissions to `MarketSnapshot` for order-side fill evaluation, then forwarding `MarketEmission` + `OrderEmission` to the outbound codec.

`QuirkViolation`, `EngineEvent`, `EngineSnapshot` are all declared here at stage-01 time. Wave 2 stages fill in logic; they do not touch the enum definitions.

**Contract ownership reference**:

| Type | Owner stage | Consumers | Extension policy |
|------|-------------|-----------|------------------|
| `EngineCmd` | 01 | Sessions, control plane, scheduler | Adding a variant requires a Stage 01 PR (plan-amendment) |
| `MarketEmission` | 01 (shape) / 03 (logic) | 03 emits; engine loop translates | Additive via extension enum `MarketEmissionExt` if Wave 2 finds gaps |
| `OrderEmission` | 01 (shape) / 04 (logic) | 04 emits; engine loop translates | Same — `OrderEmissionExt` for non-breaking additions |
| `MarketSnapshot` | 01 | 03 emits per tick; 04 consumes for fills | Frozen; new fields additive (option-typed) |
| `QuirkViolation` | 01 | 05, 02 (translates to ErrMsg) | Additive via `QuirkViolationExt` |
| `EngineSnapshot` | 01 | Control plane, scenario queries | Additive |
| `Clock` trait | 08 | Everyone | Stable by convention; only 08 changes it |
| `ContractSpec`, `OrderSpec` | `midas-broker-core` | Everyone | External; bumped only on protocol change |

### Extension-enum pattern (avoids central-type amendment bottleneck)

Freezing all variants at Stage 01 is ideal but impractical over 4–6 weeks of Wave 2 discovery. The fallback: each stage may add variants to its own `*Ext` enum merged at stage end:

```rust
// Stage 01's base:
pub enum MarketEmission { TickPrice {...}, TickSize {...}, /* ... */ }

// Stage 03's additions during Wave 2 (in its own module, not 01's):
pub enum MarketEmissionExt { NewsTick {...}, HaltNotification {...} }

// Engine loop handles both:
match emission {
    Either::Left(base) => dispatch_market(base),
    Either::Right(ext) => dispatch_market_ext(ext),
}
```

When Wave 2 closes, the `*Ext` variants are folded into the base enum as a single Stage-01 amendment PR. This decouples in-flight Wave 2 work from central-file merge conflicts.

**Amendment budget in critical path**: 3 working days reserved across Wave 2 for variant folding + re-verification. Baked into the 20–22 day parallel estimate.

## Module boundaries (the seams that enable parallelism)

| Module | Owns | Knows about | Public surface |
|--------|------|-------------|----------------|
| `protocol` | Wire codec, framing, handshake, message types | `midas-broker-core` types | `Codec`, `IncomingMsg`, `OutgoingMsg`, `Handshake` |
| `engine` | Session state, order book, dispatch | All other modules via traits | `Engine`, `SessionId`, `EngineCmd`, `EngineEvent` |
| `market_data` | Price + tick generation, replay | `Clock`, `midas-broker-core::SymbolKey` | `MarketDataEngine` trait, `Tick`, `Bar` |
| `orders` | Fill simulation, bracket state machine | `Clock`, market prices (read-only) | `OrderSimulator`, `OrderState`, `Fill` |
| `quirks` | Rate limiters, line counters, farm status | `Clock`, message traffic stats | `QuirkGuard` trait, `QuirkConfig` |
| `scenario` | Script parsing, event injection | `Engine` via command API | `Scenario`, `ScenarioRunner` |
| `control` | HTTP API for external fault injection | `Engine` command channel | `ControlApi`, `InjectCmd` |

Critical: **no module has a cyclic dependency**. The only shared dependencies are `midas-broker-core` (value types) and `Clock` (infrastructure).

## Security model

The sim runs on developer machines, CI runners, and occasionally on shared lab boxes. It must never be a foothold for attackers or misused from other processes on the same host.

**Defaults**:

- **Bind `127.0.0.1` only.** Both the TWS listener (7497) and the control plane (9497) refuse to bind a non-loopback address unless `--listen-external` is passed explicitly. Prevents accidental exposure over a network or VPN.
- **Control plane requires a bearer token.** Sim generates a random token at startup and writes it to `~/.local/share/midas-ib-sim/control.token` (0600 perms on Unix; file-ACL on Windows). Control-plane HTTP requests must include `Authorization: Bearer <token>`. Without the token, all requests return 401.
- **`allow_live`-style guard for non-loopback bind.** `--listen-external` refuses to start unless the config also has `external_bind_acknowledged = true`. Mirrors the `allow_live` pattern in `midas-core::config` — forces intent at two places, not one.
- **No account data leaves the process.** The proxy/recording mode (Stage 07) writes captures to disk only; never phones home, never offers a hosted ingestion endpoint. If that ever changes, it's a separate feature with separate review.

**Threat model**:

- **Other local processes** (the main threat) — can connect to 7497 / 9497, but not without the token; the TWS port is valid to connect to (that's the point) but can't inject faults, only drive protocol traffic
- **Network peers on the same LAN** — can't reach the sim without `--listen-external`, and if that's set the operator has opted in
- **Malicious scenarios from untrusted YAML** — scenario expressions are a closed DSL with no eval / no file I/O / no shell; YAML is parsed by serde_yaml (already scrutinized). Still: documentation warns against running untrusted scenario YAML, same stance as any test framework

**Logging**: never log the control-plane bearer token. Redact bytes of captured session traffic when log level is `debug` (raw wire bytes go to file, not stdout).

## Observability

Every interesting boundary emits a `tracing` span. Hierarchy:

```
sim                                  [root span]
├── session{sid, client_id}         [per TCP connection]
│   ├── handshake                    [one-shot during setup]
│   ├── msg_in{msg_id, req_id}      [per incoming wire frame]
│   └── msg_out{msg_id, req_id}     [per outgoing wire frame]
├── order{order_id, symbol}         [per order lifecycle]
│   ├── place
│   ├── fill{chunk}
│   └── cancel
├── market_data{symbol, req_id}     [per subscription]
│   └── tick{kind}                   [per emitted tick — debug level only]
├── quirk{kind, code}               [per quirk trigger]
├── scenario{name, step}            [per scenario event]
└── scheduler                        [debug-level scheduler activity]
```

Levels: `info` for lifecycle boundaries (connect, disconnect, place, fill, quirk-trigger), `debug` for per-tick/per-message detail, `warn` for violations, `error` for internal failures.

**Dump-state control endpoint**: `POST /control/dump` returns a JSON projection of `Engine::snapshot()` — session list, open orders, active subscriptions, scheduler queue depth, quirk counters. Parallels `midas-app`'s devloop `DumpState` command. Debugging a failing scenario never needs `println!` edits.

**Metrics**: `/control/metrics` emits Prometheus-format counters — `msg_in_total{msg_id}`, `msg_out_total{msg_id}`, `quirk_trigger_total{kind}`, `active_sessions`, `scheduler_queue_depth`. No account data in labels; only protocol-shape metadata.

## Engine actor shape (reuses `midas-broker::BrokerEngine` pattern)

The canonical `EngineCmd` variant list is the declaration above in §"Central types frozen in Stage 01." The struct shell below is illustrative only — it does not re-declare enum variants.

```rust
pub struct Engine {
    clock: Arc<dyn Clock>,
    sessions: BTreeMap<SessionId, SessionState>,
    order_book: OrderBook,
    market_data: Box<dyn MarketDataEngine>,
    quirks: QuirkState,
    scheduler: EventScheduler,
    // ...
    command_rx: mpsc::Receiver<EngineCmd>,
    event_tx: broadcast::Sender<EngineEvent>,
}
```

Single `run()` loop:

```rust
async fn run(&mut self) {
    loop {
        tokio::select! {
            Some(cmd) = self.command_rx.recv() => self.handle_command(cmd),
            Some(event) = self.scheduler.next() => self.handle_scheduled(event),
            else => break,
        }
    }
}
```

## CLI surface

```bash
midas-ib-sim-server \
    --port 7497 \
    --control-port 9497 \
    --clock real | virtual | accelerated=10 \
    --mode synthetic | replay=path/to/session.dbn | hybrid=... \
    --scenario path/to/scenario.yaml \
    --seed 12345 \
    --log-level info
```

## Testability shape

- **Library crate** — `lib.rs` can be imported by integration tests without spinning the binary.
- **In-process Sim** — `Sim::start_in_process()` returns a `SimHandle` with a connected `(tx, rx)` pair. Tests talk to it via Tokio channels instead of TCP.
- **Virtual clock by default in tests** — `#[tokio::test(start_paused = true)]` + `VirtualClock`.
- **Scenario DSL tests** — every scenario YAML is an integration test fixture.

## Kill criteria

This stage's design fails if:

- **Engine actor grows past 1500 LOC.** Means we're not delegating enough to sub-modules. Refactor by moving handler sets into `engine::handlers::*` modules before continuing.
- **A module needs `&mut Engine` to do its work.** That breaks the actor boundary; re-design the command API instead.
- **Module boundaries start leaking `ibapi` types.** The simulator is a server side — it emits wire bytes, not `ibapi::Client` types.
- **Cargo.toml bloats past ~15 dependencies for the core.** Every added crate is a commitment. Anything cosmetic belongs in the binary or tests.

## Rollback signals

- `tokio::select!` branches in `run()` exceed 6 arms → break out of actor pattern; sub-actors per concern
- Cross-module imports bypass `mod.rs` → module structure is wrong
- Single integration test takes > 5s at virtual clock → clock isn't actually virtual; find the real-time leak

## Open design questions

**Resolved by plan-eval cycle 1:**

1. ~~Wire protocol buffer shape~~ — **resolved**: committed to `tokio_util::codec::Framed` with custom `Decoder`/`Encoder`. It's the idiomatic choice across `tonic`, `redis-rs`, `mqtt-async-client`; no ascendant successor; the "one layer of indirection" concern is negligible vs. the tested buffer-management we'd otherwise reimplement (per Agent 3 plan-eval).
2. ~~Process supervision~~ — **resolved**: graceful shutdown on SIGINT. Engine drains pending commands, sessions are sent a terminal `ErrMsg` code 1300 ("TWS restart"), recording writers flush. 5-second timeout then hard-kill.

**Still open (carry into implementation):**

3. **Multi-session state sharing**: orders are per-client-id but market data subscriptions can be shared. Should the subscription table key be `(SessionId, ReqId)` or `(ClientId, ReqId)`? Design tension: IB semantics say multiple `clientId`s can subscribe to the same symbol, but each pays the 100-line cap independently. Proposal: key by `(SessionId, ReqId)` for lifecycle, maintain a per-symbol fanout set for emission. Validate during Stage 03 integration with two-session test.

## TCP transport robustness

The TWS port is real TCP — half-open connections, slow clients, and network blips are not optional concerns.

- **Read timeout**: 30s per frame. If a client goes silent, drop the connection with a logged warning (no `ErrMsg` — consistent with real IB's silent timeout).
- **Write timeout**: 5s per frame. If the client socket blocks write, drop the connection (client is dead or far behind).
- **Keepalive probes**: set `SO_KEEPALIVE` on accepted sockets; Linux default ~2h idle is fine, we don't override.
- **Half-open detection**: tested via `inject_lag` scenario that freezes the session event stream; client should either drop or send a subsequent request, which will fail cleanly.
- **Graceful shutdown**: on SIGINT, stop accepting new connections, flush active sessions with `ErrMsg` code 1300, wait up to 5s, then hard-close.

## Deliverables

- Cargo.toml wired into root workspace
- All files above created with empty `pub` stubs (doc comments describing the API)
- **Central type surface frozen**: all `EngineCmd` / `MarketEmission` / `OrderEmission` / `MarketSnapshot` / `QuirkViolation` / `EngineEvent` variants declared with `todo!()` bodies so Wave 2 stages don't extend them (they use `*Ext` extension enums instead — see §Central types frozen)
- `cargo build -p midas-ib-sim` + `cargo clippy -p midas-ib-sim -- -D warnings` green with stubs
- One smoke test: `cargo test -p midas-ib-sim --test handshake_e2e` that spins up the sim in-process, does a trivial handshake round-trip, shuts down
- **Throughput benchmarks (RealClock)** — two separate benches that together validate the single-actor scale target:
  1. `cargo bench -p midas-ib-sim engine_realclock_single` — single session, synthetic command stream (no TCP). Target: **2,000 events/sec** sustained, p99 < 10ms. This is the actor's raw per-tick cost.
  2. `cargo bench -p midas-ib-sim engine_realclock_fanout` — 100 subscribed symbols, synthetic tick generator, burst rate of 5,000 ticks/sec aggregate. Target: no dropped ticks, engine-loop p99 < 10ms, scheduler queue depth stays < 10,000.
  
  Failing *either* bench means Wave 2's market-data generator must run on a sibling task feeding the engine via a bounded channel, rather than inside the engine loop. The "fanout" bench is the realistic scenario — 100 tickers during an opening-bell burst is exactly what the sim will see against real IB paper.
- Security: control-plane token written to `~/.local/share/midas-ib-sim/control.token` with correct perms; `--listen-external` flag rejects non-loopback without `external_bind_acknowledged`
- `tracing` span hierarchy wired and documented; `/control/dump` endpoint returns `EngineSnapshot`
