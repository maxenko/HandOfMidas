# Cross-Cutting Architecture Review — Hand of Midas

Scope: non-component concerns across both workspaces (root + `desktop/win`).
Evidence cutoff: repository state on `main` @ `b7a7485`. Line counts via `Grep`
or `wc -l`; version stamps via `^version` in `**/Cargo.toml`.

---

## Summary

The project punches above its weight on cross-cutting: real migration
framework for `AppConfig` (numbered `v1→v2` steps with backup files),
dual-schema story for the chart-view store and devloop fixtures, JSON-over-TCP
dev harness protocol with round-trip tests, nightly fuzz targets plus a
weekly drift guard for the IB wire protocol, and a live-trading guard
layered twice (port 4001 + `allow_live`, sim port 7497 + `sim_allowed`).
Seven feature flags across the two workspaces, each documented in its
`Cargo.toml`.

The weak spots: **persistence sprawls across three top-level `%LOCALAPPDATA%`
folders with no shared paths module**; **five of seven feature flags have no
CI ON-path coverage** (the most load-bearing ones —
`session_chart_tests`, `chart_parity_tests`, `dev_harness`); **every crate is
`0.1.0` with no semver policy and no `rust-toolchain.toml`**; and the only
CI runner is `ubuntu-latest`, while the target platform per
`desktop/win/CLAUDE.md:6` is Windows 11. Overall cross-cutting health is
better than typical single-developer trading apps, but persistence-path
sprawl and CI coverage holes will bite hardest first.

---

## 1. Configuration

Three distinct config structs, no shared base module:

| Struct | Location | Format | Scope |
|---|---|---|---|
| `AppConfig` | `desktop/win/crates/midas-core/src/config/mod.rs` (918 LOC) | TOML | UI/pane-grid/charts/watchlists/broker/DuckDB |
| `BrokerConfig` | `crates/midas-broker/src/config.rs` | TOML | IB connection, order defaults, persistence, reconnect, trading limits |
| Sim config | `crates/midas-ib-sim/` scenario YAML + CLI flags | YAML | Sim runtime |

`AppConfig` carries `CURRENT_CONFIG_VERSION = 2` plus a real migration chain
(`config/migrations.rs`). On load, older versions walk `migrate_v{N}_to_v{N+1}`
in a loop; the framework writes a one-shot backup `config.toml.bak-v1-to-v2`
and logs which steps fired. Atomic writes via `tempfile::NamedTempFile` +
rename (`mod.rs:872-883`).

**Live-trading guard** implemented in two places. `BrokerConfig::validate()`
rejects `port == 4001 && !allow_live` (`config.rs:250-259`) with four
dedicated tests. `BrokerConnectionConfig::allow_live` mirrors it in the app
layer but has no second `validate()` — the app delegates. A third gate,
`ConnectionConfig::sim_allowed`, prevents accidental connection to a real
paper-trading gateway when the sim would happen to bind 7497. Belt + braces
+ safety net is appropriate for real-money tooling.

**Issues.** `BrokerConfig` has no version field and no migration chain; if it
ever needs one, the pattern will be copy-pasted rather than abstracted. The
sim (`midas-ib-sim`), broker (`midas`), and app (`HandOfMidas`) each pick a
different top-level folder name under `%LOCALAPPDATA%`.

---

## 2. Persistence

Files landing on disk:

| Artefact | Path | Format | Schema |
|---|---|---|---|
| App config | `%LOCALAPPDATA%\HandOfMidas\config.toml` | TOML | Versioned (v1, v2) with migration chain |
| Ticker state | `%LOCALAPPDATA%\HandOfMidas\ticker_state.redb` | redb v2 | v1→v2 inside the redb handle |
| Order history | `%LOCALAPPDATA%\HandOfMidas\order_history.redb` | redb v2 | Unversioned rows |
| Annotations (legacy) | `<data>/annotations/<SYMBOL>.json` | JSON | `AnnotationFile.version=1` — **deprecated since Slice 4** |
| Broker SQLite | `%LOCALAPPDATA%\midas\broker.db` | SQLite WAL | `midas-broker` schema — **different top-level folder** |
| DuckDB cache | `%LOCALAPPDATA%\HandOfMidas\cache.duckdb` | DuckDB | `midas-store` schema |
| Sim token | `%LOCALAPPDATA%\midas-ib-sim\control.token` | raw hex | None — **third top-level folder** |
| Sim session PCAPs | `crates/midas-ib-sim/fixtures/sessions/raw/` | zstd-DBN | **Gitignored; never commit** |
| Candle files | configurable | SoA binary | `midas-data` format, mmap |
| Devloop fixtures | `desktop/win/.devloop/fixtures/<name>.json` | JSON | `DEVLOOP_FIXTURE_VERSION=2` + per-envelope `schema` (v1→v2) |
| Devloop artifacts | `desktop/win/.devloop/` | JSONL + txt | `events.jsonl`, `panic.txt`, `app.<port>.pid`, `sim.<port>.pid` |
| Chart-view store | in-memory (not persisted) | — | `CHART_VIEW_STORE_SCHEMA_V2`; stamp in `AppConfig.chart_view_store_schema` for rollback coordination only |

**Sprawl.** Three `%LOCALAPPDATA%` folders for one application
(`HandOfMidas\`, `midas\`, `midas-ib-sim\`). Each hard-coded via
`dirs::data_local_dir().join("...")` in `app.rs:2039`, `app.rs:2087`,
`broker/config.rs:176`, `sim_child.rs:195`, `security.rs:55`, etc. Grep
finds ~10 such literal joins with no central helper.

**Schema-versioning — three conventions.** `AppConfig.version: u32` with
a chain; `FixtureEnvelope.devloop_fixture_version: u32` **plus** per-envelope
`schema: u32` (two orthogonal versions); `AnnotationFile.version: u32` as a
scalar. Each correct in isolation; none share a trait.

---

## 3. Observability (`tracing`)

`Grep` counts 245 `tracing::…!` call sites across 60 files (truncated).
Heaviest concentrators: `midas-scene` (26+ across layers/tools/interaction),
`desktop/win/midas-app/src/app.rs` (35 sites), `midas-ib-sim` (10+ across
`session/`), `midas-market-data/src/router/actor.rs` (10).

Explicit `target: "..."` strings are consistently namespaced by crate and
module: `midas_axis::format::price`, `midas_scene::tools::bracket`,
`midas_app::chart_view`, `midas_app::session_chart::widget`. 30+ distinct
targets — the right shape for downstream `RUST_LOG` filter composition.
Targets frequently use narrower forms than `module_path!()` (e.g.
`midas_scene::crosshair::off_chart` for code inside
`midas_scene::layers::crosshair`), which is deliberate and useful.

Subscriber setup in two binaries (`midas-app`, `midas-ib-sim-server`) via
`tracing_subscriber::fmt().with_env_filter(...).init()`. `midas-app` defaults
to `midas=debug,wgpu=warn` when `RUST_LOG` is unset (`main.rs:51-56`) — good
onboarding behaviour.

**Gaps.** Mixed structured vs. `format!`-interpolated messages
defeat span-based queries. No `#[instrument]` on `MarketDataSource` /
`OrderClient` trait methods — spans at the trait boundary would give
sim-vs-IB comparison for free. The dev harness has its own `events.jsonl`
separate from the `tracing` stream; two logs per process makes post-mortem
harder than necessary.

---

## 4. Testing

`Grep` of `#[test]|#[tokio::test]` returns **2,285+ occurrences across 200
files** (truncated). `CLAUDE.md:34` claims "2,600+ tests passing". Plausible.

| Type | Location | Notes |
|---|---|---|
| Unit (inline `#[cfg(test)]`) | everywhere | dominant |
| Integration (`tests/`) | 19 files across both workspaces | see below |
| Dev-harness (TCP-driven) | `desktop/win/tools/*.sh` | 3 manual-run scripts |
| Fuzz | `crates/midas-ib-sim/fuzz/` | `decode_incoming` (1h nightly), `bracket_tool_fsm` (30m nightly) |
| Live-IB gate | `crates/midas-broker/tests/ib_live.rs` | 4 tests, `#[cfg_attr(not(feature = "ib_live_tests"), ignore)]` — clippy-only in CI |
| Parity harness | `tests/chart_parity_fixture.rs` + `tools/devloop-chart-parity.sh` | Gated on `chart_parity_tests` |
| Bench | `crates/midas-ib-sim/benches/` | 3 criterion benches, not in CI |

Integration files: `midas-scene/tests/` (5), `midas-broker/tests/` (6),
`midas-market-data/tests/` (2), `midas-bars-adapter/tests/` (2),
`midas-ib-sim/tests/` (9), `midas-bars/tests/` (2), `midas-stream/tests/`
(2), `midas-broker-core/tests/` (1), and `desktop/win/tests/` (7).

**Pyramid.** Unit base is solid (every config change has
`config/tests.rs`; every chart widget has `tests.rs`). Mild inversion in
`midas-scene` — 5 integration files at ~135 tests mostly assert in-process
scene behaviour that could live inline. `app_sim_e2e.rs` is 4 tests that
spawn two subprocesses each; substantial scaffolding for 4 assertions.

**Flakes.** `iced_subscription_teardown_propagates_dec_ref`
(`midas-market-data/tests/router_behavior.rs:576`) flagged flaky in prior
review (timing-dependent RAII drop cascading through broadcast channels);
not `#[ignore]`d. Only 6 `#[ignore]` uses across 3 files — project is not
hiding flakes. Plan docs call out known non-determinism vectors
(`plan/ib-sim/04-order-lifecycle.md:307`,
`plan/ib-sim/05-quirk-modeling.md:279`).

---

## 5. CI

Single workflow, `.github/workflows/rust.yml`, 8 jobs:

| Job | Trigger | Purpose | Blocking |
|---|---|---|---|
| `broker` | push/PR | Root fmt + clippy + build + test | Yes |
| `broker_ib_live_feature_lint` | push/PR | `clippy -p midas-broker --features ib_live_tests` | **No** (`continue-on-error: true`) |
| `desktop` | push/PR | Desktop fmt + clippy + build + test | Yes |
| `sim_unit` | push/PR | `test -p midas-ib-sim --lib` | Yes |
| `sim_integration` | push/PR | `test -p midas-ib-sim --test '*'` | Yes |
| `sim_workspace` | push/PR | Full `midas-ib-sim` fmt + clippy + test | Yes |
| `sim_fuzz_nightly` | cron `0 3 * * *` | 1h `decode_incoming` + 30m `bracket_tool_fsm` | Yes |
| `rust_ibapi_drift` | cron `0 4 * * 1` | Patch `ibapi` to HEAD; run `handshake_e2e` against sim; open issue on failure | Yes |

`Swatinem/rust-cache@v2` per-job with distinct `shared-key`s. The drift guard
is elegant — auto-opens a labelled GitHub issue on failure
(`rust.yml:209-243`) and ties back to the fuzz nightly's theme of protecting
wire-protocol interoperability.

**Biggest gap: five of seven feature flags have no CI ON-path.** The default
workspace test is `cargo test --workspace` only, which disables every
feature. Concretely:

- `session_chart` compile path — only exercised if tests pull it in, but
  those tests are `session_chart_tests`-gated too.
- `chart_parity_fixture.rs` — the harness-self-validation corpus written
  specifically because "the harness could silently false-positive on a real
  regression" (test docstring) is never CI-exercised.
- `bracket_tool_integration.rs`, `level_end_to_end.rs` — same story
  (`#![cfg(feature = "session_chart_tests")]`).
- `dev_harness` feature — `app_sim_e2e.rs` `#[ignore]`s itself per platform;
  CI-cold everywhere.

`broker_ib_live_feature_lint` exists as exactly the right precedent; the
pattern has not been extended to desktop features.

**Platform gap.** Only `ubuntu-latest` runners. Target platform is Windows 11
(`desktop/win/CLAUDE.md:6`). `app_sim_e2e.rs` explicitly `#[ignore]`s on Linux
because iced needs an X display. The tests that matter most to the
shipping platform run nowhere in CI.

---

## 6. Feature flags

| Feature | Crate | Purpose | Composition |
|---|---|---|---|
| `dev_harness` | `midas-app` | TCP harness, fixtures, screenshots, injection | `midas-devloop-proto` + `image` + `image-compare` + `midas-market-data/test_inject` |
| `session_chart` | `midas-app` | New-stack chart window | 7 root crates, all `optional = true` |
| `test_inject` | `midas-broker-core`, `-broker`, `-market-data` | `inject_for_test` default method | Three-hop forward |
| `ib_live_tests` | `midas-broker` | Un-`#[ignore]` live-IB tests | Only toggles `#[ignore]`, not compilation |
| `mock_clock` | `midas-clock` | Expose `MockClock` | `tokio/test-util` |
| `session_chart_tests` | `midas-workspace` | Gate `session_chart_e2e`, `bracket_tool_integration`, `level_end_to_end` | `midas-app/session_chart` |
| `chart_parity_tests` | `midas-workspace` | Gate `chart_parity_fixture` | `midas-app/dev_harness` |

Each feature has an explanatory comment block in its `Cargo.toml`. No
broken compositions found. No dead flags — all seven are used. CI per
feature:

| Feature | ON path | OFF path |
|---|---|---|
| `dev_harness` | **No CI** | `desktop` job |
| `session_chart` | **No CI** | `desktop` job |
| `test_inject` | `sim_*` jobs (transitive) | Not explicit |
| `ib_live_tests` | `broker_ib_live_feature_lint` clippy-only, non-blocking | `broker` job |
| `mock_clock` | Root `cargo test` (transitive) | Default |
| `session_chart_tests` | **No CI** | Default |
| `chart_parity_tests` | **No CI** | Default |

---

## 7. Documentation

`README.md` (131 LOC) product-facing; `CLAUDE.md` (161 LOC) developer-facing
with architecture rules + doc map; `desktop/win/CLAUDE.md` (157 LOC) desktop
conventions + performance targets. `plan/` has 22 top-level entries including
`broker/` (7), `ib-sim/` (17), `session-aware-charts/` (22 — largest),
`chart-transition/` (2), `widget-system/` (9), `devloop-spec.md` (842 LOC).
`plan/archive/` holds 20 items, largest `chartdata/` (8), `market-data-router/`
(a full duplicate after the feature landed), `provider-broker-separation/` (7),
`decorator-system/` (8).

**Archive-is-documentation.** `CLAUDE.md:155` still cites
`plan/archive/decorator-system/00-index.md` as the decorator-system design
source for *shipped* code. Deliberate — "archive" here is design history, not
trash. Rename to `plan/history/` would reduce ambiguity.

**Gaps.** No `CONTRIBUTING.md` — the onboarding path between `README.md`
(product) and `CLAUDE.md` (AI-assistant) is missing. `plan/devloop-spec.md`
mixes design rationale (stable) with command-by-command reference
(guaranteed to drift from the enum in `midas-devloop-proto/src/lib.rs`,
which is the ground truth and is already heavily doc-commented).

---

## 8. Inter-workspace contracts

Desktop references root via path-only deps:

```toml
midas-broker       = { path = "../../crates/midas-broker" }
midas-market-data  = { path = "../../crates/midas-market-data" }
midas-broker-core  = { path = "../../crates/midas-broker-core" }
# ... six more root crates from the session-chart feature
```

No `version = "..."` constraint anywhere. Rename `midas-broker-core` →
`midas-domain` and desktop breaks with a late build error instead of a
Cargo resolver error. The only semver-like signal today is a comment at
`midas-broker-core/Cargo.toml:3-8` documenting its historic rename and bump
to 0.2.0 as a communication device.

One-line mitigation: add `version = "0.2"` alongside each `path = "..."` —
gives Cargo a guardrail without publishing to crates.io.

---

## 9. Versioning

All crates are `0.1.0` except `midas-broker-core` (0.2.0), the root workspace
package (0.0.0), and `midas-ib-sim/fuzz` (0.0.0). No semver policy documented.
No `CHANGELOG.md` anywhere. `desktop/win/CLAUDE.md` claims "Rust: stable
(pinned via rust-toolchain.toml)" but `Glob rust-toolchain*` returns no file
in either workspace — reproducibility of CI failures depends on whatever
toolchain was latest stable at the time.

Given the single-contributor reality (all commits `Max Enko`) and `publish =
false` on most crates, formal semver isn't urgent. But with path-deps
ignoring version, breaking trait changes are silent — `MarketDataSource`
adding a required method trips only clippy + tests, not the resolver.

---

## 10. Build performance

Two workspaces, 23 crates total. Fuzz crate excluded from parent workspace
(`fuzz/Cargo.toml:30-32`). Profiles tuned in `desktop/win/Cargo.toml:174-190`:
- `dev` at `opt-level = 1` (GPU code in dev), deps at `opt-level = 2`.
- `release` at `opt-level = 3` + `lto = "fat"` + `codegen-units = 1` +
  `strip = "symbols"` + `panic = "abort"`.
- `release-debug` inherits release, keeps symbols for profiling.

Carefully considered. `lto = "fat"` + `codegen-units = 1` roughly 2x release
build time for <5% perf — fine for a personal tool. Feature-gate cost is
small: `session_chart` adds 7 root crates but they're `optional = true`,
`dev_harness` adds `image` + `image-compare` (both small).

**Onboarding cost.** `cargo build --workspace` is *two* invocations (root,
then `cd desktop/win && cargo build --workspace`). This is a direct
consequence of the dual-workspace structure. Every CI job replicates the
`cd desktop/win` dance. A top-level `xtask` or `cargo make` would fold them
into one command.

---

## 11. Dev-harness protocol

`desktop/win/crates/midas-devloop-proto/src/lib.rs` — 774 LOC, 25+ tests.

**Two orthogonal version dimensions**, each with a min-supported floor:
`DEVLOOP_FIXTURE_VERSION = 2` on the envelope shape,
`CURRENT_FIXTURE_SCHEMA = 2` on the payload (with `FIXTURE_SCHEMA_V1`
defaulted via `serde(default)`). Better than any other versioning pattern
in the codebase. Tests cover both "v1 envelope missing schema defaults to
v1" and "v2 envelope carries explicit schema" (`lib.rs:528-560`).

**Command set** — 26 variants: `Ping`, `Shutdown`, `LoadFixture`,
`SnapshotFixture`, `DumpState`, `WaitForEvent`, `WaitForIdle`, `Screenshot`,
`Click`, `ClickPrice`, `Drag`, `Scroll`, `Key`, `InjectTickerMsg`,
`InjectBrokerEvent`, `InjectMarketEvent`, `OpenOrdersPanel`,
`CycleThumbnail`, `SetAccountTab`, `SpawnSim`, `ShutdownSim`,
`InjectSimFault`, `CompareImages`. Each doc-comment cites the plan slice
that introduced it (`S8d`, `Stage 09B`, `Slice 0`) — uncommon discipline;
protocol is traceable to feature history.

Wire stability locked by a dedicated test
(`error_kind_wire_names_stable`, `lib.rs:751`) asserting exact JSON for
every `ErrorKind` variant.

**Issues.** No `Capabilities` handshake — an external driver has no way to
query which commands the running build supports. `ErrorKind::HarnessPanic`
exists and writes `panic.txt`, but the TCP-reader-thread panic path has no
test coverage.

---

## 12. Safety & security

**Live-trading guard** — three-layered (§1 above), tested.

**Sim control-plane bearer** (`crates/midas-ib-sim/src/security.rs`):
32 bytes of `OsRng` entropy, hex-encoded, constant-time comparison
(`security.rs:141-150`), `Debug` redacts the secret (tested line 174),
0600 file on unix, default user ACL on Windows. Sim refuses non-loopback
bind without `external_bind_acknowledged = true` (`security.rs:111-120`,
tested line 188). Well-considered for a dev tool. Minor gap: no assert on
Windows ACL; a user running the sim in a folder with inherited wide
permissions would land a world-readable token.

**Secrets-in-config.** `config.toml` stores no secrets. `.env` is
`.gitignore`d. Sim session PCAPs under `fixtures/sessions/raw/` are
`.gitignore`d with a strong warning (`.gitignore:49-51`). `tools/ci-check-anonymize.sh`
+ `tools/pre-commit-anonymize.sh` wire an anonymisation pre-commit —
best-in-class for this category.

**Gaps.** No `deny(unsafe_code)` at workspace level in
`midas-broker-core/src/lib.rs` or `midas-broker/src/lib.rs`. No
`cargo audit` / `cargo deny` in CI — the `ibapi` + `rusqlite` + `tokio`
transitive surface is ~400 crates, and at least one will have a CVE in any
given month.

---

## Top-10 cross-cutting recommendations

### P1 — address before the second contributor

**R1. Close the feature-flag CI hole.** Add four jobs to `rust.yml`:
`cargo test -p midas-workspace --features session_chart_tests`,
`--features chart_parity_tests`, `cargo clippy --features session_chart`,
`cargo clippy --features dev_harness`. Use the existing
`broker_ib_live_feature_lint` pattern (non-blocking `continue-on-error:
true` is acceptable for a first pass). Today five of seven features have no
ON-path coverage, including the self-validation corpus written specifically
so "the harness could silently false-positive" — which is what happens now.

**R2. Unify persistence paths behind one module.** Add
`midas-core::paths` returning the single root dir + subpath helpers. Fix
the three top-level folders (`HandOfMidas\`, `midas\`, `midas-ib-sim\`)
into one or document why they stay separate. Today ~10 `dirs::data_local_dir()
.join("...")` literals are scattered across `app.rs:2039`, `app.rs:2087`,
`broker/config.rs:176`, `sim_child.rs:195`, `security.rs:55` — any
cross-machine migration story is manual.

**R3. Add `version = "..."` to cross-workspace path-deps.** One-line per
consumer, nine consumers in `desktop/win/Cargo.toml`. Gives Cargo a
compile-time guardrail for crate renames and semver bumps without
publishing to crates.io.

### P2 — do before the tenth contributor

**R4. Adopt a semver policy for internal crates.** Two bullets in
`CLAUDE.md` (bump minor for new public items, major for removals/renames)
plus a root `CHANGELOG.md`. The `midas-broker-core` 0.2.0 bump already set
the precedent via comment.

**R5. Add `rust-toolchain.toml` at root and desktop.** `desktop/win/CLAUDE.md`
claims one exists; `Glob` finds none. Reproducibility of every CI failure
depends on this.

**R6. Add at least one Windows CI job.** Target platform is Windows 11 and
`app_sim_e2e` `#[ignore]`s itself on Linux. A single `windows-latest`
running desktop fmt + clippy + `cargo test --test app_sim_e2e -- --ignored`
closes "we don't test what we ship". `actions/cache` works on Windows.

**R7. Split `plan/devloop-spec.md` into design + protocol reference.**
The 842-LOC single file mixes stable rationale with command-by-command
reference that will drift from the enum. The enum in
`midas-devloop-proto/src/lib.rs` is the ground truth and is already
heavily doc-commented; the spec doc should point at `cargo doc`, not
duplicate.

### P3 — opportunistic technical debt

**R8. Reduce the 30+ inline `target: "..."` strings.** Centralise target
namespace constants per crate, or introduce a macro that defaults `target`
from `module_path!()`. Today a namespace rename is a grep-replace across
60 files.

**R9. Consolidate schema-versioning conventions.** Three patterns live
(`AppConfig::version`; `FixtureEnvelope::devloop_fixture_version` +
`schema`; `AnnotationFile::version`). Introduce
`trait SchemaVersioned { const CURRENT: u32; fn version(&self) -> u32; }`
and a generic `migrate_to_current<T: SchemaVersioned>(...)`. Not
load-bearing today but three unrelated copies of the same pattern will
rot.

**R10. Wire `cargo audit` (or `cargo deny`) as a weekly scheduled CI job.**
`rust_ibapi_drift` already runs weekly on Mondays. Add a second cron in
the same slot. Real-money trading software should track CVEs; the
transitive-dep surface is ~400 crates.

---

*Word count: ~2,700.*
