# Slice 9 — Cleanup

**Goal.** Delete deprecated code, update documentation, tighten the public API surface.

## Scope

### A. Delete deprecated trait

`crates/midas-broker/src/client.rs`:
- Delete `BrokerClient` trait.
- Delete `BrokerCallback` enum.
- Delete `TestBroker` (old impl, if still present).
- Delete `poll_callbacks` model entirely.

### B. Retire `BrokerEngine`

`crates/midas-broker/src/engine/mod.rs`:
- Delete `BrokerEngine` struct.
- Delete `start_broker_engine` function.
- Delete the 10ms poll_callbacks loop.
- Delete `BrokerCommand` variants that are no longer used (historical dispatch is now direct on `IbMarketData`).

Replace with a thin `midas-broker::start_backends(config) -> (Arc<dyn MarketDataSource>, Arc<dyn OrderClient>)` helper that instantiates both based on config.

### C. Retire `BrokerBridge`

`desktop/win/crates/midas-app/src/broker_bridge.rs`:
- Delete `BrokerBridge`.
- Delete `broker_event_stream`.
- Delete `broker_conn_stream` (replaced by `router.connection_state()` watch subscription).
- Delete `BrokerEventSource`, `BrokerConnSource`.

### D. Retire `CandleBuffer::apply_tick`

It was renamed to `apply_bar` in S7. Delete any remaining `apply_tick` shim.

### E. Retire `ProviderRegistry`

`desktop/win/crates/midas-app/src/app.rs::ProviderRegistry`:
- Replaced by `Arc<MarketDataRouter>` directly on `MidasApp`.
- Delete.

### F. Documentation updates

- `CLAUDE.md` — update "Key Architecture Patterns" section:
  - Remove mediator description.
  - Add MarketDataRouter section with the topology diagram from `plan/market-data-router/01-architecture.md`.
- `README.md` (repo root) — same.
- `desktop/win/CLAUDE.md` — update.
- `crates/midas-broker/doc/` — update API docs to reflect new traits; retire old trait pages.
- Move `plan/market-data-router/` → `plan/archive/market-data-router/` after merge. Leave `plan/archive/market-data-router/00-index.md` as the historical record.

### G. Rust-module hygiene

- Every public item has a doc comment (mandatory per project conventions).
- No `pub use` re-exports of internal types.
- `#[deprecated]` markers from S2 removed (the deprecated items are gone now).

### H. Tests sweep

- Delete any test that exercised `BrokerBridge`, `broker_event_stream`, `active_market_subs`, or `apply_tick`.
- Re-run the full suite (root + desktop) and verify green.

## Acceptance

- `git grep -n BrokerClient`, `BrokerBridge`, `broker_event_stream`, `active_market_subs`, `apply_tick`, `ensure_market_subscriptions` — all zero matches outside `plan/archive/`.
- `cargo test --workspace` green on both workspaces.
- `cargo clippy --workspace --all-targets -- -D warnings` clean.
- `cargo doc --workspace --no-deps` — builds without warnings.
- `CLAUDE.md` diff reviewed — architecture section now reflects router topology.

## Risks

- **Accidental API breakage** — if any external consumer (e.g. `app_sim_e2e` in dev_harness) held a reference to `BrokerBridge`, compilation fails. Grep before delete; fix call sites.
- **CLAUDE.md diff** — may conflict with other in-flight doc work. Resolve on merge.
