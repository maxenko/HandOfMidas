# Feature: Ticker Order State — Persistent Per-Symbol Order Intent

## Overview

Unify the chart order bracket widget and the order panel UI behind a single per-ticker "order intent" store that is durable, crash-safe, and serves both surfaces from the same source of truth. Generalises the existing `should_reposition` / `reposition_bracket` helpers into a saved-across-sessions GATR re-anchor rule, and cleans up the visually messy bracket rendering.

**Today's pain points**: the panel and bracket do not observe each other (drag → panel stays stale); per-ticker order memory dies at restart; the half-installed GATR reposition rule only fires on bracket recall, never on load or ticker activation; bracket decorators overlap at tight prices.

**Goals**:
- **G1 — bidirectional sync**: dragging a bracket updates the panel immediately; editing the panel moves the bracket. Zero drift between the two surfaces. Delivered by Slice 3.
- **G2 — per-ticker memory surviving restart, keyed by (side × entry type) compound**: last-used side, last-used entry type, and per-compound-key panel state (prices, quantity, TP/SL settings) are restored on launch, on ticker switch, and on side/type switch within a ticker. Stop Loss is **on by default for every (side, type) combination** unless the user explicitly toggled it off for that specific compound — toggling SL off in `(Buy, Stop)` does not turn it off in `(Buy, Limit)`. Delivered by Slices 1a + 2.
- **G3 — GATR session-boundary re-anchor with user-controlled undo**: brackets that drifted while the app was closed snap to the current price region on session start, unless pinned or recently edited; a 30-second undo toast catches mistakes. Delivered by Slice 4.
- **G4 — bracket visual polish**: no decorator overlap at tight prices; consistent alignment with the crosshair priceline lens. Delivered by Slice 0.

**Assumptions**:
- "Industrial strength" = ACID-durable, crash-safe, schema-versioned, interactive-speed writes. Not multi-process or networked.
- GATR here is Gerchik ATR. The absolute value is already exposed via `MarketCache::MarketSnapshot.gatr_abs: Option<f64>` (see `market_cache.rs:72-79`) — do not invent a new helper.
- Brackets remain annotations in `AnnotationStore`. A *separate* `TickerOrderIntent` carries the "last-used" memory and pointers; annotations are the display source of truth, intent is the memory source of truth.
- Single-user, single-process, local-only.

## Research Summary

### Codebase Analysis

**Existing persistence** (`desktop/win/crates/midas-app/src/`):
- `annotation_persistence.rs` — per-symbol JSON at `data/annotations/{SYMBOL}.json`, atomic write-temp-then-rename, schema version 1, forward-compat two-pass deserialize.
- `annotation_store/mod.rs` — `AnnotationStore { by_symbol: HashMap<SymbolKey, SymbolAnnotations>, global_generation, next_id }`, per-symbol generation counters, normalised-uppercase `SymbolKey` newtype with `Borrow<str>`.
- `level_store/mod.rs` — parallel domain-specific store backed by TOML config.
- `midas-store` (desktop workspace) — DuckDB via `mailbox_processor` actor on a dedicated OS thread, canonical handle at `midas-store/src/handle/mod.rs`.
- `crates/midas-broker` (root workspace) — uses `rusqlite` for the broker order log. The desktop workspace does **not** currently depend on `rusqlite`.
- **No generic per-ticker key-value store exists today.** All stores are domain-specific.

**Already-installed GATR reposition logic** — partial implementation exists:
- `order_panel/mod.rs:598` — `pub fn should_reposition(entry_price, current_price, gatr_abs: Option<f64>) -> bool` implements exactly the `|entry - current| > gatr_abs` (100% GATR) rule. Falls back to 5% when GATR is unavailable.
- `order_panel/mod.rs:605` — `pub fn reposition_bracket(bracket: &mut OrderBracket, current_price)` rigidly translates entry, entry_stop_price, take_profit, and stop_loss by `delta = current_price - bracket.entry.line.price`. Preserves R:R shape.
- Current call site: `app.rs:1683-1698` inside a bracket-recall path only. Not invoked on startup load or on ticker re-activation.
- Tests at `order_panel/tests.rs:972-1060` already exercise the threshold and the leg-shift math.

This is a **very important find**: the GATR rule is half-implemented. The feature generalises where it fires and wires it to the new persistent store, rather than inventing new logic.

**Order bracket widget** (`desktop/win/crates/midas-chart/src/widget/order_bracket/`):
- `OrderBracket { entry, take_profit, stop_loss, side, status, quantity, saved, filled_qty, entry_type, entry_stop_price, wrong_side_warning }` at `mod.rs:43-78`.
- `EntryType { Market, Limit, Stop, StopLimit }` at `mod.rs:18-28`.
- `BracketSide { Long, Short }`, `BracketStatus { Draft, Pending, PartialFill, Active, Closed, Cancelled }`.
- `compute_bracket()` at `mod.rs:493-673` renders lines / zones / labels; `leg_style()` at `mod.rs:219-271` drives per-status stroke styling.
- `decorators.rs` builds entry/TP/SL badges with flex layout, quick-create stacks, submit/save controls.
- `tests.rs` uses `make_leg()` / `make_bracket()` fixture builders with epsilon numeric asserts and serde round-trip.

**Order panel** (`desktop/win/crates/midas-app/src/order_panel/mod.rs`):
- `OrderPanelState` fields include (complete list from `mod.rs:19-94`): `side`, `quantity`, `tp_enabled`, `tp_value`, `tp_mode`, `sl_enabled`, `sl_value`, `sl_mode`, **`sl_type: StopLossType`**, **`sl_limit_value: String`**, `entry_type`, `limit_price`, `stop_price`, `errors`, `symbol`, `last_price`, `source_chart`, `bracket_active`, `bracket_annotation_id`.
- `OrderSide { Buy, Sell }` at `mod.rs:102-105`.
- `PriceInputMode { Absolute, Offset, Percent }` at `mod.rs:109-118`.
- `StopLossType { Stop, StopLimit }` at `mod.rs:122-128`.
- `validate_panel()`, `validate_bracket()`, `calculate_risk_reward()`, `resolve_price()` — pure functions, heavily tested.
- **Sync gap**: the panel holds `bracket_annotation_id` but does not observe `AnnotationStore` mutations. When a user drags a bracket on the chart, the panel's fields do not update.

**App wiring** (`desktop/win/crates/midas-app/src/app.rs`):
- `MidasApp { charts: HashMap<ChartId, ChartPanel>, annotation_store: AnnotationStore, level_store: LevelStore, order_panels: HashMap<OrderPanelId, OrderPanel>, order_annotation_links: HashMap<Uuid, OrderAnnotationLink>, … }`.
- Active ticker per chart = `charts[id].symbol`. Order panels keyed by `OrderPanelId`, linked to a chart via `source_chart`.

**GATR computation**:
- `midas-chart/src/gerchik_atr/mod.rs` — `compute_gerchik_atr()` returns `GerchikAtrRender { pct: f32, … }` — **percentage only**.
- `market_cache.rs:72` — `compute_daily_gatr(buffer)` returns `(gatr_pct, gatr_abs)` and stores both on `MarketSnapshot`. This is the absolute value surface the plan consumes.
- `midas-core/src/atr/mod.rs` — core ATR types.

**Workspace rules** (root + `desktop/win/CLAUDE.md`):
- `thiserror` in libraries, `anyhow` only in `midas-app`.
- No `unwrap()` outside tests / `main.rs`.
- `///` docs required on all public items.
- TOML for config, JSON for annotations, DuckDB for bulk candles.
- `cargo clippy --workspace -- -D warnings` in CI.
- `tokio` 1 with `rt-multi-thread, macros, sync, time` already present.
- `parking_lot`, `dirs`, `tempfile`, `serde_json`, `tracing`, `uuid` v7 already present.

### Best Practices & Idiomatic Approach

1. **Single-writer actor on `mailbox_processor`.** The project's canonical pattern (`midas-store`) — reuse it so the team has one persistence idiom, not two.
2. **Snapshot-per-ticker, not event-sourced.** Business events (orders, fills) belong in `midas-broker`'s SQLite order log. UI intent is ephemeral settings — last-write-wins snapshots.
3. **`redb` 2.x as the backend**, added as a new workspace dep. Pure-Rust, ACID, stable on-disk format since 1.0 (June 2023), per-transaction `Durability::{Eventual, Immediate}`, single file so we sidestep Windows `rename` atomicity caveats (rust-lang #123985). Sled is stale; DuckDB is columnar OLAP; `rusqlite` is a close runner-up — see D1 for the honest comparison.
4. **Coalesced write-behind**: `tokio::sync::Notify` + 75 ms debounce. On `Update`: mutate cache, mark dirty, notify. Flush task: drain dirty set in one `redb` write txn with `Durability::Eventual`. Override to `Durability::Immediate` on shutdown, order submit, GATR snap, explicit user Save.
5. **Synchronous reads / async writes boundary.** Reads go through `parking_lot::RwLock<HashMap<SymbolKey, Arc<TickerOrderIntent>>>` (zero await, safe from iced's sync `update()` loop). Writes enqueue to the actor and mutate the cache synchronously before the flush is scheduled — so the very next read sees the new value. This is the key to avoiding reducer staleness.
6. **`serde_json` inside the redb value** with an explicit `version: u32` tag. Human-debuggable, safe (no deserialization gadgets), schema evolution via `#[serde(default)]` on new fields and an at-load-time match-on-version migration. `pub const CURRENT_VERSION: u32 = 1;` at module root as single source of truth.
7. **Store file at `dirs::data_local_dir()/HandOfMidas/ticker_state.redb`** — per-user ACLs, inherits OS protection. Validate every intent on load against invariants before handing it to widgets; never auto-submit after restart.

Sources: [redb docs](https://www.redb.org/), [redb 1.0 release](https://www.redb.org/post/2023/06/16/1-0-stable-release/), [Martin Fowler — Event Sourcing](https://martinfowler.com/eaaDev/EventSourcing.html), [tokio `Notify` docs](https://docs.rs/tokio/latest/tokio/sync/struct.Notify.html), [rust-lang #123985 — Windows `rename`](https://github.com/rust-lang/rust/issues/123985).

## Design Decisions

### D1: Storage backend for per-ticker intent

**Context**: Needs to be durable, crash-safe, support 60+ writes/sec during drag, load fast, and have a schema evolution story.

**Options considered**:
1. **Extend `annotation_persistence.rs`** — one JSON file per symbol, atomic rename. Pros: zero new deps, matches existing pattern. Cons: one fsync per symbol per flush; Windows rename not fully atomic (#123985); hand-rolled coalescing.
2. **`rusqlite`** — already used by `midas-broker` in the root workspace, so the team knows it. Pros: familiar, ACID, rich query support, single file. Cons: introduces a C-FFI dep into the desktop workspace for the first time; schema migrations by hand; heavier than redb for single-table key/blob use.
3. **`redb` 2.x** — pure-Rust, ACID, per-txn durability, stable on-disk format. Pros: zero C deps, smallest API surface for a pure KV use case, per-transaction durability control is exactly the shape the drag workload needs. Cons: new dep the team must learn; manual schema migrations (acceptable for a single blob per key).
4. **`sled`** — 1.0-beta for years, format churn warned. Reject.
5. **DuckDB via `midas-store`** — wrong shape (OLAP columnar). Reject.

**Recommendation**: **Option 3 — `redb = "2"`**. The `rusqlite` alternative is the closest call — familiarity is a real advantage. Two tiebreakers push to redb: (a) no new C-FFI dep in the desktop workspace; (b) `redb::Durability::Eventual` is a first-class concept that exactly matches the drag hot-path pattern, whereas in SQLite the same effect requires `PRAGMA synchronous=NORMAL` + `WAL` and careful reasoning. If the reviewer prefers familiarity over novelty, Option 2 is a clean substitute with no change to the rest of the plan: the actor, reducer, and schema layer are backend-agnostic. Annotation persistence stays JSON.

**Confidence**: medium-high. Call out in Review Notes so the reviewer can choose.

### D2: What lives in `TickerOrderIntent` vs `AnnotationStore`

**Context**: A live, visible bracket is already an `AnnotationKind::OrderBracket` owned by `AnnotationStore`. Duplicating state across two stores is the textbook drift-bug setup.

**Recommendation**: **Source-of-truth rule — codified and tested**:
- **Annotation is the display source of truth.** When a live bracket exists, the chart and the panel both render from the annotation.
- **Intent is the memory source of truth.** When no live bracket exists (fresh session, after cancel, between trades), the panel hydrates from the intent.
- **On every mutation**, the reducer updates both atomically inside the same message handler: write intent first, then update the annotation, then refresh the panel view. Writing intent first means a crash between the two leaves the more-conservative "remember this" state persisted.
- **Divergence is a bug.** If validation quarantines an intent while its annotation is alive, emit `tracing::warn!` and treat the annotation as authoritative. Test this case explicitly.

`TickerOrderIntent` holds: `symbol`, `version`, **per-(side, type) compound-keyed settings memory** (`last_side: OrderSide`, `last_entry_type: EntryType`, and `entries: HashMap<(OrderSide, EntryType), EntryMemory>` — eight possible buckets covering Buy/Sell × Market/Limit/Stop/StopLimit), `GatrAnchor { anchor_price: Option<f64>, anchor_gatr: Option<f64> }`, `live_annotation_id: Option<AnnotationId>`, `broker_order_id: Option<Uuid>` (hook for future broker wiring), `pinned: bool` (GATR-snap opt-out), `updated_at: DateTime<Utc>`. The intent uses `OrderSide { Buy, Sell }` (the order-panel enum at `order_panel/mod.rs:102-105`) rather than `BracketSide { Long, Short }` so the wire format matches the user's "Buy|Sell" mental model and the order panel that writes most upserts. The bracket widget converts at its render boundary as needed.

**`EntryMemory`** carries the per-compound-key panel state: `entry_price_or_offset`, `quantity`, `tp_enabled: bool`, `tp_value`, `tp_mode`, `sl_enabled: bool` **(defaults to `true` — Stop Loss is on by default for every (side, type) combination unless the user explicitly toggles it off for that specific compound key)**, `sl_value`, `sl_mode`, `sl_type: StopLossType`, `sl_limit_value`. The `sl_enabled` default is the new "SL on by default per compound" rule: switching from `(Buy, Stop)` to `(Sell, Limit)` resets to defaults — including SL on — unless the user has previously touched `(Sell, Limit)` and explicitly toggled SL off for that combo. Each compound key tracks its own opt-out independently; toggling SL off in `(Buy, Stop)` does **not** turn it off in `(Buy, Limit)`.

**Confidence**: high.

### D3: Sync topology — feedback-loop-safe bidirectional updates

**Context**: Today the panel creates brackets but does not observe them. The user needs: drag bracket → panel updates; edit panel → bracket moves. Feedback loops are the top implementation risk.

**Recommendation**: **Single reducer in `app.rs`, message-tagged by source**. Every edit (from panel or chart) becomes an `OrderIntentAppMsg` routed through `apply_order_intent_msg(&mut MidasApp, msg) -> Task<Message>`.

```
        ┌─────────┐                    ┌─────────┐
        │  Chart  │                    │  Panel  │
        │  drag   │                    │  edit   │
        └────┬────┘                    └────┬────┘
             │ Message::OrderIntent(        │
             │   UpdateFromBracketDrag {    │
             │     symbol: AAPL,            │
             │     source: Chart,           │  Message::OrderIntent(
             │     bracket: ... })          │    UpdateFromPanel { ... })
             ▼                              ▼
        ┌────────────────────────────────────────┐
        │        apply_order_intent_msg          │
        │  ┌──────────────────────────────────┐  │
        │  │ 1. eq-check vs cached → NoOp?    │  │
        │  │ 2. symbol matches active chart?  │  │
        │  │ 3. intent_handle.upsert(...)     │  │ ◄─ writes cache sync,
        │  │ 4. annotation_store.update(...)  │  │    schedules flush async
        │  │ 5. panel.refresh_from(&intent)   │  │
        │  │    but SKIP matching `source`    │  │ ◄─ prevents re-emit
        │  └──────────────────────────────────┘  │
        └────────────────────────────────────────┘
```

- **`source: IntentSource { Panel, Chart, Hydration, GatrSnap }`** field on every message. The refresh step (5) skips the widget whose source matches — if the drag came from the chart, do not write back into the chart's drag state.
- **Equality check at the actor cache** before writing, so identical-value updates return `NoOp { reason }` and do not mark the symbol dirty. Second line of defense.
- **Symbol tag**: every drag message carries `symbol: SymbolKey` captured at drag-start. If `chart.current_symbol != msg.symbol` (user switched tickers mid-drag), the reducer logs and drops.

**Confidence**: high.

### D4: GATR re-anchor semantics

**Context**: The user's rule is "if price moved ≥100% of GATR since last seen, move the entire bracket set to match the current price." Three unspecified edge cases: mid-session vs ticker-switch vs startup; rigid translation vs proportional rescale; destructive vs user-controllable.

**Recommendation**: **Rigid translation (already implemented by `reposition_bracket`), fires only at session-boundary events, gated by user pinning and recency**.

- **When the rule fires**:
  - On app startup, after intent load.
  - On ticker re-activation *if* the ticker was not activated this session before (track in a `HashSet<SymbolKey>` cleared at startup).
  - **Never** mid-session while the user is actively viewing the ticker, and **never** repeatedly in the same session.
- **Pinning**: `TickerOrderIntent.pinned: bool`. When true, skip the rule entirely. Expose a pin button on the bracket decorator (Slice 4).
- **Recency guard**: if `intent.updated_at < 1 hour ago`, skip — the user just touched this, do not second-guess them.
- **NaN / non-finite guard**: `if !current_price.is_finite() || !current_gatr.is_finite() { return None; }`. NaN comparisons are always false, so without this guard, the rule silently never fires — the test matrix covers this row explicitly.
- **Missing GATR guard**: if `gatr_abs.is_none()` (daily/weekly charts), skip. `GatrAnchor` fields are `Option<f64>` for this reason.
- **First-touch-endorses semantics** (intentional, document in PR): a freshly-bootstrapped intent has `anchor_price = None`, so the snap rule cannot fire on the very first session — there is nothing to compare against. The first user-initiated upsert (`source: Panel | Chart`) records the anchor at the **current** price, which means "the user touched it here, this is the new home." Practical consequence: if a user opens a long-stale bracket and nudges it once before walking away, the snap rule will measure drift against the *post-nudge* price next session, not the original stale price. This is a feature, not a bug — touching the bracket is treated as endorsement of its current location. Users who want the snap rule to dominate should pin **before** touching, or simply leave the bracket alone for one session so the anchor seeds at the bootstrap location on the next user interaction.
- **Discoverability aid for first-touch-endorses** (lands in Slice 4 alongside the new toast UI): when the reducer handles its first `UpdateFromPanel` / `UpdateFromBracketDrag` for a ticker whose intent had `anchor_price == None` *before* the upsert, it emits a one-shot info toast: `"AAPL: bracket location recorded. Pin to lock against drift snap."` The toast carries no action button (informational only) and auto-dismisses on the standard TTL. Without this, the "first-touch endorses" rule is undocumented from the user's perspective — the trader most likely to be hurt (one who carefully placed a bracket weeks ago and glances at it today) is also the one most likely to nudge it once and silently reset the anchor.
- **Guardrail on destructive snap**: before applying, stash the pre-snap bracket in an in-memory undo slot. Emit a toast "AAPL: bracket re-anchored +$12.40 (price drifted 1.8× GATR). [Undo]". Undo is valid for 30 seconds. Clicking Undo restores the pre-snap state and writes a fresh anchor at the undone price.
- **Flush durability**: after a snap, force `Durability::Immediate` before returning — so a crash cannot lose the new anchor.
- **Logging**: `tracing::info!(target: "midas_app::ticker_order_intent::gatr", symbol, delta, drift_ratio, "bracket re-anchored")`.

**Confidence**: medium. The rule is novel; pinning + recency + undo are guardrails against "user finds a carefully-placed bracket silently moved." Reviewer should confirm.

### D5: Write coalescing and durability

**Context**: Drag = tens of writes/sec. Fsyncing each destroys SSD latency and battery; losing the last position on crash is unacceptable.

**Recommendation**:
- Coalesced write-behind: `tokio::sync::Notify` + 75 ms debounce. Dirty set is `parking_lot::Mutex<HashSet<SymbolKey>>`. Flush drains in one `redb` write txn with `Durability::Eventual`.
- **Idle opportunistic commit**: if no notifies for 750 ms, flush with `Durability::Immediate`.
- **Shutdown ordering** (non-negotiable): (1) mailbox stops accepting new `Upsert` messages, (2) drain pending channel, (3) drain the dirty set inside one final `Durability::Immediate` write txn, (4) drop `redb::Database`. Test this ordering with a test that enqueues updates, calls shutdown, reopens the DB, and asserts every update is present.
- **Override to `Durability::Immediate`** on: shutdown, order submit, GATR snap (success path), GATR snap undo, explicit user Save.

Confidence: high.

### D6: Bracket visual cleanup scope

**Context**: "The brackets are currently a mess visually." The exact defects are not knowable from the plan alone — Slice 0 opens with a prerequisite screenshot spike so the reviewer can annotate the real issues before implementation begins.

**Recommendation**: Treat visual cleanup as scoped polish, **not** a decorator-system rewrite. Split across two slices to keep each one coherent:

**Slice 0 (render-only)**:
1. No overlap between decorator badges when TP and SL are within `0.25 × gatr_abs` of entry — stack vertically in that case.
2. Badge alignment consistent with the crosshair priceline lens style (commit `f17026b`).
3. SL line stays dotted across all statuses (existing rule; add an assertion test if missing).
4. Label truncation never clips mid-digit; measure width before rendering.

**Slice 4 (requires `TickerOrderIntent` to exist)**:
5. PinToggle decorator on the entry badge. Visual state derived per frame from `TickerOrderIntent.pinned` via the context struct — not a field on `OrderBracket`. Click emits `Message::OrderIntent(TogglePin { symbol })`.

Deeper decorator-system work is out of scope.

**Confidence**: medium. Needs a reviewer with eyes on the running app for both slices.

## Implementation Plan

Slices are ordered to ship a visible win first (visual cleanup), then land the store foundation, then the user-observable sync and GATR behaviours, then cleanup. Riskiest work (new dep, new actor, novel rule) is front-loaded within that constraint.

### Slice 0: Bracket visual cleanup

**Goal**: Visible improvement to bracket rendering — ships first so the reviewer sees progress while the store foundation lands. **Render-only**. No new data fields on `OrderBracket`; no pin affordance. The PinToggle UI and `pinned` state live on `TickerOrderIntent` and land in Slice 4.

**Depends on**: None. Fully parallel with every other slice.

**Prerequisite spike** (≤30 min, blocking): capture screenshots of the current bracket rendering at three representative states — (a) normal TP/SL spacing, (b) TP and SL within `0.25 × gatr_abs` of entry, (c) a bracket near the chart's right edge. Reviewer annotates the specific defects. Slice 0's "Done when" is keyed to fixing **exactly those annotated defects**, not the speculative list in D6.

**Files to modify**:
- `desktop/win/crates/midas-chart/src/widget/order_bracket/decorators.rs` — stack TP/SL badges vertically when `|tp - sl| < 0.25 × gatr_abs`; consistent right-margin per commit `f17026b`.
- `desktop/win/crates/midas-chart/src/widget/order_bracket/mod.rs` — enforce `leg_style()` keeps the SL line dotted across every `BracketStatus` variant (add an assertion test if missing).
- `desktop/win/crates/midas-chart/src/widget/order_bracket/tests.rs` — tight-price overlap test; SL-dotted-across-statuses assertion.

**Key details**:
- **No changes to `OrderBracket`'s wire format. Zero field additions.** Existing JSON annotation files continue to load unchanged.
- Label truncation must never clip mid-digit — measure width before rendering.
- Manual screenshot before/after attached to the PR. Automated tests cannot verify visual improvements; state this explicitly in the PR description.

**Testing**:
- Numeric overlap test: entry=100.00, TP=100.10, SL=99.90, gatr_abs=1.00 → badges stack vertically, no horizontal overlap.
- SL stroke across `BracketStatus::{Draft, Pending, Active, PartialFill, Closed, Cancelled}` — all dotted.
- `cargo clippy --workspace -- -D warnings` in both workspaces.

**Done when**: Each reviewer-annotated defect from the prerequisite spike is fixed, overlap test passes, `cargo test --workspace` green. PR tagged `needs-visual-review` so it cannot merge without a human eyeball.

### Slice 1a: Core store — model + actor + coalesced flush + shutdown drain

**Goal**: Foundation that unblocks Slice 2. Persistent per-ticker intent store with an in-memory read cache, coalesced write-behind flush, clean shutdown ordering, and inline validation. **Failure-mode hardening (multi-instance, corruption, disk-full) ships separately in Slice 1b, which does not block Slice 2.**

**Depends on**: None. Can run in parallel with Slice 0.

**Files to create**:
- `desktop/win/crates/midas-app/src/ticker_order_intent/mod.rs` — `TickerOrderIntent` struct (**includes `pinned: bool` as the canonical source of truth**, defaults to `false`); **`entries: HashMap<(OrderSide, EntryType), EntryMemory>`** (per-compound-key memory keyed by Buy/Sell × Market/Limit/Stop/StopLimit — see D2); `EntryMemory` struct with `sl_enabled: bool` defaulting to `true` per compound key; `GatrAnchor { anchor_price: Option<f64>, anchor_gatr: Option<f64> }`; `pub const CURRENT_VERSION: u32 = 1;`; version-tagged serde encoding; `migrate_v0_v1()` stub (seeds migration path even though v1 is current). Serde derives `Default` for `EntryMemory` so missing buckets at load time deserialize to "fresh defaults including `sl_enabled = true`."
- `desktop/win/crates/midas-app/src/ticker_order_intent/store.rs` — `TickerOrderIntentStore` wrapping `parking_lot::RwLock<HashMap<SymbolKey, Arc<TickerOrderIntent>>>`, sync read API (`snapshot(symbol)`, `all_symbols()`), sync `upsert` that writes cache + marks dirty + returns `UpsertOutcome { Applied, NoOp }`.
- `desktop/win/crates/midas-app/src/ticker_order_intent/actor.rs` — `TickerOrderIntentActor` built on `mailbox_processor::MailboxProcessor<OrderIntentMsg, OrderIntentReply>`, owns `redb::Database`, owns the `Store`. Flush task: `tokio::sync::Notify` + 75 ms sleep loop, drains dirty set in one write txn with `Durability::Eventual`, opportunistic `Immediate` commit after 750 ms idle.
- `desktop/win/crates/midas-app/src/ticker_order_intent/handle.rs` — `TickerOrderIntentHandle` (cloneable). **Sync API (safe from iced's sync `update()` loop)**: `fn snapshot(&self, symbol: &SymbolKey) -> Option<Arc<TickerOrderIntent>>`, `fn upsert(&self, msg: OrderIntentMsg) -> UpsertOutcome` — `upsert` mutates the `RwLock` cache synchronously before scheduling the async flush. **Async API**: `async fn flush_now(&self)`, `async fn shutdown(self)`. Trait-shaped (`trait TickerIntentAccess`) so Slice 3's reducer tests can inject a mock. The actor wiring follows the pattern at `midas-store/src/handle/mod.rs` (single mailbox-owning thread, cloneable handle, sync send via `try_send`, await on a oneshot for replies) — but the message types are purpose-specific (`OrderIntentMsg` / `OrderIntentReply`, not midas-store's `DbCommand` / `DbReply`). The structural mirror is the actor lifetime model, not the wire protocol.
- `desktop/win/crates/midas-app/src/ticker_order_intent/validate.rs` — `fn validate(intent: &TickerOrderIntent, last_price: Option<f64>, gatr_abs: Option<f64>) -> Result<(), IntentDefect>`. Checks: TP above entry for Long (mirrored for Short), SL below entry for Long, prices finite, quantities ≥ 0, prices within ±5 × gatr_abs of `last_price` when both are known. Defective intents are dropped with `tracing::warn!` at load time — **no quarantine sidecar**.
- `desktop/win/crates/midas-app/src/ticker_order_intent/reducer.rs` — `OrderIntentAppMsg` enum (full variant list, see below) + `pub fn apply_order_intent_msg(app: &mut MidasApp, msg: OrderIntentAppMsg) -> iced::Task<Message>` as a **stub** that matches every variant and returns `Task::none()`. Slice 3 fills in the sync handlers; Slice 4 fills in `MaybeSnapToGatr` / `TogglePin` / `UndoSnap`. Locking the enum here means downstream slices never re-open it.
- `desktop/win/crates/midas-app/src/ticker_order_intent/tests.rs` — fixtures, round-trip, version migration stub, coalescing under `tokio::time::pause`, NaN rejection, shutdown-drain, equality no-op, `ForgetSymbol` no-op + remove path.

**Files to modify**:
- `desktop/win/Cargo.toml` — add `redb = "2"` to `[workspace.dependencies]`.
- `desktop/win/crates/midas-app/Cargo.toml` — add `redb.workspace = true`.
- `desktop/win/crates/midas-app/src/app.rs` — add the `Message::OrderIntent(OrderIntentAppMsg)` variant to the top-level `Message` enum and a routing arm in `update()` that calls `ticker_order_intent::reducer::apply_order_intent_msg(self, msg)`. The reducer is a stub at this point — every variant returns `Task::none()`. This locks the message shape so Slices 3 and 4 only fill in handlers, never re-open the enum.

**`OrderIntentMsg`** (locked at Slice 1a — no downstream slice re-opens this enum):
```rust
pub enum OrderIntentMsg {
    Upsert { symbol: SymbolKey, intent: TickerOrderIntent, source: IntentSource },
    ForgetSymbol { symbol: SymbolKey },   // handled here; wired to watchlist in Slice 5
    FlushNow,
    Shutdown { force: bool },             // `force: true` bypasses Slice 1b's disk-full guard
}
pub enum OrderIntentReply { Applied { generation: u64 }, NoOp { reason: NoOpReason } }
pub enum IntentSource { Panel, Chart, Hydration, GatrSnap, Bootstrap }
pub enum NoOpReason { IdenticalToCache, StaleSource, InvalidIntent }
```

`ForgetSymbol` lands as a working handler + test case in Slice 1a even though its call site does not appear until Slice 5. This keeps the message enum stable across every downstream slice. Consumers that want "clear the intent but keep memory" call `Upsert` with a freshly-defaulted memory block; the reducer in Slice 3 interprets the semantics.

**`OrderIntentAppMsg`** — the reducer-level (iced `Message`) wrapper. Defined in Slice 1a alongside `OrderIntentMsg` so that Slices 3 and 4 can both compile against a stable shape without re-opening enums:
```rust
pub enum OrderIntentAppMsg {
    UpdateFromPanel { symbol: SymbolKey, snapshot: TickerOrderIntent, source: IntentSource },
    UpdateFromBracketDrag { symbol: SymbolKey, snapshot: TickerOrderIntent, source: IntentSource },
    CancelLiveBracket { symbol: SymbolKey },
    RemoveLiveBracket { symbol: SymbolKey, annotation_id: AnnotationId },
    MaybeSnapToGatr { symbol: SymbolKey },
    TogglePin { symbol: SymbolKey },
    UndoSnap { symbol: SymbolKey },
}
```
Slice 1a ships the enum + a stub `apply_order_intent_msg()` that returns `Task::none()` for every variant. Slice 3 implements `UpdateFromPanel` / `UpdateFromBracketDrag` / `CancelLiveBracket` / `RemoveLiveBracket`. Slice 4 implements `MaybeSnapToGatr` / `TogglePin` / `UndoSnap`. The top-level `Message::OrderIntent(OrderIntentAppMsg)` variant is added to `app.rs` in Slice 1a as well — the routing line just calls the stub. This makes Slice 4's PinToggle decorator (which references `OrderIntentAppMsg::TogglePin`) compile against a stable type from day one of Slice 4.

**Key details**:
- Store path: `dirs::data_local_dir().join("HandOfMidas").join("ticker_state.redb")`. Parent dir created on open.
- Single `redb::TableDefinition<&str, &[u8]>` named `"ticker_intent_v1"`. Value is `serde_json::to_vec_pretty(&intent)` so the file is human-inspectable.
- **Shutdown ordering** (test-enforced, non-negotiable): (1) mailbox stops accepting new `Upsert` / `ForgetSymbol`, (2) drain queued messages into the cache, (3) one final write txn with `Durability::Immediate`, (4) drop `redb::Database`. Cover with a concurrent-upsert-storm + `shutdown().await` test.
- **Stale-cache-between-write-and-refresh rule** (document in module header): `upsert` mutates the `RwLock` cache **synchronously before returning**. The very next `snapshot()` call sees the new value. The async flush task only affects persistence, not visibility. This is the guarantee Slice 3's reducer relies on.
- **Debounce and idle intervals**: 75 ms debounce on the flush loop (assumes 60 Hz drag → ~13 writes/sec on disk regardless), 750 ms idle → opportunistic `Immediate` commit. Both are documented assumptions, not measured — revisit if profiling on Windows target hardware disagrees.
- `thiserror` only, no `anyhow`. `///` docs on every public item. No `unwrap()` outside tests.

**Testing**:
- Round-trip serde with unknown-field forward-compat.
- Version migration stub: feed a hand-written v0 blob, assert it decodes into v1 defaults.
- Coalescing: `tokio::time::pause()`; enqueue 50 updates; advance 80 ms; assert exactly one commit hit the DB via a `DatabaseSpy` wrapper trait that counts commits.
- NaN/non-finite rejection in `validate`.
- Validation drops a TP-below-entry long intent on load with `tracing::warn!` observed via a test subscriber.
- Shutdown drain: enqueue updates, `shutdown().await`, reopen, assert last state present.
- `ForgetSymbol` path: absent symbol returns `NoOp`; present symbol removes from cache, deletes the redb row, flushes before returning.
- Equality no-op: two identical `Upsert` calls → first `Applied`, second `NoOp { IdenticalToCache }`.
- **"Simulated crash" test strategy** (concrete, not vague): the crash test does **not** kill the process; it (a) writes N updates with `Durability::Immediate`, (b) drops the `TickerOrderIntentActor` and the underlying `Database` without calling `shutdown()`, (c) reopens the file with a fresh `Database::open`, and (d) asserts every `Immediate`-flushed state is present. A second test variant truncates the last 64 bytes of the file before reopen and asserts redb's recovery path either restores the previous valid state or surfaces `Error::Corrupted` (which Slice 1b's recovery path then handles). The test does not attempt to simulate `kill -9` because that requires a separate process and cannot run in cargo test cleanly — that scenario is covered by manual smoke testing on Windows target hardware before release.

**Done when**: Actor spawns, handles concurrent upserts, survives the "drop without shutdown" crash test with `Immediate`-flushed state intact, shutdown ordering test passes, `cargo clippy --workspace -- -D warnings` passes.

### Slice 1b: Store hardening — multi-instance, corruption, disk-full recovery

**Goal**: Harden the store against the four named failure modes. Ships in parallel with Slices 2-4 on a second workstream. **Not on the critical path.**

**Depends on**: Slice 1a only.

**Files to modify**:
- `desktop/win/crates/midas-app/src/ticker_order_intent/actor.rs` — on `Database::open`, catch `redb::Error::DatabaseAlreadyOpen` (multi-instance) and `redb::Error::Corrupted` / unparseable header (whole-file corruption). Multi-instance: propagate up to `MidasApp::new()` which surfaces a user-facing dialog via the existing iced window path ("Another Hand of Midas instance is running") → graceful `std::process::exit(1)`. Corruption: rename the file to `ticker_state.redb.corrupt.<timestamp>`, `tracing::error!`, open a fresh empty DB, enqueue a startup `Message::ShowToast("Order memory reset — previous file was corrupt")` via the existing toast infrastructure (`app.rs:478-482`).
- `desktop/win/crates/midas-app/src/ticker_order_intent/actor.rs` — flush task catches `std::io::ErrorKind::StorageFull` on `commit()`, keeps symbols in the dirty set, backs off 1s/2s/5s/10s/30s, and refuses a plain `Shutdown { force: false }` while the backoff is active by surfacing a modal "Cannot save order memory — disk full. Free space to exit cleanly." `Shutdown { force: true }` bypasses this guard (data loss accepted).
- `desktop/win/crates/midas-app/src/ticker_order_intent/tests.rs` — new cases: multi-instance open, truncated/junk-header file, disk-full commit via a test-only `redb::StorageBackend` wrapper that returns `StorageFull` on the N-th `write_all`, backoff progression, force-shutdown bypass.

**Key details**:
- Multi-instance dialog uses the existing iced window surface, not a new subsystem.
- Corruption toast reuses `Message::ShowToast` — no new UI infrastructure.
- Disk-full test fixture is a thin `StorageBackend` wrapper that can be instructed to fail the N-th write. Does **not** require a new VFS layer.
- The `force: true` shutdown variant is already part of the enum from Slice 1a, so 1b only changes behavior, not wire protocol.

**Testing**:
- Multi-instance: two `Database::open` calls on the same path; second returns `DatabaseAlreadyOpen`; `MidasApp::new()` returns a clean error that the main loop converts to `exit(1)`.
- Corruption: write junk to the file header; `open`; assert the file was renamed with a `.corrupt.<ts>` suffix and a fresh DB was created; assert the startup toast is queued.
- Disk-full: fixture returns `StorageFull` on commit #3 of a 10-commit sequence; dirty set retains the unwritten symbols; backoff scheduler fires at 1s/2s/5s; recovery succeeds once the fixture allows writes again.
- Shutdown-during-backoff: plain `Shutdown { force: false }` surfaces the modal and does not drop the actor; `Shutdown { force: true }` skips the guard and drops the actor with data loss logged.

**Done when**: All four failure modes have green tests; no regression on Slice 1a's test suite.

### Slice 2: Panel hydration + bootstrap from existing annotations

**Goal**: When no live bracket exists, the order panel hydrates from the per-ticker intent. On first run after upgrade, existing bracket annotations seed the intent so the panel does not disagree with the visible bracket.

**Depends on**: **Slice 1a only.** Does **not** depend on Slice 1b — the core store is sufficient for hydration to land. Consumes only the sync read API (`handle.snapshot()`, `all_symbols()`) + `shutdown()`, so implementation can begin against the frozen `TickerIntentAccess` trait shape on day 1 of Slice 1a.

**Files to modify**:
- `desktop/win/crates/midas-app/src/app.rs` — add `order_intent_handle: TickerOrderIntentHandle` field; construct in `MidasApp::new()`; call `handle.shutdown().await` from the existing shutdown path; on `ChartActivated(chart_id)` handler, read the chart's symbol → `snapshot(&symbol)` → `panel.hydrate_from_intent(&intent, last_price)`; at startup, iterate `annotation_store` for every symbol that has a live `OrderBracket` annotation but no intent row, call `apply_order_intent_msg(Upsert { source: Bootstrap, … })` to seed.
- `desktop/win/crates/midas-app/src/order_panel/mod.rs` — add `dirty: bool` field to `OrderPanelState` (set on any user edit, reset on submit/cancel/hydrate; see "`dirty` semantics" below). Add `fn hydrate_from_intent(&mut self, intent: &TickerOrderIntent, last_price: Option<f64>)` that uses **`intent.last_side` and `intent.last_entry_type` as the compound key** to look up `intent.entries.get(&(side, entry_type))`. If found, populate `side`, `entry_type`, `tp_*`, `sl_*` (including `sl_enabled`), `sl_type`, `sl_limit_value`, `limit_price`, `stop_price` from that `EntryMemory`. **If the compound key has no entry yet, fall back to `EntryMemory::default()` — which sets `sl_enabled = true`** (the "SL on by default per compound" rule). Skip hydration when `self.dirty == true && self.symbol == intent.symbol`. Add a second helper `fn rehydrate_for_compound(&mut self, intent: &TickerOrderIntent, new_side: OrderSide, new_type: EntryType)` invoked when the user toggles side or entry type within the same ticker — it re-reads the panel state from `intent.entries.get(&(new_side, new_type))` (or defaults), so switching from "Buy Stop" to "Sell Limit" lands the user on whatever they last had for that combo (or fresh defaults including `sl_enabled = true` if they have never used that combo).
- `desktop/win/crates/midas-app/src/order_panel/tests.rs` — hydration happy-path, skip-when-dirty, ticker-switch re-hydration, missing-intent-falls-back-to-defaults, **compound-key fallback (`(Sell, Limit)` never touched → defaults with `sl_enabled = true`)**, **side/type switch within a ticker re-hydrates from the new compound bucket**, **SL toggle persists per compound (toggling SL off in `(Buy, Stop)` does not affect `(Buy, Limit)`)**.

**`dirty` semantics**:
- Set `dirty = true` in every setter that is invoked from a user edit (keyboard/mouse).
- Set `dirty = false` on: successful submit, explicit cancel, successful hydrate from a *different* ticker, explicit "reset" button.
- Hydration from the *same* ticker while `dirty = true` is a no-op.

**Bootstrap rule**: at startup, for every live bracket annotation without a matching intent, create a fresh `TickerOrderIntent` by reading the bracket's fields. `source: Bootstrap` so the reducer knows not to treat it as a user edit. Ensures that the first hydration after upgrade does not silently forget existing brackets.

**Testing**:
- Hydrate a fresh panel from an intent → fields populated for `(intent.last_side, intent.last_entry_type)` compound bucket.
- Hydrate with `dirty = true` same symbol → no-op.
- Ticker switch → re-hydrate from the new ticker's intent (using *its* `last_side`/`last_entry_type`).
- Bootstrap: annotation present, intent absent → intent created on startup.
- No intent, no annotation → panel defaults (regression check), including `sl_enabled = true`.
- **Compound-key default**: intent exists with `(Buy, Stop)` populated and `sl_enabled = false`, but `(Sell, Limit)` empty. Switch to `(Sell, Limit)` → panel hydrates with defaults including `sl_enabled = true`.
- **SL toggle per compound**: user toggles SL off in `(Buy, Stop)`; assert `intent.entries[(Buy, Stop)].sl_enabled == false` AND `intent.entries[(Buy, Limit)]` is unchanged (still default-true or absent).
- **Side/type switch within a ticker**: panel on `(Buy, Stop)` with custom prices → user toggles entry type to `Limit` → assert `rehydrate_for_compound` was called with `(Buy, Limit)` and the panel reflects that bucket's state (or defaults if absent).

**Done when**: Switching tickers in the UI restores the last-used side, type, and prices per ticker; no regressions on existing `order_panel::tests`.

### Slice 3: Bidirectional sync reducer

**Goal**: Drag bracket → panel updates. Edit panel → bracket moves. Cancel button → intent forgets the live link but keeps memory. Feedback loops are impossible by construction.

**Depends on**: Slices 1a, 2.

**Files to create**:
- `desktop/win/crates/midas-app/src/ticker_order_intent/reducer/tests.rs` — full round-trip matrix with a mock `TickerOrderIntentHandle` implementation behind the `TickerIntentAccess` trait introduced in Slice 1a.

**Files to modify**:
- `desktop/win/crates/midas-app/src/ticker_order_intent/reducer.rs` — fill in the `UpdateFromPanel`, `UpdateFromBracketDrag`, `CancelLiveBracket`, `RemoveLiveBracket` handlers (`OrderIntentAppMsg` enum and the stub function were created in Slice 1a). The `MaybeSnapToGatr` / `TogglePin` / `UndoSnap` arms remain stubs until Slice 4.
- `desktop/win/crates/midas-app/src/chart_widget.rs` — after a bracket drag commits, capture `symbol_at_drag_start: SymbolKey` and emit `Message::OrderIntent(OrderIntentAppMsg::UpdateFromBracketDrag { symbol: symbol_at_drag_start, source: IntentSource::Chart, snapshot })`. The reducer rejects if `active chart symbol != symbol_at_drag_start`.
- `desktop/win/crates/midas-app/src/order_panel/mod.rs` — when the user edits a price/type field, emit `Message::OrderIntent(UpdateFromPanel { symbol, source: IntentSource::Panel, snapshot })`. Wire the existing Cancel button to `CancelLiveBracket { symbol }`. Hook the observer of `AnnotationStore::remove` so that any external removal emits `RemoveLiveBracket { symbol, annotation_id }` and the intent's `live_annotation_id` is reconciled.

**Key details**:
- **Reducer flow** (strict order): (1) cache-equality check — if the incoming snapshot equals the cached intent, drop as NoOp; (2) symbol consistency — if the drag message's captured symbol does not match the active chart's symbol, log `warn!` and drop; (3) `handle.upsert(Upsert { … source })` — cache mutates sync; (4) if `live_annotation_id.is_some()`, update the annotation via `annotation_store.update`; (5) refresh the view of the surface that did **not** originate the message (`source == Chart` → refresh panel only; `source == Panel` → refresh chart only). The fifth step is what prevents the echo.
- **`CancelLiveBracket`** clears `live_annotation_id` and removes the annotation, but preserves `last_side`, `last_entry_type`, and all `EntryMemory` prices — "forget this particular bracket, not the user's preferences." Intent is then a memory without a live link, and the next panel hydration picks it up.
- **`RemoveLiveBracket`** is the inverse hook for external removals (undo, hotkey, drag-off-chart). Called when `AnnotationStore::remove` returns an `OrderBracket`.
- **`dirty` flag reset rule** (both `CancelLiveBracket` and `RemoveLiveBracket`): once the live bracket is gone, the user is no longer "in the middle of editing it." Both handlers must reset the linked panel's `dirty = false` after clearing `live_annotation_id`. Otherwise the next `ChartActivated` skips hydration (Slice 2's guard) and silently leaves the panel out of sync until the recency window expires — a real data-loss path. Test enforced (see "Testing" below).
- **Compound-key write rule** (`UpdateFromPanel` / `UpdateFromBracketDrag`): the reducer reads `(snapshot.side, snapshot.entry_type)` from the inbound message and writes the panel state into `intent.entries[(side, type)]` — the rest of `intent.entries` is untouched. It then updates `intent.last_side = side` and `intent.last_entry_type = type` so the next hydration finds the right bucket. Toggling SL off in the panel writes `entries[(side, type)].sl_enabled = false` and is therefore *sticky per compound only* — switching to a different `(side, type)` combo still sees the default-true. Switching from `(Buy, Stop)` to `(Sell, Limit)` does **not** carry any state forward; the panel re-hydrates from `entries.get(&(Sell, Limit))` (or defaults including `sl_enabled = true`).
- **No `Task<Message>` from the reducer** other than the optional toast/undo emission in Slice 4. The reducer is sync-observable for testing.

**Testing**:
- Drag → reducer → both surfaces in lockstep.
- Panel edit → reducer → bracket moves; intent updated; no follow-up message emitted (assert via returned `Task` == `Task::none()`).
- Cancel → live link cleared, memory preserved, panel hydrates from memory next tick.
- External annotation removal → intent's link cleared without error.
- **Edit-then-undo regression test**: user edits panel (`dirty = true`), then external removal fires `RemoveLiveBracket`. Assert: `live_annotation_id == None`, `dirty == false`, next `ChartActivated` successfully hydrates from intent memory. Mirror test for `CancelLiveBracket` after a user edit.
- Mid-drag ticker switch → reducer drops the message.
- No-op suppression → second identical update replies `NoOp`.
- Panel with no live bracket: editing fields updates intent only, does not create a phantom annotation.
- **Compound-key write isolation**: seed `entries[(Buy, Stop)]` with `sl_enabled = false`. Send `UpdateFromPanel` with `(side: Buy, entry_type: Limit, sl_enabled: true)`. Assert: `entries[(Buy, Limit)]` now reflects the new state, `entries[(Buy, Stop)]` is unchanged (still has `sl_enabled = false`), `intent.last_side = Buy`, `intent.last_entry_type = Limit`.
- **SL-off persists per compound**: toggle SL off in `(Buy, Stop)` via `UpdateFromPanel`, switch to `(Sell, Stop)`, toggle SL off there too. Assert both buckets are sticky-off but `(Buy, Limit)` and `(Sell, Limit)` remain default-on (or absent).

**Done when**: Manual test in the running app confirms bidirectional sync at all zoom levels; automated integration test in `desktop/win/tests/` spins up the real reducer + mock chart/panel and asserts lockstep; `cargo test --workspace` passes.

### Slice 4: GATR re-anchor + PinToggle UI + toast action button

**Goal**: Implement the "≥100% GATR drift" re-anchor rule at session boundaries, reusing the existing `should_reposition` / `reposition_bracket` helpers. Guardrails prevent destructive surprises. Also lands the PinToggle decorator UI (single source of truth: `TickerOrderIntent.pinned`) and **ships the first user-visible toast in the app** — the `toast_message` state field exists today (`app.rs:151-154`) but is never rendered by the view layer, so Slice 4 owns building the toast UI from scratch as well as adding the action-button capability needed for the undo affordance.

**Depends on**: Slices 1a, 3. (Slice 3 transitively hard-depends on Slice 2, so Slice 2 is a transitive dependency — the previous "soft dependency" framing was misleading and is dropped. The startup bootstrap iteration point in Slice 4 can use either Slice 2's bootstrap loop or `annotation_store.all_symbols()` directly; this is a freestanding implementer's choice with no ordering implication.)

**Files to create**:
- `desktop/win/crates/midas-app/src/ticker_order_intent/gatr_snap.rs` — `fn maybe_snap(intent: &TickerOrderIntent, current_price: f64, gatr_abs: Option<f64>) -> Option<SnapPlan>`. Returns `Some(SnapPlan { delta, new_anchor, reason: SnapReason })` or `None`. Pure function; takes all inputs, no state. Internally reuses `order_panel::should_reposition` for the threshold check.
- `desktop/win/crates/midas-app/src/ticker_order_intent/gatr_snap/tests.rs` — table-driven coverage of every scenario.

**Files to modify**:
- `desktop/win/crates/midas-app/src/ticker_order_intent/reducer.rs` — fill in the three GATR/pin handlers (the enum variants are already declared in Slice 1a, the four sync handlers in Slice 3): `MaybeSnapToGatr` handler — look up `last_price` and `gatr_abs` via `app.market_data_cache.get(&symbol)`, call `maybe_snap`, if `Some` → stash pre-snap state in the undo slot, apply via `reposition_bracket`, write a fresh `GatrAnchor`, **force `Durability::Immediate` flush**, emit `Message::ShowToast` with a populated `ToastAction::Undo`. `TogglePin` handler — flip `TickerOrderIntent.pinned` via `upsert`. `UndoSnap` handler — restore the pre-snap bracket, write a fresh anchor at the undone price, `Durability::Immediate` flush. **Also extend the existing Slice 3 `UpdateFromPanel` / `UpdateFromBracketDrag` arms** with the GATR-anchor seeding rule from D4: if the cached `intent.gatr_anchor.anchor_price.is_none()` *before* the upsert, after writing the new anchor at the current price, emit a one-shot info toast `"{symbol}: bracket location recorded. Pin to lock against drift snap."` (no action button). This is the discoverability aid that makes the "first-touch endorses" semantics visible to users — without it, the rule is silent and the trader most likely to be hurt by it never learns it exists.
- `desktop/win/crates/midas-app/src/app.rs` — (1) **replace state model**: swap `toast_message: Option<String>` + `toast_created_at: Option<Instant>` (lines 151-154) for `toast: Option<ToastState { message: String, created_at: Instant, action: Option<ToastAction { label: String, on_click: Box<Message> }> }>`. (2) **Replace the `Message::ShowToast(String)` variant** with `Message::ShowToast { message: String, action: Option<ToastAction> }` (struct variant — disambiguated). Every existing call site at `app.rs:3414,5002,5168,5173,5189,5273,5306` is updated to the new shape with `action: None`; the `Message::ShowToast` arm at line 5305 mirrors the new fields. Mechanical, ~20 LOC of plumbing. (3) Track a `HashSet<SymbolKey> snapped_this_session` cleared at startup; `snapped_this_session` is initialized empty in `MidasApp::new()` and is **not persisted** — a crash-and-relaunch resets it (intentional). (4) On startup, after the bootstrap loop, iterate symbols and emit `MaybeSnapToGatr`. (5) In `ChartActivated` handler, emit `MaybeSnapToGatr` only if the symbol is not yet in `snapped_this_session`.
- `desktop/win/crates/midas-app/src/app/views.rs` — **build the toast view layer from scratch.** No `toast` references exist in this file today. Add a floating overlay (iced 0.14 `stack!` or absolute-positioned container) anchored bottom-right of the main window, rendering `app.toast` when `Some` as a row of `text(message)` plus an optional `button(label).on_press(action.on_click.clone())`. Style follows the bracket-decorator badge palette for visual consistency. Click anywhere on the toast (or on the action button) emits `Message::DismissToast` after firing the action. Realistic total scope: **100-200 LOC** including the style struct, container/positioning math, the `ToastAction` boxing plumbing across call sites, dismiss-on-click wiring, and z-ordering over the existing layout. This is its own visual deliverable and inherits the `needs-visual-review` PR gate.
- `desktop/win/crates/midas-chart/src/widget/compute.rs` — **add `pub pinned: bool` field to `ComputeContext`** at lines 20-55 (the per-frame render context that already carries `camera`, `viewport`, `theme`, `hovered_annotation`, `selected_annotation`, etc.). Default `false`. Populated per chart-render-pass by `midas-app` from `order_intent_handle.snapshot(&chart.symbol).map(|i| i.pinned).unwrap_or(false)`. **No field added to `OrderBracket`.**
- `desktop/win/crates/midas-chart/src/widget/decorator/action.rs` (or wherever `DecoratorAction` is defined — see `interaction/mod.rs:172` for the consumer) — add `TogglePin` variant to the existing `DecoratorAction` enum, alongside the current `CloseAnnotation`, `CreateTakeProfit`, `CreateStopLoss`, `CycleEntryType`, `EditQuantity`, `EditPrice`, `ToggleLocked`, `Submit`, `Save`, `RemoveStopLoss`, and `Custom(u32)` variants. **This is the actual pattern bracket decorators use** — clicks emit `HitZoneKind::Decorator { action: DecoratorAction, .. }` which bubbles via `ChartAction::DecoratorClick`. **No new `HitZoneKind` variant needed.**
- `desktop/win/crates/midas-chart/src/widget/order_bracket/decorators.rs` — add `pin_toggle_group()` decorator builder; visual state is read from `ctx.pinned` per frame. The decorator emits the existing `HitZoneKind::Decorator { action: DecoratorAction::TogglePin, annotation_id, .. }` for its hit zone — same plumbing as `CycleEntryType`, `EditQuantity`, etc.
- `desktop/win/crates/midas-app/src/chart_widget.rs` (or wherever `ChartAction::DecoratorClick` is handled — see `interaction/mod.rs:145`) — add a match arm for `DecoratorAction::TogglePin` that emits `Message::OrderIntent(OrderIntentAppMsg::TogglePin { symbol: chart.symbol.clone() })`. ~5 LOC alongside the existing `DecoratorClick` arms. **No field added to `OrderBracket`.**
- `desktop/win/crates/midas-app/src/ticker_order_intent/store.rs` — add an in-memory undo slot: `HashMap<SymbolKey, PreSnapState>` with 30-second TTL. Not persisted — undo is intentionally session-bounded.

**`maybe_snap` guards** (all tested):
- `intent.pinned == true` → `None`.
- `intent.updated_at` within the last hour → `None` (recency guard).
- `!current_price.is_finite() || gatr_abs.map_or(true, |g| !g.is_finite() || g <= 1e-9)` → `None`.
- `intent.gatr_anchor.anchor_price.is_none()` → `None` (first save — record anchor, do not snap).
- `intent.live_annotation_id.is_none()` → `None` (no bracket to snap).
- Otherwise, reuse `should_reposition(anchor_price, current_price, Some(anchor_gatr))`. If true, compute `delta = current_price - anchor_price`, return `Some(SnapPlan { delta, new_anchor: GatrAnchor { anchor_price: Some(current_price), anchor_gatr: Some(gatr_abs) }, reason: DriftExceeded })`. The `intent.updated_at` field on the parent `TickerOrderIntent` carries the timestamp; `GatrAnchor` itself is timestamp-free.

**Key details**:
- **Reuse `should_reposition` and `reposition_bracket` at `order_panel/mod.rs:598,605`** — do not duplicate the math. The only new code is "when does it fire" and "what happens around it."
- **Toast UI is net-new** (corrected from a previous "small extension" framing). The `toast_message` state field exists at `app.rs:151-154,478-482` and is set in ~10 places, but **nothing in `app/views.rs` reads it** — users have never seen a toast in this app. Slice 4 ships the entire toast view layer for the first time: floating bottom-right overlay, message text, optional action button, dismiss-on-click, z-ordering. Realistic estimate: ~20 LOC of state/message-shape plumbing in `app.rs` + 80-150 LOC of new view code in `app/views.rs`. The action-button capability needed for the GATR-snap undo affordance is bundled here because there is no point shipping a toast without it for this use case.
- **Undo TTL** reuses the existing `toast_created_at` auto-dismiss path; the 30-second session-bounded window aligns with the "memory source of truth" rule (undo belongs to the current session, not across restarts).
- **`GatrAnchor` lifecycle rule (single source of truth)**: `anchor_price` and `anchor_gatr` transition from `None` to `Some(current_price)` / `Some(current_gatr_abs)` the **first time the intent is persisted via an `Upsert` whose `source` is `IntentSource::Panel` or `IntentSource::Chart`**. `Bootstrap` and `Hydration` sources do **not** seed the anchor — bootstrap reads what is already in the annotation but deliberately leaves the anchor unset so the first *user* touch records it. Subsequent user upserts bump `updated_at` but do not change `anchor_price` unless (a) a GATR snap fires and writes a fresh anchor at the new location, or (b) the user clicks Undo, which writes a fresh anchor at the undone price. This rule lives in `reducer.rs` and is test-enforced.
- **PinToggle click path**: decorator emits `Message::OrderIntent(TogglePin { symbol })` → reducer reads current intent → upserts with `pinned: !pinned` → next frame the decorator derives its visual state from the refreshed snapshot. No drift because the intent is the only source of truth.

**Testing** (table-driven pure-function tests in `gatr_snap/tests.rs`):
| Case | anchor_price | current_price | anchor_gatr | current_gatr | pinned | updated_at | live_id | Expected |
|---|---|---|---|---|---|---|---|---|
| Drift < 100% | 100 | 100.5 | 1.0 | 1.0 | false | 2h ago | Some | None |
| Drift = 100% | 100 | 101.0 | 1.0 | 1.0 | false | 2h ago | Some | None (> not ≥) |
| Drift > 100% | 100 | 102.0 | 1.0 | 1.0 | false | 2h ago | Some | Some(delta=+2) |
| Pinned | 100 | 110 | 1.0 | 1.0 | true | 2h ago | Some | None |
| Recent edit | 100 | 110 | 1.0 | 1.0 | false | 10min ago | Some | None |
| NaN price | 100 | NaN | 1.0 | 1.0 | false | 2h ago | Some | None |
| NaN gatr | 100 | 110 | 1.0 | NaN | false | 2h ago | Some | None |
| No anchor | None | 110 | None | 1.0 | false | 2h ago | Some | None (seed anchor, no snap) |
| No GATR (daily) | 100 | 110 | None | None | false | 2h ago | Some | None |
| Tiny GATR | 100 | 110 | 1e-10 | 1e-10 | false | 2h ago | Some | None (denominator guard) |
| No live bracket | 100 | 110 | 1.0 | 1.0 | false | 2h ago | None | None |

- Integration: seed intent with stale anchor; bump `MarketCache` `last_price` by 2 × gatr_abs; emit `MaybeSnapToGatr`; assert annotation legs moved by exactly `delta`, new anchor recorded, `Durability::Immediate` flush observed, `Message::ShowToast { action: Some(ToastAction { label: "Undo", … }) }` emitted.
- Undo: emit the undo message within 30s, assert pre-snap state restored and a fresh anchor written at the undone price.
- Session tracking: `ChartActivated(AAPL)` twice in one session → second activation is a no-op.
- Pin round-trip: seed intent with `pinned: false`; emit `TogglePin`; assert `snapshot().pinned == true`; emit `MaybeSnapToGatr` with drift > 100% GATR; assert `None` (pinned guard wins).
- Anchor lifecycle: `Upsert { source: Bootstrap }` leaves `anchor_price == None`; subsequent `Upsert { source: Panel }` sets `anchor_price == Some(current_price)`; third `Upsert { source: Panel }` leaves `anchor_price` unchanged but bumps `updated_at`.
- **Discoverability toast** (D4 first-touch endorses): `Upsert { source: Bootstrap }` followed by `Upsert { source: Panel }` emits a `Message::ShowToast { action: None, .. }` with the "bracket location recorded" message. A *second* `Upsert { source: Panel }` does **not** re-emit the toast (one-shot per ticker per anchor-seed).
- Toast call-site compat: every pre-existing `ShowToast(String)` path at `app.rs:3414,5002,5168,5173,5189,5273,5306` still renders correctly with `action: None`.

**Done when**: Every table row passes, integration tests cover the apply/undo/pin/anchor-lifecycle paths, manual review confirms the toast appears with a working Undo button and the PinToggle reflects intent state, `cargo clippy --workspace -- -D warnings` passes. PR tagged `needs-visual-review` to match Slice 0's gate.

### Slice 5a: Watchlist hygiene

**Goal**: Prevent the intent store from growing unbounded as users add and remove tickers.

**Depends on**: Slices 1a + 2. (1a defines the `ForgetSymbol` handler; 2 plumbs the `order_intent_handle` field onto `MidasApp`, which the watchlist call site needs.) Independent of Slices 1b, 3, 4.

**Files to modify**:
- `desktop/win/crates/midas-app/src/watchlist/*` — on symbol removal, call `order_intent_handle.upsert(OrderIntentMsg::ForgetSymbol { symbol })`. The `ForgetSymbol` variant and its handler are already part of `OrderIntentMsg` as of Slice 1a; this slice only adds the call site.

**Testing**:
- Watchlist-remove path: add symbol, save intent, remove from watchlist, assert redb row gone and cache entry cleared.

**Done when**: No orphaned intent rows after watchlist removal; `cargo clippy --workspace -- -D warnings` passes.

### Slice 5b: Documentation + optional dump tool

**Goal**: Document the feature; ship a debugging helper.

**Depends on**: Slices 1a–4 (for accurate documentation of the finished behavior). Can ship at any point after the feature is visibly working.

**Files to create**:
- `desktop/win/crates/midas-app/src/bin/dump_ticker_intent.rs` (optional) — read-only dump of every intent as pretty JSON. For support/debugging. Guarded behind a `--feature debug-tools` cargo feature so it is not part of release builds.

**Files to modify**:
- `desktop/win/CLAUDE.md` — append to the Documentation Map: `| Ticker order state | desktop/win/plan/ticker-order-state/README.md |`.

**Testing**:
- `dump_ticker_intent` smoke test behind the feature gate (only if the tool ships).

**Done when**: Docs updated; `cargo clippy --workspace -- -D warnings` passes in both workspaces.

### Dependency Summary

- **Critical path**: Slice 1a → Slice 2 → Slice 3 → Slice 4.
- **Parallelizable workstreams**:
  - **Slice 0** (visual cleanup) is mostly independent — but it does share `desktop/win/crates/midas-chart/src/widget/order_bracket/decorators.rs` with Slice 4 (Slice 0 reworks badge layout; Slice 4 adds `pin_toggle_group()`). **Resolution**: Slice 0 lands first, Slice 4 rebases onto it. This is the natural order anyway since Slice 4 is at the end of the critical path. No other file overlap with any other slice.
  - **Slice 1b** (failure-mode hardening) depends only on Slice 1a and runs in parallel with Slices 2-4 on a second workstream. It is **not** on the critical path.
  - **Slice 2** has two halves: the `order_panel/mod.rs` work (hydrate fn, dirty semantics, tests) can begin on day 1 of Slice 1a against the frozen `TickerIntentAccess` trait shape using a mock impl. The `app.rs` wiring half (handle field on `MidasApp`, construct in `new()`, shutdown wiring, `ChartActivated` hook) needs the concrete `TickerOrderIntentHandle` and waits for Slice 1a to reach handle-ready state. Net: Slice 2 is partially-parallel from day 1, fully-unblocked once 1a's handle compiles.
  - **Slice 5a** (watchlist hygiene) depends on Slices 1a + 2; can ship any time after both land.
  - **Slice 5b** (docs + dump tool) depends on the full feature landing for accurate documentation; ships at any later point.
- **Riskiest slice**: Slice 1a (new dep, new actor, new persistence layer). Front-loaded after Slice 0's cheap visible win. Failure-mode recovery is deliberately pushed to 1b so it cannot block the downstream chain.
- **Visual review gates**: Slice 0 (bracket polish), Slice 4 (PinToggle decorator + new toast UI). Both PRs carry a `needs-visual-review` tag and require a human reviewer to look at the running app — automated tests cannot verify visual correctness.

## Risks & Unknowns

1. **`redb` is a new dependency.** Stable since 1.0 (June 2023), pure-Rust, no C bindings, but still unfamiliar to the team. Mitigation: follow the `midas-store` actor structure verbatim; keep redb behind the `TickerOrderIntentHandle` so swapping for `rusqlite` is a single-crate change. **Rollback honesty**: once Slices 2-4 ship, rollback is not trivial — reverting requires retaining the sync gap, which is the original bug. If redb turns problematic mid-rollout, the clean path is to swap the actor's backend to `rusqlite`, not to revert Slices 2-4.
2. **GATR rule is novel.** Pinning + recency + undo guardrails reduce the blast radius. Reviewer should confirm D4 before Slice 4 ships. If rigid translation is wrong, only `gatr_snap.rs` changes.
3. **Feedback-loop bugs.** Research's #1 footgun. Multiple defenses: `source` tag on every message, reducer skip-matching-source, actor equality-check no-op suppression, integration test asserting `Task::none()` output. Reviewer should manually verify at least one drag→panel→chart→drag cycle.
4. **Windows `rename` atomicity caveat.** `annotation_persistence.rs` is still subject to rust-lang #123985. Out of scope; the new store sidesteps it via redb's single-file transactional format.
5. **"Currently a mess visually" is underspecified.** Slice 0 is scoped polish only. If the reviewer's real complaint is a decorator-system architecture issue, that belongs to the widget-system track.
6. **Intent vs broker order-state reconciliation.** Once a bracket is submitted, the broker order log (SQLite) is authoritative for lifecycle. The `broker_order_id: Option<Uuid>` field is a forward hook only. Out of scope beyond reserving the field.
7. **`ForgetSymbol` hygiene** depends on Slice 5a's watchlist wiring. The message handler itself is live from Slice 1a, so the store can never ship with a stale enum — but if 5a slips, a long-lived user accumulates orphaned rows (bounded by the number of symbols they have ever touched). Non-critical; no correctness impact.
8. **Slice 4 toast extension touches ~10 existing call sites** (`app.rs:3414,5002,5168,5173,5189,5273,5306`). Low risk — each call site is a mechanical `ShowToast(String)` → `ShowToast { message, action: None }` substitution — but worth grep-verifying during review.

## Testing Strategy

- **Unit**: pure functions (`maybe_snap`, `validate`, `hydrate_from_intent`) follow existing `order_panel::tests.rs` fixture/builder patterns.
- **Store**: round-trip serde, version migration stub, coalesced flush counting with `tokio::time::pause` (fast synthetic), corruption/disk-full/multi-instance failure modes.
- **Realistic-load coalescing**: one `#[ignore]`-by-default test in `desktop/win/tests/` runs a non-paused 60 Hz stream for 2 seconds and asserts commit count ≤ 30. Runs on nightly CI.
- **Reducer**: unit tests with a `TickerIntentAccess` trait mock exercise every `OrderIntentAppMsg` variant + feedback-loop guards.
- **Integration**: one end-to-end test in `desktop/win/tests/` spawns the real actor, simulates drag → switch-ticker → reload → GATR snap → undo, asserts state at each step.
- **Manual UI**: `cargo run -p midas-app` and exercise: drag updates panel, panel updates chart, switching tickers restores memory, GATR snap fires on session boundary with toast + working undo, pin toggle prevents snap, visuals are not a mess. Capture before/after screenshots for Slice 0 and Slice 4.
- **CI**: `cargo fmt --all -- --check`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace` on both root and desktop workspaces.

## Non-Goals / Out of Scope

- **Full decorator-system rework.** Slice 0 is polish only.
- **Multi-process / multi-instance writer coordination.** Second instance exits gracefully with a dialog.
- **Cross-device sync / cloud backup** of ticker intent.
- **Replacing `annotation_persistence.rs` with redb.** Annotations stay JSON.
- **Consolidating `annotation_persistence.rs` onto the `TickerOrderIntent` actor or storage backend.** The two stores remain independent. Slice 2's bootstrap is a read-only copy from annotations into intent, not a migration of annotations into redb. A future "unify storage backends" effort is a separate track.
- **Event-sourced order history.** Lives in `midas-broker`'s SQLite order log.
- **Proportional rescale of brackets on GATR snap.** Rigid translation only, reusing `reposition_bracket`.
- **Continuous tick-driven re-anchor.** Only fires at session boundaries.
- **Manual edits to the `.redb` file.** Unsupported; corruption triggers file reset with a toast.
- **Backfilling intent from historical annotations on every launch.** Bootstrap runs once per symbol — the first time a live annotation is seen without a matching intent.
- **Undoing a GATR snap after 30 seconds or across sessions.** Session-bounded by design.

## Review Notes

- **D1 (`redb` vs `rusqlite`) is the one decision the reviewer should actively weigh.** `rusqlite` is already used in `midas-broker`, so the team knows it. `redb` is chosen for per-transaction durability control and to avoid a new C-FFI dep in the desktop workspace, not because `rusqlite` is wrong. The actor/reducer/schema layers are backend-agnostic — swapping is a single-crate change before Slice 2 ships. Flag: medium confidence.
- **D4 (GATR re-anchor semantics) is the second decision to weigh.** The plan picks rigid translation at session boundaries with pin + 1-hour recency + 30-second undo guardrails. Proportional rescale, mid-session ticks, and permanent undo are all explicitly rejected but easy to revisit. Flag: medium confidence. One specific edge case the reviewer should confirm: percent-mode stops (the `PriceInputMode::Percent` variant) get rigid translation, which effectively discards the "% of price" intent. If percent-mode is a real use-case, branch `maybe_snap` on `intent.last_entry_type.price_mode()` — otherwise accept as a known limitation.
- **Slice 0 ships first.** The reviewer sees a visible improvement immediately while Slice 1a lands in parallel.
- **`should_reposition` / `reposition_bracket` already exist** at `order_panel/mod.rs:598,605`. The plan reuses them verbatim. The "new" GATR work is firing logic and guardrails, not math.
- **`broker_order_id: Option<Uuid>`** is reserved in the schema but unused in this plan. Hook for future broker wiring without requiring another schema version bump.
- **Toast UI is net-new in Slice 4, not an "extension"** (corrected from a previous framing). The `toast_message` field exists at `app.rs:151-154,478-482` and is set in ~10 places, but **`app/views.rs` does not render it** — users have never seen a toast in this app. Slice 4 ships the entire toast view layer for the first time. Realistic budget: **100-200 LOC total** (style struct + view function + container/positioning + `ToastAction` boxing across call sites + dismiss-on-click + z-ordering). The action-button capability is bundled because the GATR-snap undo affordance requires it. Slice 4 inherits a `needs-visual-review` PR gate for the toast itself.
- **Single source of truth for `pinned`** is `TickerOrderIntent.pinned`, period. The chart widget does not carry its own `pinned` field. The PinToggle decorator derives its visual state per frame from `ComputeContext.pinned` (added in Slice 4 at `desktop/win/crates/midas-chart/src/widget/compute.rs:20-55`) populated by `midas-app` from `order_intent_handle.snapshot(&symbol)`. Click path: `DecoratorAction::TogglePin` (added to the existing `DecoratorAction` enum, not a new `HitZoneKind` variant) → `HitZoneKind::Decorator` bubbles via `ChartAction::DecoratorClick` → `chart_widget.rs` match arm → `Message::OrderIntent(TogglePin { symbol })` → reducer → next frame reflects the new state. Same plumbing as `CycleEntryType`, `EditQuantity`, and the other existing decorator actions.
- **`OrderIntentAppMsg` enum is locked in Slice 1a as a stub.** All seven variants (`UpdateFromPanel`, `UpdateFromBracketDrag`, `CancelLiveBracket`, `RemoveLiveBracket`, `MaybeSnapToGatr`, `TogglePin`, `UndoSnap`) are defined up front; the Slice 1a `apply_order_intent_msg` returns `Task::none()` for every arm. Slice 3 fills in the four sync handlers; Slice 4 fills in the three GATR/pin handlers. Downstream slices never re-open the enum, eliminating a coordination hazard between Slice 3 and Slice 4.
- **Two rejected alternatives the reviewer may want to revisit**: (1) a much simpler `Arc<RwLock<HashMap>>` + idle-timer JSON flush could ship ~40% of the value in ~150 LOC — rejected because the user explicitly asked for "industrial strength"; (2) a quarantine sidecar table for defective intents — rejected as YAGNI in favour of inline `tracing::warn!` + drop at load time.
- **Open question from plan-eval critique (Low severity)**: D4's firing rule ("on startup and on first ticker re-activation this session") is not user-configurable. A future enhancement could add a `GatrSnapPolicy { Automatic, PromptOnDrift, Disabled }` config enum. Deliberately out of scope for this plan but trivial to bolt on without a schema bump.
- **Compound-keyed memory + Stop Loss default**: per-ticker memory is keyed by `(OrderSide, EntryType)` — eight buckets total per ticker. Stop Loss is **on by default for every (side, type) combination** unless the user explicitly toggled it off for that specific compound. Toggling SL off in `(Buy, Stop)` does not turn it off in `(Buy, Limit)` — each compound is independent. This matches "BUY|SELL + Market|Limit|Stop|StopLimit" as the user described it. Two implementer notes: (1) `EntryMemory` derives `Default` so missing buckets at load time deserialize to fresh defaults including `sl_enabled = true`; (2) when the user switches side or type within a ticker, `OrderPanelState::rehydrate_for_compound` is called to load the new bucket — this is a soft re-hydration (does not bump `dirty` because the user did not type a value).
