# Architecture Audit: Hand of Midas

*Snapshot: 2026-04-18, after the Account-panel arc + sparkline scissor fix landed.*
*Scope: both workspaces (root for the broker engine, `desktop/win` for the app).*

## Summary

- **86 kLOC across 14 crates split into two workspaces.** Engine + sans-IO chart core are tidy and disciplined; the desktop application binary (`midas-app`, 33 kLOC) is where every architectural problem concentrates.
- **One real god object — `MidasApp`** — drives 80% of the pain: 76 public fields, 134 `Message` variants, ~10.8 kLOC of methods spread across `app.rs` + `app/views.rs` + `app/handlers.rs` (all on the same `impl MidasApp`). Every new feature has to reach into this struct.
- **Top 1–3 moves**: (1) split `MidasApp` into 6–8 sub-controllers along bounded contexts (workspace, charts, brackets, broker IO, watchlist, account-panel set, link-routing) — `Message` follows; (2) delete the `midas_core::broker` mirror types now that `midas-app` already depends on `midas-broker`; (3) introduce a thin `view-model` projection layer so view files don't read 50+ fields off `&self`.
- The codebase is *unusually* well-instrumented (TickerState's `apply()` invariant, midas-chart's sans-IO discipline, the midas-broker actor pattern, `MarketDataSource` trait, dev-harness, decorator system) — almost all the structural debt sits in the binary glue, not the libraries.

## Healthy aspects

- **`midas-chart` sans-IO discipline** — zero GPU/iced deps, `ChartAction`/`ChartEvent` enums make it a true headless model. This is paying off: chart-render churn is contained and tests are fast.
- **`midas-broker` actor pattern** — `BrokerEngine` is a single `tokio::spawn` actor with split channels (`broadcast(4096)` market / `broadcast(8192)` orders / `watch` connection). Boundary is clear, types don't leak.
- **`MarketDataSource` / `BrokerClient` traits** — keep `ibapi` quarantined behind two interfaces. Test broker is a first-class implementor, not an `#[cfg(test)]` afterthought.
- **`TickerState::apply(msg) -> Vec<TickerEffect>` invariant** — deepest module in the codebase; private fields, single mutator, returns effects rather than performing them. Exactly the Ousterhout "narrow interface, deep implementation" shape.
- **`midas-ui::clip_layer`** (just landed) — surgical fix to a real iced abstraction leak; doesn't reinvent the renderer.

## Findings

### [P1] `MidasApp` is a god object — file split is cosmetic, not architectural
**Location**: `crates/midas-app/src/app.rs` (struct, 76 fields), `app.rs` (2952 LOC, hottest file in repo), `app/views.rs` (4017 LOC), `app/handlers.rs` (3852 LOC), `app/persistence.rs`, `app/fixture.rs`, `app/ticker_wiring.rs` — **all `impl MidasApp` blocks** on the same struct.

**Evidence**:
- Struct has 76 `pub` fields (chart panels, account panels, watchlists, drag state, link picker, 3 distinct column-resize triples, `last_config_save`, `crosshair_sync`, `placing_preview`, …). Co-mingles workspace layout, market data cache, dock state, drag IO, broker bridge, level placement, undo/redo nascent state.
- `Message` enum: **134 variants** (counted in `app.rs`).
- Hottest churn in the codebase: `app.rs` 83 commits/year × 2952 LOC; `views.rs` 60 × 4017 (= ~240k churn-LOC each — top hotspots by Tornhill formula).
- `handlers.rs` touches 50+ distinct `self.*` fields (top: `self.charts` 58×, `self.workspace` 31×, `self.link_picker_open` 23×, `self.account_panels` 15× …) — divergent change is structural.

**Lens**: Cohesion + Coupling + Evolution

**Principle**: Brooks (accidental complexity), Ousterhout (shallow class with wide interface), Fowler (Divergent Change + Shotgun Surgery).

**Impact**: Every new pane type requires editing the struct, the `Message` enum, the dispatcher, persistence, fixture, and view code at once. Change cost scales with `MidasApp` size, not with the new feature's intrinsic size. Recent additions (Account panel) added 8 new column-resize/picker fields; the next pane type will add another 8.

**Refactor**:
1. **Inventory** the 134 Message variants. Bucket by bounded context — workspace, chart-interaction, watchlist, account-panel, broker-IO, drag, link, lifecycle (window/config). Confirm against the existing `handle_*_msg` dispatch (already 14 buckets — good seam).
2. Introduce `pub struct WatchlistController { panels: HashMap<WatchlistId, WatchlistPanel>, drag: DragState, link_picker: ... }` with `update(WatchlistMsg) -> WatchlistEffect` and `view(&self) -> Element<WatchlistMsg>`. Top-level `MidasApp.update` becomes pure routing: `Message::Watchlist(m) => self.watchlists.update(m).into()`.
3. Repeat for `ChartController`, `AccountPanelSetController`, `LinkRoutingController`, `BrokerBridgeController`, `WindowChromeController`. Each owns its slice of state + its own enum. `Message` becomes 6–8 variants of `enum Message { Workspace(..), Chart(..), Watchlist(..), Account(..), Link(..), Broker(..), Window(..) }`.
4. Cross-controller coordination (e.g. ticker-link broadcast on watchlist click) goes through an `Effect` enum returned to the top-level loop, not direct field access. Same shape as `TickerState::apply` already proves works.

**Seam**: start with `WatchlistController` because it's already well-bounded (`watchlist/`, `watchlist_columns/`, `WatchlistPanel`, `WatchlistMsg` already half-exist). One PR ships the pattern; later contexts copy-paste.

**Rollback signal**: if `WatchlistController` ends up needing more than 4 borrows of `&MidasApp` (chart cache, broker, link store, …), the boundary was wrong — the missing piece is probably a shared `MarketDataCache` service it should depend on, not a coupling problem.

**Confidence**: high — three corroborating signals (field count, Message arity, churn) plus the dispatch-by-bucket already done in `handlers.rs` proves the seam exists.

### [P1] Duplicated broker domain types — `midas_core::broker` mirror is dead architectural debris
**Location**: `desktop/win/crates/midas-core/src/broker.rs` (mirror types), `crates/midas-broker/src/orders/types/` (source of truth).

**Evidence**:
- `desktop/win/crates/midas-core/src/broker.rs` defines `OrderAction`, `TimeInForce`, `EntryKind` with explicit comment: *"These types mirror their counterparts in `crates/midas-broker/src/orders/types.rs`. The desktop workspace cannot depend on midas-broker (which depends on ibapi). Changes to either side must be kept in sync manually."*
- Mirror is used in **3 files** (all in midas-app): `account_panel/history_columns.rs`, `order_blotter/columns.rs`, `order_blotter/mod.rs`.
- Real broker types are imported in **15 files** including `broker_bridge.rs:13` (`use midas_broker::{BrokerCommand, BrokerEvent, BrokerHandle};`) and `account_panel/subscription.rs`.
- `midas-app/Cargo.toml` already lists `midas-broker = { workspace = true }`.
- `OrderStatus` enum exists in **3 places**: `midas_broker::orders::state`, `midas_app::order_blotter`, `midas_broker::test_broker::SimOrderStatus`.

**Lens**: Coupling + Abstraction

**Principle**: Single Source of Truth; connascence-of-meaning across module boundaries — variants must agree by name AND semantics with no compiler help. Hickey: complecting two representations of the same concept.

**Impact**: Add an `OrderAction::Short` variant tomorrow → silent divergence until someone notices a missing `match` arm in the Buy/Sell-only mirror. Mirror's premise ("desktop cannot depend on broker") is no longer true.

**Refactor**:
1. Delete `desktop/win/crates/midas-core/src/broker.rs` entirely.
2. Replace the 3 importers' `use midas_core::broker::{...}` with `use midas_broker::{...}`.
3. If a thinner public face is desired (the original intent was probably to hide ibapi), introduce `midas_broker::types` (or a `midas-broker-api` crate with no transport deps) re-exporting only the value-types — but only if a real reason emerges.

**Seam**: `midas-core/src/lib.rs` `pub mod broker;` line — delete the module declaration and follow the 3 compile errors.

**Rollback signal**: if removing the mirror introduces a cycle (`midas-broker` ends up depending on something in `midas-app`), the broker has its own coupling problem to address first — but `Cargo.toml` says it doesn't.

**Confidence**: high — direct contradiction between the comment and `Cargo.toml`; only 3 callers to migrate.

### [P1] Views read 50+ fields off `&MidasApp` — no projection layer
**Location**: `crates/midas-app/src/app/views.rs:1-4017` (notably `view_account_body:2371`, `view_pane_body:854`, `view_watchlist_body:1262`).

**Evidence**:
- Top `self.*` reads in views: `self.workspace` 6, `self.link_picker_open` 6, `self.recent_symbols` 5, `self.providers` 5, `self.broker_connection_display` 5, `self.charts` 4, `self.account_panels` 4, `self.level_store` 4, `self.placing_preview` 4, `self.crosshair_sync` 2, `self.dragging_annotation` 2, … (≈18 distinct fields directly read in view code).
- `views.rs` imports types from 5 domain crates inline (`midas_chart`, `midas_grid`, `midas_ui`, `midas_core`, `midas_render`) and assembles them into iced widget trees — no intermediate render model.
- `view_pane_body` is 180 LOC dispatching on `PanelContent` and reaching into chart + order + account + watchlist state inline.

**Lens**: Abstraction (complected: data lookup + presentation), Cohesion

**Principle**: Hickey — separate "what to show" from "how to fetch what to show". Anemic-VM smell: there is no view-model; each view function is its own bespoke projection.

**Impact**: Refactoring any field on `MidasApp` requires touching the view layer because views reach across the whole struct. Tests for view shape are impossible without booting the entire app.

**Refactor**:
1. For one pane type (Account is the cleanest because it's recently rewritten): introduce `AccountPanelViewModel` with explicit fields the view needs (`tabs: Vec<TabSummary>`, `active_tab: AccountTab`, `body: AccountTabBody`, `link_color: Option<LinkColor>`, …). Build it in one place: `MidasApp::account_view_model(account_id) -> AccountPanelViewModel`.
2. Rewrite `view_account_body` to take `&AccountPanelViewModel`. Now the view function has *one* parameter and zero `self.*` reads.
3. Test the view-model builder in isolation — no iced needed.
4. Repeat per pane type. The number of `self.*` reads in `views.rs` should drop to 0 over time.

**Seam**: new module `crates/midas-app/src/app/view_models/account.rs`; `view_account_body` signature changes from `(&self, account_id)` to `(vm: &AccountPanelViewModel)`.

**Rollback signal**: if a view-model builder needs to call back into iced types (`Element`, `widget::*`), the model is leaking presentation — drop iced types from the model.

**Confidence**: medium-high — the pattern is well-known; risk is in scope creep ("project everything at once"). One pane at a time.

### [P2] `chart_widget::action_to_message` is a 132-LOC fan-out point — `Message` complecting transport with intent
**Location**: `crates/midas-app/src/chart_widget.rs:1529-1660` (`action_to_message`).

**Evidence**:
- `midas_chart::ChartAction` has 26 variants (sans-IO, clean).
- `action_to_message` translates each into a `Message::Chart*` variant (1:1 fan-out → contributes ~30 of the 134 Message variants).
- Every new `ChartAction` variant requires: edit chart-core enum, edit `action_to_message`, edit `Message` enum, edit `handle_chart_interaction_msg`, edit dispatcher.
- `chart_widget.rs:51` imports `crate::app::Message` directly — chart-widget is no longer purely a presentation widget; it's a Message factory bound to MidasApp's enum.

**Lens**: Abstraction + Coupling

**Principle**: Connascence of name across a 4-step fan-out. The `Message::Chart*` variants are a redundant restatement of `ChartAction` after a chart_id is added.

**Impact**: Charts are the highest-churn feature surface; this fan-out is a tax on every chart-interaction PR.

**Refactor**:
1. Replace 30 `Message::Chart*(chart_id, ...)` variants with one: `Message::Chart(ChartId, ChartAction)`.
2. `action_to_message` collapses to `|a| Message::Chart(chart_id, a)`.
3. `handle_chart_interaction_msg` matches on the inner `ChartAction` (dispatch tree it would do anyway).
4. `Camera2D`-dependent translation (`Zoom { center_x } → ChartZoom(pivot_time, factor)`) stays in chart_widget but is the only special case rather than the rule.

**Seam**: Message enum's `ChartPan` … `ChartBatch` block (29 variants); collapse to one variant.

**Rollback signal**: if the inner-match in `handle_chart_interaction_msg` exceeds the size of the original 29 separate handlers, the indirection wasn't worth it.

**Confidence**: high — counts and translation function are right there.

### [P2] Workspace persistence has no schema versioning — every config change risks one-shot migrations
**Location**: `crates/midas-core/src/config/mod.rs` (739 LOC, `AppConfig`), `crates/midas-core/src/config/migrations.rs` (recent: order-blotter → account-panel).

**Evidence**:
- `AppConfig` has no top-level `version: u32` field; migrations are detected by structural shape (presence of `order_blotters` array) and write a one-time `.bak-account-migration` backup.
- `PanelSlot` and `LayoutNode` have a `#[serde(other)] Unknown` variant for forward-compat, but the schema as a whole has no version handshake.
- Recent feature work (Account panel arc) had to ship a custom migration function with bespoke backup logic; the next pane type will need the same.

**Lens**: Evolution + Abstraction

**Principle**: Schemas evolve; migrations are first-class. Without a version, migration logic complects "is this old?" with "what is the old shape?".

**Impact**: Each new persistence change reinvents the migration mechanism. A future user with two-versions-old config falls into a custom code path per change.

**Refactor**:
1. Add `version: u32` to `AppConfig`; bump it whenever the schema changes (start at 2, treating absence-of-field as v1).
2. Migrations become `fn migrate_v1_to_v2(cfg: V1) -> V2` chained by version compare. One backup path, one log message, one error type.
3. Move the existing `migrate_order_blotters_to_account_panels` into this framework as `migrate_v1_to_v2`.

**Seam**: `crates/midas-core/src/config/mod.rs` `pub struct AppConfig { … }` — add field; `load_or_default` becomes the migration driver.

**Rollback signal**: if v1→v2 migration needs to consult more than 3 fields beyond the renamed ones, the schema change was too coupled — split it.

**Confidence**: medium — call this strategic if no other config changes are imminent; mandatory before a third pane refactor.

### [P2] Symbol identity: defined twice, used inconsistently, in the wrong crate
**Location**: `crates/midas-app/src/annotation_store/mod.rs:25` (`pub struct SymbolKey(String);`), `crates/midas-core/src/id/mod.rs:53` (`SymbolId(u32)`).

**Evidence**:
- `SymbolKey` (newtype around `String`) used 181 times across `midas-app` and `midas-chart`. Lives in an internal feature module (`annotation_store`), not in `midas-core`.
- `SymbolId(u32)` exists in `midas-core::id` but has **0 uses** in the codebase.
- Raw `String`/`&str` symbol fields appear in 22 files (top: `app.rs` 22, `annotation_store` 12, `thumbnail_data` 6, `watchlist` 5).
- Three representations of the same domain concept: `SymbolKey`, raw `String`, unused `SymbolId`.

**Lens**: Abstraction + Coupling

**Principle**: Connascence of name + value — raw strings let `"AAPL"` and `"aapl"` and `" AAPL"` collide silently. Module of definition matters: shared types belong in a leaf crate.

**Impact**: Every interface that takes "a symbol" picks a different representation; conversions sit at boundaries (`.to_uppercase()`, `.trim()`) reactively. New symbol-equality bugs land in the same shape: someone passed a raw string.

**Refactor**:
1. Move `SymbolKey` from `midas-app/annotation_store/mod.rs` to `midas-core` (or a tiny `midas-symbols` crate).
2. Make construction normalize (uppercase, trim) — current callers' `.trim().to_uppercase()` calls become unnecessary.
3. Delete unused `SymbolId(u32)` (or repurpose it as the interned integer key if a 4-byte ID is wanted alongside the string).
4. Migrate raw `String`/`&str` symbol fields opportunistically — start with public boundaries (broker bridge, market cache, chart panel struct), leave hot paths last.

**Seam**: `crates/midas-app/src/annotation_store/mod.rs:25` → `crates/midas-core/src/symbol.rs`. Re-export at `midas_core::SymbolKey`.

**Rollback signal**: if normalization breaks a code path that depended on case-sensitive symbols (futures expiry codes? exchange-prefixed?), revisit the contract.

**Confidence**: high.

### [P3] `midas-core` defined twice across workspaces with overlapping role
**Location**: `crates/midas-core/` (root, 281 LOC: `ContractSpec`, `OptionRight`, `SecurityType`, `OrderAction`); `desktop/win/crates/midas-core/` (4897 LOC: `AppConfig`, IDs, `LinkMode`, `MarketSnapshot`, `provider::*`, `broker::*` mirror).

**Evidence**: Both are named `midas-core`; the broker workspace's one is included by `midas-broker`; the desktop one is included by every desktop crate. They share zero content. The naming collision will confuse newcomers.

**Lens**: Coupling (naming hazard) + Abstraction

**Principle**: Names are interface — same name implies same role.

**Impact**: Low today (Cargo never confuses them because they're in different workspaces) but actively misleading. Imports look identical (`use midas_core::SecurityType` vs `use midas_core::AppConfig`) but resolve to different crates.

**Refactor**: Rename one of them. Likely candidates: root → `midas-broker-domain` (or fold into `midas-broker`'s `pub mod types`), or desktop → `midas-app-core`. Modest mechanical change; touches every desktop crate's import paths once.

**Confidence**: high; low priority because the cost (confusion) is paid by readers, not by code.

## Strategic debt (not fixing now)

- **Dual workspaces (`/` for broker, `/desktop/win` for app)** are load-bearing because of `ibapi` Windows vs cross-platform constraints. Folding them costs more than it saves until a non-Windows desktop target appears. Document the seam in `CLAUDE.md` (already done) and leave it.
- **`midas-broker::engine::BrokerEngine` (1861 LOC, single actor)** is a god struct on paper but a coherent actor: 24 `handle_*` methods grouped by concern (commands, IB callbacks, lifecycle, reconnect). Splitting would require extracting sub-actors with their own mailboxes. The actor pattern is what's keeping the broker's complexity manageable; don't break the actor boundary chasing class size.
- **`midas-chart::interaction::ChartAction` (26 variants)** is large but semantically cohesive (one variant per legal user gesture). Unlike `Message`, this enum is the *purpose* of the module. Leave alone.

## Explicitly dropped

- "Use SOLID/DRY everywhere" — no, the broker actor and chart-interaction enum are intentionally large because the *concept* is large. Splitting them is shallowing.
- "Add async traits to broker_bridge" — already there (`#[async_trait]`); not a smell.
- "GPU pipeline files (`midas-render/src/pipelines/*.rs` 200–400 LOC each)" — appropriate size for self-contained `wgpu` pipelines; LOC is essential complexity (binding layouts, shader compile, bind groups).
- "`midas-app/src/main.rs` knows about every module" — yes, it's a binary entry point. That's its job.
- "Tests live next to source" — Rust convention; doing this right.
- "Persistence file growth (`order_history.redb`, `ticker_state.redb`, `cache.duckdb`)" — separate concerns, separate files; correct.

## Fitness functions (prevent regression)

1. **Cap `MidasApp` field count** in CI: `grep -cE "^    pub " crates/midas-app/src/app.rs` should not exceed today's count. Force every new field to either (a) live in a sub-controller or (b) add a paragraph explaining why MidasApp is the right home.
2. **Cap `Message` variant count** in CI similarly. Forces the controller-split refactor's gains to compound rather than erode.
3. **Forbid `midas_core::broker::*` imports** post-deletion: a `cargo deny` rule or simple grep in CI catches any reintroduction of the mirror.
4. **Cycle detector** at the workspace level: `cargo +nightly build -Z print-tree` or `cargo-modules` in CI — should be a no-op today (no cycles exist) but locks in the discipline.

## TL;DR ranked

1. Split `MidasApp` god object (P1, high payoff, incremental).
2. Delete `midas_core::broker` mirror (P1, ~30 min, prevents silent divergence).
3. Introduce view-models (P1, gates the MidasApp split).
4. Collapse `Message::Chart*` into `Message::Chart(ChartId, ChartAction)` (P2, easy reduction).
5. Versioned config migrations (P2, before next schema change).
6. `SymbolKey` to `midas-core` + normalize (P2).
7. Rename one `midas-core` (P3).
