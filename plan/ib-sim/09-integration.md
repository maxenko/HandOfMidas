# Stage 09 — Integration

*How the simulator connects to `midas-broker`, the `midas-app` dev loop, and CI. Proves the sim is drop-in compatible: zero code changes in the client side, one config flag flips sim vs real IB.*

**Depends on**: 02, 03, 04, 05, 06, 07, 08
**Blocks**: 10 (rollout — integration tests gate the milestones)
**Parallel-safe with**: nothing (last major stage)

## Scope

Three integration points:

1. **`midas-broker` client** — uses `rust-ibapi` already; sim must be byte-compatible so zero code changes are needed.
2. **`midas-app` dev loop** — devloop harness spawns sim on a second port, lets Claude/tests script scenarios via the sim control plane.
3. **CI** — GitHub Actions job that runs the full canonical-scenario suite against a sim process.

## Integration 1: `midas-broker` client

### Target state

```bash
# Production — connects to real IB paper
cargo run -p midas-app -- --broker-port 7497

# Development — connects to our sim on the same port
midas-ib-sim-server --port 7497 --scenario my_test.yaml &
cargo run -p midas-app -- --broker-port 7497
```

The app doesn't know the difference. `rust-ibapi` speaks the same wire protocol to both.

### Required work

- **Nothing in `midas-broker`** — the TWS wire protocol is identical. If we've done Stages 02/04/05 correctly, `rust-ibapi::Client::connect()` succeeds against the sim exactly as it succeeds against real IB.
- **One flag in `midas-core::config`** — `broker.sim_allowed: bool`. Mirrors the existing `allow_live` guard: the sim uses port 7497 just like IB paper, so disambiguating needs a user-intent signal. Default: sim is disabled; the user explicitly enables it via config.

### Acceptance test

```bash
# 1. Start sim
midas-ib-sim-server --scenario fixtures/scenarios/bracket_happy.yaml &

# 2. Start app pointing at sim
cargo run -p midas-app -- --feature dev_harness

# 3. Via dev harness, inject a broker connect
echo '{"Command":"ConnectBroker","args":{"host":"127.0.0.1","port":7497}}' | nc 127.0.0.1 9898

# 4. Assert: broker connection established, tick data flowing to watchlist
echo '{"Command":"DumpState","args":{"path":"/broker/connection_state"}}' | nc 127.0.0.1 9898
# Expects: "Ready" within 5 virtual seconds
```

## Integration 2: `midas-app` dev loop

The dev harness (`midas-devloop-proto`) already exposes a TCP control interface on port 9898. We add two commands that coordinate with a running sim:

```rust
pub enum DevLoopCmd {
    // Existing
    Ping, Shutdown, LoadFixture, /* ... */

    // New
    SpawnSim { port: u16, scenario: Option<String> },
    InjectSimFault { fault: SimFault },
}

pub enum SimFault {
    Disconnect,
    PacingViolation,
    FarmOutage,
    PriceJump { symbol: String, magnitude_pct: f64 },
    // ...
}
```

These proxy through to the sim's control-plane HTTP API (see Stage 01). The dev harness becomes the single entry point for orchestrating the sim alongside the app:

```python
# Claude's dev loop script (pseudocode)
devloop.send(SpawnSim(port=7497, scenario="bracket_happy.yaml"))
devloop.send(ConnectBroker(host="127.0.0.1", port=7497))
devloop.send(WaitForEvent("BrokerConnected", timeout=5))
devloop.send(Screenshot("bracket_placed"))
devloop.send(InjectSimFault(PriceJump(symbol="AAPL", magnitude_pct=-5.0)))
devloop.send(WaitForEvent("OrderStatus(Filled, order_id=0)"))
devloop.send(Screenshot("after_sl_fill"))
```

### Process lifecycle

- Dev harness spawns the sim as a child process on `SpawnSim`.
- Sim's PID tracked in `desktop/win/.devloop/sim.<port>.pid`.
- On `Shutdown`, harness SIGTERMs the sim; sim's graceful shutdown flushes recording (if enabled).
- On dev-harness crash, a supervisor timer kills the sim after 5 minutes of orphanage.

## Integration 3: CI

### New CI jobs

Add to `.github/workflows/rust.yml`:

```yaml
sim_unit:
  name: IB sim unit tests
  runs-on: ubuntu-latest
  steps:
    - uses: actions/checkout@v5
    - run: cargo test -p midas-ib-sim --lib

sim_integration:
  name: IB sim integration scenarios
  runs-on: ubuntu-latest
  needs: sim_unit
  steps:
    - uses: actions/checkout@v5
    - run: cargo test -p midas-ib-sim --test '*'

sim_app_e2e:
  name: Full app-to-sim end-to-end
  runs-on: ubuntu-latest
  needs: [sim_integration, desktop]
  steps:
    - uses: actions/checkout@v5
    - run: cd desktop/win && cargo build -p midas-app --features dev_harness
    - run: cargo build --release -p midas-ib-sim
    - run: cargo test --test app_sim_e2e -- --test-threads=1
```

The `app_sim_e2e` test suite:
- Spawns `midas-ib-sim-server`
- Spawns `midas-app --features dev_harness`
- Via devloop, runs a scripted scenario
- Asserts final UI state via `DumpState` + screenshot SSIM checks

Target runtime: **all 3 jobs under 10 minutes**. Sim scenarios run under virtual clock so duration is bounded by scenario complexity, not wall time.

### Platform coverage

- **Linux (GitHub runners)**: primary CI — sim + app
- **Windows (optional matrix)**: app workspace is Windows-primary; sim as a separate process runs fine on Linux but the app requires Windows. For the `app_sim_e2e` job, we either: (a) wait for Windows runner availability, (b) run the app in a Windows VM in-CI, (c) use a headless iced backend on Linux. Phase 1 picks (a) for correctness.

## Feature parity gate

Before declaring Stage 09 complete, run this checklist:

| Test | Against sim | Against real IB paper | Parity? |
|------|-------------|----------------------|---------|
| Connect + MANAGED_ACCTS + NEXT_VALID_ID | | | |
| Farm-status bulletins appear within 500ms | | | |
| reqCurrentTime round-trip | | | |
| reqContractData for AAPL returns conId | | | |
| reqMktData streaming, 60s of ticks received | | | |
| Market order fills, OrderStatus + Execution arrive | | | |
| Limit order rests, cancels cleanly | | | |
| Bracket order: parent + TP + SL full lifecycle | | | |
| reqHistoricalData returns 1-day of minute bars | | | |
| reqPositions returns current positions snapshot | | | |
| 50 msg/sec burst triggers error 100 + disconnect | | | |
| reqMktData for 101st symbol returns error 10197 | | | |

For each row: run the test against both backends, record the event stream, `diff` them. Diff should be "semantically equivalent" — same message types in the same order, with content matching modulo timestamps, request IDs, and perm IDs.

Any `N/A` row for real-IB (e.g., "triggers error 100 on demand" — can't deterministically reproduce against real IB) is OK if documented.

## Parallelism within this stage

| Sub-team | Scope |
|----------|-------|
| **A** | CI workflow + test harness for `sim_integration` job |
| **B** | Devloop TCP commands + sim child-process lifecycle |
| **C** | `app_sim_e2e` test suite — at least 3 end-to-end scenarios |
| **D** | Parity gate — manual test runs against real IB to validate each row above |

## Rollback signals

- Sim and real-IB event streams diverge on any row of the parity gate → stage incomplete; go back to the responsible stage (02/04/05) and fix.
- Dev-loop commands feel awkward (test authors write too much glue) → expand the DSL, don't expect users to contort around it.
- CI `app_sim_e2e` tests flake (pass/fail non-deterministically) → virtual clock leak somewhere, or a race in the child-process lifecycle.

## Kill criteria

- **Parity gate has > 3 permanent failures** → sim is not actually drop-in compatible; document which code paths the app must gate on sim-vs-real and keep going anyway (degraded acceptance).
- **`app_sim_e2e` runtime exceeds 20 minutes** → integration tests have scope creep; reduce per-test coverage, add faster unit tests in the responsible stage.

## Deliverables

- All 3 CI jobs green on main
- Parity gate: every row green against sim, documented N/As for real-IB-undeliverable rows
- Dev loop docs updated in `desktop/win/plan/devloop-spec.md` with sim commands
- `midas-app` configured to connect to sim via a single config flag
- One end-to-end scenario filmed (screen recording) showing the sim driving the app through a full bracket lifecycle in 30 seconds of accelerated virtual time
