# Devloop v1 — state pre-seeding + in-process harness for Hand of Midas

The goal is narrow: stop burning afternoons on "launch the app, click around to get
into the state I want to test, take a screenshot, read it, change one thing, repeat."
A fixture-driven in-process harness that Claude Code can drive from normal bash tool
calls during a coding session.

This is a dev accelerator. Not a CI system. Not a QA replacement. Not an a11y effort.

## Motivation — what "accelerator" means in numbers

Before: reproducing a bug like "stop-loss decorator misplaces on drag when entry
type is stop-limit" takes roughly 10 minutes of manual clicks per iteration —
launch, load symbol, zoom the camera, place the entry, place the TP, start
dragging the SL, notice the bug, patch code, rebuild, start over. A 20-iteration
afternoon is ~3 hours of setup for ~30 minutes of thinking.

After: a saved fixture restores that exact state in under a second. An
iteration becomes `load_fixture` → `inject_ticker_msg` → `dump_state` or
`screenshot --diff`. Twenty iterations fit in under an hour, most of which is
actually looking at the problem. That's the payoff.

---

## Centerpiece: fixtures

The single thing that makes the loop worth building is **loading a known state on
launch**. Everything else (input injection, screenshots, event log) supports this.

A fixture is a named JSON blob on disk that captures:

- Every `TickerState` in `MidasApp::tickers` (already serde-ready — see
  `desktop/win/crates/midas-app/src/ticker_state/mod.rs`, schema v2).
- Per-chart viewport: `Camera2D { time_start, time_end, price_low, price_high,
  viewport_width, viewport_height, dpi_scale }` per bound symbol.
- Workspace layout + pane-grid splits, captured through the **existing**
  `AppConfig` / `LayoutNode` / `PanelSlot` IR (see `midas-core::config` +
  `midas-app/src/app/persistence.rs`). Fixtures reuse the same IR path as config
  load — they do NOT serialize `WorkspaceLayout` directly.
- Window size + position.
- Bound symbols per chart panel + active timeframes.
- The current ticker the user is "focused" on (for single-chart dev).

What a fixture is NOT: market data. Fixtures reference symbol/timeframe; candle data
is loaded from the usual CSV/DuckDB path at fixture-apply time. A fixture is a
position in the state space, not a reproduction of the entire universe.

### Fixture file format

```
.devloop/fixtures/<name>.json
```

Top-level shape:

```json
{
  "devloop_fixture_version": 1,
  "ticker_state_version": 2,
  "captured_at": "2026-04-17T14:22:00Z",
  "note": "bracket half-placed on AAPL, camera zoomed to March 2026",
  "window": { "width": 2560, "height": 1440, "x": 100, "y": 60 },
  "layout": { /* AppConfig layout tree: Vec<LayoutNode> + Vec<PanelSlot> */ },
  "active_ticker": "AAPL",
  "charts": [
    {
      "chart_id": "chart_0",
      "symbol": "AAPL",
      "timeframe": "Daily",
      "camera": { "time_start": 1_740_787_200_000, "time_end": ..., "price_low": ..., "price_high": ..., "viewport_width": 1200, "viewport_height": 800, "dpi_scale": 1.0 }
    }
  ],
  "tickers": [ /* Vec<TickerState>, one per touched symbol */ ]
}
```

`devloop_fixture_version` is independent of `ticker_state_version` (currently 2,
const `CURRENT_VERSION` in `ticker_state/mod.rs`). A mismatch on either loads with a
loud error, not silent migration — devloop is dev-only, not a persistence layer.

### Fixture save/load workflow

- **Max saves manually during dev**: he gets into an interesting state by hand,
  hits a dev hotkey or sends `snapshot_fixture` over the socket, and names it
  (`sl-placement-bug-2026-04-17`).
- **Claude loads at launch**: `cargo run -p midas-app --features dev_harness -- --fixture sl-placement-bug-2026-04-17`.
  Or live: send `load_fixture { name }` to a running app, which drops all state
  and reapplies from the fixture. Live reload is cheaper than a restart for quick
  iteration.

### What needs to be made serializable

- `TickerState` — already done.
- `Camera2D` in `midas-chart/src/camera/mod.rs` — currently no serde impl. Add
  `#[derive(Serialize, Deserialize)]`. All fields are `f64`/`u32`/`f32` — trivial.
- `WorkspaceLayout` in `midas-app/src/layout/` — **not** directly serializable.
  It wraps `iced::widget::pane_grid::State<PaneState>` with no serde derives.
  Persistence today goes through an `AppConfig` / `LayoutNode` / `PanelSlot` IR
  (`midas-core::config`, applied in `midas-app/src/app/persistence.rs`). Fixtures
  reuse that IR: capture with the existing config-build path, restore with the
  existing config-apply path. No new layout IR.
- `ChartState` and `ChartScene` — do NOT try to serialize these. They're derived.
  Rebuild from `(symbol, timeframe, camera)` + a fresh data load.
- A new `FixtureEnvelope` struct in the proto crate (see below).

---

## Non-goals for v1 (making this explicit)

- No AccessKit. The chart is a single wgpu canvas; AccessKit buys ~0% coverage on
  what actually matters. For the handful of iced widgets outside the chart, we use
  string IDs by convention. Revisit in v2 only if that hurts.
- No `devloop-drive` CLI binary. Claude sends raw JSON over a TCP socket via
  `curl` / `python -c`. One less crate to maintain, one less thing to update when
  the protocol changes.
- No `claude -p` headless orchestration, no phase wrappers, no session-resume
  choreography. Max drives the loop in normal Claude Code sessions.
- No auto-PR, no auto-merge. Commits are manual per project rule.
- No `enigo` / OS-level input injection. Everything is in-process `iced::Event`
  synthesis.
- No cross-app drag-and-drop, no IME.
- Not a unit-test replacement. `midas-chart` sans-IO tests are still the first
  line of defense for state-machine logic. The loop is for things tests can't
  easily reach: full-app interaction, layout, live visual state.

---

## Crate layout

New crate:

```
desktop/win/crates/midas-devloop-proto/
  Cargo.toml
  src/lib.rs          # Command enum, Response enum, FixtureEnvelope, shared types
```

Why a crate, not a module: future tooling (a Python script, a test harness, a
different editor integration) should be able to depend on it without pulling in
the whole of `midas-app`. Right now only the harness listener consumes it, but
splitting it from day one is free. Pure types, no deps beyond `serde` /
`serde_json`.

Harness listener lives inside `midas-app`:

```
desktop/win/crates/midas-app/src/dev_harness/
  mod.rs              # #[cfg(feature = "dev_harness")] gate, TCP listener
  handlers.rs         # one fn per Command variant
  event_log.rs        # JSONL writer + subscriber
  fixture.rs          # save/load fixture against MidasApp
  screenshot.rs       # wgpu surface readback
  input.rs            # synthesize iced::Event for click/drag/scroll/key
```

All exposed behind a `dev_harness` Cargo feature on `midas-app` (and on the
`midas-devloop-proto` dep of `midas-app`, so release builds don't compile the proto
crate at all). Zero cost when disabled: `mod dev_harness;` line itself gated.

---

## Protocol

TCP on `127.0.0.1:<port>`. Default port `9898`; overrideable via
`DEVLOOP_PORT` env var for parallel app instances. One JSON request per line,
one JSON response per line. Newline-delimited, not HTTP. Simpler on both sides.

Pseudo-schema (defined concretely in `midas-devloop-proto`):

```rust
#[derive(Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Command {
    Ping,
    Shutdown,

    // -- Fixtures --
    LoadFixture { name: String },
    SnapshotFixture { name: String, note: Option<String> },

    // -- State inspection --
    DumpState { path: Option<String> },  // jq-like path, e.g. "tickers.AAPL.live_bracket"
    WaitForEvent { event_type: String, timeout_ms: u64, since_cursor: Option<u64> },
    WaitForIdle { timeout_ms: u64 },     // no Messages processed for N frames

    // -- Output --
    Screenshot { out_path: PathBuf },

    // -- Input injection --
    Click { x: f32, y: f32, button: MouseButton, modifiers: Modifiers },
    ClickPrice { symbol: String, price: f64, bar_index: i64, button: MouseButton, modifiers: Modifiers },
    Drag {
        from: Point,
        to: Point,
        pause_at_hover: bool,
        interpolation_steps: u32,   // default 4
    },
    Scroll { x: f32, y: f32, dx: f32, dy: f32 },
    Key { combo: String },           // "Ctrl+S", "Escape", "Enter"

    // -- Fast path: bypass input wiring, hit the domain directly --
    InjectTickerMsg { symbol: String, msg_json: serde_json::Value },
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Response {
    Ok { body: serde_json::Value, log_cursor: u64 },
    Error { kind: ErrorKind, message: String, log_cursor: u64 },
}
```

### Error responses

`ErrorKind` variants, one line each:

- `ParseError` — malformed JSON on the wire.
- `UnknownCommand` — command variant not recognised.
- `SymbolNotBound` — `click_price` / `inject_ticker_msg` references a symbol
  with no chart panel.
- `FixtureNotFound` — named fixture does not exist on disk.
- `FixtureVersionMismatch` — `devloop_fixture_version` or
  `ticker_state_version` disagrees with the current build.
- `Timeout` — `wait_for_event` / `wait_for_idle` expired.
- `HarnessPanic` — panic hook caught a panic during command handling; process
  is in an unstable state, client should shut down.
- `Internal` — catch-all with a message payload.

### Addressing

All pixel coordinates in the protocol are **iced logical pixels**, origin
**top-left** of the main window's client area. Same space `click_price`
resolves into. Physical-pixel reconciliation happens only inside the screenshot
pipeline via `Screenshot::scale_factor` — callers never see physical pixels.

- **Pixels** `(x, y)` — always available, resolved against the main window's
  client area.
- **Chart coords** `(symbol, price, bar_index)` — resolved by looking up the
  chart panel bound to `symbol`, then projecting via `Camera2D::time_to_x` +
  `price_to_y` (see `midas-chart/src/camera/mod.rs`). Fails loudly if no chart is
  bound to that symbol.
- **String widget IDs** — reserved for non-chart iced widgets (the toolbar,
  order panel inputs). Convention: `{panel}_{name}` e.g. `order_panel_quantity`,
  `toolbar_add_chart`. Introduce only as needed; do NOT preemptively tag every
  widget. For v1 the chart is the whole interesting surface.

### `inject_ticker_msg` vs `click`

Expose both, tell the caller what each is for:

- `inject_ticker_msg` — fast-path. Skips input wiring. Use this when testing
  domain logic (e.g. "what happens to the bracket when `SetLegPrice` fires with a
  wrong-side price?"). Direct call to `TickerState::apply`. Effects run exactly
  as production.
- `click_price` / `click` / `drag` — slow-path. Full `iced::Event` synthesis
  through the runtime. Use this when testing input wiring itself (e.g. "does
  dragging the TP decorator actually fire `SetLegPrice`?").

Never use one when the other is correct. `inject_ticker_msg` should NOT be used
to assert "clicking on the decorator updates price" — that's the thing being
tested.

**Blast radius — read this**: `inject_ticker_msg` is full-strength by design.
Effects fire exactly as in production, which includes `TickerEffect::PersistDirty`
(writes the live bracket to redb via `ticker_persist`) and — once Phase 1
IB paper lands — broker-submit effects that send real orders to the paper IB
gateway. The harness does NOT sandbox these. The contract is: after an
`inject_ticker_msg` session, `load_fixture` again before the next journey
starts, so persisted state doesn't leak across iterations. If that contract
ever proves too error-prone in practice, add an optional `drop_effects:
["PersistDirty", "SubmitBracket"]` param on the command — listed in Future
growth, not v1.

### Timing

No `sleep`. Ever. `wait_for_event { event_type, timeout_ms, since_cursor }` or
`wait_for_idle { timeout_ms }`. The harness tracks last-Message-processed time;
"idle" means no **input-origin or state-mutating** `Message` processed in the
last 3 frames (~50ms @ 60fps).

**Log cursor semantics**: every response (`Ok` and `Error`) carries a
monotonic `log_cursor` — the event-log line count immediately after the
command's own writes completed. Callers pass the cursor from the
previous response as `since_cursor` to `wait_for_event`, and the harness
only matches lines strictly after that cursor. Without this, the example
bash loop at the bottom of the plan races: a `click_price` might append
the awaited `SetLegPrice` to the log before the following `wait_for_event`
arrives, and a naive tail-from-current-end implementation would miss it.
Cursor threading makes the ordering deterministic.

`wait_for_idle` explicitly excludes market-data and broker-tick message
variants — with live feeds attached, those never stop arriving, and a literal
"no `Message` at all" definition would never resolve. The filter set is the
same one used to suppress tick-rate events from the event log.

If the caller genuinely needs strict idle-all-messages (rare — usually it's a
symptom of asking the wrong question), prefer `wait_for_event` on a specific
target instead.

### Crash handling

- On harness startup, write PID to `.devloop/app.<port>.pid`, overwriting any
  existing. Port-scoped from day one so parallel instances (see Future growth)
  don't collide — costs nothing now, prevents cleanup later.
- Listener holds each client socket open for the duration of the connection.
  Claude's driver script treats a dropped socket mid-command as a crash: check
  the matching port's `app.<port>.pid`, if process is gone, emit a clear error
  and stop the journey.
- `ping` on every new connection as a 50ms health check before the first real
  command.
- Panic hook in `main.rs` (behind `dev_harness` feature) writes
  `.devloop/panic.txt` with the backtrace before the process exits.

---

## Event log

Every `TickerMsg` passed to `TickerState::apply` is serialized to
`.devloop/events.jsonl` with:

```json
{"ts_mono_ns": 12345678, "ts_wall": "2026-04-17T14:22:01.123Z", "symbol": "AAPL", "msg": {...TickerMsg...}, "effects": [...TickerEffect...], "generation_before": 41, "generation_after": 42}
```

Hook point: inside `TickerState::apply` — or wrapping it — in
`ticker_state/apply.rs`. Cheapest place to intercept. Log after `apply()` returns,
so `effects` are captured.

**Serde dependency**: this serialises `TickerMsg` + `Vec<TickerEffect>`, and
neither derives `Serialize` today (both are `Debug, Clone` only). The event log
is the first consumer of the derive cascade described in Step 7 — the two
steps share that preparatory work. Schedule whichever lands first to do the
derives; the other picks them up for free.

Also log a smaller set of chart-scene transitions if they turn out to matter:
ticker switched, camera settled after pan/zoom. Hook these via the Message handler
in `app/handlers.rs`. Don't overthink it in v1 — add events when a specific
diagnosis needs one.

**Launch discipline**: truncate `.devloop/events.jsonl` at startup when
`dev_harness` is enabled. Otherwise every run contaminates the previous one.

**Default filter**: exclude high-frequency tick-rate variants — today
`TickerMsg::UpdateMarketData` fires per market-data tick, and once IB paper
trading is attached (Phase 1) a long session produces a multi-GB JSONL. Filter
these by default. Opt in via a protocol knob (e.g. `set_event_log_filter`) or a
config flag when a diagnosis needs them. The set of tick-rate variants lives
alongside the log module — keep it current as `TickerMsg` grows.

**Rotation**: when the active log file exceeds 100MB, rotate with a timestamp
suffix (`events-<ts>.jsonl`) and start a fresh file. Truncation would lose
history mid-session; rotation costs one `rename` call and preserves the
afternoon's evidence. Keep at most the last N rotated files (N=5 is plenty).

---

## Screenshots — token-aware strategy

Each PNG Claude reads is ~1–3k tokens. Uncapped screenshotting burns budget fast.

Tiered capture:

1. `screenshot --out foo.png` always writes the PNG.
2. If a reference exists at `.devloop/refs/<fixture>__<journey>.png`, the
   harness computes a **perceptual** diff — SSIM via the `image-compare` crate
   (pure Rust), or a pixelmatch-style YIQ delta if we end up porting the
   ~300 LOC core. Writes `.devloop/diffs/<fixture>__<journey>.png` plus a numeric
   delta in the response body.
3. The Claude-side driver only READS the PNG into context when:
   - No reference exists (first run / novel verification), OR
   - The perceptual delta exceeds threshold (tune empirically; start at SSIM
     < 0.995 or YIQ-differing pixel count > 0.2% of frame), OR
   - The human in the loop (Max) explicitly asks for it.

Why not RMS or max-channel RGB delta: GUI screenshots are dominated by
antialiasing jitter on text and chart lines. A straight RGB delta produces
false positives on every subpixel text shift, forcing the threshold high
enough that real visual regressions slip through. `pixelmatch` (Mapbox, >6k
stars) uses YIQ perceptual distance for exactly this reason.

Max captures references once during manual dev: `curl ... snapshot_fixture` then
`curl ... screenshot --out .devloop/refs/<fixture>__<journey>.png`. Subsequent
automated runs compare silently and only surface visuals when something actually
changed.

Implementation: use iced 0.14's public screenshot API. `iced::window::screenshot(id)`
returns a `Task<Screenshot>`; the `Screenshot` carries raw RGBA bytes plus a
`scale_factor` for DPI reconciliation. This is the supported path — no reaching
into iced's wgpu context, no custom surface readback.

`midas-render/src/renderer.rs` (`ChartRenderer`) does not own a `wgpu::Surface`:
it receives a `&mut wgpu::RenderPass` from the caller each draw. The surface is
owned by iced's shader-widget runtime, which we do not touch in v1.

Steps:
1. Issue `window::screenshot(main_window_id)` from a harness handler, await the
   returned `Task<Screenshot>`.
2. Encode the RGBA bytes to PNG via the `image` crate.
3. Use `Screenshot::scale_factor` to map between logical client-area coordinates
   (what `click_price` uses) and physical pixel coordinates in the diff output.
   DPI reconciliation is the one real gotcha here.

Contingency only: if the task-based API proves inadequate for some pane layout
edge case, fall back to Win32 `PrintWindow` / `BitBlt` by HWND. Note the pitfall:
DWM composition combined with a wgpu-backed window on Windows is known to return
black frames via `BitBlt` — do not reach for this reflexively.

---

## Build order

Nine steps. Step 1 is infrastructure only; Step 2 delivers the first usable
thing. Step 5 is the biggest win and should land within one sitting of completing
Step 4. Step 9 is the validation milestone, not new infra.

**Dependencies**: Steps 1 → 2 are linear. After Step 2, Steps 3 / 4 / 5 / 6 / 7
can proceed in parallel. Step 8 wants `wait_for_idle` from Step 3. Step 9 is a
validation milestone that requires 5 + 6 + 7 + 8. **Caveat**: Steps 3 and 7
both consume the `TickerMsg` + `TickerEffect` serde cascade — whichever lands
first does the derives, the other picks them up for free. Parallel in
calendar, coordinated on that one PR.

### 1. Proto crate (scaffolding, ~1 hour)

Create `desktop/win/crates/midas-devloop-proto/`. Define `Command`, `Response`,
`FixtureEnvelope`, `Point`, `MouseButton`, `Modifiers`, `ErrorKind`. Add to
workspace members. Pure serde types, no logic.

### 2. `dev_harness` feature + `ping` + `shutdown` (~2 hours)

- Add `dev_harness` feature to `midas-app/Cargo.toml`. Feature-gates the proto dep.
- `src/dev_harness/mod.rs`: TCP listener on `127.0.0.1:{DEVLOOP_PORT or 9898}`.
  Spawn on app boot, behind `#[cfg(feature = "dev_harness")]`. Tokio runtime
  already present (iced uses tokio feature).
- Dispatch via `mpsc::UnboundedSender<Command>` to iced's update path —
  reuse iced's command/subscription machinery to marshal harness commands onto
  the UI thread. Wrap in `Message::DevHarness(Command, ResponderChannel)`.
- Implement `Ping` → `Ok { body: {"pid": N} }` and `Shutdown` → graceful exit.
- Smoke test: `cargo run --features dev_harness` then
  `echo '{"cmd":"ping"}' | nc 127.0.0.1 9898`.
- **While smoke-testing**, issue a one-off `iced::window::screenshot(id)` call
  against the idle app — just await the `Task<Screenshot>` and confirm RGBA
  bytes come back. If they do, Step 6's scope is confirmed and can be slotted
  in parallel with fixture work. Half an hour of de-risking Step 6 for free.

**Use it**: Max can now `curl` ping the running app from Claude sessions.

### 3. Event log + `wait_for_event` + `wait_for_idle` (~2 hours)

- `src/dev_harness/event_log.rs`: JSONL appender, truncates on startup.
- Wrap `TickerState::apply` directly in `ticker_state/apply.rs`, or hook the
  call sites in `app.rs` (around the `ticker_mut(...).apply(msg)` pattern) and
  `handle_ticker_effects` in `app/ticker_wiring.rs`. Serialize
  `(TickerMsg, Vec<TickerEffect>)` after `apply()` returns so effects are
  captured.
- Track last-Message-processed `Instant` in `MidasApp`; expose via harness for
  `wait_for_idle`.
- `wait_for_event`: spawn a tail task that reads new JSONL lines and matches on
  `msg.variant == event_type`.

**Use it**: Claude can now drive the app manually, then read the event log to
verify what happened.

### 4. `dump_state` (~1 hour)

- Serialize `MidasApp` state projection: `{"tickers": {symbol:
  TickerState}, "active_chart_id": ..., "charts": [...] }` — matches the real
  `MidasApp::tickers` field name.
- Support `path: "tickers.AAPL.live_bracket.entry.price"` via a small
  `serde_json::Value` walker. No jq dep.

**Use it**: Claude inspects state without taking screenshots.

### 5. Fixture save + load (~3–4 hours — THE BIG WIN)

- Add `Serialize`/`Deserialize` to `Camera2D`.
- `snapshot_fixture`: walk `MidasApp`, build a `FixtureEnvelope`, write to
  `.devloop/fixtures/<name>.json`.
- `load_fixture`: read envelope, validate versions, call
  `MidasApp::reset_and_apply_fixture` (new method — drops all panels, recreates
  from envelope, reloads data).
- CLI flag `--fixture <name>` on `midas-app` that auto-loads at boot.
- Document: when `TickerState` schema changes, OLD fixtures fail loudly. That's
  the contract. Fixtures are disposable.

**Use it**: Max records `sl-placement-bug` once by hand. Every subsequent Claude
session starts with `load_fixture` and is already on the exact state. This is
the payoff.

### 6. Screenshot (~2 hours)

- `src/dev_harness/screenshot.rs`. Call `iced::window::screenshot(main_window_id)`
  from the command handler, await the `Task<Screenshot>`, encode RGBA bytes to
  PNG via the `image` crate.
- Use `Screenshot::scale_factor` to reconcile logical coords (what
  `click_price` produces) with physical pixel positions in diff output.
- Perceptual-diff infra: SSIM via `image-compare` (pure Rust), or pixelmatch-
  style YIQ delta. Write diff PNG with highlighted regions; return numeric
  delta in the response body.
- Reference-image storage convention under `.devloop/refs/`.

**Use it**: visual regression via the pixel-diff tiering — Claude only reads
the PNG when the perceptual delta exceeds threshold or no reference yet exists,
keeping token budget under control even across long iteration runs.

### 7. `inject_ticker_msg` (~2–3 hours)

The injection itself is trivial — ~20 LOC to route a deserialized `TickerMsg`
through the same `apply_ticker_msg` path the app uses in production. The real
work is the serde derive cascade.

**Verify first** (before coding): `TickerMsg` and `TickerEffect` currently
derive only `Debug, Clone` (see `ticker_state/apply.rs`). Enumerate the
transitive type set reachable from both enums — walk every variant field, and
every type those fields mention in turn — and confirm each has
`Serialize, Deserialize`. Known set reachable from `TickerMsg`:

- `TickerMsg`, `TickerEffect` themselves — add derives.
- `OrderSide`, `EditingField`, `StoredLevel`, `ToastAction`, `TickerState`
  (already serde via persistence work), `uuid::Uuid` (serde via feature).
- `LegRole`, `EntryType`, `OrderBracket`, `BracketLeg`, `BracketSide`,
  `BracketStatus` — already serde-ready in `midas-chart/src/widget/order_bracket`.
- `AnnotationId` — confirm.

Budget the cascade, not the plumbing. If the verify-first pass turns up a type
with a non-serde field (e.g. an `Instant` or a closure), that's where the time
goes: replacing it with a serialisable substitute or skipping via
`#[serde(skip, default = ...)]`.

**Use it**: fast domain tests without input simulation latency.

### 8. Click / drag / scroll / key (~4–6 hours)

- `src/dev_harness/input.rs`. Synthesize `iced::Event::Mouse` /
  `iced::Event::Keyboard` and feed into iced's event pipeline.
- `click_price` resolves `(symbol, price, bar_index)` → pixel via the chart
  panel's camera and its window-relative offset. Panel lookup via
  `MidasApp::find_chart_by_symbol`.
- `drag`: emit `ButtonPressed` → N interpolated `CursorMoved` → `ButtonReleased`.
  Each event processed on a fresh frame tick; use `wait_for_idle` between
  steps inside the drag, not wall sleeps.
- `key "Ctrl+S"`: parse combo, emit modifier-press + key-press + key-release +
  modifier-release.

**Use it**: full input-wiring tests.

### 9. First real journey (validation, not new infra)

Pick one genuine HoM pain point. Candidate: the bracket-placement flow
(`BracketTool` state machine — see `midas-chart/src/widget/bracket_tool/`).
Wire end-to-end:

1. Start with fixture `empty-aapl-daily-chart`.
2. Inject `SetBracketMode(Some(Buy))`.
3. Click at chart coords (entry price).
4. Click at chart coords (TP).
5. Click at chart coords (SL).
6. `dump_state tickers.AAPL.live_bracket` — verify 3 legs, correct prices.
7. `screenshot` — diff against reference.

If this works, the loop is real. If it doesn't, we know where.

---

## Practical gotchas

- **Clear `.devloop/events.jsonl` at launch**, every launch. Stale events poison
  `wait_for_event`.
- **Fixture version mismatch errors loudly**. `TickerState` schema
  (`CURRENT_VERSION = 2`) evolves. Don't silently migrate dev fixtures — just
  refuse, with a message telling Max to re-record.
- **`Camera2D` isn't serde today**. Add derives when implementing Step 5.
- **`ChartState` / `ChartScene` are not snapshot targets**. Derived state.
  Rebuild from `(symbol, timeframe, camera)` on fixture apply.
- **TCP port collisions**. If Max runs two instances in parallel, the second
  crashes on bind. Default port `9898`, env override `DEVLOOP_PORT`; error on
  bind is fine for v1.
- **Feature flag hygiene**. "Zero cost when disabled" is partially true and
  worth being precise about:
  - *Feature-gated* (absent from release builds): the `midas-devloop-proto`
    dep on `midas-app`, the entire `src/dev_harness/` module, the TCP listener,
    the `Message::DevHarness` variant, the event-log writer, the harness panic
    hook extension.
  - *Unconditional* (shipped in release): `Serialize, Deserialize` derives
    added to domain types (`Camera2D`, `TickerMsg`, `TickerEffect`, and any
    transitive types the cascade reaches). Those crates don't know about
    `dev_harness`. Cost is negligible — serde is already transitive in the
    workspace — but it's honest to say so.
  - CI grep for `devloop` / `dev_harness` identifiers in the release binary
    verifies no harness surface leaks past the feature gate.
- **iced update ordering**. `Message::DevHarness` must be processed on the same
  thread that owns `MidasApp`. The TCP listener is on a tokio task; use an
  mpsc channel into iced's subscription to marshal commands back to the UI
  thread. Same pattern as `broker_bridge.rs`.
- **Screenshot DPI reconciliation.** The main gotcha here is not readback —
  iced 0.14 exposes `window::screenshot` as a first-class API. It's the
  `scale_factor` the `Screenshot` comes with: `click_price` works in logical
  client-area coords, but the PNG is physical pixels. Every diff region,
  every reference lookup has to multiply through `scale_factor` consistently
  or high-DPI boxes will mis-map. Get it right once in the diff helper.
- **Chart coordinates are per-chart, not global**. `click_price` needs the
  chart's window-relative offset (pane-grid position), not just the camera
  projection. Look up via `ChartPanel` lookup in `MidasApp`.
- **Mid-apply failure in `load_fixture`.** If deserialisation succeeds but
  applying the envelope to `MidasApp` fails partway — panel creation errors,
  data load fails, IR-apply hits a missing symbol — the app must either
  complete a rollback to its pre-load state or fail loudly and exit. No silent
  half-loaded state: a fixture that "kind of" loaded is strictly worse than no
  fixture. Simplest implementation: apply into a staging `MidasApp` clone,
  swap on success, discard on error.

---

## Future growth (not v1)

Keeping these on the map for when v1 proves valuable:

- **Real OS input via `enigo`** for a final smoke-pass (catches winit / OS
  event-path bugs that in-process synthesis misses).
- **Parallel app instances on unique `DEVLOOP_PORT`s** — multi-scenario
  fixtures, concurrent Claude sessions.
- **Golden-image CI**: reference PNGs under git, CI job running fixtures and
  diffing. Requires GPU-capable CI runner.
- **Headless orchestration** (`claude -p` with session resume) for autonomous
  feature pipelines. Most of the pieces here are reusable; it's the wrapper
  layer that changes.
- **AccessKit** — only if we grow significant non-chart UI surface and find
  string-ID-by-convention painful.
- **Fixture migration tooling** — if `TickerState` schema bumps end up killing
  expensive-to-record fixtures enough times to be annoying.

---

## Shape of a Claude-driven loop

Rough sketch of what a tool-call sequence looks like. The entire exchange stays
inside a normal Claude Code session.

```
bash: cargo run -p midas-app --features dev_harness -- --fixture sl-placement-bug-2026-04-17 &
bash: until curl -s --data '{"cmd":"ping"}' 127.0.0.1:9898; do sleep 0.2; done
bash: curl ... '{"cmd":"inject_ticker_msg","symbol":"AAPL","msg_json":{"SetBracketMode":{"side":"Buy"}}}'
bash: curl ... '{"cmd":"click_price","symbol":"AAPL","price":184.50,"bar_index":-10,"button":"Left","modifiers":{}}'
bash: curl ... '{"cmd":"wait_for_event","event_type":"SetLegPrice","timeout_ms":1000}'
bash: curl ... '{"cmd":"screenshot","out_path":".devloop/shots/step3.png"}'
bash: curl ... '{"cmd":"dump_state","path":"tickers.AAPL.live_bracket"}'
[... Claude reads dump, decides to fix code, iterates ...]
bash: curl ... '{"cmd":"shutdown"}'
```

The loop is literally bash + curl. No CLI to maintain. No phase harness. No
headless orchestration. Max presses Enter, Claude drives.
