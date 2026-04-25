# Architecture-Review Fixes — Execution Plan

Plan target: the consolidated P1 action list from `plan/architecture-review/00-index.md`. Three groups, eleven slices, designed for maximum parallel execution before the next big feature lands.

| Group | Goal | Files |
|-------|------|-------|
| A — Types extraction | Unblock slice 9c by moving load-bearing non-chart types out of `midas-chart` first | [`01-group-a-types-extraction.md`](01-group-a-types-extraction.md) |
| B — IB pre-flight | Fix three bugs that will fire on first real IB connection | [`02-group-b-ib-preflight.md`](02-group-b-ib-preflight.md) |
| C — CI coverage | Close the 5-feature CI hole + add Windows runner | [`03-group-c-ci-coverage.md`](03-group-c-ci-coverage.md) |

## Dependency map

```
A1 (annotation types) ─┐
A2 (GPU types)         ├─→ unblocks slice 9c (existing chart-transition plan)
A3 (parity baseline)   ┤
                       └─ (soft) ── C2 (Windows runner)
                                     ↑
                                     │ runs the chart_parity_tests job

B1 (Ready semantics)   ─┐
B2 (disconnect policy) ├─→ unblocks Phase-1 IB paper trading
B3 (live-guard assert) ─┘

C1 (feature-flag jobs) ─┐
C2 (Windows runner)    ├─→ standalone CI hardening
C3 (root rust-toolchain)─┘
```

**Independence claim.** All eleven slices can be **drafted in parallel**; their file edits do not collide. The execution-order constraints are:

- A1b depends on A1 (consumer migration after types move).
- A2b depends on A2 (same pattern for GPU types).
- A1+A1b+A2+A2b+A3 must all land before slice 9c (atomic legacy deletion) can ship.
- A3 must land before C2 wires the `chart_parity_tests` step — without A3, the step has no test to run. C2 can ship with the parity step `if: false`-gated and flip when A3 lands.
- All other slice pairs are independent.

## Slice list

| ID | Slice | Crate(s) touched | Est. LoC | Risk |
|----|-------|-----------------|----------|------|
| A1 | Extract annotation types into `midas-annotation-types` | new crate + `midas-chart` (deprecated shim) | ~350 moved + ~50 shim | Medium — serde wire format |
| A1b | Migrate consumer imports off `midas-chart` annotation shims | `midas-app/**` import updates | ~30 import-only edits | Low |
| A2 | Extract GPU instances into `midas-gpu-types` | new crate + `midas-chart` (deprecated shim) + `midas-render` + `session_chart/primitives_bridge.rs` | ~250 moved + ~50 shim | Medium — `Pod`/`Zeroable` layout |
| A2b | Migrate consumer imports off `midas-chart` GPU-instance shims | `midas-render/**` + `session_chart/**` import updates | ~10 import-only edits | Low |
| A3 | Capture pre-9c parity reference + Windows-only parity test | `desktop/win/tests/data/parity-baseline/reference/` + new test | ~100 test + N PNGs | Low |
| B1 | Gate `Ready` on first `FarmStatus { code: MarketDataFarmOk, connected: true }` | `crates/midas-broker/src/ib/market_data.rs` | ~30 + 1 test | Low |
| B2 | Router disconnect policy on `RecvError::Closed` only | `crates/midas-market-data/src/router/{actor,publisher,state}.rs` | ~80 + 1 test | Medium — design choice |
| B3 | Defence-in-depth live-port guard in `IbMarketData::connect` | `crates/midas-broker/src/ib/{market_data,config}.rs` + new error variant | ~25 + 3 tests | Low |
| C1 | Three feature-flag CI lint jobs | `.github/workflows/rust.yml` | ~60 YAML | Low |
| C2 | Windows desktop CI job (incl. parity test once A3 lands) | `.github/workflows/rust.yml` | ~40 YAML | Low — first-pass non-blocking |
| C3 | Pin root toolchain via `rust-toolchain.toml` | new file at repo root | ~5 lines | Low |

## Design decisions (cross-cutting)

### D1. Where do the new type crates live? — Desktop workspace, not root

**Context:** `midas-annotation-types` and `midas-gpu-types` are consumed by `midas-app`, `midas-chart`, `midas-render`, and the `session_chart` module — all in the desktop workspace. The root workspace has no consumer.

**Decision:** Both new crates land under `desktop/win/crates/`.

**Why:** Adding a leaf crate to the root workspace would force every root crate (broker, market-data, sim, mailbox_processor, midas-clock, etc.) to participate in dependency resolution for types they don't use. Desktop-side keeps the recompile blast radius scoped.

**Confidence:** high.

### D2. Re-export shims during transition — `#[deprecated]`, with consumer-migration sub-slices

**Context:** Moving types breaks every `use midas_chart::widget::Annotation` site at once. Hard-cutting all consumers in one PR is unreviewable; leaving shims unchecked is the classic strangler-fig failure mode where shims persist forever.

**Decision:**
1. The shim re-exports inside `midas-chart` are marked `#[deprecated(note = "import from midas_annotation_types directly; midas-chart is being deleted in slice 9c")]`. Every existing import site emits a compiler warning until migrated.
2. Consumer-side import migration is an explicit sub-slice (A1b for annotation types, A2b for GPU types) with a grep-based acceptance criterion: `grep -r "midas_chart::widget::Annotation" desktop/win/crates/` returns only the shim definition itself.
3. `cargo clippy --workspace -- -D warnings` running in CI converts the deprecation warnings into hard errors, mechanically forcing migration on any new file that touches these imports.

**Why:** The deprecated-attribute + clippy-deny-warnings pair is a forcing function. Shims cannot persist quietly; every PR that touches a shim path either migrates the import or gets red CI. Grep-gate as a CI step is unnecessary — the type system + clippy already enforce it.

**Confidence:** high.

### D3. Disconnect policy — tear down only on full upstream close, NOT on farm transitions

**Context:** Review 2 R2 asked for a defined behaviour for upstream `Disconnected`. There are two distinct events that look adjacent but are not:

- **Full upstream close** (`broadcast::error::RecvError::Closed` on the publisher's stream): the `MarketDataSource` dropped its sender. The connection is gone. There is nothing more coming until reconnect.
- **Farm transition** (`FarmStatus { code: MarketDataFarmInactive, connected: false }` on the `farm_status_tx` lane): IB gateway routinely emits transient farm-down/up cycles in the seconds-long range. The connection is still up; data may be temporarily unavailable.

**Decision:** Slice B2 tears down `SymbolHub`s **only on full upstream close**. Farm transitions remain a side-channel signal on the existing `farm_status_tx` broadcast — no behaviour change for them. Each `SymbolHub` whose upstream closed publishes `EndReason::Disconnected` on its tick / RT-bar lanes, drops itself from `state.per_symbol`, and lets remaining `SubscriptionHandle`s observe `Closed`. Consumers (chart widget, watchlist, `TickerState`) re-subscribe on next user action.

**Why:** Tearing down on every farm blip would cause massive resubscription churn and visible UI data gaps even when the provider recovers in a few seconds. Treating only true upstream close as teardown matches the existing channel semantics — `RecvError::Closed` already means "no more data ever" regardless of connection state.

**Alternative considered:** Retain hubs even on full close, silently re-subscribe on reconnect. Rejected because it hides connection-state changes from consumers — the chart would silently miss data during the gap. Reconnect policy is a UI-layer choice; the router stays mechanism, not policy.

**Confidence:** high.

### D4. Live-guard placement — both at config validation AND at adapter connect

**Context:** Review 2 R3 wanted defence-in-depth. The TOML guard catches misconfigured config files; the adapter-side check catches programmatic mutation and direct `IbMarketDataConfig` construction.

**Decision:** Add a runtime check at the start of `IbMarketData::connect` that re-derives `is_live_port && !allow_live` and returns `MarketDataError::LiveTradingNotConfirmed` (new variant). Thread `allow_live` through `IbMarketDataConfig` so the adapter has the data it needs.

**Why:** Asserting on a panic would be wrong — this is a recoverable misconfiguration, not a programmer error. A typed error lets callers surface it cleanly.

**Confidence:** high.

### D5. CI rollout — non-blocking first, scheduled flip with a hard date

**Context:** Existing `broker_ib_live_feature_lint` job uses `continue-on-error: true` and has stayed that way indefinitely. Without a forcing function, every new non-blocking job becomes permanent noise.

**Decision:**
1. All three new feature-flag jobs (C1) and the Windows runner job (C2) ship with `continue-on-error: true`.
2. The C1/C2 PR descriptions include a hard date 14 days after merge for the flip-to-required follow-up. The merger files a calendar reminder or a `gh issue` with that date.
3. The flip PR is a one-line `continue-on-error: true` → `false` change per job, gated on "ten consecutive green runs since merge". If a job has flaked even once in those 14 days, do not flip; investigate first.
4. The existing `broker_ib_live_feature_lint` job is also flipped in this same PR — it has been stable for the entire chart-transition period and the precedent of leaving it `continue-on-error` forever is what we are explicitly correcting.

**Why:** A non-required check with no scheduled promotion is noise forever. Hard date + commit-to-flip prevents drift.

**Confidence:** medium — the operational discipline (calendar reminder, manual flip PR) is a process gap. If it slips, the jobs stay non-blocking. Acceptable; alternative is no rollout.

## Out of scope

These are deliberately not part of this plan:

- **`MidasApp` God-object split** (Review 3 P2 #5–#8). Real work, scheduled after 9c lands.
- **`midas-scene` annotation-layer file split** (Review 1 P2a). Can wait until the next tool or indicator.
- **`MarketDataError` taxonomy** (Review 2 R4). Cosmetic; do during the next major broker refactor — this plan does add ONE new variant (`LiveTradingNotConfirmed`) but does not split `Other(String)` into a full taxonomy.
- **Subscription registry unification** (Review 3 P2 #9). Doesn't block anything.
- **R21 tick-only aggregator unification** (Review 2 R6). Design exercise; not a fix.
- **Slice 9c deletion itself.** A separate PR / plan, gated on Group A completing.
- **`AnnotationStore` (runtime container) move.** It lives in `midas-app`, not `midas-chart`, so it does not block 9c. If 9c reveals a hidden reference, it is a one-line import update, not a slice.
- **Automatic reconnection on disconnect.** Slice B2 emits a typed `Disconnected` event but does not implement automatic reconnect. Consumers decide; the UI layer (or a future supervisor) is the right home for retry policy.
- **A full bincode/redb/postcard schema audit beyond annotation types.** Slice A1 audits the annotation type tree across all serde formats; it does not audit other type trees. If the audit surfaces unexpected encodings, treat as A1 scope; otherwise, separate work.

## Acceptance gate (whole plan)

- All 11 slices have landed PRs.
- Both workspaces still green: `cargo test --workspace` (root + desktop), `cargo clippy --workspace -- -D warnings` (both).
- New CI jobs visible in Actions tab; passing under `continue-on-error: true`.
- Round-trip test for annotation persistence (reading existing `data/annotations/*.json` after type move) is in CI.
- Pre-9c parity baseline images exist on disk under `desktop/win/tests/data/parity-baseline/reference/`; comparison test wired into the Windows CI runner.
- The four IB integration tests added by B1/B2/B3 pass via the sim.
- `grep -r "midas_chart::widget::Annotation" desktop/win/crates/` returns only the shim definition itself (consumer migration complete).
- `grep -r "midas_chart::instances::" desktop/win/crates/` returns only the shim re-export (consumer migration complete for GPU types).

### End-to-end smoke gate

A single test in the dev-harness suite confirms all moving parts integrate:

- Boot dev harness, load a fixture exercising annotation rendering (bracket on a chart with horizontal levels).
- Capture screenshot.
- Compare against the slice-A3 reference baseline (SSIM ≥ 0.995, diff_fraction ≤ 0.002).
- This test runs on the Windows CI runner from C2, exercising A1+A2 (moved types) + A3 (baseline) + B1/B2/B3 (IB sim path) end-to-end. If the smoke test goes red and individual slice tests are green, suspect cross-slice integration.

When all of the above is true, the chart-transition slice 9c PR can open with reduced scope and the IB Phase-1 paper-trading work can start.

## Review notes

- **Why not one-shot a "delete midas-chart" megatickets?** Because the crate holds 21 350 LoC across 462 tests including non-chart types that are still consumed in production code paths. A megaticket means a megareview, which means megaregression risk. Group A turns it into a sequence of three reviewable PRs.
- **Why three IB fixes instead of waiting for the IB integration sprint?** Because the bugs are present in code that runs against the sim today. They could surface in the wrong test scenario and burn debugging time. Fixing them now while the context is fresh is cheaper than later.
- **Why the toolchain pin (C3)?** Because every CI failure on root-workspace code currently has a "is it the floating stable that bumped?" risk built in. A 5-line file removes that ambiguity forever.
