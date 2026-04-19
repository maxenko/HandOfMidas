# Stage 06 — Failure Injection + Scenario Scripts

*YAML scenarios that drive the sim through scripted sequences — disconnects, pacing violations, bad data, slow fills, flash crashes. The test-author's DSL.*

**Depends on**: 02 (protocol), 04 (order lifecycle), 05 (quirks)
**Blocks**: 09 (integration)
**Parallel-safe with**: 07

## Scope

Every scripted behavior the sim can exhibit is expressible in a YAML scenario file. Scenarios are:
- Git-diffable (plain text)
- Deterministic (seed + scenario = identical sim behavior)
- Composable (one scenario can `include` another)
- Versioned (schema has explicit version number; migrations are possible)
- Validateable (schema errors caught at load, not at runtime)

## Scenario schema

```yaml
version: 1
name: "Bracket submitted during farm outage"
description: "Tests that the client correctly queues order intent when 1100 fires mid-submit."
seed: 42
clock: virtual
duration: 5min

symbols:
  - symbol: AAPL
    preset: MidCap
    initial_price: 175.00
  - symbol: SPY
    preset: Liquid
    initial_price: 450.00

accounts:
  - acct_code: "DU1234567"
    starting_cash: 100000.00

quirks:
  msg_rate:
    limit_per_sec: 50
  line_limit:
    max_l1_lines: 100

events:
  # Time-absolute events (HH:MM:SS in virtual time from session start)
  - at: 00:00:05
    do: subscribe_market_data
    args: { symbol: AAPL, subscription: streaming_l1 }

  - at: 00:00:10
    do: accept_order
    args: { order_kind: bracket, side: buy, quantity: 100, entry: market,
            tp_offset: +2.00, sl_offset: -1.00 }

  # Time-relative events (after a previous named event)
  - named: outage_start
    at: 00:01:30
    do: inject_farm_outage
    args: { code: 1100, farms: [usfarm] }

  - after: outage_start
    delay: 45s
    do: inject_farm_restore
    args: { code: 1101, farms: [usfarm] }  # 1101 means "data lost, re-subscribe"

  # Pattern-triggered events (fire when a condition becomes true)
  - when: "orders[0].status == Filled"
    do: assert
    args: { cond: "positions[AAPL] == 100", message: "Position should be 100 after fill" }

  # Fault injection
  - at: 00:02:00
    do: inject_pacing_violation
    args: { session_id: 0 }  # emits error 100 + disconnects client 0

  - at: 00:02:30
    do: inject_lag
    args: { session_id: 0, duration: 5s }

  - at: 00:03:00
    do: inject_bad_frame
    args: { session_id: 0, bytes: "hex:deadbeef" }  # tests client's framing robustness

  - at: 00:04:00
    do: inject_price_jump
    args: { symbol: AAPL, magnitude_pct: -3.5 }

asserts:
  # Run at session end
  - cond: "all_orders_have_terminal_status"
  - cond: "no_orphan_bracket_children"
  - cond: "session_duration <= 5min"
```

## Event verbs

The scenario DSL has a closed set of verbs. Adding a verb is a breaking schema change (version bump).

### Setup verbs
- `subscribe_market_data` — mimics a client's `reqMktData`
- `unsubscribe_market_data`
- `accept_order` — sim injects an order as if a client sent `placeOrder`
- `cancel_order`

### Injection verbs (faults)
- `inject_disconnect` — close one or all sessions
- `inject_farm_outage` — emit 1100
- `inject_farm_restore` — emit 1101 or 1102
- `inject_pacing_violation` — emit error 100 + disconnect
- `inject_lag` — freeze the session's event stream for a duration
- `inject_bad_frame` — send malformed bytes on the wire
- `inject_price_jump` — one-shot mid-price jump for a symbol
- `inject_gap` — open-gap style; mid-price teleports from A to B
- `inject_halt` — stop generating ticks for a symbol for a duration
- `inject_burst` — temporarily crank market-data rate (test throughput)
- `inject_duplicate_order_status` — one-shot duplicate emission
- `inject_slow_commission_report` — delay commission report by a scripted duration
- `inject_out_of_order_events` — emit Y before X despite logical ordering
- `inject_daily_restart`

### Control verbs
- `sleep` — virtual time fast-forward (also implicit between `at:` events)
- `set_clock_mode` — switch real/virtual/accelerated mid-scenario
- `include` — compose another scenario file

### Assertion verbs
- `assert` — expression over sim state (orders, positions, sessions)
- `assert_client_received` — check wire bytes sent to client contain a specific message
- `assert_client_event_order` — verify the sequence of emitted OutgoingMsg against a pattern

## Expression language

A tiny expression language for `when:` and `assert` predicates. No Turing completeness. Domain types: `Order`, `Position`, `Session`, `Symbol`.

```
orders[0].status == Filled
positions[AAPL].quantity > 0
sum(orders[*].filled_qty) == 200
session[0].msg_count_since(last_5s) > 100
```

Implemented via a hand-rolled PEG parser + interpreter (~300 LOC with `nom` or `pest`). No `eval` / no embedding Lua.

## Scenario runner

```rust
pub struct ScenarioRunner {
    scenario: Scenario,
    engine_tx: mpsc::Sender<EngineCmd>,
    engine_events: broadcast::Receiver<EngineEvent>,
    clock: Arc<dyn Clock>,
    named_anchors: BTreeMap<String, VirtualInstant>,
    pending_when_clauses: Vec<(Expression, Action)>,
}

impl ScenarioRunner {
    pub async fn run(&mut self) -> Result<ScenarioResult, ScenarioError> {
        loop {
            let next_time = self.next_event_time();
            let next_when = self.evaluate_when_clauses()?;

            match earliest(next_time, next_when) {
                Some(ScheduledAction::Fixed { at, action }) => {
                    self.clock.advance_to(at).await;
                    self.execute_action(action)?;
                }
                Some(ScheduledAction::WhenFired(action)) => {
                    self.execute_action(action)?;
                }
                None => break,
            }
        }
        self.run_final_asserts()
    }
}
```

## Canonical scenarios (shipped fixtures)

The `fixtures/scenarios/` directory ships a baseline suite every contributor can run:

| File | Purpose |
|------|---------|
| `smoke.yaml` | Connect, subscribe, fill one order, disconnect cleanly. |
| `bracket_happy.yaml` | Bracket submitted, parent fills, TP hits, SL cancels. |
| `pacing_violation.yaml` | Client spams requests, gets error 100, disconnects. |
| `farm_outage_mid_order.yaml` | 1100 during order submission, 1101 after, verifies re-subscription. |
| `fast_market.yaml` | High `lambda_base`, market order sees Pattern B (execDetails before OrderStatus). |
| `flash_crash.yaml` | -10% jump + halt + restore; tests chart + UI resilience. |
| `daily_restart.yaml` | 11:45 PM virtual disconnect, client reconnects, orders survive, market data re-requested. |
| `line_limit_overflow.yaml` | Subscribe 101 tickers, verify 101st gets error 10197. |
| `partial_fill_drift.yaml` | Large market order, multiple partial fills interleaved with drifting mid-price. |

Each scenario has a companion `.expected.jsonl` recording the expected outgoing-message sequence for regression testing.

## Expression evaluation over live engine state

The expression language needs a read-only view of the engine:

```rust
pub trait ScenarioQuery {
    fn orders(&self) -> &[OrderSnapshot];
    fn positions(&self) -> &BTreeMap<SymbolKey, Position>;
    fn session_metrics(&self, session: SessionId) -> SessionMetrics;
    fn ticks_since(&self, sym: &SymbolKey, since: VirtualInstant) -> usize;
}
```

Engine implements `ScenarioQuery` via a snapshot mechanism — scenarios never mutate engine state directly; they always go through commands.

## Schema validation

Scenarios are validated at load time:
- Unknown verbs → hard error
- Type mismatches in args → hard error
- Dangling `after:` references → hard error
- Circular `include:` → hard error
- Version mismatch → fail loudly with migration hint

Schema defined once in `scenario/schema.rs` with `serde` derives. Auto-generate JSON Schema from Rust types for editor tooling (VS Code YAML extension loads it, gives autocomplete).

## Schema versioning + migration

The scenario schema evolves. Scenarios in the wild (fixtures in other branches, users' local test scripts) must not break silently when we add or rename a verb.

**Versioning discipline**:

- **Every scenario YAML declares `version: N`** (top-level, required).
- **v1 is frozen on release.** Any change that would alter semantics is a v2.
- **Breaking changes** (verb rename, arg rename, semantics change): bump version, author a migration in `scenario/migrations/v1_to_v2.rs`. Migrations are pure functions `fn migrate_v1_to_v2(yaml: V1Scenario) -> V2Scenario`.
- **Additive changes** (new verb, new optional arg): *not* a version bump; new verb is tagged with `since: "v1.2"` in doc-comments and skipped with a warning on older schema versions.

**Loader logic**:

```rust
pub fn load(path: &Path) -> Result<Scenario, ScenarioError> {
    let raw: serde_yaml::Value = serde_yaml::from_reader(File::open(path)?)?;
    let version = raw.get("version").and_then(|v| v.as_u64()).unwrap_or(1) as u32;
    match version {
        1 => {
            let v1: V1Scenario = serde_yaml::from_value(raw)?;
            Ok(migrate_v1_to_current(v1))
        }
        2 => serde_yaml::from_value(raw).map_err(Into::into),
        v if v > CURRENT_VERSION => Err(ScenarioError::VersionTooNew { got: v, max: CURRENT_VERSION }),
        _ => Err(ScenarioError::UnsupportedVersion(version)),
    }
}
```

**Deprecation window**: a verb is deprecated for at least one minor version before removal. Removal is always a major-version bump. Removal PRs include the migration + update every fixture that uses the removed verb.

**Cross-version fixture test**: CI runs every past-version fixture in `fixtures/scenarios/legacy/v1/` and asserts they load + run successfully after migration. Catches migration regressions.

**JSON Schema per version**: `fixtures/scenarios/schema/v1.json`, `schema/v2.json`. Editor tooling picks the right schema by the document's declared version.

## Parallelism within this stage

| Sub-team | Scope | LOC |
|----------|-------|-----|
| **A** | YAML schema + loader + validator | ~400 |
| **B** | Verb dispatch + scenario runner | ~500 |
| **C** | Expression parser + interpreter | ~300 |
| **D** | Canonical scenario fixtures + expected-output recording | ~200 + YAML |

## Rollback signals

- Scenario YAML grows > 1000 lines for simple tests → DSL is too verbose; add composite verbs.
- Contributors bypass the DSL to inject faults via direct Rust API → DSL is missing a common verb; add it.
- `.expected.jsonl` files diff unreadably → the runner isn't producing deterministic output; find the RNG leak.

## Kill criteria

- **Cannot express a canonical test scenario (e.g., "farm outage mid-order") after 2 weeks** → DSL design failed; switch to Rust test macros and drop YAML.
- **Scenario runner becomes a bottleneck** (adding 50% to test runtime) → simplify the expression evaluator; cache parsed scenarios.

## Deliverables

- All 9 canonical scenarios run green in CI
- Schema docs at `plan/ib-sim/references/scenario-reference.md`
- JSON Schema export at `fixtures/scenarios/schema.json`
- `cargo test -p midas-ib-sim --test scenario_canonical` runs every canonical scenario and asserts expected output
