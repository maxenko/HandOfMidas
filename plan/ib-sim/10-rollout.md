# Stage 10 — Rollout + Parallelism Map

*The dependency graph, wave-by-wave delivery plan, milestone gates, kill criteria, and the critical path.*

## Dependency DAG

```
                        ┌───────────────────┐
                        │ 01 Architecture   │
                        │ (scaffold + types)│
                        └─────────┬─────────┘
                                  │
               ┌──────────────────┼──────────────────┐
               ▼                  ▼                  ▼
        ┌────────────┐     ┌────────────┐     ┌────────────┐
        │02 Protocol │     │08 Clock    │     │06 Scenarios│
        │  Layer     │     │            │     │ (skeleton) │
        └─────┬──────┘     └─────┬──────┘     └─────┬──────┘
              │                  │                  │
      ┌───────┼──────────────────┼──────────┐       │
      ▼       ▼                  ▼          ▼       │
  ┌──────┐┌──────┐          ┌────────┐ ┌────────┐   │
  │03 MD ││04 Ord│          │05 Quirks│ │07 Rec │   │
  │Engine││Life  │          │         │ │ording │   │
  └───┬──┘└──┬───┘          └───┬────┘ └───┬────┘   │
      │     │                   │          │        │
      └─────┴───────────────────┴──────────┴────────┘
                            ▼
                  ┌───────────────────┐
                  │ 06 Scenarios      │
                  │ (full DSL + runs) │
                  └─────────┬─────────┘
                            ▼
                  ┌───────────────────┐
                  │ 09 Integration    │
                  │ (app + CI + parity)│
                  └───────────────────┘
```

## Wave-by-wave plan

Each wave is a set of stages that can start in parallel. A stage enters a wave only when all its dependencies are complete.

### Wave 0 — Foundation (serial)
- **Stage 01** — Architecture scaffold. One agent, ~2 days.
- **Gate**: `cargo build -p midas-ib-sim` green with stubs; module boundaries established.

### Wave 1 — Core plumbing (3 parallel agents)
- **Stage 02** — Protocol layer (3 sub-agents: framing, incoming, outgoing)
- **Stage 08** — Clock + scheduler (1 agent)
- **Stage 06 skeleton** — Scenario YAML schema + loader (1 agent)
- **Gate**: `handshake_e2e` test green — real `rust-ibapi` client connects to our sim

### Wave 2 — Engines (4 parallel teams, each 2–4 sub-agents)
- **Stage 03** — Market data engine (4 sub-agents: generator, replay, hybrid, validation)
- **Stage 04** — Order lifecycle (4 sub-agents: state machine, fills, brackets, accounts)
- **Stage 05** — Quirks (5 sub-agents: msg rate, line limit, historical pacing, farm status, config)
- **Stage 07** — Session recording (4 sub-agents: pcap format, dbn integration, proxy, calibration)
- **Stage 06 DSL** (can start in parallel against a mock engine) — Expression interpreter + verb dispatch (2 sub-agents). The interpreter consumes the `ScenarioQuery` trait which Stage 01 froze; canonical fixtures wait for Wave 2's engines to land.
### Wave 1.5 — M3 spike (named, budgeted, owned)

A real-IB paper-gateway capture is a prerequisite for M3's parity gate, but the full Stage 07 tooling lands at M6. To unblock M3 without backloading it, a **2–3 day mini-spike** runs between Waves 1 and 2:

| Field | Value |
|-------|-------|
| **Owner** | User (or designated engineer with IB paper account access) |
| **Entry criteria** | Stage 02 handshake_e2e green (so proxy mode can frame the connection); control-plane token infra from Stage 01 ready (for anonymization tool) |
| **Budget** | 2 working days for a skilled engineer, 3 for a first-timer |
| **Deliverable** | One anonymized `fixtures/sessions/m3_spike.dbn` covering ≥ 30 min of 09:30-10:00 ET market-open traffic across AAPL, SPY, and one small-cap |
| **Fallback if it slips** | M3's parity gate falls back to a synthetic-only stylized-facts pass. Parity against captured data defers to M6. Stage 03 still ships; the M3 re-eval notes "parity gate deferred to M6" in `progress.md`. |
| **Risk** | Paper account provisioning, TWS install on a Windows box, network-trace tooling can all introduce 1–2 day delays per hurdle |

The spike does NOT depend on Stage 07's full tooling — it uses a minimal proxy: `tokio::io::copy_bidirectional` from port 7497 → real IB, with an inline `dbn` encoder for outbound market-data messages only. ~100 LOC throwaway. Stage 07 later replaces this with the production implementation.
- **Gate**: all per-stage E2E tests green; stylized-facts validation passes; `rust-ibapi` round-trips a market order end-to-end

### Wave 3 — Canonical scenarios + DSL completion (2 parallel agents)
- **Stage 06 finalization** — Canonical fixtures (9 YAML scenarios) + `.expected.jsonl` recordings. Depends on Wave 2 engines existing so the runner can drive real scenarios.
- **Gate**: all 9 canonical scenarios run green in CI

### Wave 4 — Integration (3 parallel agents)
**Entry gate** (required before any Wave 4 sub-team starts): all Wave 2 + Wave 3 E2E tests green on `main` for **≥ 48 h**, OR **72 h with ≤ 1 documented non-reproducible flake**. A flake restarts the 48h clock only if it's reproducible on second run; non-repro flakes are logged in `progress.md` under a "flake log" subsection and counted against the 72h window. This avoids Stage 09 being starved by an unreproducible tick-timing test that fires once every 200 runs. The 72h fallback also has a hard ceiling: no more than 3 non-repro flakes per 7-day rolling window; past that, stop Wave 4 prep and triage the flake source first.

- **Stage 09** A — CI workflow + test harness
- **Stage 09** B — Devloop integration
- **Stage 09** C — `app_sim_e2e` suite
- **Stage 09** D — Parity gate (manual real-IB test runs)
- **Exit gate**: parity gate filled out; `midas-app` drives the sim through a full bracket lifecycle via dev loop

## Maximum parallelism peak

Wave 2 is the widest: **17 agents working simultaneously** across 4 stages. Each agent owns a narrow vertical slice (tests included). Worktree isolation prevents clobbering.

Sub-agent allocation in Wave 2:

| Stage | Sub-agents | Total |
|-------|-----------|-------|
| 03 | generator, replay, hybrid, validation | 4 |
| 04 | state machine, fills, brackets, accounts | 4 |
| 05 | msg rate, line limit, historical pacing, farm status, config | 5 |
| 07 | pcap, dbn, proxy, calibration | 4 |
| **Wave 2 total** | | **17** |

Each agent runs in `.claude/worktrees/ib-sim-<stage>-<sub>/`, commits on a branch, reports back. Merges happen serially on `main` at the end of each stage (not end of each sub-agent).

## Milestone ladder + evaluation gates

Every milestone triggers a re-eval:

| M | Milestone | Eval cadence | Target |
|---|-----------|--------------|--------|
| M0 | Plan SOLID (4-agent plan-eval cycle) | 4-agent full eval | Plan itself |
| M1 | Stage 01 lands | **1-agent spot-check** (scaffold only; no behavior) | Architecture file + stubs |
| M2 | Stage 02 + 08 + 06-skeleton land | **1-agent spot-check** | Protocol handshake + clock determinism |
| M3 | Stage 03 lands | **4-agent full eval + code-scrutiny** (complexity lands here) | Stylized facts; parity against `m3_spike.dbn` |
| M4 | Stage 04 lands | **4-agent full eval + code-scrutiny** | Order lifecycle E2E; Pattern B reproduction |
| M5 | Stage 05 lands | **2-agent medium eval** | Quirks: every T1 has captured-real-IB validation |
| M6 | Stage 07 lands | **1-agent spot-check** (mostly tooling) | Proxy mode captures + deterministic replay |
| M7 | Stage 06 full (all canonical scenarios) | **2-agent medium eval** | Scenario DSL stress — can express every test we've thought of |
| M8 | Stage 09 lands | **4-agent full eval + code-scrutiny** | Parity gate ≥ 90% green vs real IB |
| M9 | Production readiness | Post-mortem review (no formal eval) | `midas-app` uses sim daily for off-market dev |

### Re-eval cadence — budgeted

The plan allocates **eval overhead ≤ 15% of calendar time**. Mechanics:

- **Full eval (4 agents)**: ~60 min wall clock + synthesis time. Triggered only at M0/M3/M4/M8 (4 events).
- **Medium eval (2 agents)**: ~30 min wall clock. Triggered at M5/M7 (2 events).
- **Spot-check (1 agent)**: ~15 min wall clock. Triggered at M1/M2/M6 (3 events).
- **Code-scrutiny**: runs on M3/M4/M8 code only — the milestones where real logic lands. ~45 min each.

Total eval overhead: ~6 hours of wall time across a 4–6 week arc. Not a week — 2% of calendar time. The earlier concern was "eval overhead could eat a week"; the fix is to not re-run full 4-agent evals on scaffold-only milestones.

**Parity-gate updates** (M8 iteration): update `progress.md`'s parity-gate table each test run. These are cheap; no formal eval attached.

## Kill criteria (feature-arc-level)

| If after N weeks... | Consider |
|---------------------|----------|
| M2 (protocol) not landed by week 3 | Wire protocol is harder than expected; consider FORK-then-vendor `rust-ibapi` in reverse (use its codec verbatim as a server-side library) |
| M4 (order lifecycle) can't reproduce Pattern B | Drop the non-atomic ordering as a T1 quirk; document as known gap |
| M5 (quirks) slips past M6 | Quirks are a bottomless pit; ship with T1 only, move T2 to follow-up feature arc |
| M6 (recording) produces non-replayable captures | Accept that wire-level replay is too ambitious; use domain-level (dbn) only, skip the `.pcap` layer |
| Parity gate stuck below 70% at M8 | Sim is not usable as drop-in; keep it as a dev aid but require real IB for CI integration tests |

## Critical path

The longest serial chain:

```
01 → 02 → 04 → 06 full → 09
```

Estimated days: 2 (01) + 5 (02) + 4 (04) + 3 (06) + 4 (09) = **~18 working days** worst-case, assuming no parallelism.

With parallelism (Wave 2 runs 4 stages at once):

```
01 → 02 → (03 || 04 || 05 || 07) → 06 full → 09
2   + 5  +         6-7           + 3       + 4  = 20-22 days
```

The bottleneck is Wave 2's longest stage. Stages 05 and 07 each total ~1400–1600 LOC with internal serial dependencies (config + trait first, then parallel leaves, then integration) — realistically 6–7 working days per stage, not 5. Stage 04 is ~1550 LOC. All three are plausible critical-path candidates; don't assume 04 is the longest just because it's architecturally central.

**Realistic total: 4–6 weeks** with full parallelism (20–30 working days including integration, eval, and scrutiny overhead), 7–10 weeks serial. Conservative target; flag critical-path drift early.

## Risk ranking

| Risk | Likelihood | Impact | Mitigation |
|------|-----------|--------|------------|
| `rust-ibapi` v201+ protobuf is actually required | Medium | High | Implement text-only first, add protobuf path only if blocked |
| IB protocol has undocumented quirks we miss | High | Medium | Recording-corpus-driven testing (Stage 07) catches these |
| Synthetic data fails stylized-facts tests | Medium | Medium | Validation suite catches at PR time; tuning budget of ~1 week |
| Scenario DSL becomes a DSL maintenance burden | Low | Medium | Lock verb list at v1; no churn without feature-flag |
| CI can't run the parity gate against real IB | High | Low | Parity gate is a local-manual task; CI only runs sim-vs-sim |
| Cross-platform binary (Windows app + Linux sim in CI) | Medium | Low | Sim is a process; talks TCP; platform doesn't matter |

## Roadmap beyond M9

Once the sim is production-ready and adopted:

- **T2 quirks** — enable the feature flags, run regression suite against each
- **T3 quirks** — FA accounts, conditional orders, timestamp format modes
- **Protobuf messages** — add for v221+ features that matter (push notifications, new order types)
- **Multi-account support** — FA fan-out, sub-account subscriptions
- **L2 order book** — if/when Hand of Midas consumes depth data
- **Options** — when options trading lands in Hand of Midas
- **Performance tuning** — benchmarks at 10,000 ticks/sec across 1,000 symbols

## Deliverables for this plan stage (10)

- This file (✅)
- `progress.md` updates at every milestone
- Wave/gate dashboard kept live in `progress.md`
- Post-mortem after M9 capturing what worked / what didn't
