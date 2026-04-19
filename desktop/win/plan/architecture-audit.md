# Architecture Audit: Hand of Midas

*Snapshot: 2026-04-18, after the Account-panel arc + sparkline scissor fix landed.*
*Scope: both workspaces (root for the broker engine, `desktop/win` for the app).*

## Status

| # | Finding | Status |
|---|---------|--------|
| P1 | Split `MidasApp` god object | **slices 0 + 2 shipped** (Toast — `f4015da`, `9614b4b`; WindowGeometry — `f338700`, `2395864`). Pattern proven for trivial state AND multi-field + persistence. Watchlist still pending `SharedServices` + `Controller` trait per pattern-scaling review. |
| P1 | Delete `midas_core::broker` mirror | **shipped** (commit `7254f79`, 2026-04-18) |
| P1 | Introduce view-models | **shipped** across slices 3A–3E + 4 + 5 + 6 + 7 + 8 + 9 + 10 + 11 + 12 (commits `d58c75d`, `4cbad6e`, `deb9e2b`, `cac474b`, `78f114b`, `012a800`). Account panel (4 sub-tabs + chrome), Watchlist, Chart pane (snapshot + overlays + title-bar), Order panel, Status bar, Toolbar. ~30 unit tests on the projection layer. Audit's three named view targets (Account, Watchlist, Pane body) all VM-driven. |
| P2 | Collapse `Message::Chart*` into `Message::Chart(ChartId, ChartAction)` | **shipped** (commits `468fd30`, `e8f06c1`, `bf5a807`, `9204054`) — wrapper introduced + all 23 chart-action variants deleted. Top-level `Message`: 134 → ~110 variants. `chart_widget::action_to_message` 132 LOC → 5. Bracket bodies live as `handle_chart_bracket_*` methods on `MidasApp` (kept fn-extracted because each is 50–150 LOC). |
| P2 | Versioned config migrations | **shipped** (commit `51080ce`). `version: u32` field + `migrate_to_current` framework chains v_n → v_{n+1}. Existing order_blotter→account_panel migration folded in as v1→v2. Single backup path (`<name>.bak-v{from}-to-v{to}`). |
| P2 | `SymbolKey` to `midas-core` + normalize | **shipped** (commit `316503a`). Promoted to `midas_core::SymbolKey`; old `crate::annotation_store::SymbolKey` is a re-export. Construction normalises trim+upper. Unused `SymbolId(u32)` deleted. |
| P3 | Rename one `midas-core` | **shipped** (commit `c7b3daf`). Root workspace's crate renamed to `midas-broker-core`; desktop one keeps the `midas-core` name (smaller blast radius). |

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

### [P1] ~~Duplicated broker domain types — `midas_core::broker` mirror is dead architectural debris~~ **(SHIPPED 2026-04-18, commit `7254f79`)**

**Outcome**: mirror file deleted, `OrderRow` now stores `midas_broker::{OrderAction, OrderKind, TimeInForce}` directly, all five `translate_*` helpers in `broker_bridge.rs` gone (kept `translate_connection_state` — different abstractions). `OrderKind::TrailingStop` now displays correctly instead of being silently downgraded. Net −411 / +63 LOC. 1205 desktop + 275 broker tests still pass.

The original finding is preserved below for historical motivation.

---

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

1. ~~Delete `midas_core::broker` mirror (P1, ~30 min, prevents silent divergence).~~ **shipped — commit `7254f79`**
2. Split `MidasApp` god object (P1, high payoff, incremental). **Slices 0 + 2 shipped (Toast, WindowGeometry); Watchlist controller still gated on SharedServices + Controller trait.**
3. ~~Introduce view-models (P1, gates the MidasApp split).~~ **shipped — slices 3A–3E, 4–11 (commits `d58c75d`, `4cbad6e`, `deb9e2b`, `cac474b`, `78f114b`, `012a800`)**
4. ~~Collapse `Message::Chart*` into `Message::Chart(ChartId, ChartAction)` (P2, easy reduction).~~ **shipped — commits `468fd30`, `e8f06c1`, `bf5a807`, `9204054`**
5. ~~Versioned config migrations (P2, before next schema change).~~ **shipped — commit `51080ce`**
6. ~~`SymbolKey` to `midas-core` + normalize (P2).~~ **shipped — commit `316503a`**
7. ~~Rename one `midas-core` (P3).~~ **shipped — commit `c7b3daf`**

**Audit status: round 1 closed except for MidasApp split (gated)**.

---

# Re-audit 2026-04-18 (round 2)

*Re-run after round 1 closed out. Focus: what *new* structural debt has surfaced or was missed last time, now that the big moves (view-models, `Message::Chart` collapse, SymbolKey promotion, mirror deletion) are behind us.*

## Status — round 2

| # | Finding | Status |
|---|---------|--------|
| P1 | Parallel column-resize state + message triples (4×) | **open** |
| P2 | `charts` + `floating_charts` parallel maps of `ChartPanel` | **open** |
| P2 | `LevelStore` ↔ `AnnotationStore` dual per-symbol stores | **open** (migration known-deferred; documenting as a finding so it doesn't rot) |
| P3 | Raw `String` symbol keys in `MarketDataCache` + `LevelStore` | **open** (finishes the round-1 SymbolKey promotion) |
| P3 | Session-scoped per-symbol side-car maps on `MidasApp` | **open** |

## Summary — round 2

- The big-hammer round-1 findings have mostly landed. The structural problems now visible are **parallel-change / copy-paste-extension smells** concentrated in the UI dispatch layer — what Fowler calls Parallel Hierarchies + Shotgun Surgery. These are the kind of debt that survives a god-object split if you don't specifically target them, because each `self.foo` access is innocent-looking individually.
- **Top 1–3 moves**: (1) collapse the four `*ColumnResize*` message triples and the four matching `resizing_*_column` fields into one parameterised `ColumnResize` shape; (2) unify `charts` + `floating_charts` behind a single `HashMap<ChartHandle, ChartPanel>` or equivalent iterator abstraction; (3) finish the `LevelStore → AnnotationStore` migration so the render pipeline has one per-symbol store, not two with a bridging function.
- **None of these are blockers for the MidasApp split** — in fact, #1 and #2 make the future Chart / Account / Watchlist controller extractions noticeably cleaner, because the per-controller message surface shrinks before the split rather than after.

## Findings — round 2

### [P1] Four parallel `*ColumnResize*` triples — 12 Message variants, 4 state fields, 4 near-identical handler blocks
**Location**:
- State: `crates/midas-app/src/app.rs:312-322` — `resizing_column`, `resizing_account_column`, `resizing_account_history_column`, `resizing_account_recents_column`, all of type `Option<(PanelId, usize, f32, f32)>`.
- Messages: `crates/midas-app/src/app.rs:621-688` — `WatchlistColumnResize{Start,ing,End}`, `AccountOrdersColumnResize{Start,ing,End}`, `AccountHistoryColumnResize{Start,ing,End}`, `AccountRecentsColumnResize{Start,ing,End}` — 12 variants total.
- Handlers: `crates/midas-app/src/app/handlers.rs:1760-1885` — three consecutive Orders/History/Recents handler triples that differ only in which `column::ids()` function and which `<panel>.grid_state` field they touch; `2346-2368` has the Watchlist variant.

**Evidence**:
- Four `Option<(X, usize, f32, f32)>` fields with the *same shape* and *same lifecycle* (set on Start, mutated on first Move, cleared on End). Primitive obsession + connascence of position across the 4-tuple.
- 12 Message variants driving exactly 3 semantic operations (begin / move / end). That's 12 lines in the enum, 12 dispatcher entries, 12 handler cases — all for parameterization on (grid target).
- Orders-tab and History-tab handler blocks are byte-for-byte identical except for `orders.grid_state`/`OrderBlotterColumn::ids` vs `history.grid_state`/`HistoryColumn::ids`. Recents-tab is a third copy. The Watchlist variant is a 4th. A fifth tabbed-grid (plan mentions a Trades tab as future work) will add 3 more variants + 1 more field + 1 more handler block by copy-paste.
- Zero abstraction over "a grid's column-resize state machine" despite `midas_grid::GridState` already exposing `column_width`/`set_column_width` as the unifying primitive.

**Lens**: Cohesion (divergent change — 4 places edit for one concept) + Coupling (connascence of position repeated 4×).

**Principle**: Fowler — Parallel Hierarchies / Shotgun Surgery. Connascence: same concept represented four times without a unifying type. Ousterhout — interface surface is 4× wider than it needs to be for a single concept.

**Impact**: Any change to column-resize UX (snap-to-grid, min-width enforcement, persistence rules, keyboard cancel, touch support) has to be made in 4 places. Last-write-wins bugs have already appeared once (per git history; `f32::NAN` sentinel for "no cursor delta yet" is repeated in each copy, one easily forgotten).

**Refactor** (bare-enum shape — no trait):
1. Introduce `ColumnResizeTarget` enum (4 variants today — `Watchlist(WatchlistId)`, `AccountOrders(AccountPanelId)`, `AccountHistory(AccountPanelId)`, `AccountRecents(AccountPanelId)`). The variant data carries the panel ID; no trait or dynamic dispatch — Rust idiom favours bare `match` for closed, known, few-variant sum types.
2. Add one `resizing_column: Option<ColumnResizeState>` field on `MidasApp` (`struct ColumnResizeState { target: ColumnResizeTarget, col_idx: usize, start_x: f32, start_width: f32 }`).
3. Collapse the 12 Message variants into one: `Message::ColumnResize(ColumnResizeEvent)` where `ColumnResizeEvent = enum { Begin(ColumnResizeTarget, usize), Move(f32), End }`. (Three separate top-level variants also fine if preferred; the single-wrapper shape matches the round-1 `Message::Chart(ChartId, ChartAction)` collapse.)
4. Handler: one `handle_column_resize(&mut self, ev: ColumnResizeEvent) -> Task<Message>` that pattern-matches on `self.resizing_column.as_ref().map(|s| s.target)` to route to the right panel's `grid_state`. Column-ids and persistence rules live inline per-variant — `AccountOrders` calls `self.flush_config()` on End; `AccountHistory` doesn't; `AccountRecents` doesn't; `Watchlist` does.
5. Emit-site change: every widget that currently emits `Message::AccountOrdersColumnResizeStart(id, i)` now emits `Message::ColumnResize(ColumnResizeEvent::Begin(ColumnResizeTarget::AccountOrders(id), i))`. One line at each call site; compile errors find all of them.

**Seam**: new module `crates/midas-app/src/column_resize.rs` exports `ColumnResizeTarget`, `ColumnResizeState`, `ColumnResizeEvent`. The handler moves into `handlers.rs` as a single `handle_column_resize` function; emit-sites and the Message variant shape define the boundary.

**Rollback signal**: if the inline per-variant `match` in the handler exceeds the size of the 4 original handler blocks combined, the indirection isn't paying off — back out the Message collapse and keep only the field consolidation. If a 5th target genuinely won't fit the enum, reconsider a trait *then* (not now).

**Confidence**: high — all 4 triples sit side-by-side in one file, shape is identical, semantics are identical, the unifying primitive (`GridState`) already exists.

**LOC delta estimate**: −200 / +80 net. Plus the fitness-function win: future tabbed grids cost 10 LOC (impl the trait) instead of 60 (enum + field + handler block).

### [P2] `charts` + `floating_charts` — parallel maps of the same `ChartPanel` type keyed by different IDs
**Location**:
- `crates/midas-app/src/app.rs:255` `charts: HashMap<ChartId, ChartPanel>`
- `crates/midas-app/src/app.rs:277` `floating_charts: HashMap<window::Id, ChartPanel>` — same value type.
- `crates/midas-app/src/app.rs:635-637` — `FloatingSetSymbolLink`, `FloatingSetTimeframeLink` parallel to `SetSymbolLink` / `SetTimeframeLink`.
- `crates/midas-app/src/app/handlers.rs:2562-2644` (docked handlers) vs `2646-2760` (floating handlers) — the `SetSymbolLink` and `FloatingSetSymbolLink` blocks are near-identical ~50 LOC copy-paste, differing only in which map gets `.get_mut(&id)` and which map orders the `.chain()` in the siblings iterator.
- `crates/midas-app/src/app/handlers.rs:3856-3920` `broadcast_symbol_to_link_group` — two parallel loops (one for docked, one for floating) routing the same semantic event.

**Evidence**:
- 16 direct `self.floating_charts.*` references in handler code; most paired with a `self.charts.*` sibling.
- `find_link_targets(source_link, self.floating_charts.iter().map(...))` pattern repeated 4× — explicit parallel enumeration.
- Whenever new chart-level state is added to `ChartPanel`, both code paths have to be kept in sync. Drift history: `crosshair_sync` (docked-only for two commits until the floating path was back-filled), `bound_symbol` (same).
- One Message variant exists only because of the split: `FloatingWindowClosed(window::Id)`. No equivalent `DockedChartClosed` exists — docked-close is workspace-driven.

**Lens**: Coupling + Abstraction (connascence of type across two call chains).

**Principle**: Fowler — Parallel Hierarchies. The two maps represent the same concept ("a chart panel the user is interacting with") with a different location identity. Connascence of type across sibling enumerations — every caller has to *know* there are two containers and *know* to scan both.

**Impact**: The main UI event that has to iterate "all charts" (symbol-link propagation, timeframe broadcast, crosshair sync, market-snapshot fan-out, drag-drop target search) repeats the same two-loop pattern. Future features — cross-chart cursor sync, synchronized-scroll groups, per-chart layout persistence — each pay this tax.

**Refactor** — Option B is the target shape; Option A is contingent:

**Prep step (required before either option)**: the two loops in `broadcast_symbol_to_link_group` are not strictly parallel — the docked path calls `self.load_symbol_for_chart(id, symbol)` while the floating path does inline `bound_symbol`/`symbol`/`symbol_input`/`load_state` mutation before calling `self.load_floating_chart_async(wid, ...)`. Extract a helper `fn apply_symbol_to_panel(panel: &mut ChartPanel, symbol: &str, sym_key: SymbolKey)` that performs the shared mutation; both paths then use it. This is the pre-refactor — removes a hidden asymmetry the iterator collapse would otherwise trip over.

Option B — **iterator abstraction** (target shape; ~80 LOC touched):
1. Introduce `pub enum ChartHandle { Docked(ChartId), Floating(window::Id) }` — used only in iterator items and the collapsed Message variant, NOT as a HashMap key. Derive `Hash, Eq, Copy, Clone` anyway (cheap and future-proof).
2. Add `fn all_chart_panels(&self) -> impl Iterator<Item = (ChartHandle, &ChartPanel)>` (+ `_mut` variant) on `MidasApp` that chains `self.charts.iter()` and `self.floating_charts.iter()`, wrapping each key.
3. Collapse `SetSymbolLink` + `FloatingSetSymbolLink` (and the timeframe pair) into `Message::SetSymbolLink(ChartHandle, LinkMode)`. The handler matches `ChartHandle` to route to the right storage map.
4. Rewrite `broadcast_symbol_to_link_group`'s two loops as one iteration over `all_chart_panels_mut()`, using the `apply_symbol_to_panel` helper from the prep step.
5. Keep both storage maps — `window::Id`-keyed `floating_charts` preserves iced 0.14's native window-event routing (iced dispatches `window::Event::Closed`/`Resized` keyed on `window::Id`; a unified `HashMap<ChartHandle, _>` would force a wrap on every window-event handler path for no benefit).

Option A — **unified map** (contingent; only if Option B proves the asymmetry is cosmetic):
Replace both maps with `charts: HashMap<ChartHandle, ChartPanel>`. Promote to Option A ONLY IF, after Option B ships, ≥3 call sites that currently use `all_chart_panels()` never care about the `Docked`/`Floating` distinction AND no handler that receives a raw `window::Id` from iced has to wrap it. Known iced production apps (Halloy, cosmic apps) keep the two maps separate for exactly the routing-asymmetry reason; don't fight that default without evidence.

**Seam**: `crates/midas-app/src/app.rs:255-277` (the two field declarations stay); new `impl MidasApp` block near the field block for `all_chart_panels`/`_mut`; `handlers.rs:2562-2760` is where the four Link handlers collapse to two (one SetSymbolLink, one SetTimeframeLink).

**Promotion criterion (Option B → Option A)**: after Option B lands and bakes for one release, count call sites that (a) iterate `all_chart_panels()` and never branch on `ChartHandle` variant, vs. (b) sites that must branch (window-event routing, persistence boundaries). If (a) ≥ 3× (b), promote. If not, leave as Option B — the two-map shape is the correct end state.

**Rollback signal (Option B)**: if `ChartHandle` needs to sprout helper methods like `.window_id() -> Option<window::Id>` in more than 3 call sites, the enum is acting as a tuple — drop `ChartHandle` from the Message shape and restore the Link-variant pair, keeping only the iterator helper.

**Confidence**: high on Option B; Option A status downgraded to contingent based on iced 0.14 routing semantics.

### [P2] `LevelStore` and `AnnotationStore` — dual per-symbol stores with a bridge function
**Location**:
- `crates/midas-app/src/level_store/mod.rs:62-69` `LevelStore { levels: HashMap<String, Vec<StoredLevel>>, generations: HashMap<String, u64>, next_id: u64 }`
- `crates/midas-app/src/annotation_store/mod.rs:49-57` `AnnotationStore { by_symbol: HashMap<SymbolKey, SymbolAnnotations>, global_generation: u64, next_id: u64 }` — module doc: *"It replaces `LevelStore` as the single source of truth for chart annotations."*
- `crates/midas-app/src/chart_widget.rs:73-75` `ChartRenderSnapshot { levels: Vec<StoredLevel>, bracket_annotations: Vec<Annotation>, ... }` — both kinds pass through the render path.
- `crates/midas-app/src/chart_widget.rs:1557` `fn old_levels_to_annotations(levels: &[StoredLevel]) -> Vec<Annotation>` — explicit migration-bridge function.

**Evidence**:
- Two per-symbol stores, same role, different key types (`String` vs `SymbolKey`).
- Chart render pipeline reads from both and bridges at the seam.
- Feature-work comment chains reference a "LevelStore→AnnotationStore migration deferred" from slice 10 of the unified-annotation work (user memory `project_unified_annotation_tracks.md`); the deferral has now outlived the original work.
- Both stores bump generations independently; the dirty-tracking path has to watch both.

**Lens**: Abstraction — Single Source of Truth principle violated.

**Principle**: Evans (DDD) — a single authoritative model per concept. Hickey — complecting "level" and "annotation" with a bridge function means every consumer has to know there are two representations.

**Impact**: Every new feature on annotations (e.g., an "annotation history" undo stack, a per-annotation lock state that isn't already modelled, a persistence schema change) has to decide which store it lives in and how it bridges. Test doubles multiply — `annotation_store/tests.rs` + `level_store/tests.rs` cover overlapping invariants.

**Refactor**:
1. Freeze `LevelStore`'s API: no new methods; no new call sites.
2. Introduce `AnnotationKind::Level(HorizontalLevel)` with a `locked: bool` that lives on `Annotation` itself (it already does per the slice-7 note in `level_store/mod.rs:7-16`). Write the small shim that makes `.iter()` on AnnotationStore yield what `StoredLevel` yielded.
3. Flip call sites one-by-one — the render snapshot builder, the level editor, the ticker_state effect handler for level creation — from `self.level_store.*` to `self.annotation_store.*`. `old_levels_to_annotations` becomes the identity function and can be deleted.
4. Delete `LevelStore` and its tests.

**Seam**: `crates/midas-app/src/chart_widget.rs:1557` — the bridge function is the canary. When it's gone, the migration is done.

**Rollback signal**: if `AnnotationStore`'s public API has to grow a level-specific helper set (`add_level`, `find_level_mut`, `levels_for`) that duplicates `LevelStore`'s shape, the migration missed the generalization; revisit whether `AnnotationKind` should carry a richer per-kind index instead of a flat `Vec<Annotation>`.

**Confidence**: high on the smell (two stores, explicit bridge), medium on scope — 17 files reference `self.level_store`, so mechanical migration time is real. Roughly 1–2 days.

### [P3] Raw `String` symbol keys in `MarketDataCache` and `LevelStore` — finishes the round-1 SymbolKey promotion
**Location**:
- `crates/midas-app/src/market_cache.rs:13` `snapshots: HashMap<String, MarketSnapshot>`; public accessors take `&str`.
- `crates/midas-app/src/level_store/mod.rs:64` `levels: HashMap<String, Vec<StoredLevel>>`; accessors take `&str`.

**Evidence**:
- Round 1 promoted `SymbolKey` to `midas-core` with normalizing construction (trim + uppercase). `annotation_store` and `tickers` use `SymbolKey`. These two stores didn't get the migration.
- `MarketDataCache::insert(String, _)` puts whatever the caller passes — `"AAPL"` and `"aapl"` insert as different keys. No compile-time guard.
- `LevelStore::levels_for(ticker: &str)` uses raw `get(ticker)` — no trim, no uppercase; relies on callers to be disciplined.

**Lens**: Abstraction (Primitive Obsession).

**Principle**: Connascence of name/value across module boundaries. The whole point of round 1's SymbolKey move was to make invalid symbol representations unrepresentable at the type level — leaving two stores out of it defeats the purpose.

**Impact**: Low today (every producer happens to uppercase before insert) but one bug away. `MarketSnapshot` is read by every watchlist row; a missed uppercase anywhere upstream shows as "no quote" silently.

**Refactor**: mechanical — replace `HashMap<String, _>` with `HashMap<SymbolKey, _>`; change accessor signatures from `&str` to `&SymbolKey` (or accept `impl Borrow<SymbolKey>`). Callers that still have `&str` call `SymbolKey::new(s)`. About 30 call sites.

**Persistence-boundary note**: `LevelStore::from_config` / `to_config` at `crates/midas-app/src/level_store/mod.rs:169,199` cross the `data/config.toml` persistence boundary as `HashMap<String, Vec<LevelConfig>>`. The in-memory migration is only zero-cost at the disk schema if `SymbolKey` is `#[serde(transparent)]` (it's a newtype around `String` so this is trivial) — verify first, add the attribute if missing. If `SymbolKey` ever becomes something richer than a transparent string (e.g., interned ID), this P3 needs to couple to a config-schema bump using the versioned-migration framework that already landed in round 1 P2.

**Seam**: `crates/midas-app/src/market_cache.rs:13` and `crates/midas-app/src/level_store/mod.rs:64` — change the field, follow the compile errors. Verify `SymbolKey` serde representation at `crates/midas-core/src/symbol.rs` before starting.

**Confidence**: high; trivial. Defer `LevelStore` half until P2 #3 (LevelStore retirement) lands so we don't migrate a store we're about to delete; do the `MarketDataCache` half now.

### [P3] Session-scoped per-symbol side-cars on `MidasApp` shadow `TickerState`
**Location**: `crates/midas-app/src/app.rs:386-403`
- `snapped_this_session: HashSet<SymbolKey>` — GATR snap-once guard
- `anchor_seed_toasts_shown: HashSet<SymbolKey>` — toast dedup guard (currently `#[allow(dead_code)]`)
- `gatr_undo_slots: HashMap<SymbolKey, PreSnapState>` — undo-snap slot (currently `#[allow(dead_code)]`)

**Evidence**:
- The project architecture rule (CLAUDE.md §Key patterns) says "All per-ticker state lives in `TickerState`... fields are private, read via getters." These three maps violate that rule — they're per-ticker state sitting on the god-struct.
- Two of the three are dead-code-with-planned-API flags, meaning the design has already decided they belong *somewhere*; nobody has circled back to fold them into `TickerState`.

**Lens**: Abstraction (SSoT violation for a small set of fields) + Cohesion (related state, wrong home).

**Principle**: the project has an explicit invariant — per-ticker state goes through `TickerState::apply() -> Vec<TickerEffect>`. These fields are the ones that escaped.

**Impact**: Small today. The cost is that `TickerState`'s "single source of truth" claim has three known exceptions; future readers will wonder whether there are more.

**Refactor**:
1. Add `session_flags: TickerSessionFlags { snapped: bool, anchor_seed_toast_shown: bool, gatr_undo: Option<PreSnapState> }` to `TickerState`. Not persisted (per the existing `snapped_this_session` comment — "deliberately not persisted").
2. Expose `session_flags(&self) -> &TickerSessionFlags` / mutations via `TickerMsg::MarkSnappedThisSession`/`ClearGatrUndo` etc. Keeps the `apply()` invariant.
3. Delete the three side-car fields.

**Seam**: `crates/midas-app/src/ticker_state/mod.rs` struct definition.

**Confidence**: high on the principle, low on urgency — the three maps are 5 LOC total and read cleanly where they're used. Touch when the next per-ticker session flag is added.

## Explicitly dropped (round 2)

- **"`handle_chart_bracket_submit` is 150 LOC of broker-params translation in a handler"** — it's genuinely essential structural complexity (bracket side, entry type, TP/SL legs → `BracketParams`). Extracting it to a `to_broker_params()` function on the bracket type is a drive-by cleanup, not an architectural concern. Do it if you're in the file anyway.
- **"`ChartRenderSnapshot` is 30+ fields captured per frame"** — it's the boundary between sans-IO chart core and GPU render. Flat-struct shape is correct for a frame-time hot path; the snapshot is rebuilt from refs, not persisted.
- **"handlers.rs is 3928 LOC"** — same god-object concern as round-1 P1. Don't double-count.
- **"Message enum is still ~111 variants"** — same god-object concern. Will shrink further when the ColumnResize collapse (round-2 P1) lands and when the link-message pair (round-2 P2) collapses.
- **"`broker_bridge.rs` translates ConnectionState"** — intentional thin translator, not a mirror. Round 1 audit already covered this.

## Fitness functions — round 2 additions

5. **Cap `resizing_*_column` field count** at 1: `grep -cE "^    pub resizing_.*_column:" crates/midas-app/src/app.rs` must be `1` after P1 lands (`resizing_column`). The pattern is line-anchored, so stray references in handlers/comments don't inflate the count.
6. **Cap `ColumnResize` Message variant count**: prefer an AST-based check over grep. Add a short test in `crates/midas-app/tests/fitness_functions.rs` (if none exists, create it) that uses `syn` to parse `app.rs`, find the `Message` enum, and assert the number of variants matching `*ColumnResize*` or `ColumnResize` equals 1 (the wrapper) — or 3 (Begin/Move/End) if the Begin/Move/End shape is chosen over the single-wrapper shape. `grep -cE "ColumnResize"` over the whole 2952-LOC file is too coarse: it also counts dispatcher arms, handler references, and doc comments, inflating the number against the post-refactor baseline.
7. **Forbid direct `old_levels_to_annotations` call sites** once P2 #3 lands: `grep -r "old_levels_to_annotations" crates/midas-app/src/ | wc -l` must be 0. The bridge function's deletion is the migration done-marker; the function name is unique enough that `grep` is fine here.
8. **Post-P3 cap: no `HashMap<String, _>` keyed by symbol in `midas-app/src`**. After both P3 halves land, add a grep check that flags new `HashMap<String, ` occurrences in files that handle symbol-keyed state (watchlist, market, level, annotation, ticker, account modules). Locks in the SymbolKey invariant so the next contributor doesn't silently reintroduce a raw-string store.

## TL;DR — round 2 ranked

1. **Parallel `*ColumnResize*` collapse (P1, ~1 day, decisive shrink)** — deletes 9 Message variants + 3 state fields, removes the copy-paste channel for future tabbed grids.
2. **`charts` + `floating_charts` unification (P2, ~1–2 days, Option B first)** — collapses the longest-running copy-paste in the codebase; shrinks the Message enum by 2 more.
3. **Finish `LevelStore → AnnotationStore` migration (P2, 1–2 days)** — deletes a store and a bridge function; round-1 follow-up.
4. **Raw-string symbol keys in remaining stores (P3, mechanical).**
5. **Fold session-scoped per-symbol maps into `TickerState` (P3, tiny but preserves the SSoT invariant).**
