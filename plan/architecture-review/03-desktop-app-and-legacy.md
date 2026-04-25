# Desktop Workspace + Legacy Chart — Architectural Audit

**Scope:** `desktop/win/crates/` — 11 crates totalling **~88.7 kLoC** of Rust.
**Snapshot:** after chart-transition slice 9a (root 1558 / desktop 1619 tests green);
slice 9c (atomic legacy deletion) pending a user-owned 14-day soak.
**Lens:** iced GUI shell + the legacy chart stack being strangled by the
`session_chart` path.

## Summary

The desktop workspace is in the middle of a well-structured strangler-fig
migration: `midas-chart` + `chart_widget.rs` + `HistoricalDataRegistry` +
`midas-feed::TestProvider` are slated for atomic deletion in 9c, while the
new stack (`session_chart/`, `midas-scene`, `midas-axis`, `midas-bars` from
the root workspace) has already proven itself end-to-end for M1/M5/D1-RTH
charts. The overall dependency flow is sound (strictly downward, no cycles),
the `TickerState` encapsulation is actually enforced by Rust's module system
(not just convention), and the dev harness is a genuinely differentiating
piece of infrastructure.

But the GUI shell is showing its age. `midas-app/src/app.rs` is a **4,765-LoC
file** containing a **63-field `MidasApp` struct** and a **112-variant
`Message` enum**. `app/handlers.rs` weighs in at **4,994 LoC** and
`app/views.rs` at **3,846 LoC**. The `update()` dispatcher itself is a
respectable 241 LoC (domain-grouped match), but the weight has been pushed
into the sibling `handlers.rs` file — the complexity is still there, just
relocated. `midas-chart` is **21,350 LoC / 462 tests** with `ChartScene`
read at **58 sites across 17 files**; deleting it in 9c is non-trivial and
the pre-deletion audit needs to be real.

Four themes below.

---

## Lens 1 — Scale and the God-Object Drift

`MidasApp` fields counted by `grep` (lines 357–576 of `app.rs`): **63 fields**.
They fall into at least 12 cognitive buckets (charts, workspace, config, OS
window, cursor/drag, annotation/levels, watchlist, order panels, account
panels, recents, toasts, tickers, router, IB bridge, session-chart, dev
harness). Metz's "7 ± 2" rule has been exceeded by **roughly 7×**.

The `Message` enum has **112 top-level variants** (lines 605–1133).
The update dispatcher (`app.rs:3881–4123`) routes 22 logical groups into
domain-specific `handle_*_msg` methods in `app/handlers.rs`. That's the
right pattern — *don't* inline 112 arms — but `handlers.rs` is still one
**4,994-LoC file** and `views.rs` another **3,846 LoC**, so the dispatcher
pattern has not reduced the god-object, only split it across three files
inside one module.

**Notable weight concentrations in `app/`:**

| File | LoC | Purpose |
|---|---|---|
| `app/handlers.rs` | 4,994 | Every `handle_*_msg` body |
| `app.rs` | 4,765 | `MidasApp`, `Message`, `update()`, ChartPanel, RouterReadyPayload |
| `app/views.rs` | 3,846 | Widget tree construction |
| `app/ticker_wiring.rs` | 696 | TickerState ↔ iced subscription wiring |
| `app/subscription_registry.rs` | 349 | Hashable-key keyed SubscriptionHandle registry |

There's been genuine effort here: the audit rounds mentioned in code
comments (`audit P1 slice 2`, `audit P2 #4 batch 3`, `round-2 P3b`) have
extracted sub-controllers (`WindowGeometry`, `ToastController`,
`ColumnResizeState`, `SubscriptionContext`). But the result is a
*medium-sized struct with 63 fields* rather than a *small struct that
composes 12 controllers* — the delta is still load-bearing.

Where to split (preserving the iced Elm loop):

1. **`MidasApp` → `AppShell` + 12 controllers.** Each domain slice (watchlist,
   order_panel, account_panel, bracket_menu, thumbnail, drag, link,
   session_chart) becomes a controller with its own `update(msg) -> Vec<Effect>`
   and its own `Msg` enum. `AppShell` routes `Message::Watchlist(m)` into
   `watchlist.update(m)` and interprets effects. This is already the shape
   of `ToastController`/`WindowGeometry` — finish the pattern.
2. **`Message` sub-enums.** `Message::Account`, `Message::Window`, and
   `Message::Toast` already wrap child enums. Extend to
   `Message::Watchlist(WatchlistMsg)`, `Message::OrderPanel(OrderPanelMsg)`,
   `Message::Link(LinkMsg)`, `Message::Bracket(BracketMsg)`. That turns the
   top-level enum from 112 variants into ~15.
3. **Split `handlers.rs`.** One file per domain (`handlers/watchlist.rs`,
   `handlers/broker.rs`, etc.), mirroring `dev_harness/` which correctly
   splits into 13 files.

---

## Lens 2 — TickerState Is Structurally Enforced

Architecture Rule 8 ("all bracket mutations through TickerState") is not
merely convention. In `ticker_state/mod.rs:208–317`:

- All 30+ fields on `TickerState` are **private** (no `pub`).
- The module doc explicitly calls this out: "No public setter exists for
  any field. The only public mutation method is `apply(msg: TickerMsg)`."
- Public API is read-only getters (lines 319+) and the single `apply()`
  entry point in `apply.rs`.
- `grep` confirms: **all** `self.bracket_mode = …`, `self.live_bracket = …`,
  `self.gatr_anchor = …`, `self.last_price = …` assignments live inside
  `ticker_state/mod.rs` (lines 482–520 for test/factory-only mutators) or
  `ticker_state/apply.rs` (lines 399, 512, 530, 540, 569, 844, 846, 870).
  Zero external mutation.

This is the right shape. One follow-on note: a few setter-looking methods
on `TickerState` (`set_live_annotation_id`, `set_live_bracket`,
`set_last_price`, `set_bracket_mode`, `set_gatr_anchor`) exist at
`ticker_state/mod.rs:482–520`. If these are meant to be `apply()`-only
internals, make them `pub(super)` / `#[cfg(test)]` instead of `pub`;
otherwise the "only mutation path is apply()" invariant is technically
escapable by a future caller.

The `ticker_state/` module itself is well-proportioned: 711 LoC `mod.rs` +
1,204 LoC `apply.rs` + 512 LoC `persist.rs` + 1,752 LoC of tests. The test
ratio (59%) is healthy for a core state machine.

---

## Lens 3 — Annotation Surface

`AnnotationStore` (`annotation_store/mod.rs`, 597 LoC; 512 LoC of tests)
owns all per-symbol annotations. The persistence shape
(`annotation_persistence.rs`) is:

- Per-symbol JSON file (`data/annotations/<SYM>.json`) — not a single
  combined file.
- Atomic write (`.tmp` + rename).
- `AnnotationFile { version: u32, symbol: String, annotations: Vec<Annotation> }`.
- Forward-compatible deserialization: unknown `AnnotationKind` variants
  are skipped silently with a debug log (`annotation_persistence.rs:138–146`).
- Corrupt file recovery: malformed JSON renamed to `.corrupt.bak`, empty
  store returned (`:116–128`).

This format is coherent, forward-compat, and genuinely testable — the
`tests/` module exercises round-trip, corruption, and unknown-variant
paths. **But** the file exposes a legacy leak that the plan
acknowledges: `midas_chart::widget::Annotation` is the persistent
annotation type. Slice 9c's atomic deletion must move `Annotation` +
`AnnotationKind` + `AnnotationId` + `HorizontalLevel` + `PriceLine` +
`LineStyle` + `LineExtent` + `LineStroke` + `Presence` out of
`midas-chart` and into a new home (`midas-core::annotation`?) before
the legacy crate can be deleted. The header comments at
`annotation_persistence.rs:7–13` and `annotation_store/mod.rs:9–20`
already flag this — good discipline, but concrete target location isn't
named.

Slice 4 (plan) retires `save_symbol` in favour of the redb-backed
`TickerStatePersistHandle`. Tests still use `save_symbol` (`#[cfg(test)]`);
production writes go through ticker-state persist. That's fine, but means
the JSON format is now *read-only-in-production* + *test-only-in-writes* —
document the retire-date for the on-disk JSON files or they'll become
zombie data.

The `AnnotationStore` is the correct seam. Both legacy `chart_widget.rs`
and new `session_chart/` read from it (with session-chart projecting to
`midas_scene::layers::LevelView` rather than consuming `Annotation`
directly). This is exactly the right shape for a strangler — one data
model, two renderers.

---

## Lens 4 — Legacy Chart (`midas-chart`) Deletion Risk

**Size:** 21,350 LoC across 52 files; 462 `#[test]` + `#[tokio::test]`.

**Largest modules:**

| File | LoC | Role |
|---|---|---|
| `interaction/mod.rs` + `interaction/tests.rs` | 1,822 + 2,426 | Event routing |
| `compute/mod.rs` + `compute/tests.rs` | 1,621 + 1,268 | ChartScene assembly |
| `widget/order_bracket/` | 845 + 611 + 1,870 | Bracket widget + tests |
| `widget/decorator/` | ~1,100 + 1,411 | Decorator system |
| `levels.rs` | 730 | Horizontal price level primitive |

**`ChartScene` fan-in:** 58 references across 17 files. In the desktop
workspace alone:

- `midas-chart/src/compute/mod.rs` (producer)
- `midas-chart/src/scene.rs` (definition — 28 `pub` fields)
- `midas-chart/src/instances.rs`, `input.rs`, `lib.rs` (internal)
- `midas-render/src/renderer.rs`, `lib.rs` (GPU consumer)
- `midas-core/src/config/mod.rs` (no direct `ChartScene` use — matched via
  grep false positive on a `chart_view_store_schema` comment)
- `midas-app/src/chart_widget.rs` (legacy renderer)
- `midas-app/src/dev_harness/dump.rs` (state dump)
- `midas-app/src/session_chart/` (5 files — the new stack *still builds a
  `ChartScene`-compatible stream through `primitives_bridge.rs`*)

That last point is the key deletion risk: `session_chart/primitives_bridge.rs`
(**756 LoC**) translates `midas_scene::ScenePrimitives` into
`midas_chart::instances::*` so the existing `midas-render` pipelines can
still draw the new-stack scene. When `midas-chart` is deleted in 9c, the
GPU-layout types (`CandleInstance`, `BadgeInstance`, `VolumeInstance`,
`GridLineInstance`, `AxisLabel`, `CrosshairRender`, `TimelineLabel`) must
either move to `midas-core` / a new `midas-gpu-types` crate, or
`midas-render` must accept `midas_scene::ScenePrimitives` directly and
`primitives_bridge.rs` must evaporate.

**`midas_chart::` import fan-in in `midas-app` alone:** 366 references
across 23 files — not just `chart_widget.rs`.

### 9c Deletion-Risk Matrix — Top 10

| # | Item | Fan-in | Blast radius | Mitigation |
|---|---|---|---|---|
| 1 | `midas_chart::widget::Annotation` (+ `AnnotationId`, `AnnotationKind`, `HorizontalLevel`) | Persistence format; read by `AnnotationStore`, `annotation_persistence`, both chart paths | Breaks disk format load; breaks both renderers | Move types to `midas-core::annotation`; keep serde tag compat. |
| 2 | `midas_chart::widget::order_bracket::{OrderBracket, BracketSide, BracketStatus, BracketLeg, LegRole, EntryType}` | Used by `TickerState`, `Message::BrokerBracketCreated`, `order_panel`, `order_blotter`, `session_chart` | Every bracket touchpoint — hot surface | Same move — `midas-core::bracket` or `midas-broker-core`. |
| 3 | `midas_chart::instances::{CandleInstance, BadgeInstance, …}` | GPU layout for both renderers | `midas-render` won't link | New `midas-gpu-types` crate, or move into `midas-render`. |
| 4 | `midas_chart::scene::ChartScene` | 58 refs, 17 files | Only matters if new stack still emits one | Delete after `primitives_bridge.rs` is retired and session-chart renders directly to `midas-render` from `midas_scene::ScenePrimitives`. |
| 5 | `midas_chart::camera::Camera2D` | `ChartPanel`, `chart_view.rs`, `chart_widget.rs`, used to translate pixel→data for `ChartAction::Zoom`/`ZoomY` | Camera math goes away with legacy panels | New stack uses `midas-axis`; make sure `ChartViewStore` has a clean non-Camera2D store by 9c. |
| 6 | `midas_chart::state::{ChartState, InteractionMode}` | Each `ChartPanel` holds one | `ChartPanel` struct deletion cascades | Remove `ChartPanel` wholesale alongside `chart_widget.rs`. |
| 7 | `midas_chart::widget::bracket_tool::BracketTool` | `BracketToolMode` on the legacy chart; session-chart has its own in `midas_scene::tools` | Dual implementation already exists | Already replaced in `session_chart/widget.rs`; just delete the legacy copy. |
| 8 | `midas_chart::widget::decorator::*` (badges, buttons, groups) | Decorator system used by both paths | Session chart has its own scene-native equivalents | Verify every decorator type has a `midas_scene` analog before delete. |
| 9 | `midas_chart::level_tool::LevelTool` | `chart_widget.rs` only | Narrow | Delete with `chart_widget.rs`. |
| 10 | `midas_chart::levels::HorizontalLevel` | Stored in `AnnotationStore` as part of `AnnotationKind::Level` | Disk format | Same move as #1 — type-level migration. |

**Secondary risks:**

- `midas-core::CandleBuffer` and `midas-core::Timeframe` — 231 refs to
  `CandleBuffer` in the desktop workspace; 17 files in `midas-app` touch
  `Timeframe`. The plan says these are on the chopping block. They can't
  actually be deleted — `midas-bars::CandleSeries` and
  `midas-calendar::BarPeriod` are their new-stack replacements, but
  thumbnail data (`thumbnail_data.rs`), market cache, historical data
  registry, and watchlist all still consume `CandleBuffer`. Plan for a
  gradual migration slice, not a 9c delete.
- `midas-feed::TestProvider` — only consumed from `midas-app/src/app.rs:3441`
  and `registry.rs:112` (tests). Safe to delete once those call sites
  move to `MarketDataSource::historical_bars`.

**Pre-deletion audit checklist for 9c:**

1. `rg -t rust 'midas_chart::'` returns 0 in `midas-app/` and `midas-render/`
   (besides the things being deleted).
2. Every `Annotation*` / `HorizontalLevel` / `OrderBracket` import points
   to the new home.
3. `primitives_bridge.rs` deleted; `midas-render` consumes
   `midas_scene::ScenePrimitives` directly.
4. `ChartPanel` deleted; `MidasApp::charts` holds only session-chart
   panels (or the field is deleted).
5. Existing `data/annotations/*.json` files load under the new type home
   (serde tag stability test covering all `AnnotationKind` variants).
6. `ChartViewStore` schema at v3 (new-stack-only); pre-9c configs migrate
   v2→v3 correctly.
7. `cargo test --workspace` before + after shows new-test-count ≥
   old-legacy-test-count-kept (i.e. what session-chart tests replace the
   462 chart tests isn't an open question).
8. CI renders a 20-chart fixture via dev harness and the screenshot diff
   passes SSIM > 0.97 vs legacy baseline.

---

## 9c Deletion-Risk Matrix — Blast Radius Summary

| Item | Files touched | Risk |
|---|---|---|
| `midas-chart` crate delete | 23 files in `midas-app`, 2 in `midas-render` | **HIGH** — type moves required for #1–10 above |
| `midas-feed::TestProvider` delete | 2 call sites | **LOW** |
| `midas-app/src/chart_widget.rs` delete | 1,553 LoC + `ChartPanel` refs | **MEDIUM** — `ChartPanel` is heavily referenced in app.rs / handlers.rs |
| `HistoricalDataRegistry` delete | 2 call sites in `app.rs`, 1 in `registry.rs` | **LOW** if `MarketDataSource::historical_bars` covers every caller |
| `CandleBuffer` delete | 231 refs in desktop workspace | **DO NOT DO IN 9c** — multi-slice migration |
| `Timeframe` delete | 17 files in `midas-app` alone | **DO NOT DO IN 9c** — multi-slice migration |

---

## Specific Concerns

### C1 — Dev harness sprawl is correct

`dev_harness/` is 3,265 LoC across 13 files (`listener`, `fixture`, `dump`,
`inject`, `screenshot`, `idle`, `sim_child`, `event_log`, `input`,
`broker_inject`, `router_inject`, `variant_names`, `mod`). That sounds
like a lot, but each file has a single narrow responsibility: `inject.rs`
(371 LoC) handles `InjectTickerMsg`/`InjectBrokerEvent`; `dump.rs` (565
LoC) is the JSON-pointer DumpState projection (it's large because
`MidasApp` is large — see Lens 1); `listener.rs` (122 LoC) is the TCP
socket. This is the reference for how the rest of `midas-app` should
look — one narrow module per concern. Keep.

### C2 — Link-groups complexity is load-bearing

`link.rs` (410 LoC) + `LinkMode::{Unlinked, ListenAll, Color(_)}`. The
4-step propagation sequence in `link.rs:37–73` (ClearCaches →
DropSubscription → AcquireSubscription → ResetAndAutoScale) is explicit
enough to be testable (`LINK_PROPAGATION_ORDER` constant asserted by
tests + `log_link_propagation_step` tracing). The comment calls out why
ordering matters ("skipping leaves SPY's ATR band painted over QQQ's
candles"). This is correct design: the sequence is complexity, not
accidental complexity. Do not simplify.

### C3 — Config schema: growing, migration story scales

`AppConfig` (`midas-core/config/mod.rs:59–142`) has 16 fields across 14
sub-structs. Migration is straightforward: `migrations.rs:34–58` chains
`migrate_vN_to_vN+1(&mut AppConfig)` functions gated by `cfg.version`.
Current version is 2 (order_blotters → account_panels). The
`chart_view_store_schema: u32` field (line 140) is a second, independent
schema stamp for the in-memory store; plan says it bumps to 3 on slice 9c.
This two-stamp pattern (global + component) is clean and will scale;
don't add a third stamp unless a module *also* has user-visible
migrations.

One latent issue: `AppConfig.levels: HashMap<String, Vec<LevelConfig>>`
(line 77) is the legacy per-symbol level list. Plan says annotations
persist via redb + JSON files now; this field is migration-input-only.
Set it to `#[serde(default, skip_serializing_if = "HashMap::is_empty")]`
so migrated configs stop writing the dead field.

### C4 — Two subscription registries will need to merge

`session_chart/registry.rs::SymbolSeriesRegistry` (389 LoC) and
`app/subscription_registry.rs` (349 LoC) solve *different* problems today:
the first is a `DashMap<SymbolKey, Weak<RwLock<CandleSeries>>>` for
resolving shared series across session-chart panels; the second is a
process-scoped registry of `SubscriptionHandle<Bar>` / `<Tick>` used
*because `iced::Subscription::run_with` requires a `fn` pointer*. They
are not duplicates.

**But** after 9c, the legacy chart path is gone; the `SubscriptionContext`
in `app/subscription_registry.rs` will only be used by watchlists +
ticker-state subscriptions, all of which are reachable from
`session_chart_registry`-analogous shapes. Unify in a post-9c slice:
single `SubscriptionContext` keyed by `(handle_kind, SymbolKey,
Timeframe?)`, holding `SubscriptionHandle<T>` + `Weak<RwLock<CandleSeries>>`
side-by-side.

### C5 — `midas-store` two-tier pattern is implemented, not documented

`midas-store/src/handle/mod.rs` exposes:
- `insert_candles` — awaited (`.await`).
- `fire_and_forget_insert` — queues through the actor, caller doesn't await.
- `query_candles`, `query_candles_range`, `list_cached`, `shutdown` — all awaited.

The two-tier rule ("critical awaited; non-critical fire-and-forget") is
honored in the API surface but *not encoded at the type level*. A caller
who awaits `fire_and_forget_insert` gets the immediate queue
acknowledgement — not the actual insert result — and there is no
compiler prevention against confusing the two. Consider: rename to
`insert_candles_critical()` + `insert_candles_cached()`, or wrap the
fire-and-forget result in a distinct `QueuedWrite` type that cannot
accidentally be `?`-propagated as a real error.

Schema today: **3 tables** in `market.candles`, `meta.data_ranges`,
`meta.symbols`. Version pinned at 1. The migration scaffolding is there
(`schema_version` table + `if current < 1` idiom), just no v2 to exercise
yet. The pattern scales.

### C6 — Thumbnail pipeline: separate but justified

`thumbnail_data.rs` (965 LoC) + `thumbnail_store.rs` + `thumbnail_widget.rs`
+ `midas-render::sparkline` is not a second god-path. Rationale in the
module header: thumbnails have different cadence (cycle interval on
click, lazy on-demand loading) and different rendering (sparkline, not
candle). They share `CandleBuffer` for data; they don't share the
pipeline. The `DEFAULT_MAX_CONCURRENT_LOADS: usize = 6` cap at
`thumbnail_data.rs:45` is the right back-pressure knob. Keep as-is.

### C7 — Window geometry is well-separated

`window_geometry/mod.rs` (196 LoC + 138 LoC tests) owns `position`,
`size`, `monitor_size`, `main_window`. It has its own `Msg`/`Effect`/
`update()` triad. `MidasApp` routes `Message::Window(m)` into
`self.window.update(m)` and interprets effects (`app.rs:4001`). Clean.
`layout/mod.rs` (449 LoC) handles `WorkspaceLayout` / `PanelContent` /
`LayoutPresetKind` — pane-grid state, separate from OS-window state. The
separation is correct.

### C8 — Performance budgets: architecture enables, doesn't threaten

The `<4ms single-chart / <14ms 20-chart / <200MB` targets are enabled by:

- SoA `CandleBuffer` + version-counter dirty tracking (`midas-data`).
- Per-chart `Subscription::channel` with frame-rate coalescing
  (batches bars between iced's `update()` ticks — see `ChartBarBatch`
  docstring at `app.rs:990–997`).
- Weak references in `SymbolSeriesRegistry` (closed panels free
  deterministically).
- GPU instance batching (`CandleInstance` / `BadgeInstance` are `#[repr(C)]`
  Pod/Zeroable — SIMD-friendly upload).
- Subscription RAII — drop the handle and the router's refcount cascades
  an upstream cancel (no zombie subscriptions).

**Threats:**

1. Every `update()` on a `MidasApp` with 20 charts iterates a
   **63-field** struct's worth of `self.*` accesses across handlers. CPU
   cost is negligible, but dirty-tracking discipline is the thing to
   watch: every handler that mutates a field must correctly mark the
   right dirty flag, or a subsequent view() won't rebuild.
2. `HashMap<ChartId, ChartPanel>` lookups on every per-chart message —
   20 charts × 60 fps × ~10 messages/frame = 12k lookups/s. Fine today;
   watch if it grows.
3. The **112-variant `Message`** is large. `Box<BrokerEvent>` +
   `Box<OrderEvent>` are already boxed for size reasons; audit periodically
   to confirm the enum stays under ~128 bytes.
4. 462 tests in `midas-chart` all compile into legacy binaries too.
   Migrating *test coverage* — not just code — into the session-chart
   stack is the real work item for 9c.

---

## Strengths

- **Rule-by-type enforcement.** TickerState's private-fields +
  `apply()`-only mutation is the correct pattern and *works*.
  `AnnotationStore::update()` similarly gates every mutation through one
  method. Rust's privacy system is being put to real use.
- **Strangler-fig discipline.** Every legacy seam has a comment pointing
  to the migration slice (see `annotation_store/mod.rs:9–20`,
  `annotation_persistence.rs:7–13`, `registry.rs:15–17`,
  `chart_widget.rs:6–15`). A reader who's new to the codebase can tell
  what's staying and what's leaving.
- **Sub-controller extraction has started.** `WindowGeometry`,
  `ToastController`, `ColumnResizeState`, `SubscriptionContext` are
  existing examples of the right pattern — they just haven't been
  applied aggressively enough.
- **Dev harness is a differentiator.** The 13-file `dev_harness/` module
  gives Claude (or any client) deterministic driveability: fixture load
  → inject → wait for idle → screenshot → diff. Very few native apps
  have anything like this.
- **Root-workspace isolation.** Desktop never imports `ibapi` directly;
  market data flows through `MarketDataRouter` abstracting
  `Arc<dyn MarketDataSource>`. Rule 1 is honored.
- **Dependency flow is strictly downward.** `midas-core` → `midas-data`
  / `midas-indicators` → `midas-chart` → `midas-render` → `midas-app`.
  No cycles. Verified by reading `Cargo.toml` dependency sections
  (the CLAUDE.md table matches the actual graph).
- **Forward-compatible on-disk formats.** JSON annotations skip unknown
  variants. DuckDB schema is versioned. Config schema chains migrations.
  A user can install a newer version, downgrade, and not lose data.

---

## Recommendations

### P1 (block 9c or do alongside)

1. **Move `Annotation`, `AnnotationId`, `AnnotationKind`, `HorizontalLevel`,
   `OrderBracket` + bracket support types, `PriceLine`, `LineStyle`,
   `LineExtent`, `LineStroke`, `Presence` out of `midas-chart`** before
   the crate can be deleted. Home: `midas-core::annotation` (or a new
   `midas-annotation-types` crate if `midas-core` recompile cost
   is a concern). Serde tag stability is load-bearing — round-trip every
   existing `data/annotations/*.json` file under the new type path.
2. **Move GPU instance types** (`CandleInstance`, `BadgeInstance`,
   `VolumeInstance`, `GridLineInstance`, `AxisLabel`, `CrosshairRender`,
   `TimelineLabel`) into `midas-render` (or a new `midas-gpu-types` crate).
   `primitives_bridge.rs` (756 LoC) evaporates when `midas-render`
   consumes `midas_scene::ScenePrimitives` directly.
3. **Real pre-9c screenshot baseline.** Use the dev harness to capture
   20-chart / single-chart reference images for the legacy stack *before*
   deletion. Slice 9c's PR must include SSIM-diff proof that the new
   stack matches.
4. **Do not delete `CandleBuffer` / `Timeframe` in 9c.** 231 refs to
   `CandleBuffer`, 17 files touching `Timeframe`. Multi-slice migration.

### P2 (post-9c cleanup, high leverage)

5. **Split `MidasApp` into `AppShell` + 12 controllers.** Already
   demonstrated by `WindowGeometry` / `ToastController`. Each controller
   owns its fields, exposes `update(Msg) -> Vec<Effect>`, and
   `AppShell.update()` becomes a 60-LoC router. Target: reduce
   `MidasApp` to ~15 fields.
6. **Split `Message` into sub-enums.** `Message::Watchlist(WatchlistMsg)`,
   `Message::OrderPanel(OrderPanelMsg)`, `Message::Link(LinkMsg)`,
   `Message::Bracket(BracketMsg)`, `Message::Session(SessionMsg)`.
   Target: top-level `Message` = 15–20 variants.
7. **Split `app/handlers.rs` (4,994 LoC).** One file per domain
   (`handlers/watchlist.rs`, `handlers/broker.rs`, `handlers/chart.rs`,
   `handlers/order_panel.rs`, `handlers/account.rs`, `handlers/router.rs`,
   `handlers/toast.rs`, `handlers/window.rs`). Mirrors `dev_harness/`.
8. **Split `app/views.rs` (3,846 LoC).** Same pattern. Each widget
   sub-tree should live next to its domain controller.
9. **Unify subscription registries.** Single `SubscriptionContext`
   holding both `SubscriptionHandle<T>` and
   `Weak<RwLock<CandleSeries>>` keyed by `(SymbolKey, Timeframe?)`.
10. **Encode midas-store two-tier in types.** `fire_and_forget_insert`
    returns a `QueuedWrite` marker type; `insert_candles` returns
    `Result<InsertOk, StoreError>`. Caller cannot confuse them.

### P3 (polish / hygiene)

11. **Tighten TickerState setter visibility.** The setter-looking
    methods at `ticker_state/mod.rs:482–520` (`set_live_bracket`,
    `set_last_price`, etc.) should be `pub(super)` or `#[cfg(test)]`
    so the "only mutation path is `apply()`" invariant is truly
    un-escapable.
12. **Retire legacy config fields.** `AppConfig::order_blotters` is
    migration-input-only post-v2; mark
    `#[serde(default, skip_serializing_if = "Vec::is_empty")]` so v3
    configs stop writing the dead field. Same for
    `AppConfig::levels` after the annotation-migration slice lands.
13. **Document the on-disk JSON annotation format's retirement.** If
    writes are now redb-only, write a plan entry for when / how the
    `data/annotations/*.json` read path is retired.
14. **`Message` enum size audit.** Add a `const_assert!(size_of::<Message>()
    < 128)` or similar in a test — catches accidental large payload adds.
15. **`ChartScene` field count (28 `pub` fields).** If the legacy scene
    lives longer than 9c, consider a builder + typed layer-end
    indexing rather than 28 parallel `Option<Vec<_>>` + count fields.
