# IB Simulator — Progress Tracker

*Live tracker for the full-parity Interactive Brokers TWS simulator feature arc. Updated as milestones land.*

## Arc status

**Current phase**: Research + planning
**Started**: 2026-04-18
**Target**: Production-ready Rust simulator that speaks TWS wire protocol, runs as a standalone process, and supports deterministic market-data replay, failure injection, and full-parity testing of `midas-broker`.

## Milestone timeline

| # | Milestone | Status | Commits / PRs | Eval verdict |
|---|-----------|--------|---------------|--------------|
| 0 | Research: 4 parallel agents (TWS protocol, IB quirks, existing sims, microstructure) | **complete** | `research/*.md` × 4 | — |
| 1 | Plan draft: index + 10 stage files + progress.md | **complete** | `00-index.md` + `01-10.md` + `progress.md` | — |
| 2 | Plan-eval cycle 1 (4 parallel agents) | in progress | — | — |
| 3 | Plan-eval cycle N (iterate until SOLID) | pending | — | — |
| 4 | Stage A: core skeleton (process shell, CLI, tokio runtime, TCP listener) | pending | — | — |
| 5 | Stage B: wire protocol codec (frame encode/decode, versioning, handshake) | pending | — | — |
| 6 | Stage C: deterministic virtual clock + event scheduler | pending | — | — |
| 7 | Stage D: market data engine (synthetic generator + session replay) | pending | — | — |
| 8 | Stage E: order lifecycle simulator (fills, brackets, state machine) | pending | — | — |
| 9 | Stage F: quirk modeling (pacing, line limits, async ordering) | pending | — | — |
| 10 | Stage G: failure injection (disconnect, lag, pacing violation, bad data) | pending | — | — |
| 11 | Stage H: session recording (capture real IB sessions for replay) | pending | — | — |
| 12 | Stage I: integration with `midas-broker` + devloop | pending | — | — |
| 13 | Stage J: CI wiring + cross-platform build | pending | — | — |
| 14 | Code scrutiny pass on each completed large chunk | ongoing | — | — |

Each stage is a plan file in this directory. See [00-index.md](00-index.md).

## Parallelism map

The stages are ordered by dependency, but many can run in parallel once Stage B (codec) lands. Full map in [10-rollout.md](10-rollout.md).

## Decisions log

Recording critical calls that were made autonomously vs. bounced to the user.

| Date | Decision | Made by | Rationale |
|------|----------|---------|-----------|
| 2026-04-18 | Simulator runs as **separate process** with TCP port, speaking TWS wire protocol (not in-process library). | Claude (autonomous) | User signaled openness; separate process gives: multi-client support, shared use across UI + CI, exercises full `ib_client.rs` code path, deterministic replay across restarts, process-level failure injection. In-process would skip the IB client's pacing / handshake / reconnect logic entirely. |
| 2026-04-18 | Target language: **Rust**, new crate in root workspace. | Claude (autonomous) | Codebase is Rust; reuse types from `midas-broker-core`; no C dep tax. |
| — | Port number default (7497 paper / 7498 sim / other) | pending | flag in plan for user confirmation |

## Risks / unknowns

Recorded as encountered, closed when resolved.

| # | Risk / unknown | Status |
|---|----------------|--------|
| R1 | TWS protocol is undocumented by IB; community references vary in quality. | Research phase address |
| R2 | Wire-format version drift — must pick a target version range. | Research phase addresses |
| R3 | `rust-ibapi` client may send request shapes we haven't anticipated. | Research phase addresses |
| R4 | Synthetic data realism — naive random walks give false confidence. | Research phase addresses |
| R5 | Cross-platform build (Windows primary, Linux CI) — does IB's own TWS run on Linux cleanly? Separate process makes this irrelevant for the sim side. | Mitigated by separate-process choice |

## Eval results log

Each plan-eval + code-scrutiny cycle logged here with verdict + cycle-over-cycle improvement.

| Cycle | Target | Verdict | Critical | High | Medium | Low | Praise |
|-------|--------|---------|----------|------|--------|-----|--------|
| 1 | Full plan (4 agents: structure / soundness / best-practices / executability) | NEEDS REFINEMENT | 0 | 6 | 12 | 5 | 7 |
| 1 refinements applied | All 6 High + 12 Medium + 4/5 Low resolved | — | 0 | 0 | 0 | 1 partial | — |
| 2 | Full plan re-eval after refinements (4 agents) | NEEDS REFINEMENT | 0 | 2 | 6 | 8 | 5 |
| 2 refinements applied | All 2 High + 6 Medium + 7/8 Low resolved | — | 0 | 0 | 0 | 1 partial | — |
| 3 | Confirmation re-eval (2 agents: soundness + executability) | **SOLID** (after 1 residual fix) | 0 | 0 | 1 | 0 | 0 |
| 3 residual fix applied | MarketEmission / OrderEmission propagated through 03, 04, and 01 deliverables | — | 0 | 0 | 0 | 0 | — |

### Cycle 3 summary

**Soundness agent**: "All cycle 2 soundness issues resolved. Plan is SOLID from a soundness perspective." All 7 items verified line-by-line.

**Executability agent**: Wave 1.5 M3 spike has real teeth (owner, budget, fallback all concrete), extension-enum pattern works, flake policy operational. One residual Medium — tick-push contract split was frozen in Stage 01 but not propagated to Stages 03/04 (still used old unified `Emission`). Mechanical fix applied: `Emission` → `MarketEmission` in 03, `Emission` → `OrderEmission` in 04 with `on_tick()` → `on_market_snapshot(&MarketSnapshot)`, plus 2 residual `Emission` mentions in Stage 01 corrected.

**Final verdict**: SOLID. Plan ready for execution.

### Cycle 2 summary

**Highs resolved** (both in Stage 04):
- Jitter-seed canonical: `hash(order_id, step_idx)` declared authoritative; reconciled against earlier `hash(event_type, order_id)` mention
- Const_assert invariant + runtime `debug_assert!` two-layer enforcement + proptest over jitter extremes

**Mediums resolved** (all 6):
- Tick-push contract: `MarketEmission` + `OrderEmission` named distinctly; `MarketSnapshot` intermediate type for mid/bid/ask/last flow from 03 → 04; pull-based `step()` model clarified
- M3 mini-spike promoted to named "Wave 1.5" with owner, 2–3 day budget, entry criteria, fallback if slipped
- Central-type amendment: extension-enum pattern (`*Ext`) added for Wave 2 in-flight additions; 3-day budget in critical path
- Scheduler wake-latency race: bounded to one loop iteration with explicit paragraph note
- Throughput threshold: split into single-session (2,000/sec) + 100-symbol fanout (5,000/sec aggregate) benches
- Error codes: single `error_codes.rs` table with `VERIFIED`/`[unverified]` tags so capture becomes one-file diff

**Lows resolved** (7 of 8):
- 00-index.md version range drift `v176..201` → `v176..221`
- Decisions/index dedup: index table now references ADR numbers + notes ADR file as source of truth
- GARCH eps_grid vs eps_tick independence documented as deliberate with reasoning
- ADR template enhanced: Status, Supersedes, Superseded-by, Related fields
- Jitter sub-ms gap note re: scheduler `seq` tie-breaker added
- `EngineCmd` duplication: §Engine actor shape marked illustrative; canonical declaration in §Central types frozen
- Wave 4 entry gate: 72h fallback + ≤1 non-repro flake + 3-per-week ceiling

**ODQ consistency** (still partial, Stage 01 only): documented as by-design in progress. Stage-specific open questions resolve inline via rollback signals + kill criteria per stage; ODQ section consolidation deferred as cosmetic.

### Cycle 1 summary

**Highs resolved** (all 6):
- Security model added to 01 (bind localhost only, bearer token on control plane, `--listen-external` opt-in)
- Data lifecycle policy added to 07 (retention, pre-commit hook, CI grep, incident runbook)
- Version range widened `176..201` → `176..221` with drift strategy
- Pattern B event ordering: jitter invariant pinned; max_jitter < min(gap) so consecutive steps cannot reorder
- Central enums frozen in Stage 01 (all variants declared at scaffold time; Wave 2 agents fill bodies, never add variants)
- Tick-push contract `MarketDataEngine → OrderSimulator` declared in Stage 01

**Mediums resolved** (all 12):
- Decision alternatives: new `11-decisions.md` with 7 ADRs
- Backwards-compat: scenario schema versioning + migration policy (06); wire-protocol version-drift strategy (02)
- Observability: tracing span hierarchy + `/control/dump` + `/control/metrics` endpoints (01)
- Scheduler cancel-safety: `peek_deadline` + `pop_if_due` primitives; engine `select!` only awaits cancel-safe futures (08)
- RealClock throughput bench: 1,000 events/sec sustained, p99 < 10ms as Stage 01 deliverable
- `[unverified]` error codes: pre-release gate captures from real IB (05)
- `Framed` open question: closed — committed to `tokio_util::codec::Framed` (01)
- GARCH-Hawkes time-basis: GARCH on fixed 1-second grid, tick-scale by sqrt(dt) (03)
- Critical-path estimate: 19 → 20–22 days parallel; realistic 4–6 weeks (10)
- Stage 06 interpreter: moved into Wave 2 against mock engine (10)
- M3 gate: early real-IB spike capture in Wave 2 unblocks the gate (10)
- Eval cadence: budgeted, ≤6 hours across the arc; spot-check for scaffold milestones, full eval only at complexity-landing milestones (10)

**Lows resolved** (4 of 5): TCP robustness (01), cargo-fuzz (02), Wave 4 entry gate (10), data lifecycle (07). Open Design Questions consistency: partially — Stage 01 only.

**Praise** (7): plan structure uniformity, determinism rigor, wire-byte fixture corpus, actor+tokio pattern, Framed choice, ABIDES scoping, parallelism granularity.

## Implementation results log

Per-stage landed commits, test counts, follow-up items.

| Stage | Branch | Commits | LOC +/- | Tests added | Eval verdict | Scrutiny verdict |
|-------|--------|---------|---------|-------------|--------------|------------------|
| — | — | — | — | — | — | — |
