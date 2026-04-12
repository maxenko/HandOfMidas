# Feature: Ticker State Machine — Per-Symbol Single Source of Truth

## Overview

Replace the current fragmented per-ticker state management (5 competing stores, 28 direct annotation_store mutation sites in app.rs, a 414-line legacy bracket manager) with a centralized `TickerState` struct that is the **sole owner** of all per-symbol state: order brackets, entry type memories, GATR anchors, price levels, and market data snapshots. Every UI surface (charts, order panels, watchlists) binds to a `TickerState` via the existing `LinkMode::Color` symbol-link system and renders from it as a pure view. No UI surface mutates `TickerState` directly — all writes go through `Message::Ticker(SymbolKey, TickerMsg)` → `TickerState::apply()`, which returns a `Vec<TickerEffect>` that the caller interprets. This eliminates the class of competing-writer bugs (bracket flicker, phantom lines, stale panel inputs) by construction.

**Supersedes**: `desktop/win/plan/ticker-order-state/README.md`. The previous plan's infrastructure — redb, mailbox_processor actor, price_defaults, gatr_snap, toast UI — is reused wholesale. The data model evolves from `TickerOrderIntent` to `TickerState`.

**Who benefits**: The developer (one mutation path to debug instead of 30+), and the user (brackets that reliably match the panel, no flicker, no phantom lines, instant binding via symbol-link color groups).

## Research Summary

### Codebase Analysis

**Current fragmentation — five per-ticker state stores with independent write paths:**

| Store | Key | Mutation authority | Direct writes in app.rs |
|---|---|---|---|
| AnnotationStore | SymbolKey | 28 direct call sites | 4 `.add()`, 2 `.remove()`, 22 `.update()` across bracket lifecycle, field mutations, broker events |
| LevelStore | String | Direct | `from_config()` / `to_config()` |
| TickerOrderIntent | SymbolKey | Reducer (5 handlers) + direct | `reducer.rs` + handle.upsert calls from app.rs |
| MarketDataCache | String | Direct | `market_cache.rs` (populated by feed) |
| OrderPanelState | per-panel | 15+ direct field mutations | SetQuantity, SetSide, SetEntryType, etc. |

**Legacy bracket lifecycle manager**: `handle_set_bracket_mode()` at `app.rs:1741` — **414 lines** of bracket create/recall/hide/delete/flip logic. Predates the reducer and competes with `apply_ensure_draft_bracket`. This is the primary source of the bracket flicker and phantom-line bugs. Must be deleted entirely.

**Symbol link infrastructure (already exists, production-ready)**:
- `LinkMode { Unlinked, Color(LinkColor), ListenAll }` in `midas-core/src/link.rs`
- `LinkColor`: 8 colors (Blue, Red, Orange, Green, Purple, Violet, Teal, Brown)
- `find_link_targets()` at `midas-app/src/link.rs:64-82`
- Charts, order panels, and watchlists each carry `symbol_link: LinkMode`
- Propagation already wired: `SetSymbolLink`, `WatchlistSetSymbolLink`, `OrderPanelSetSymbolLink` messages

**Persistence today**: Annotations as JSON per symbol (`data/annotations/<SYMBOL>.json`, 500ms debounce); TickerOrderIntent in redb (`ticker_state.redb`, 75ms debounce); Levels in TOML config; Layout in TOML config.

### Best Practices & Idiomatic Approach

1. **Flat entity store, single writer** (Redux normalized state / LMAX Architecture — Fowler): `HashMap<SymbolKey, TickerState>` on `MidasApp`, all writes through one `apply()` method.
2. **Bloomberg-style link groups** = topic-keyed pub/sub by color. The existing `LinkMode::Color(c)` infrastructure is already this pattern.
3. **Strangler Fig migration** (Fowler): grow the new system around the old one, migrate one category of direct writes at a time, delete legacy code when no references remain.
4. **iced 0.14 idiom**: flat top-level state + `Message::Ticker(SymbolKey, TickerMsg)` dispatch. Standard `update()` loop, no channels or actors for the mutation path.
5. **Effects-return pattern**: `apply()` returns effects instead of taking `&mut AnnotationStore` — cleaner testing, prevents coupling between TickerState and the annotation store API.

## Design Decisions

### D1: TickerState returns effects; caller projects into AnnotationStore

**Context**: TickerState owns bracket geometry. The chart reads from AnnotationStore. The two must stay in sync.

**Options**:
1. Pass `&mut AnnotationStore` into `apply()` — one function does everything. Simpler but couples TickerState to AnnotationStore's API.
2. `apply()` returns `Vec<TickerEffect>` — the caller (`MidasApp`) maps effects to downstream actions. TickerState is a pure state machine; it has no knowledge of AnnotationStore.
3. Move brackets out of AnnotationStore entirely; change the chart to read from TickerState. Breaks the midas-chart sans-IO boundary.

**Recommendation**: **Option 2**. `apply()` returns effects like `ProjectBracket(OrderBracket)`, `RemoveBracket(AnnotationId)`, `Toast(String, Option<ToastAction>)`, `PersistDirty`. The handler in `update()` maps these mechanically. Unit tests assert on effects without mocking AnnotationStore.

**Confidence**: high.

### D2: TickerState fields are private; apply() + getters only

**Context**: The old architecture's problem was "anyone can write." The new one must prevent bypass structurally, not just by policy.

**Recommendation**: All mutable fields on `TickerState` are `pub(crate)` at most (or private with pub getters). The only `pub` mutation method is `apply(msg: TickerMsg) -> Vec<TickerEffect>`. No `pub` setter for `live_bracket`, `entries`, `gatr_anchor`, etc. This is module-boundary enforcement, not type-level (Rust can't prevent `ticker_state::apply.rs` from writing to private fields) — but it's the same level of protection `AnnotationStore` uses for its internals, and it's strong enough to prevent the casual "I'll just poke this field directly" that created the 28 competing writes.

**Confidence**: high.

### D3: TickerState struct shape

```rust
pub struct TickerState {
    // Identity
    symbol: SymbolKey,
    version: u32,
    
    // Order entry memory (from TickerOrderIntent)
    last_side: OrderSide,
    last_entry_type: EntryType,
    entries: HashMap<(OrderSide, EntryType), EntryMemory>,
    gatr_anchor: GatrAnchor,
    pinned: bool,
    
    // Live bracket (owned; projected to AnnotationStore via effects)
    live_bracket: Option<OrderBracket>,
    live_annotation_id: Option<AnnotationId>,
    
    // Levels (from LevelStore)
    levels: Vec<StoredLevel>,
    
    // Market data (from MarketDataCache, not persisted)
    last_price: Option<f64>,
    gatr_abs: Option<f64>,
    
    // Editing focus lock (replaces OrderPanelState.dirty)
    editing_field: Option<EditingField>,
    editing_value: Option<String>,  // in-progress text for the locked field
    
    // Undo
    pre_snap: Option<(Box<PreSnapState>, Instant)>,
    
    // Metadata
    updated_at: DateTime<Utc>,
    generation: u64,
}
```

All fields private. Public API: `apply()`, `symbol()`, `last_side()`, `last_entry_type()`, `active_entry_memory()`, `live_bracket()`, `levels()`, `last_price()`, `gatr_abs()`, `pinned()`, `is_editing()`, `editing_value()` (returns `Option<&str>` — the in-progress text for the panel's `view()` to display), `generation()`.

### D4: TickerMsg enum — the complete mutation vocabulary

```rust
pub enum TickerMsg {
    // Bracket lifecycle
    EnsureDraftBracket { side: OrderSide, entry_type: EntryType },
    CancelBracket,
    SaveBracket,
    DeleteBracket,
    RecallBracket,
    
    // Bracket field mutations
    SetLegPrice { role: LegRole, price: f64 },
    SetTpEnabled(bool),
    SetSlEnabled(bool),
    SetQuantity(f64),
    SetSide(OrderSide),
    SetEntryType(EntryType),
    DragLeg { role: LegRole, new_price: f64 },
    
    // Text editing focus (replaces dirty flag — three-phase lifecycle)
    BeginEdit(EditingField),          // focus-in: sets editing_field, clears editing_value
    UpdateEditValue(String),          // each keystroke: updates editing_value without re-triggering implicit-commit
    CommitEdit { field: EditingField, value: String },  // Enter / blur: applies value, clears lock
    CancelEdit,                       // Escape: reverts to pre-edit state, clears lock
    
    // GATR
    MaybeSnap { current_price: f64, gatr_abs: Option<f64> },
    TogglePin,
    UndoSnap,
    
    // Market data
    UpdateMarketData { last_price: f64, gatr_abs: Option<f64> },
    
    // Levels
    AddLevel(StoredLevel),
    RemoveLevel(usize),
    UpdateLevel { index: usize, level: StoredLevel },
    ToggleLevelLock(usize),
    
    // Broker events
    SubmitOrder,
    OrderPending { order_id: uuid::Uuid },
    OrderFilled { filled_qty: f64, avg_price: f64 },
    OrderPartialFill { filled_qty: f64 },
    OrderRejected { reason: String },
    OrderCancelled,
    
    // Persistence
    Hydrated(Box<TickerState>),
}

pub enum EditingField {
    LimitPrice, StopPrice, TpValue, SlValue, SlLimitValue, Quantity,
}
```

The `BeginEdit` / `CommitEdit` / `CancelEdit` flow replaces the old `OrderPanelState.dirty` flag. When the user starts typing in a text field, the panel emits `BeginEdit(LimitPrice)`. While `editing_field == Some(LimitPrice)`, any `MaybeSnap` or `UpdateMarketData` that would modify the limit price is suppressed — the user's in-progress keystrokes are not clobbered. `CommitEdit` applies the final value and clears the lock. `CancelEdit` reverts to pre-edit state.

**Three-phase editing lifecycle**: `BeginEdit(field)` fires once on focus-in (sets `editing_field`, clears `editing_value`). `UpdateEditValue(text)` fires on each keystroke (writes to `self.editing_value` — no lock change, no implicit-commit trigger). `CommitEdit { field, value }` fires on Enter or blur (applies the value and clears the lock). `CancelEdit` fires on Escape (reverts to pre-edit state). This gives `apply()` three clean entry points instead of overloading `BeginEdit` for both "start editing" and "update text."

**Implicit-commit transition rule**: `BeginEdit(F2)` while `editing_field == Some(F1)` implicitly commits F1 using `self.editing_value` (the in-progress text stored on TickerState), then begins F2. Because `BeginEdit` only fires on focus-in (not on keystrokes), this transition only triggers when the user actually moves to a different field — not on every keystroke. Standard browser/desktop Tab-commit behavior. Test-enforced.

### D5: Effects enum

```rust
pub enum TickerEffect {
    ProjectBracket(OrderBracket),
    RemoveBracket(AnnotationId),
    ProjectLevel { index: usize, level: StoredLevel },
    RemoveLevel { annotation_id: AnnotationId },
    Toast { message: String, action: Option<ToastAction> },
    PersistDirty,
    SubmitToBroker { bracket: OrderBracket },
}
```

The handler in `MidasApp::update()`:
```rust
Message::Ticker(sym, msg) => {
    let state = self.ticker_mut(&sym);
    let effects = state.apply(msg);
    for effect in effects {
        match effect {
            TickerEffect::ProjectBracket(b) => {
                // ONE annotation_store.update call — the only bracket-write in the app
                self.annotation_store.upsert_bracket(&sym, state.live_annotation_id(), &b);
            }
            TickerEffect::RemoveBracket(id) => {
                self.annotation_store.remove(&sym, id);
            }
            TickerEffect::Toast { message, action } => {
                self.show_toast(message, action);
            }
            TickerEffect::PersistDirty => {
                self.persist_ticker(&sym);
            }
            TickerEffect::SubmitToBroker { bracket } => {
                self.broker_bridge.submit(bracket);
            }
            // ...
        }
    }
}
```

This is the ONE place in the entire app that writes OrderBracket data to AnnotationStore. If a developer wants to add a new bracket mutation, they must add a TickerMsg variant and handle it in `apply()` — the effects system enforces the path.

**No-reentry invariant**: effect handling is sequential and must not re-enter `apply()` in the same dispatch cycle. If a `Toast` effect's handler or a `SubmitToBroker` callback needs to mutate TickerState, it emits a new `Message::Ticker` that is dispatched in a *subsequent* `update()` call, never inline. This prevents the feedback-loop class of bugs that has bitten every Redux codebase that allowed middleware to dispatch synchronously. Enforce via an `assert!(!self.ticker_dispatch_active)` runtime guard (not `debug_assert!` — that evaporates in release builds, letting accidental reentry silently succeed in production). The cost is one bool check per dispatch cycle — zero measurable impact at 60 Hz.

### D6: Factory and binding model

```rust
// On MidasApp:
pub tickers: HashMap<SymbolKey, TickerState>,

fn ticker_mut(&mut self, symbol: &SymbolKey) -> &mut TickerState {
    self.tickers.entry(symbol.clone())
        .or_insert_with(|| TickerState::new(symbol.clone()))
}
```

Charts and panels carry `bound_symbol: Option<SymbolKey>` determined by their LinkMode color group. When a symbol-link broadcast resolves, it updates `bound_symbol` and `view()` reads from `tickers[bound_symbol]`. `bound_symbol = None` → empty placeholder. `bound_symbol` is persisted in the chart/panel config so it survives restart.

### D7: Persistence — redb blob per symbol, one-way migration

Same `ticker_state.redb` file, same 75ms debounce flush actor, same `Durability::Eventual` / `Immediate` split. The table value grows from `TickerOrderIntent` (v1) to `TickerState` (v2, excluding ephemeral fields like `last_price`). Bump `CURRENT_VERSION` to 2 with a `migrate_v1_v2()`.

**One-way migration**: v2 blobs are not downgraded to v1. Users who upgrade cannot roll back to a pre-migration build without losing their TickerState data. This is acceptable because: (a) the per-symbol JSON annotation files are not deleted during migration — they remain as a read-only archive; (b) levels persist in TOML config independently; (c) the v1→v2 migration is additive (v2 = v1 + bracket + levels + market data) so no data is lost, only reformatted.

## Implementation Plan

### Slice 0: TickerState struct + factory + Message::Ticker routing + persistence

**Goal**: Land the data model, factory, message routing, and persistence upgrade. No UI behavior changes — the old system runs alongside. Foundation for every subsequent slice.

**Depends on**: None.

**Files to create**:
- `ticker_state/mod.rs` — `TickerState` struct (D3, all fields private), `TickerMsg` enum (D4), `TickerEffect` enum (D5), `EditingField` enum, `CURRENT_VERSION = 2`, `migrate_v1_v2()`, getters, re-exports.
- `ticker_state/apply.rs` — `impl TickerState { pub fn apply(&mut self, msg: TickerMsg) -> Vec<TickerEffect> }`. Every variant starts as a stub returning `vec![]`. Slices 1-2 fill them in.
- `ticker_state/factory.rs` — `TickerState::new(symbol)`, `TickerState::new_with_defaults(symbol, current_price, gatr_abs)`, `TickerState::from_legacy(intent, levels, bracket)`. `from_legacy` accepts a `Vec<StoredLevel>` read from the TOML config's `LevelStore` entries for this symbol and injects them into `TickerState.levels` — this is the concrete migration path for levels (TOML → redb). If both TOML and redb have levels for the same symbol (e.g., partial migration interrupted), `from_legacy` uses last-write-wins: redb data takes priority because it's the newer source. Reuses `price_defaults::default_initial_prices` for bracket defaults.
- `ticker_state/persist.rs` — redb table upgrade: v1 → v2 migration. Reuse existing actor + flush loop.
- `ticker_state/tests.rs` — serde round-trip, v1→v2 migration, factory defaults.

**Files to modify**:
- `app.rs` — add `tickers: HashMap<SymbolKey, TickerState>`, `Message::Ticker(SymbolKey, TickerMsg)` variant, routing arm with effect handler, `ticker_mut()` helper.
- `main.rs` — `mod ticker_state;`.

**Rollback**: `git revert`. Slice 0 is inert — it adds infrastructure but changes no behavior. No data migration runs in this slice; the redb v1→v2 upgrade is deferred to Slice 4.

**Done when**: `MidasApp.tickers` exists, `Message::Ticker` routes through `apply()`, persistence reads/writes v2 blobs, existing app behavior unchanged.

### Slice 1: Migrate ALL bracket operations + editing focus lock (delete handle_set_bracket_mode)

**Goal**: Migrate bracket lifecycle (create/delete/cancel/recall/save) AND bracket field mutations (price, quantity, side, TP/SL toggles, entry type, drag) into `TickerState::apply()` as a single atomic slice. Delete the 414-line `handle_set_bracket_mode()`. Replace the `dirty` flag with the `BeginEdit`/`CommitEdit` focus-lock pattern. The order panel becomes a view that reads from TickerState. **Migrating lifecycle and field mutations together is non-negotiable** — splitting them would temporarily reintroduce the competing-writer bug (TickerState owns lifecycle while direct writes own fields, and `ProjectBracket` would overwrite field changes).

**Depends on**: Slice 0.

**Rollback**: `git revert` the slice commit. The old `handle_set_bracket_mode()` and direct annotation_store writes are restored. No redb migration in this slice — pure code routing change — so no data cleanup needed.

**Files to modify**:
- `ticker_state/apply.rs` — fill in ALL bracket-related handlers: `EnsureDraftBracket`, `CancelBracket`, `SaveBracket`, `DeleteBracket`, `RecallBracket`, `SetLegPrice`, `SetTpEnabled`, `SetSlEnabled`, `SetQuantity`, `SetSide`, `SetEntryType`, `DragLeg`, `BeginEdit`, `CommitEdit`, `CancelEdit`. Each mutates `self.live_bracket` and/or `self.entries[(side, type)]` and returns the appropriate `TickerEffect`.
- `app.rs` — every call site of `handle_set_bracket_mode()` becomes `Message::Ticker(symbol, TickerMsg::EnsureDraftBracket { ... })` or equivalent. ALL 21 bracket-related direct `annotation_store.add/update/remove` sites (lifecycle lines 1366, 1370, 1399, 1471, 2071, 2117, 2128 + field mutation lines 1498, 1652, 1730, 1783, 4368, 4380, 4389, 4428, 4471, 4916, 5319, 5519, 5584, 5632, 5746) route through TickerMsg. **DELETE** `handle_set_bracket_mode()` entirely.
- `order_panel/mod.rs` — shrink: bracket-related fields (`limit_price`, `stop_price`, `tp_value`, `sl_value`, etc.) become read from `TickerState` in `view()`, not owned. Remove `dirty: bool`. Panel input handlers emit `Message::Ticker(BeginEdit/CommitEdit)` instead of directly mutating fields.
- `chart_widget.rs` — drag paths emit `Message::Ticker(symbol, TickerMsg::DragLeg { role, new_price })`.
- `ticker_state/tests.rs` — lifecycle tests (create, cancel, recall, save, delete) + per-field-variant tests + compound-key isolation + editing-focus-lock suppresses MaybeSnap mid-keystroke + robustness: `apply(SetLegPrice)` when `live_bracket.is_none()` must not panic (returns empty effects).

**Done when**: `handle_set_bracket_mode()` is deleted and does not compile. Zero `annotation_store.add/update/remove` calls for ANY bracket operation (lifecycle or field mutation) remain in app.rs. Panel reads from TickerState. `dirty` flag removed. No bracket flicker on ticker activation.

### Slice 2: Migrate broker + GATR + levels

**Goal**: Route broker events, GATR snap/pin/undo, and level mutations through TickerMsg.

**Depends on**: Slice 1.

**Rollback**: `git revert`. No persistence format change — pure code routing. Existing tests continue to pass against the pre-slice state.

**Files to modify**:
- `ticker_state/apply.rs` — fill in `SubmitOrder`, `OrderPending`, `OrderFilled`, `OrderPartialFill`, `OrderRejected`, `OrderCancelled`, `MaybeSnap`, `TogglePin`, `UndoSnap`, `AddLevel`, `RemoveLevel`, `UpdateLevel`, `ToggleLevelLock`, `UpdateMarketData`.
- `app.rs` — broker writes (lines 5746, 5793, 5883, 5936, 6112) and level writes become `Message::Ticker` dispatches.
- `ticker_state/tests.rs` — submit→pending→filled lifecycle, rejection reverts to Draft, GATR snap with stale anchor, level CRUD.

**Done when**: Zero direct `annotation_store` writes for broker or level operations remain. Zero direct LevelStore mutations from app.rs.

### Slice 3: Bind charts + panels via symbol link groups

**Goal**: Charts and panels read from `tickers[bound_symbol]`. Symbol-link color groups drive binding.

**Depends on**: Slices 1-2.

**Rollback**: `git revert`. Charts and panels revert to `symbol: String`. The `bound_symbol` config field is ignored by pre-slice builds (forward-compat via `#[serde(default)]`).

**Files to modify**:
- `app.rs` — `ChartPanel` gains `bound_symbol: Option<SymbolKey>` replacing `symbol: String`. `OrderPanel` gains `bound_symbol: Option<SymbolKey>` replacing `state.symbol`. Symbol-link broadcasts resolve to `chart.bound_symbol = Some(symbol)` + lazy factory creation + `EnsureDraftBracket`.
- `app/views.rs` — panel and chart views read from `tickers[bound_symbol]`.
- `link.rs` — propagation resolves to binding, not direct state mutation.
- `chart_widget.rs` — chart reads annotations from `tickers[bound_symbol]` projection.
- `midas-core/src/config/mod.rs` — `ChartConfig` and `OrderPanelConfig` persist `bound_symbol`.

**Done when**: `ChartPanel.symbol: String` gone. `OrderPanelState.symbol` gone. Unbound panels show empty placeholder.

### Slice 4: Startup/shutdown + delete legacy code

**Goal**: Startup loads TickerStates for watchlist symbols, restores bindings. Shutdown flushes all dirty states. Delete dead code.

**Depends on**: Slice 3.

**Rollback**: This is the one-way-door slice. The v1→v2 redb migration runs here. After this slice, users cannot downgrade to a pre-migration build without losing TickerState data (though the original JSON annotation files and TOML level config are preserved as read-only archives). `git revert` restores the code but redb v2 blobs require the dump tool to inspect / manually reconstruct v1 data. Ship in a release with clear upgrade notes.

**Files to modify**:
- `app.rs` — startup: load TickerStates from redb, migration from v1 + JSON annotations + **TOML levels** (see below). Shutdown: flush all dirty.
- **DELETE**:
  - `ticker_order_intent/` module (subsumed by `ticker_state`)
  - `handle_set_bracket_mode()` (already gone from Slice 1)
  - `reconcile_ticker_activation()`, `sync_panel_to_intent()`, `sync_drag_to_intent()`, `hydrate_order_panel_for_chart()`, `snap_panel_inputs_for_symbol()` and similar legacy helpers
  - `LevelStore` (absorbed into TickerState.levels)
  - `MarketDataCache` (absorbed into TickerState.last_price / gatr_abs)
  - `OrderPanelState` bracket-related fields
  - `annotation_persistence.rs` WRITE path (keep READ for v1 migration)
- **Audit gate for non-bracket annotations**: check if `AnnotationKind::Level` / `TextNote` / `Marker` are actively used. If yes, AnnotationStore survives for those types. If no, delete entirely.

**Done when**: Zero `annotation_store.add/update/remove` calls for OrderBracket remain anywhere. Zero competing writers. The architecture is structurally sound: one type (`TickerState`), one mutation path (`apply()`), one persistence backend (redb), effects-driven projection.

### Dependency Summary

- **Critical path**: 0 → 1 → 2 → 3 → 4 (strictly sequential — app.rs is the bottleneck file).
- **Riskiest slice**: Slice 1 (deleting the 414-line `handle_set_bracket_mode` + migrating all 21 bracket direct-write sites at once). Intentionally early. Merged from the draft's Slices 1+2 to eliminate a competing-writer window that would have temporarily reintroduced the exact bug class the plan exists to fix.
- **One-way door**: Slice 4 (startup/shutdown + legacy deletion). The v1→v2 redb migration is irreversible. All prior slices are pure code routing changes and can be reverted cleanly.
- **Each slice leaves the app working**: Slice 0 adds inert infrastructure. Slice 1 migrates all bracket operations atomically. Slice 2 migrates broker/GATR/levels. Slice 3 changes binding. Slice 4 cleans up and persists.

## Risks & Unknowns

1. **app.rs is 6328 lines.** Every slice touches it. Risk: merge conflicts. Mitigation: strict sequencing, commit after each slice. Follow-up: split app.rs into sub-modules (out of scope for this plan).
2. **One-way migration (Slice 4 only).** v1→v2 redb upgrade is irreversible. Users cannot downgrade to a pre-Slice-4 build without losing TickerState data. Mitigation: JSON annotation files and TOML level config are preserved as read-only archives; the dump tool can inspect v2 redb blobs for manual recovery. Slices 0-3 are pure code routing changes with no persistence format change — they can be reverted cleanly via `git revert`. Documented as a known constraint — ship Slice 4 in a release with clear upgrade notes.
3. **Module-boundary enforcement, not type-level.** `TickerState` fields are private and `apply()` is the only pub mutation path, but nothing in Rust's type system prevents a future developer from adding a pub setter inside the `ticker_state` module itself. Mitigation: code review discipline + a module-level `// INVARIANT: all state mutations go through apply()` doc comment. The effects-return pattern helps: `apply()` doesn't even take `&mut AnnotationStore`, so a developer can't accidentally couple a mutation to the store without going through the effect handler.
4. **Panel reactivity on text input.** The `BeginEdit`/`CommitEdit` flow is slightly more complex than the old `dirty` flag. Risk: subtle UX bugs where edits are committed too early or too late. Mitigation: comprehensive unit tests on the focus-lock suppression path.
5. **Non-bracket annotation types.** If `AnnotationKind::Level` or `TextNote` or `Marker` are used by the chart, AnnotationStore survives for them. Slice 4 includes a concrete audit gate.
6. **60 Hz drag performance.** `apply(DragLeg)` is synchronous inside `update()` → effect projection is a HashMap lookup + struct copy → negligible. The 75ms debounce handles persistence. No concern at desktop scale.

## Testing Strategy

- **Unit tests on `TickerState::apply()`**: table-driven per-variant tests asserting state transitions + returned effects. No mock AnnotationStore needed — assert on effects directly.
- **Robustness tests**: `apply(SetLegPrice)` with `live_bracket == None` → no panic, empty effects. `apply(CommitEdit)` without matching `BeginEdit` → no panic.
- **Factory tests**: `new_with_defaults` produces sensible bracket geometry for each (side, entry_type).
- **Migration tests**: v1→v2 round-trip, JSON annotation import, TOML levels import, cold start, warm start, **corrupt/partial v1 blob** (missing or extra fields deserializes to sensible defaults via `#[serde(default)]` rather than panicking).
- **Editing-focus-lock tests**: `BeginEdit(LimitPrice)` → `MaybeSnap` → limit price NOT overwritten → `CommitEdit` → limit price is the user's value.
- **Integration tests**: full `Message::Ticker` dispatch through `update()`, verify the one AnnotationStore write site reflects the effects.
- **Manual UI testing**: bracket create/drag/cancel/submit, side/type switching, ticker switching via link groups, startup/shutdown persistence.

## Non-Goals / Out of Scope

- **Splitting app.rs** into sub-modules. Desirable but separate work.
- **Multi-ticker order management** (portfolio view, multi-leg cross-ticker orders).
- **Real-time market data feed refactor.** TickerState accepts `UpdateMarketData`; who sends it is unchanged.
- **Broker engine refactor.** The broker bridge emits TickerMsg variants; its internals are untouched.
- **midas-chart refactor.** The chart crate stays sans-IO; it reads AnnotationStore as usual.
- **Event sourcing.** TickerState is snapshot-based (last-write-wins).
- **Undo/redo for manual bracket edits.** Only GATR snap has an undo affordance. General undo is a separate feature track.
- **Formal FSM states.** "State machine" here means "per-ticker state object with a single mutation path." If formal lifecycle states (Draft → Pending → Active → Filled) are needed, they can be added inside `apply()` without changing the external interface.

## Review Notes

- **Effects-return pattern** was the main change from the draft. Critique agent C recommended it for testability and separation. `apply()` is now a pure state machine: it takes a message, mutates self, returns effects. The caller interprets effects. No coupling to AnnotationStore's API inside `apply()`.
- **Editing focus lock** was flagged by critique agent B as a must-fix. Without it, the "panel becomes a pure view" transition would let GATR snaps overwrite partially-typed text fields (e.g., user is typing "100" and the snap fires after "10", resetting the field to "14.45"). The `BeginEdit`/`CommitEdit` flow suppresses conflicting mutations while the user is actively typing, then commits the user's value.
- **Private fields** were flagged by both agent B and C. `pub live_bracket` in the draft would have let any code in midas-app poke bracket fields directly, recreating the exact problem this plan solves. Now all fields are private; `apply()` + getters is the only public API.
- **One-way migration** is acknowledged as a known constraint, not hidden. v1 JSON annotation files are preserved as read-only archives.
- **The "state machine" name is aspirational.** `TickerState` is a struct with a reducer method, not a formal FSM. The user's mental model is "each ticker has its own object that manages everything" — the plan delivers that without over-engineering a state machine framework.
