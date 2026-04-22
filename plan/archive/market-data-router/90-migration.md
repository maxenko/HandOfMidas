# Migration Strategy

The refactor is structured so that **every slice leaves the app buildable and runnable**. There is NO branch where `cargo run -p midas-app` fails. Tests stay green per-slice.

## Slice-by-slice state

### After S0 (prep)

`mailbox_processor` lives at the root workspace. `ibapi = "=2.10"` pinned. iced 0.14 subscription API confirmed (scratch POC compiled and ran, then deleted). No behavioural change to the app. Build + tests green on both workspaces. (BR-16, BR-17, BR-10.)

### After S1 (core types)

New types exist in `midas-broker-core::market_data`. Nothing consumes them yet. Existing code untouched. Build + tests green.

### After S2 (new traits)

New traits exist in `midas-broker`. Old `BrokerClient` still exists. Nothing uses the new traits. Build + tests green.

### After S3 (sim backend)

`SimMarketData` + `SimOrderClient` exist in `crates/midas-broker/src/sim/`. Old `TestBroker` still exists. Both are valid. The app still uses the old one. Build + tests green. New fidelity tests pass independently.

### After S4 (IB backend)

`IbMarketData` + `IbOrderClient` exist in `crates/midas-broker/src/ib/`. Old `IbClient` still exists. Both are valid. The app still uses the old one. Build + tests green.

### After S5 (router)

`midas-market-data` crate exists with `MarketDataRouter`. Router can be constructed but the app doesn't use it. Unit tests use `SimMarketData` directly. Build + tests green.

### After S6 (aggregator)

`BarAggregatorRegistry` exists in `midas-market-data`. Router exposes `subscribe_bars`. App doesn't use it. Build + tests green.

### After S7 (app migration — the big-bang)

**This slice deletes the old central `BrokerEvent::Tick` match arm, the `active_market_subs` field, `ensure_market_subscriptions`, etc.** Between starting and finishing S7, the app may be broken temporarily (in-progress branch). Land S7 as ONE commit.

App now uses `MarketDataRouter` for all market data. Old `BrokerEngine` still runs for order events (order_client is instantiated through the old engine path until S9 replaces it).

Build + tests green at slice end.

### After S8 (iced polish)

FrameCoalescer, snapshot resync, visibility filters. Incremental improvement over S7. Build + tests green.

### After S9 (cleanup)

Old `BrokerClient`, `BrokerEngine`, `start_broker_engine`, `BrokerBridge`, `ProviderRegistry` deleted. `CLAUDE.md` updated. Plan archive moved. Build + tests green.

## Ordering flexibility

- **S0 is strictly first** — all subsequent slices assume the mailbox_processor move + ibapi pin are in place.
- **S1 and S2 are NOT parallelizable** (M-37): S2 depends on S1's type definitions.
- **S3 and S4 can run in parallel** after S2. Two independent agents: one on sim backend, one on IB backend.
- **S5 depends on S3** for testing (S5's router tests use the sim). But the router itself doesn't depend on the IB impl being done.
- **S6 depends on S5** (registry needs router). Can start as soon as S5's public API is stable.
- **S7 depends on S5 + S6**. Cannot parallelize with them.
- **S8 can start in parallel with S7** once S7's subscription-helper skeleton is checked in.
- **S9 is strictly last**.

Revised ordering: **S0 → S1 → S2 → S3 || S4 → S5 → S6 → S7a–e → S8 → S9**.

## Commit granularity

One commit per slice. Each commit:
- Has a focused message: "router refactor: slice N — <headline>".
- Leaves the workspace green.
- Is independently revertable.

S7 is the biggest commit (maybe 2k LOC). Consider splitting into sub-commits that each leave the tree buildable:
- S7a: add subscription helpers, don't wire them.
- S7b: wire chart subscription, leave old Tick handler untouched.
- S7c: wire watchlist subscription.
- S7d: wire ticker subscription.
- S7e: delete old Tick handler + `active_market_subs` + `ensure_market_subscriptions`.

Each of S7a-e can be its own commit, all landing on the same branch, all green.

## Rollback plan

If S7 proves unstable in QA, the last safely-mergeable point is S6. S7's big-bang nature is unavoidable — you can't half-route a tick — but because the old path stays functional through S5/S6, rolling back means reverting S7's single commit (or the S7e sub-commit specifically).

After S9, there is no rollback: the old code is gone. Land S9 only after S7+S8 have soaked.

## Feature flag?

**No.** The plan deliberately avoids a `router_v2` feature flag. Per the user's directive ("no half measures"), the big-bang swap is in S7 and the cleanup is S9. A feature flag would prolong the dual-path maintenance burden and inevitably leave dead code paths.

## CI gates per slice

Each slice's PR must pass:
1. `cargo test --workspace` (root).
2. `cargo test --workspace` (desktop).
3. `cargo clippy --workspace --all-targets -- -D warnings` (both workspaces).
4. `cargo fmt --all -- --check` (both workspaces).
5. Manual smoke: `cargo run -p midas-app --features dev_harness`, visually verify watchlist prices move (S7+) or unchanged behavior (S1-S6).

No slice lands if any gate fails. If a slice uncovers a problem in an earlier slice's design, back up and fix the earlier slice first.
