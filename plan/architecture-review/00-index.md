# Architecture Review — Hand of Midas

Repository-wide architectural audit performed after the chart-transition
plan landed (slices 0 through 9a; 9b/9c pending user-owned soak + atomic
deletion). Evidence cutoff: `b7a7485` on `main`. Four reviewers worked in
parallel across the four natural scopes below; this index consolidates
their findings into a single action list.

## Reports

| # | File | Scope | LoC reviewed | Key verdict |
|---|------|-------|--------------|-------------|
| 1 | [`01-session-aware-stack.md`](01-session-aware-stack.md) | Root-workspace `midas-clock` / `-calendar` / `-bars` / `-stream` / `-axis` / `-scene` / `-bars-adapter` + the desktop `session_chart` module | ~14k new | Strong. Acyclic DAG enforced by workspace split; traits carve joints cleanly. Main soft spots are `midas-scene` size and the legacy-type bridge in `session_chart::gpu_renderer`. |
| 2 | [`02-broker-router-sim.md`](02-broker-router-sim.md) | Root-workspace `midas-broker-core` / `midas-broker` / `midas-market-data` / `midas-ib-sim` / `mailbox_processor` | ~12k | Architecturally the strongest layer. RAII refcounts, `Arc<dyn MarketDataSource>`, live-trading guard. Four concrete issues to resolve before Phase-1 IB paper trading. |
| 3 | [`03-desktop-app-and-legacy.md`](03-desktop-app-and-legacy.md) | Desktop workspace: 11 crates, `midas-chart` legacy + `midas-app` shell | ~88.7k | Strangler-fig migration is on track but `midas-app/src/app.rs` (4 765 LoC / 63 fields / 112-variant `Message`) and `handlers.rs` (4 994 LoC) are God-objects drifting. `midas-chart` at 21 350 LoC / 462 tests cannot be atomically deleted without first moving annotation + GPU types out. |
| 4 | [`04-cross-cutting.md`](04-cross-cutting.md) | Non-component: config / persistence / tracing / tests / CI / feature flags / versioning / dev-harness / safety | both workspaces | Above-average for a single-developer project, but persistence paths sprawl across three `%LOCALAPPDATA%` folders, 5-of-7 feature flags have no CI ON-path coverage, and the only CI runner is Linux while the target is Windows 11. |

## Cross-cutting themes

Four patterns reappeared independently in multiple reports:

1. **The 9c deletion is a multi-slice project, not an atomic PR.** Review 1 (P1b), Review 3 (P1 #1–#4), and Review 4's versioning pressure all converge: `midas-chart` holds load-bearing non-chart types (`Annotation*`, `OrderBracket`, `PriceLine`, `LineStyle`, `LineExtent`, plus GPU instance structs). 231 refs to `CandleBuffer`, 17 files touching `Timeframe`. These have to migrate before the crate can go. A single "delete 9c" PR will not land clean.
2. **Trait-doc contracts vs. actual state transitions are drifting.** Review 2 (R1) flagged `IbMarketData::connect` marking `Ready` before farm-up; Review 2 (R2) flagged the router having no documented policy for upstream `Disconnected`. Both are doc-vs-reality gaps in the same layer.
3. **Feature-flag coverage is thinner than it looks.** Review 4 (R1) + Review 1's mention that `session_chart_tests` and `chart_parity_tests` gates exist to catch "harness false-positives" but are not wired into CI — exactly the scenario they were built for. Review 3's strangler-fig migration depends on parity tests that currently only run when an engineer remembers.
4. **Growth is concentrating in `midas-app` and `midas-scene`.** Review 1 (P2a) flagged `midas-scene::layers::annotations` at 1 342 LoC with 4 `InteractiveLayer` impls; Review 3 flagged `app.rs` / `handlers.rs` / `views.rs` as God-objects. Both need the same treatment (extract-controller-per-domain) before the next major feature.

## Consolidated P1 action list

Ranked by blast-radius and blocking relationship to near-term work
(Phase-1 IB paper connection + slice 9c). Each item cites its source
report so the detail and evidence are one hop away.

### Block or do alongside slice 9c

1. **Move `Annotation`, `AnnotationId`, `AnnotationKind`, `HorizontalLevel`, `OrderBracket` + bracket support, `PriceLine`, `LineStyle`, `LineExtent`, `LineStroke`, `Presence` out of `midas-chart`.** Home: `midas-core::annotation` (or a new `midas-annotation-types` crate if the `midas-core` recompile cost is a concern). Serde tag stability is load-bearing — round-trip every existing `data/annotations/*.json` under the new path. _(Review 3, P1 #1)_
2. **Move GPU instance types** (`CandleInstance`, `BadgeInstance`, `VolumeInstance`, `GridLineInstance`, `AxisLabel`, `CrosshairRender`, `TimelineLabel`) into `midas-render` or a new `midas-gpu-types` crate. `primitives_bridge.rs` (756 LoC) evaporates when `midas-render` consumes `midas_scene::ScenePrimitives` directly. _(Review 3, P1 #2)_
3. **Capture a real pre-9c screenshot baseline via the dev harness** (20-chart + single-chart reference images under the legacy stack). Slice 9c's PR must attach SSIM-diff proof that the new stack matches the captured baseline. _(Review 3, P1 #3; reinforces Review 1 P1b)_
4. **Do not delete `CandleBuffer` / `Timeframe` in 9c.** 231 refs to `CandleBuffer`, 17 files touching `Timeframe` — that is a follow-up migration, not a line item in the deletion PR. _(Review 3, P1 #4)_

### Before Phase-1 IB paper trading

5. **Fix `Ready` semantics in `IbMarketData::connect`.** Gate the transition on first `FarmStatus::Ok(MKT)`, or split into `OrderingReady` + `MarketDataReady` + `Ready` and update the trait doc. File: `ib/market_data.rs:138-141`. _(Review 2, R1)_
6. **Define router policy for mid-stream disconnects.** Add an explicit `handle_upstream_disconnect` to the control actor — either tear down all `SymbolHub`s and surface typed errors or retain hubs and re-subscribe on reconnect. Document the choice. File: `router/actor.rs`. _(Review 2, R2)_
7. **Harden the live-trading guard.** Add `assert!(port != 4001 || allow_live)` inside `IbMarketData::connect` as defence-in-depth against programmatic `BrokerConfig` mutation that bypasses TOML validation. File: `ib/market_data.rs:109`. _(Review 2, R3)_

### Before the second contributor

8. **Close the feature-flag CI hole.** Add jobs to `.github/workflows/rust.yml` for `session_chart`, `session_chart_tests`, `chart_parity_tests`, `dev_harness`, and `test_inject`. The existing `broker_ib_live_feature_lint` job is the pattern. Five of seven flags currently have no ON-path coverage — including the self-validation corpus written so the harness can't silently false-positive. _(Review 4, R1)_
9. **Unify persistence paths behind `midas-core::paths`.** One module returning the single root dir + subpath helpers; either fold the three top-level `%LOCALAPPDATA%` folders (`HandOfMidas\`, `midas\`, `midas-ib-sim\`) into one or document why they stay separate. ~10 `dirs::data_local_dir().join(...)` literals today. _(Review 4, R2)_
10. **Add `version = "..."` to cross-workspace path-deps.** One-line per consumer, nine consumers in `desktop/win/Cargo.toml`. Gives Cargo a compile-time guardrail for renames and semver bumps without publishing. _(Review 4, R3)_

### Documentation-only

11. **Document the `ExchangeCalendar` dual error policy.** One doc-comment explaining why `classify` is infallible (hot path) while `trading_day` is fallible (cold path). File: `crates/midas-calendar/src/exchange.rs:43-65`. _(Review 1, P1a)_

## P2 summary (next refactor window)

Collected from all four reports; see each file for detail and evidence.

- Split `midas-scene/src/layers/annotations.rs` (1 342 LoC, 4 `InteractiveLayer` impls) into `order_bracket.rs`, `price_line.rs`, `level.rs`, `decorator_layer.rs`. _(Review 1 P2a)_
- Refactor `ToolEffect` before the third tool: today's per-tool-per-variant does not scale. Move toward `CreateAnnotation(AnnotationSpec)` + `UpdateAnnotation(AnnotationId, AnnotationPatch)`. _(Review 1 P2b)_
- Move `midas-indicators` to the root workspace so `midas-scene::layers::indicator` can consume it instead of carrying duplicated ATR / G-ATR math. _(Review 1 P2c)_
- Rename `midas-bars-adapter` → `midas-broker-session-bridge`; current name understates scope. _(Review 1 P2d)_
- Split `MidasApp` into `AppShell` + per-domain controllers (`WindowGeometry` / `ToastController` already show the pattern). Target: `MidasApp` ~15 fields. _(Review 3 #5)_
- Split `Message` into sub-enums (`Message::Watchlist(WatchlistMsg)`, etc.); target top-level variants = 15–20. _(Review 3 #6)_
- Split `app/handlers.rs` (4 994 LoC) and `app/views.rs` (3 846 LoC) into per-domain files mirroring `dev_harness/`. _(Review 3 #7–#8)_
- Unify the two subscription registries into a single `SubscriptionContext` keyed by `(SymbolKey, Timeframe?)`. _(Review 3 #9)_
- Encode the `midas-store` two-tier pattern in types (`fire_and_forget_insert` returns `QueuedWrite`; `insert_candles` returns `Result<InsertOk, StoreError>`). _(Review 3 #10)_
- Taxonomise `MarketDataError`: split `Other(String)` into `Timeout { call }`, `Handshake(String)`, `ConnectFailed(String)`. _(Review 2 R4)_
- Unify `Lagged` policy across `SubscriptionHandle::recv` and `GuardedStream`. _(Review 2 R5)_
- Design R21 tick-only aggregator as per-provider opt-in (sim enables, IB keeps RT-bars) — parallel `SessionedBarAggregator`, don't remove the RT-bar faucet. _(Review 2 R6)_
- Adopt a semver policy for internal crates + root `CHANGELOG.md`. _(Review 4 R4)_
- Add `rust-toolchain.toml` at both workspace roots. _(Review 4 R5)_
- Add at least one Windows CI job (`windows-latest` runs desktop fmt + clippy + `cargo test --test app_sim_e2e -- --ignored`). _(Review 4 R6)_
- Split `plan/devloop-spec.md` into design doc + protocol reference; point the reference at `cargo doc` on `midas-devloop-proto`. _(Review 4 R7)_

## P3 summary (watch list / opportunistic)

- `midas-scene` split into `-core` + `-layers` + `-tools` when LoC crosses ~15k. _(Review 1 P3a)_
- Rename `SceneBuildError` → `BuilderError` inside `midas-scene`. _(Review 1 P3b)_
- Design non-time `BarSpec` (range / volume / tick bars) before the feature request arrives — do not extend `BarPeriod`. _(Review 1 P3c)_
- Reassess `Arc<RwLock<CandleSeries>>` vs `ArcSwap` if fan-out crosses ~100 panels. _(Review 1 P3d)_
- Extract `trait Indicator` when the third indicator lands. _(Review 1 P3e)_
- Outgoing-encoder fuzz target (`MarketEmission → encode → decode → MarketEmission`). _(Review 2 R7)_
- Extract RAII subscription pattern into `midas-broker-core::refcount` only when a second router materialises. _(Review 2 R8)_
- Tighten `TickerState` setter visibility (`pub(super)` / `#[cfg(test)]`) so the "only mutation path is `apply()`" invariant is un-escapable. _(Review 3 #11)_
- Retire legacy config fields (`AppConfig::order_blotters`, `AppConfig::levels`) with `skip_serializing_if`. _(Review 3 #12)_
- Plan retirement of the on-disk JSON annotation read path. _(Review 3 #13)_
- `const_assert!(size_of::<Message>() < 128)` to catch accidental payload bloat. _(Review 3 #14)_
- Consider a builder + typed layer-end indexing for `ChartScene`'s 28 `pub` fields if legacy lives longer than 9c. _(Review 3 #15)_
- Centralise the 30+ inline `target: "..."` strings per-crate (constant or `module_path!()` macro). _(Review 4 R8)_

## What is deliberately not in scope

- No rewrites recommended. Every P1 is surgical.
- No microservice / multi-process split suggested. The monolith is the right shape for this team size.
- No deviation from the ecosystem defaults (Tokio, iced, wgpu, DuckDB, tracing) recommended — existing choices are the community-default for their respective niches.
- The chart-transition plan itself is not re-reviewed here; it is the reference the reviewers measured against.

## Reading order for a new contributor

1. `00-index.md` (this file) — what's broken, what's great, ranked.
2. `02-broker-router-sim.md` — shortest; sets the bar for what "good" looks like in this codebase.
3. `01-session-aware-stack.md` — the current active construction zone.
4. `03-desktop-app-and-legacy.md` — where the God-objects live; needed before touching `app.rs`.
5. `04-cross-cutting.md` — read last, execute P1s first.
