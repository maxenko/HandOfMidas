# IB Simulator — Plan Index

*A full-parity Interactive Brokers TWS gateway simulator for Hand of Midas. Runs as a standalone Rust process, speaks TWS wire protocol, supports synthetic and replayed market data, models IB's quirks (pacing, line limits, async ordering), and injects faults deterministically from scenario scripts.*

**Status**: planning phase. See [progress.md](progress.md) for the live milestone tracker.

## Why this matters

Testing against real IB paper gateway is network-bound, non-deterministic, rate-limited, and IB resets accounts periodically. A full-parity simulator:

- **Enables off-market development** — work Saturday afternoon on live-data features
- **Enables deterministic CI** — every test runs against the same bytes in the same order
- **Enables failure-mode testing that IB physically cannot provide** — pacing violations on demand, disconnect mid-fill, out-of-order events
- **Exercises the entire `ib_client.rs` + reconnect + pacing code path** — things an in-process `TestBroker` mock skips entirely

Research shows no mature open-source TWS wire-protocol simulator exists in any language — see [research/existing-simulators.md](research/existing-simulators.md). Every "IB mock" on GitHub is library-level (monkey-patches `EClient` methods). Building this fills a real gap.

## Architecture one-liner

**Separate Rust binary listening on TCP port 7497** (IB paper default) that impersonates the TWS gateway's server side, speaking wire protocol `v176..221` (pure text framing, never emits protobuf). Our app's `midas-broker` connects to it via `rust-ibapi` exactly as it would connect to real IB — no special code paths.

See [01-architecture.md](01-architecture.md) for the full model.

## Plan files

| # | File | Scope | Parallel-safe? |
|---|------|-------|----------------|
| 01 | [architecture.md](01-architecture.md) | Crate layout, process model, module boundaries, public API | — |
| 02 | [protocol-layer.md](02-protocol-layer.md) | TWS wire codec, framing, handshake, message subset | — |
| 03 | [market-data-engine.md](03-market-data-engine.md) | Synthetic generator (Roll-GARCH-U), replay engine (Databento `dbn`), hybrid | Yes (after 01) |
| 04 | [order-lifecycle.md](04-order-lifecycle.md) | Order state machine, fill simulation, bracket parent-child, execDetails timing | Yes (after 02) |
| 05 | [quirk-modeling.md](05-quirk-modeling.md) | 50 msg/sec limiter, 100 L1 line cap, historical pacing budgets, farm-status events | Yes (after 02) |
| 06 | [failure-injection.md](06-failure-injection.md) | YAML scenario scripts, disconnect/lag/pacing/bad-data injection | Yes (after 02 + 04) |
| 07 | [session-recording.md](07-session-recording.md) | Capture real IB sessions as `.tws.pcap` + `.dbn`, deterministic replay | Yes (after 02) |
| 08 | [deterministic-clock.md](08-deterministic-clock.md) | Virtual time, event scheduler, real-time / accelerated / virtual modes | Yes (after 01) |
| 09 | [integration.md](09-integration.md) | How `midas-broker` connects, devloop TCP-to-sim bridge, CI wiring | Yes (after all above) |
| 10 | [rollout.md](10-rollout.md) | Phased delivery, parallelism map, milestone gates, kill criteria | — |
| 11 | [decisions.md](11-decisions.md) | ADRs for every consequential choice (separate process, Rust, wire format, synthetic model, DSL, threading) | — |

## Research inputs (frozen)

- [research/tws-wire-protocol.md](research/tws-wire-protocol.md) — message subset, wire format, version strategy
- [research/ib-quirks-and-limits.md](research/ib-quirks-and-limits.md) — pacing rules, line limits, order-event ordering
- [research/existing-simulators.md](research/existing-simulators.md) — landscape, lessons learned, design references
- [research/microstructure-models.md](research/microstructure-models.md) — Roll-GARCH-U, validation tests, scope traps

## Key decisions (quick reference — full reasoning in [11-decisions.md](11-decisions.md))

| # | Decision | Short rationale | ADR |
|---|----------|-----------------|-----|
| 1 | **Separate process** (not in-process library) | Exercises full `ib_client.rs` path, supports multi-client, deterministic across restarts | ADR-001 |
| 2 | **Rust, new crate `midas-ib-sim`** in broker workspace | Matches project language; reuses `midas-broker-core`; no C dep tax | ADR-002 |
| 3 | **TCP port 7497** default (drop-in for IB paper) | `rust-ibapi` config works unchanged — one env var flip toggles sim vs real | ADR-001 |
| 4 | **Wire protocol `v176..221`, text framing only** | Covers `rust-ibapi`'s `MIN_VERSION≥201` + forward headroom; avoids protobuf entirely | ADR-003 |
| 5 | **Synthetic model: Roll-GARCH-U** | ~500 LOC, produces required stylized facts (clustering, bounce, heavy tails, U-shape) | ADR-004 |
| 6 | **Replay format: Databento `dbn`** | Rust-native, schema'd, well-maintained, streaming-friendly | ADR-005 |
| 7 | **Scenario DSL: YAML** (QuickFIX pattern) | Human-editable, git-diffable, deterministic | ADR-006 |
| 8 | **Async runtime: tokio; single-actor engine** | Matches `midas-broker`; determinism over throughput sharding | ADR-007 |
| 9 | **Deterministic clock: explicit `Clock` trait + `VirtualClock`** | Virtual time for CI, real-time for dev loop; not dependent on `tokio::time::pause` semantics | (see 08-deterministic-clock.md) |

11-decisions.md has the full Context / Alternatives Considered / Consequences / Re-evaluation trigger for each. Any disagreement between this quick table and the ADR file should be resolved in the ADR's favor.

## Parallelism philosophy

Every stage file has explicit sections on:

1. **Depends on** — which other stages must land first
2. **Internal parallelism** — sub-work items within the stage that can fan out to separate agents
3. **Seams** — exact module/trait boundaries that enable parallel development
4. **Rollback signals** — observable indicators the design is going wrong

The dependency graph in [10-rollout.md](10-rollout.md) identifies which stages can run in parallel waves and which are serialization points.

## Non-goals

Things that are EXPLICITLY out of scope for this feature arc:

- **Real L2 order book dynamics** — IB default market data is L1; building full LOB is a rabbit hole
- **Options chain modeling** — defer until options trading is scoped in Hand of Midas
- **Protobuf messages (v201+)** — stay in text framing; revisit if a specific feature forces it
- **FIX protocol support** — this is TWS wire, not FIX
- **Backtesting engine** — this is a broker simulator (event-stream fidelity), not a strategy backtester (portfolio accounting)
- **Tick-size/sub-penny rules** — round to symbol's tick, done
- **Exchange-specific quirks** (auction imbalance, reg-SHO, LULD) — out of scope for IB L1 sim
- **Commercial-grade matching engine** — our fills are synthetic, not price-time-priority-matched
- **Strategy backtest harness** — separate feature arc

## How to use this plan

- **Read 00-index.md → 01-architecture.md → 10-rollout.md first** to see the shape
- **Implementers jump to the stage they own**, read its file end-to-end, use the Seams section to know what not to touch
- **Reviewers use the Rollback signals** to flag when a stage's design is drifting
- **Progress updates land in [progress.md](progress.md)** — not in individual stage files (keeps stage files stable)
