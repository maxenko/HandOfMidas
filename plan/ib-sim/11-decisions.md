# Stage 11 — Architectural Decision Records

*One ADR per consequential choice. Each entry answers: what was decided, what alternatives were considered, why this one won, and what would trigger re-evaluation.*

## ADR format

Every ADR has these fields:

- **Status**: `proposed` | `accepted` | `superseded`
- **Date**: ISO-8601
- **Supersedes**: ADR-NNN (optional — lists ADRs this one replaces)
- **Superseded by**: ADR-NNN (optional — filled in when this ADR is later replaced)
- **Related**: ADR-NNN, ADR-MMM (optional — cross-references)
- **Context**: why a decision is needed
- **Decision**: the chosen option
- **Alternatives considered**: rejected options with rejection reasons
- **Consequences**: trade-offs accepted
- **Re-evaluation trigger**: observable condition that would prompt reconsideration

When an ADR is revised: create a new ADR with `Status: accepted` and `Supersedes: ADR-NNN`; mark the old one `Status: superseded` and `Superseded by: ADR-MMM`. Never edit an accepted ADR's decision in place.

## ADR-001: Separate process with TCP port (not embedded library)

**Status**: accepted · **Date**: 2026-04-18

### Context

We need a full-parity IB test broker. Two shapes are plausible:

1. **Embedded library** — implements `BrokerClient` + `MarketDataSource` traits; linked into `midas-broker`; no wire protocol involved.
2. **Separate process speaking TWS wire protocol** — `rust-ibapi` connects to it exactly as it would connect to real IB.

### Decision

Separate process with TCP port 7497.

### Alternatives considered

- **Embedded library (rejected)**: simpler, faster (no IPC), trivial to test in-process. But it **skips the entire `ib_client.rs` code path** — pacing, handshake, version negotiation, reconnect, framing. The most likely source of prod bugs is the part we'd silently bypass. Also can't be shared between `midas-app`, CI, and devloop as a single running instance.
- **Unix domain sockets** (rejected): cross-platform concern on Windows (UDS exists but iced/Windows dev flow doesn't use it). TCP is strictly more general.
- **In-memory channel faking the whole broker interface** (rejected): explicitly the library-level mocking pattern every Python IB mock adopts. Research showed this produces tests that pass locally and break in production.

### Consequences

- Pro: exercises full IB client code path, supports multi-client, testable from any language.
- Pro: process-level failure injection (kill -9 the sim to simulate crash recovery).
- Con: ~100µs IPC overhead per message (negligible at our scale).
- Con: requires spawning/managing a separate process in dev loop and CI.

### Re-evaluation trigger

Process management becomes a bigger pain point than IPC overhead (observation threshold: if dev-loop startup / teardown reliably adds > 2s to iteration cycle).

---

## ADR-002: Rust, new crate `midas-ib-sim` in broker workspace

**Status**: accepted · **Date**: 2026-04-18

### Context

The codebase is multi-language-capable (rusqlite + DuckDB C deps). Choosing the sim's language influences dev velocity and dependency tax.

### Decision

Rust. New crate `midas-ib-sim` in root (broker) workspace. Binary target `midas-ib-sim-server`.

### Alternatives considered

- **Python** (rejected): ecosystem has mature `ib_insync` but no TWS server implementation; we'd be writing most of it anyway, and Python's async + TCP story is worse than Tokio.
- **Go** (rejected): excellent TCP server ergonomics but introduces a second language in the repo for one crate. Not worth it unless the Rust tokio path proves inadequate.
- **C++** (rejected): matches IB's own samples language, but zero synergy with the rest of the Hand of Midas codebase.
- **TypeScript/Node** (rejected): poor fit for binary protocols and low-level determinism.

### Consequences

- Pro: shared `midas-broker-core` types; single language; cargo workflow.
- Pro: zero additional C deps for the sim itself.
- Con: none identified.

### Re-evaluation trigger

Rust async ecosystem has a major shift (e.g., Tokio is retired) — unlikely in a 6-month window.

---

## ADR-003: Wire protocol — text framing only, no protobuf (v201+)

**Status**: accepted · **Date**: 2026-04-18

### Context

TWS wire protocol is text-framed at v100–v200, adds selective protobuf at v201+. `rust-ibapi` currently requires `MIN_VERSION ≥ 201`.

### Decision

Advertise version range `v176..v221`. Implement text framing only. Never emit protobuf.

### Alternatives considered

- **Text-only with narrower range `v176..v201`** (rejected): fragile — `rust-ibapi`'s next MIN_VERSION bump silently breaks us. Widening to 221 gives headroom.
- **Full v201+ protobuf implementation** (rejected): doubles the codec surface for features we don't need (push notifications, new algo types). Defer until blocked.
- **Pin `rust-ibapi` to a specific version that supports v176** (rejected): creates an upgrade lock that slows `midas-broker`'s dependency hygiene.

### Consequences

- Pro: ~40 messages × text codec is tractable; protobuf adds schemas + breaking changes.
- Pro: can widen version range without a codec change, only a constant.
- Con: future `rust-ibapi` versions may require a protobuf message we can't emit. When that happens, add only the specific protobuf message; don't rewrite the whole codec.

### Re-evaluation trigger

A `rust-ibapi` feature we need (or a `midas-broker` feature) requires a v201+ protobuf message. At that point add only the specific required messages; text framing still handles everything else.

---

## ADR-004: Synthetic data — Roll-GARCH-U, not ABIDES or pure replay

**Status**: accepted · **Date**: 2026-04-18

### Context

Synthetic tick generation has a spectrum from "naive random walk" to "agent-based LOB simulation" (ABIDES, JPMorgan 2019).

### Decision

Four-layer classical model: Roll bid-ask bounce + GARCH(1,1) volatility + Hawkes-lite arrival clustering + intraday U-shape. ~500 LOC Rust.

### Alternatives considered

- **Naive GBM** (rejected): fails all 6 stylized-facts tests. Creates false confidence.
- **ABIDES** (rejected): full agent-based LOB simulation with matching-engine dynamics. Overkill — Hand of Midas consumes L1, not L2. ABIDES is a Python framework, and a Rust port would be massive. Plus: research on the plan doc confirms ABIDES is reference for academic *research*, not UI/broker testing.
- **Pure session replay** (rejected as sole strategy, kept as hybrid mode): 1:1 fidelity but covers only recorded scenarios. Can't stress-test throughput or inject rare events.
- **Heston / SABR stochastic volatility** (rejected): derivatives pricing models, not tick generation. Expensive (more parameters, more sampling cost) with no UI-visible benefit at Tier 1.
- **GAN-based generators (Quant GANs)** (rejected): research-grade; 10× the code + training data + GPU dependency.

### Consequences

- Pro: covers Tier 1 stylized facts with bounded complexity. 6 validation tests catch regressions automatically.
- Pro: hybrid mode (replay + synthetic perturbations) gets the best of both strategies.
- Con: can't simulate realistic L2 depth. Explicit non-goal.

### Re-evaluation trigger

Hand of Midas starts consuming L2 depth data, or testing requires realistic options-chain dynamics.

---

## ADR-005: Replay format — Databento `dbn`, not Parquet or custom

**Status**: accepted · **Date**: 2026-04-18

### Context

Need a format for recorded + replayed market-data ticks.

### Decision

Databento `dbn` format (via the `dbn` Rust crate).

### Alternatives considered

- **Apache Parquet / Arrow IPC** (rejected): excellent for analytics, awkward for temporal streaming replay. Row-vs-columnar tradeoff is wrong for our access pattern.
- **CSV (LOBSTER format)** (rejected): human-readable but massive disk usage and slow parse. Fine for academic work, not for CI fixtures.
- **Custom binary** (rejected): NIH. `dbn` already solves this problem.
- **Raw `.pcap`** (rejected as primary): kept as a secondary format for wire-level regression testing, but not our main tick format.

### Consequences

- Pro: Rust-native crate, zstd compressed, schema'd, streaming-friendly. Interop with Polars, DuckDB, arrow-rs.
- Pro: Databento publishes the format; stability guaranteed.
- Con: adds one external crate dependency.

### Re-evaluation trigger

Databento deprecates the format (unlikely — it's their commercial output format) or a Hand of Midas feature requires per-symbol metadata `dbn` doesn't model.

---

## ADR-006: Scenario DSL — YAML with closed verb list

**Status**: accepted · **Date**: 2026-04-18

### Context

Test authors need a way to script scenarios against the sim. Options span from "hand-rolled Rust tests" to "full programming language embedded (Lua, WASM, JS)."

### Decision

YAML schema with a closed verb list + a tiny expression language for `when:` / `assert:` predicates. Versioned schema with migration policy.

### Alternatives considered

- **Rust test macros** (rejected): each scenario is a Rust function. Flexible but loses the git-diffable, human-editable property. Can't share fixtures with non-Rust users (e.g., Python IB notebook users).
- **Embedded Lua / Python** (rejected): Turing-complete DSL is a classic scope-creep trap. Users start writing business logic in the test harness.
- **QuickFIX `.scen` format** (rejected as-is): FIX-specific; we'd end up re-specifying it for TWS.
- **JSON instead of YAML** (rejected): YAML's multi-line strings and anchors are valuable for scenarios that reference fixtures. JSON is fine for serialization but bad for authorship.

### Consequences

- Pro: git-diffable, reviewable, deterministic.
- Pro: closed verb list means the DSL can't rot into an embedded-language disaster.
- Pro: versioning + migration policy keeps old fixtures running.
- Con: adding verbs requires schema bump + possibly a migration.

### Re-evaluation trigger

Test authors repeatedly escape the DSL (writing glue Rust instead of scenarios). Signal: > 10% of sim tests are Rust harness, not YAML scenarios.

---

## ADR-007: Single-threaded engine actor

**Status**: accepted · **Date**: 2026-04-18

### Context

The engine owns all state (sessions, orders, market-data generators, quirks, scheduler). Concurrency model matters for throughput and determinism.

### Decision

One tokio task owns all engine state. Sessions are per-connection tasks that send commands via `mpsc` and receive events via `broadcast`.

### Alternatives considered

- **Sharded actors** (per-symbol, per-session) (rejected): more throughput but kills determinism (cross-shard event ordering becomes non-trivial). Not needed at our scale (tens of sessions, thousands of ticks/sec).
- **Lock-free state with atomics** (rejected): concurrency bugs in trading simulators are catastrophic. Single-owner state is easier to reason about + debug.
- **Async runtime other than Tokio** (rejected): Tokio is the default; no reason to differ.

### Consequences

- Pro: perfect determinism; easy mental model.
- Pro: matches `midas-broker::BrokerEngine` pattern; consistent architecture.
- Con: throughput ceiling bounded by single-core CPU. Mitigated by moving market-data generation to a sibling actor if RealClock throughput bench fails.

### Re-evaluation trigger

RealClock throughput bench fails (< 1000 events/sec sustained). Mitigation: split market-data generator into its own actor feeding the engine via a bounded channel.
