# Order Blotter Panel

Make order brackets actually reach the broker engine, persist the resulting
order lifecycle, and show it in a scrollable grid panel that matches the
provided target design.

## Motivation

Users need a running log of every leg submitted this session to confirm fills,
spot stuck working orders, and audit after close — today there's no UI
surfacing any of this.

Today, brackets drawn on the chart end up in `TickerState::live_bracket` and
produce a `TickerEffect::SubmitToBroker` effect that `broker_bridge.rs` turns
into a `BrokerCommand::CreateBracket`. The engine (TestBroker) simulates the
lifecycle and emits `BracketCreated`, `OrderSubmitted`, `OrderStatusChanged`,
`OrderFilled`, `BracketStatusChanged`, etc. over a broadcast channel.

Those events reach `Message::BrokerEventReceived` in `app/handlers.rs` and then:
- `BracketCreated` → reconciles an annotation-to-order mapping in
  `order_annotation_links: HashMap<Uuid, OrderAnnotationLink>` (ephemeral).
- Per-leg status / fill / cancel events → mostly logged and dropped.

Nothing on the desktop side persists those events, and no UI surfaces them.

This plan closes the gap: every per-leg order that the engine creates becomes
a row in an `OrderBlotter` (desktop-side, redb-persisted), and a new pane-grid
panel renders the blotter as a sortable table matching the target screenshot.

## Target UI

Reference: `C:\Users\max\Pictures\Screenshots\Screenshot 2026-04-17 195447.png`

Columns, left to right:

| # | Column        | Source                              | Notes                                           |
|---|---------------|-------------------------------------|-------------------------------------------------|
| 1 | Symbol        | symbol badge (icon + ticker pill)   | blue for Buy, red for Sell (matches Side)       |
| 2 | Side          | `Buy` / `Sell` text                 | coloured (#4C8AF6 / #E04F4F approx)             |
| 3 | Type          | `Stop Loss` / `Stop Limit` / etc.   | plain                                           |
| 4 | Qty           | filled quantity (decimal)           | right-aligned                                   |
| 5 | Avg Fill Price| f64, blank until any fill           | right-aligned, 2–4 decimals                     |
| 6 | Limit Price   | leg limit price or blank            | right-aligned                                   |
| 7 | Stop Price    | leg stop price or blank             | right-aligned                                   |
| 8 | Take Profit   | sibling TP price or blank           | right-aligned                                   |
| 9 | Stop Loss     | sibling SL price or blank           | right-aligned                                   |
|10 | Status        | Filled/Cancelled/Working/Rejected   | colour-coded green / amber / grey               |
|11 | Last Update   | local wall clock HH:MM:SS           | right-aligned                                   |
|12 | Instruction   | `Good Till Cancel` / `Day` / etc.   | from TimeInForce                                |
|13 | Duration      | same as Instruction in the mock     | deferred: collapse into one column for v1       |
|14 | Order ID      | broker-assigned id                  | sortable (default descending)                   |
|15 | ⋮⋮⋮ menu    | per-row actions (cancel / modify)   | v2 — stub for now                               |

One row per **leg**, not per bracket. A three-leg bracket produces three rows;
they share the same `parent_id` but carry per-leg `order_id`.

## Non-goals for v1

- No interactive column resizing beyond what `midas-grid` already provides —
  if the Watchlist panel supports it, so does this; nothing new.
- No per-row right-click menu yet. The `⋮⋮⋮` button in the mock renders as a
  non-interactive placeholder.
- No editing / cancelling orders from the panel. The rows are read-only.
  Cancel flows already exist on the chart (`ChartBracketCancel`).
- No multi-account separation. One store, all accounts (today there's one).
- No export-to-CSV.
- No IB gateway wiring. The path exercises `TestBroker` only; IB code paths
  are unchanged but also untouched.
- No changes to the SQLite store inside `midas-broker`. The desktop-side
  redb store is its own artefact; we don't dual-write, don't query the
  broker's SQLite from the desktop.

## Alternatives considered

- **redb (chosen)**: mirrors the in-repo `ticker_state/persist.rs` pattern, one
  mental model for desktop-side persistence.
- **midas-store / DuckDB (rejected)**: DuckDB is for candles/analytics; order
  history is a transactional write-heavy small-row workload that fits redb
  better, and keeps midas-store focused.
- **Querying the broker engine's SQLite directly (rejected)**: cross-process
  coupling, the engine's schema serves its own needs, and desktop-side
  hydration from redb is faster than a cross-DB join on startup.

## Architecture overview

```
┌────────────────────────────────────────────────────────────────────┐
│  midas-broker (existing, unchanged)                                │
│  TestBroker → broadcast::Sender<BrokerEvent>                       │
└────────────────────────────┬───────────────────────────────────────┘
                             │ subscribed via BrokerEventSource
                             ▼
┌────────────────────────────────────────────────────────────────────┐
│  broker_bridge.rs (existing; returns iced Subscription)            │
└────────────────────────────┬───────────────────────────────────────┘
                             │ Message::BrokerEventReceived(event)
                             ▼
┌────────────────────────────────────────────────────────────────────┐
│  app/handlers.rs :: handle_broker_msg                              │
│   already receives events; now ALSO drives:                        │
│                                                                     │
│   self.order_blotter.apply(&event);     // new                     │
│   self.order_history_persist.mark();    // new (debounced write)   │
└────────────────────────────┬───────────────────────────────────────┘
                             │
                             ▼
┌────────────────────────────────────────────────────────────────────┐
│  OrderBlotter (new)                                                │
│   BTreeMap<OrderKey, OrderRow>    (key = (broker_order_id))        │
│   + Vec of "display rows" rebuilt on mutation for the grid         │
│   + generation counter for iced state diffing                      │
└────────────────────────────┬───────────────────────────────────────┘
                             │ read-only borrow
                             ▼
┌────────────────────────────────────────────────────────────────────┐
│  OrderBlotterPanel (new pane_grid content)                         │
│   midas-grid rendering with 14 columns                             │
│   sortable, scrollable, row colouring                              │
└────────────────────────────────────────────────────────────────────┘
```

No new tokio tasks. One new native thread (the persist flusher, copied
from the TickerState pattern). Everything else runs on the iced update
path.

## Component breakdown

### `midas-broker` changes

**None.** `TestBroker` already covers the full lifecycle. The BracketParams
shape and event stream give us everything we need.

One small addition only: confirm that `BrokerEvent` variants all carry the
fields the UI needs (e.g. `OrderSubmitted { order_id, ib_order_id, ib_perm_id }` —
yes; `OrderStatusChanged { filled_qty, remaining_qty, avg_fill_price }` — yes;
`OrderFilled { ib_exec_id, shares, price, commission }` — yes). If a field
we want for the UI isn't present, plan revisits this.

### `midas-core` additions

- New id newtype: `OrderBlotterId(u32)` in `id/mod.rs`, matching the existing
  `define_id!` macro. Serde already derived by the macro.
- New config struct `OrderBlotterConfig` in `config/mod.rs`:
  - `name: String` (e.g. `"Orders"`)
  - Any display prefs that survive restart (column widths, default sort).
  - No order rows — those live in redb.
- New `PanelSlot::OrderBlotter { order_blotter_index: usize }`.
- New `LayoutNode::OrderBlotter { order_blotter_index: usize }`.
- `AppConfig::order_blotters: Vec<OrderBlotterConfig>` field (default empty).

### `midas-app` — new modules

#### `src/order_blotter/mod.rs` (new)

```rust
pub struct OrderBlotter {
    /// Keyed by the broker-assigned uuid (parent / TP / SL each get one).
    rows: BTreeMap<Uuid, OrderRow>,
    /// Monotonic counter; bumped on any mutation so views can short-circuit.
    generation: u64,
    /// Secondary index: bracket parent → all legs.
    by_parent: HashMap<Uuid, Vec<Uuid>>,
}

pub struct OrderRow {
    pub order_id: Uuid,
    pub parent_id: Uuid,            // same as order_id for the entry leg
    pub ib_order_id: Option<i32>,
    pub ib_perm_id: Option<i64>,
    pub symbol: String,
    pub side: OrderSide,
    pub kind: OrderKind,            // Market | Limit | Stop | StopLimit
    pub leg_role: LegRole,          // Entry | TakeProfit | StopLoss
    pub quantity: f64,
    pub filled_qty: f64,
    pub remaining_qty: f64,
    pub avg_fill_price: Option<f64>,
    pub limit_price: Option<f64>,
    pub stop_price: Option<f64>,
    pub tp_price: Option<f64>,      // sibling reference
    pub sl_price: Option<f64>,      // sibling reference
    pub status: OrderStatus,        // Working | Filled | Cancelled | Rejected | PartiallyFilled
    pub time_in_force: TimeInForce,
    pub created_at: DateTime<Utc>,
    pub last_update_at: DateTime<Utc>,
}

impl OrderBlotter {
    pub fn apply(&mut self, event: &BrokerEvent) -> bool;   // returns dirty
    pub fn row(&self, id: Uuid) -> Option<&OrderRow>;
    pub fn rows(&self) -> impl Iterator<Item = &OrderRow>;
    pub fn generation(&self) -> u64;
}
```

Event mapping (one-way lossy: BrokerEvent → OrderRow mutations):

| Event                  | Action                                                                                 |
|------------------------|----------------------------------------------------------------------------------------|
| `BracketCreated`       | Create 1–3 rows (entry always; TP/SL if priced). Set status=Working. Idempotent on re-receive for an existing `parent_id`: no-op, immutable fields (symbol, side, quantity, `created_at`) never overwritten. |
| `OrderSubmitted`       | Store `ib_order_id`, `ib_perm_id` on the matching row. Stamp `last_update_at`.         |
| `OrderStatusChanged`   | **Authoritative** for cumulative fields: write status / filled_qty / remaining_qty / avg_fill_price. Stamp `last_update_at`. |
| `OrderFilled`          | Stamp `last_update_at` only. **No** mutation of Qty / AvgFill fields — those come from `OrderStatusChanged`. (Optional future: per-execution fill log; not v1.) |
| `OrderCancelled`       | Status = Cancelled. Stamp `last_update_at`.                                            |
| `OrderRejected`        | Status = Rejected. Stamp `last_update_at`.                                             |
| `BracketStatusChanged` | Summary only — no row mutation beyond stamping `last_update_at` for siblings.          |

All status-mutating events (`OrderStatusChanged`, `OrderFilled`,
`OrderCancelled`, `OrderRejected`) rely on uuid stability across restart and
are applied unconditionally.

All types are `Serialize + Deserialize` so redb rows are cheap to write.

#### `src/order_blotter/persist.rs` (new)

Mirror of `ticker_state/persist.rs`. One redb table `order_history_v1`, keyed
by `order_id` (uuid bytes), value is JSON-serialized `OrderRow`. Same 75ms
debounce, native flush thread, shutdown-blocking semantics.

On `MidasApp::new`, hydrate the in-memory `OrderBlotter` from redb before any
event subscriptions connect.

#### `src/order_blotter/panel.rs` (new)

```rust
pub struct OrderBlotterPanel {
    pub id: OrderBlotterId,
    pub name: String,
    pub grid_state: midas_grid::GridState,
    pub sort: OrderSort,
    last_seen_generation: u64,
    display_rows: Vec<DisplayRow>, // cached; rebuilt on generation change
}
```

`DisplayRow` is a flat, render-ready struct for the grid — avoids borrow-life
issues between the panel and the blotter.

#### `src/order_blotter/columns.rs` (new)

14 column implementations of `midas_grid::GridColumn`, one per visible
column above. Reuses the Watchlist column pattern as the template.

#### `src/widgets/symbol_badge.rs` (new, simple)

Not a new crate — just a helper `fn symbol_badge(sym: &str, side: OrderSide) -> Element<'_, Message>` used by the Symbol column and any future consumer. Keeps colour + icon logic in one place.

### `midas-app` — modifications to existing code

- `src/layout/mod.rs`: extend `PanelContent` with `OrderBlotter(OrderBlotterId)`.
- `src/app.rs`:
  - `MidasApp::order_blotters: HashMap<OrderBlotterId, OrderBlotterPanel>` field.
  - `MidasApp::order_blotter: OrderBlotter` field (one live blotter; shared
    across panels but there's only one instance for v1).
  - `MidasApp::order_history_persist: OrderHistoryPersistHandle` field.
  - `Message::AddOrderBlotter`, `Message::CloseOrderBlotter(OrderBlotterId)`.
  - `Message::OrderBlotterGrid(OrderBlotterId, midas_grid::GridMessage)`.
  - In the `handle_broker_msg` handler, route every `BrokerEvent` into
    `self.order_blotter.apply(&event)`; if dirty, bump `order_history_persist`.
- `src/app/handlers.rs`: new `handle_order_blotter_msg` mirroring the watchlist
  handler.
- `src/app/views.rs`:
  - New `view_order_blotter_title_bar` + `view_order_blotter_body` renderers.
  - Toolbar: new `"Orders"` button pressing `Message::AddOrderBlotter`
    (user-facing label stays "Orders").
  - Panel dispatch match arm for `PanelContent::OrderBlotter(_)`.
- `src/app/persistence.rs`: `build_config` walker now emits
  `LayoutNode::OrderBlotter` + `OrderBlotterConfig`, and `restore_from_layout_tree`
  recognises the new node type.

### Actual order placement — audit and fix

The existing path `ChartBracketSubmit` → `TickerMsg::SubmitOrder` →
`TickerEffect::SubmitToBroker` → `bridge.create_bracket()` →
`BrokerCommand::CreateBracket` is already wired end-to-end per the
exploration pass. What is missing is the user-visible submission path
that fires `TickerMsg::SubmitOrder`.

The plan's **first verification slice** confirms the path works: inject
`SubmitOrder` via the devloop harness, observe `BracketCreated` arrive over
the broker event stream, confirm the new `OrderBlotter` receives rows. If
that is already green, no chart-side wiring is needed — the submission UX
is out of scope and already works through the existing bracket context menu
or chart click path. This plan does not add new submit affordances.

## Build order

Slices are each independently useful and testable. Every slice ends green on:

- `cargo clippy --workspace --features "midas-app/dev_harness" -- -D warnings`
- `cargo clippy --workspace -- -D warnings` (default-feature build — ensures
  feature-off still compiles cleanly)
- `cargo test -p midas-app --features dev_harness`

### Slice 0 — Verify the existing submit path (spike, ~30 min)

1. With a running app under `dev_harness`, inject:
   - `SetBracketMode { side: "Buy" }`
   - `EnsureDraftBracket { side, entry_type: "Limit" }`
   - `SetQuantity { quantity: 100 }`
   - `SetLegPrice { role: "Entry", price: 184.5 }`
   - `SubmitOrder`
2. Expect: the event log (via `wait_for_event`) shows `SubmitOrder`; the broker
   engine's event stream (observed via tracing logs for now) shows
   `BracketCreated`.
3. If anything is broken here, fix before moving to Slice 1.
4. **Record fixture `empty-aapl-d1`**: launch the app under `dev_harness`,
   select AAPL on a daily chart with no bracket, send
   `snapshot_fixture { name: "empty-aapl-d1" }`. Commit the resulting JSON
   under `.devloop/fixtures/empty-aapl-d1.json` if the team chooses to track
   fixtures in git, otherwise document it as a per-developer artefact.

> If Slice 0 exceeds 2 hours, pause and re-plan; a broken submit path
> indicates a deeper problem than this plan accounts for.

### Slice 1 — `OrderBlotter` (~2h)

- New `src/order_blotter/` module.
- `OrderBlotter::apply(&BrokerEvent)` with unit tests covering every event.
  Must include: "`OrderFilled` on its own does not change Qty/AvgFill; only
  `OrderStatusChanged` does."
- `MidasApp::order_blotter` field.
- Hook into `handle_broker_msg`: after the existing matching arms, always
  call `self.order_blotter.apply(&event)`.
- No UI yet. **Use it**: re-run the Slice-0 injection sequence;
  `dump_state order_blotter.rows` now returns populated rows. (The original
  Slice-0 run happened before this wiring existed — re-inject to verify.)

### Slice 2 — redb persistence (~1.5h)

- `src/order_blotter/persist.rs` copied from `ticker_state/persist.rs`.
- `OrderHistoryPersistHandle::open(path)` in `MidasApp::new`; path is
  `AppData\Local\HandOfMidas\order_history.redb`.
- Hydrate on startup; persist on dirty with 75ms debounce.
- Shutdown flushes blocking.
- Unit test: **hydrate → receive same `BracketCreated` → no duplicate, no
  field clobber, no timestamp change.** Covers the re-emit-on-reconnect path.
- **Use it**: restart the app, confirm past rows survive.

### Slice 3 — Panel skeleton (~2h)

- `OrderBlotterId` newtype.
- `PanelContent::OrderBlotter`, `LayoutNode::OrderBlotter`, `PanelSlot::OrderBlotter`.
- `OrderBlotterConfig` in `AppConfig`.
- `OrderBlotterPanel` struct + empty `view_order_blotter_body` that renders a
  title + "no orders yet" placeholder.
- `Message::AddOrderBlotter`, toolbar button ("Orders"), handlers for add/close.
- Persistence: layout round-trips.
- **Use it**: Click "Orders" in the toolbar, get a new pane with placeholder.

### Slice 4 — Grid rendering (~3h)

- Column definitions in `src/order_blotter/columns.rs` matching the target UI.
- `DisplayRow` projection; rebuild on generation change.
- `symbol_badge` helper.
- Sort / scroll / column widths via `midas_grid`.
- Colour coding for Side and Status cells.
- **Use it**: Place a bracket through the chart UI (or via the devloop
  harness); watch rows appear in the grid; sort by Order ID.

### Slice 5 — Cosmetics to match the target (~1h)

- Row padding, horizontal rules, hover highlight.
- Header styling.
- Empty-state message.
- Default sort: Order ID descending.
- **Use it**: screenshot-diff against a reference capture of the target mock.
- **Capture canonical reference**: after cosmetic polish, run the completed
  Slice-7 journey by hand, take
  `screenshot out=.devloop/refs/orders-panel-filled.png`, commit it. Slice 7's
  SSIM compares against this file, not the external mock. The external mock
  (`Screenshot 2026-04-17 195447.png`) is the visual target for humans
  comparing by eye; the `.devloop/refs/` image is what SSIM sees.
- **Stem alignment constraint**: Slice 7's shot uses the stem
  `orders-panel-filled` (written to `.devloop/shots/orders-panel-filled.png`).
  The reference written here uses the same stem at `.devloop/refs/`. The
  devloop harness auto-compares by matching stems across the two directories;
  renaming either file silently breaks the pairing with no loud failure.
  Keep the stem `orders-panel-filled` on both sides.

### Slice 6 — Devloop additions (~2h, see next section)

Add whatever the verification loop actually needed while building slices
1–5. These are scoped in the devloop additions section below.

### Slice 7 — Validation journey (~1h)

Concrete scripted journey a driver can run end-to-end. Entry is **Market** so
TestBroker fires the fill instantly (Limit fills require market ticks and would
hang the journey — see "Practical gotchas"):

```
load_fixture "empty-aapl-d1"
inject SetBracketMode { side: Buy }
inject EnsureDraftBracket { side: Buy, entry_type: Market }
inject SetQuantity { quantity: 100 }
inject SetLegPrice { role: TakeProfit, price: 195.00 }
inject SetLegPrice { role: StopLoss, price: 178.00 }
inject SetTpEnabled { enabled: true }
inject SetSlEnabled { enabled: true }
inject SubmitOrder
wait_for_event "BracketCreated" timeout=2000
wait_for_event "OrderFilled" timeout=5000
dump_state order_blotter.rows
screenshot out=.devloop/shots/orders-panel-filled.png
```

If `dump_state` shows three rows (Entry/TP/SL) with the expected prices and
the screenshot matches the reference, Slice 7 passes and the feature is done.

Later scripted journeys that exercise Limit/Stop fill paths rely on D3
(`InjectBrokerEvent`) to synthesise the fill, since TestBroker fills Market
orders instantly but Limit/Stop orders only via `check_limit_fills_inner`
which needs market ticks.

## Devloop integration

Three harness additions are genuinely useful here. Each is scoped small.

### D1 — `dump_state order_blotter` path

Trivial: `dump::build()` gains a `"order_blotter"` key with a JSON projection
of `OrderBlotter::rows`. No new types in the proto crate. `dump_state` walker
already supports dotted paths, so `order_blotter.rows.0.status` just works.

### D2 — Broker event visibility in the devloop event log

Today the event log only captures `TickerMsg`. A broker event that changes
the order blotter should also appear in `events.jsonl` so `wait_for_event
"BracketCreated"` works.

Add: a new event-log variant tagged `"broker"` with `{variant,
debug, symbol}`. Populated alongside the existing ticker logging in
`handle_broker_msg`. No protocol change — `wait_for_event` already matches
on string variant names; new variants are free.

### D3 — `InjectBrokerEvent` command (required, v1)

The primary mechanism for testing Limit/Stop fill paths in later scripted
journeys. TestBroker fills Market orders instantly but Limit/Stop orders only
via `check_limit_fills_inner`, which needs market ticks — scripted journeys
can't conjure those, so synthesising the fill event directly is the sanctioned
path. Slice 7 itself uses a Market entry and doesn't need D3, but D3 lands in
v1 so follow-on Limit/Stop journeys can be authored without blocking.

Add a new proto variant:

```rust
Command::InjectBrokerEvent {
    /// One of the known BrokerEvent variants, serialised as
    /// {"type": "BracketCreated", "parent_id": "...", ...}
    event_json: serde_json::Value,
},
```

Hand-parsed like `InjectTickerMsg`.

### D4 — Fixture expansion

`FixtureEnvelope` currently doesn't capture the order blotter. For v1 this is
fine — Slice-7 starts from an empty blotter and builds up. If preserving order
history across fixture snapshots becomes useful, add
`envelope.order_history: Vec<OrderRow>` later. **Not v1.**

### Devloop plan sub-evaluation

The three devloop additions above are small and non-architectural — they
inherit the devloop spec's policies (event log, token-aware screenshots,
etc.). They do not warrant a separate plan-eval pass unless implementation
reveals surprises. If during Slice 6 any of D1/D2/D3 becomes bigger than
"about an hour," pause and loop-eval the devloop additions as a minor plan
update.

## Data flow — step by step

When the user places a bracket through the normal chart UI:

1. User drags bracket legs + clicks Submit → `TickerMsg::SubmitOrder`.
2. `TickerState::apply` returns `TickerEffect::SubmitToBroker { bracket }`.
3. `handle_ticker_effects` converts to `BracketParams`, calls
   `broker_bridge.create_bracket(params)`.
4. Bridge sends `BrokerCommand::CreateBracket` over mpsc.
5. `TestBroker` receives, validates, persists to its SQLite, schedules a
   simulated fill, and broadcasts `BracketCreated`.
6. `broker_event_stream` iced subscription translates to
   `Message::BrokerEventReceived(event)`.
7. `handle_broker_msg`:
   - existing: reconciles `order_annotation_links`.
   - **new**: calls `self.order_blotter.apply(&event)` → creates 1–3 rows.
   - **new**: marks the persist handle dirty.
8. `OrderBlotterPanel` on next frame sees `order_blotter.generation()` change,
   rebuilds its `display_rows`, re-renders the grid.
9. `TestBroker` simulates fills; emits `OrderFilled`; events flow through
   the same pipe; blotter rows update; panel re-renders.

At no point does the panel have its own subscription. One subscription —
`broker_event_stream` — feeds everything.

## Persistence lifecycle

- `OrderHistoryPersistHandle::open(path)` in `MidasApp::new` — matches the
  TickerState pattern exactly.
- On open: hydrate all rows into `OrderBlotter`.
- On each `apply(&event)` that returns `dirty=true`: call
  `order_history_persist.upsert(row.order_id, row.clone())`.
- Native background thread debounces for 75ms, then writes.
- On app shutdown: `shutdown_blocking(Duration::from_secs(5))`.

**Retention policy for v1**: unbounded. Orders accumulate. A retention /
prune policy is future growth — when the table exceeds, say, 10k rows the
UI pagination and redb cost both justify looking at it. Emit a
`tracing::warn!` once when the row count crosses 10k so the payback condition
is observable in logs rather than relying on memory.

## Practical gotchas

- **Row identity = broker order uuid, not bracket parent**. Each leg gets
  its own uuid; rows are keyed by that. The bracket parent id is a
  secondary index for grouping (e.g. "Stop Loss" column refers to the
  sibling SL row's stop_price).
- **Sibling columns update late**. When `BracketCreated` fires, all three
  rows are created together, so each row knows its siblings' `tp_price` /
  `sl_price`. If a later partial state emerges (e.g. user cancels SL),
  the surviving rows need their sibling references refreshed. Implement
  by re-computing sibling fields via the `by_parent` secondary index —
  O(legs_in_bracket), typically 3. Never a full store scan.
- **TestBroker fill paths are structural, not a latency knob**. TestBroker
  fills Market orders instantly; Limit/Stop orders fill only via
  `check_limit_fills_inner`, which consumes market ticks. A scripted journey
  with a Limit entry and no tick stream will never see `OrderFilled`. Use
  Market entries (Slice 7) for end-to-end natural-path journeys, and use
  `InjectBrokerEvent` (D3) to drive Limit/Stop fill paths directly.
- **Hydration order**. `OrderBlotter` must hydrate BEFORE the broker
  subscription starts delivering events, or we risk a race where a
  persisted row is overwritten by a stale event from reconnection. Fix by
  ordering `MidasApp::new`: open persist → hydrate blotter → build bridge →
  register subscription.
- **Config backward compatibility**. Existing config files have no
  `order_blotters` section. `serde(default)` on the field means old configs
  load fine; `LayoutNode::OrderBlotter` is a new enum variant and the
  existing `LayoutNode::Unknown` forward-compat catch-all handles the
  reverse case.
- **Column widths per-user**. Watchlist persists column widths in its
  config; match that convention. Default widths live alongside the column
  definitions.
- **Symbol badge visual**. The target has a coloured pill with an icon
  glyph. For v1 we render: a coloured background (side-tinted), a small
  grey placeholder disc for the icon (no real ticker-specific glyph in
  the codebase today), and the ticker text. Real logos are future growth.

## Future growth (not v1)

- Per-row right-click menu: Cancel, Modify, Copy ID.
- Ticker logo icons in the Symbol badge (requires an icon asset pipeline).
- Filter/search bar above the grid.
- Retention policy / pruning.
- CSV export.
- Real IB gateway wiring (the path is there; just point at `BrokerConfig`
  with a Live data source and let `BrokerBridge` carry it through).
- Multi-account separation if / when accounts become a thing.
- Duration column separated from Instruction — currently collapsed into one
  column in the grid; the mock shows them separately. Easy to split later.
- Partial-fill visualisation (progress bar in the Qty cell).

## Acceptance criteria

1. Running `cargo run -p midas-app --features dev_harness -- --fixture
   empty-aapl-d1` and executing the Slice-7 journey produces:
   - Three rows in the `Orders` panel (Entry Buy Market, TP Sell Limit,
     SL Sell Stop).
   - Rows show correct symbol, side, type, quantity, prices.
   - Status transitions from Working → Filled (or whatever the TestBroker
     simulates for that journey).
   - Screenshot matches the reference within SSIM ≥ 0.99 (or the pixel-diff
     fraction threshold we settle on).
2. Restarting the app preserves the rows (redb hydration).
3. `cargo test --workspace`, `cargo clippy --workspace --features
   "midas-app/dev_harness" -- -D warnings`, and `cargo clippy --workspace --
   -D warnings` (default features) all green.
4. Default (feature-off) build unaffected: no broker changes, no ICED
   changes visible.

## Build order summary

| Slice | Scope                          | Estimate | Gate                                    |
|-------|--------------------------------|----------|-----------------------------------------|
| 0     | Verify existing submit path    | 0.5 h    | `SubmitOrder` → `BracketCreated` seen   |
| 1     | `OrderBlotter` + event mapping | 2 h      | `dump_state order_blotter` populated    |
| 2     | redb persistence               | 1.5 h    | survives restart                        |
| 3     | Empty panel skeleton           | 2 h      | toolbar adds panel; layout round-trips  |
| 4     | Grid rendering                 | 3 h      | rows visible and sortable               |
| 5     | Visual polish                  | 1 h      | matches reference screenshot            |
| 6     | Devloop additions (D1+D2+D3)   | 2 h      | `dump_state` path + broker event log + InjectBrokerEvent |
| 7     | End-to-end validation journey  | 1 h      | scripted journey passes                 |
| **Total** |                            | **~13 h**   |                                     |

Dependencies:
- Slice 0 → Slice 1 (linear).
- After Slice 1: Slices 2, 3, D1 and D2 all independent — run in parallel.
- Slice 4 requires Slice 1 (data model) + Slice 3 (panel shell). Slice 2 is
  not a prerequisite for Slice 4.
- Slice 5 requires Slice 4.
- D3 (`InjectBrokerEvent`) required before Slice 7 — now v1 scope, not
  optional.
- Slice 7 requires Slices 4 + 5 + D1 + D2 + D3.
