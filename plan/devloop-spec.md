# Devloop v1 — state pre-seeding + in-process harness for Hand of Midas

The goal is narrow: stop burning afternoons on "launch the app, click around to get
into the state I want to test, take a screenshot, read it, change one thing, repeat."
A fixture-driven in-process harness that Claude Code can drive from normal bash tool
calls during a coding session.

This is a dev accelerator. Not a CI system. Not a QA replacement. Not an a11y effort.

---

## Centerpiece: fixtures

The single thing that makes the loop worth building is **loading a known state on
launch**. Everything else (input injection, screenshots, event log) supports this.

A fixture is a named JSON blob on disk that captures:

- Every `TickerState` in `MidasApp::ticker_states` (already serde-ready — see
  `desktop/win/crates/midas-app/src/ticker_state/mod.rs`, schema v2).
- Per-chart viewport: `Camera2D { time_start, time_end, price_low, price_high,
  viewport_width, viewport_height, dpi_scale }` per bound symbol.
- Workspace layout + pane-grid splits (already in `AppConfig`).
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
  "layout": { /* serialized WorkspaceLayout */ },
  "active_ticker": "AAPL",
  "charts": [
    {
      "chart_id": "chart_0",
      "symbol": "AAPL",
      "timeframe": "Daily",
      "camera": { "time_start": 1_740_787_200_000, "time_end": ..., "price_low": ..., "price_high": ..., "viewport_width": 1200, "viewport_height": 800, "dpi_scale": 1.0 }
    }
  ],
  "ticker_states": [ /* Vec<TickerState>, one per touched symbol */ ]
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
- `WorkspaceLayout` in `midas-app/src/layout/` — check, likely already done for
  config persistence.
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
    DumpState { path: Option<String> },  // jq-like path, e.g. "ticker_states.AAPL.live_bracket"
    WaitForEvent { event_type: String, timeout_ms: u64 },
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
    Ok { body: serde_json::Value },
    Error { kind: ErrorKind, message: String },
}
```

### Addressing

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

### Timing

No `sleep`. Ever. `wait_for_event { event_type, timeout_ms }` or
`wait_for_idle { timeout_ms }`. The harness tracks last-Message-processed time;
"idle" means no `Message` processed in the last 3 frames (~50ms @ 60fps).

### Crash handling

- On harness startup, write PID to `.devloop/app.pid`, overwriting any existing.
- Listener holds each client socket open for the duration of the connection.
  Claude's driver script treats a dropped socket mid-command as a crash: check
  `app.pid`, if process is gone, emit a clear error and stop the journey.
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

Also log a smaller set of chart-scene transitions if they turn out to matter:
ticker switched, camera settled after pan/zoom. Hook these via the Message handler
in `app/handlers.rs`. Don't overthink it in v1 — add events when a specific
diagnosis needs one.

**Launch discipline**: truncate `.devloop/events.jsonl` at startup when
`dev_harness` is enabled. Otherwise every run contaminates the previous one.

---

## Screenshots — token-aware strategy

Each PNG Claude reads is ~1–3k tokens. Uncapped screenshotting burns budget fast.

Tiered capture:

1. `screenshot --out foo.png` always writes the PNG.
2. If a reference exists at `.devloop/refs/<fixture>__<journey>.png`, the
   harness also computes a pixel diff (root-mean-square or max-channel delta) and
   writes `.devloop/diffs/<fixture>__<journey>.png` plus a numeric delta in the
   response body.
3. The Claude-side driver only READS the PNG into context when:
   - No reference exists (first run / novel verification), OR
   - The diff exceeds a threshold (default: >0.5% of pixels differ by >8/255).
   - The human in the loop (Max) explicitly asks for it.

Max captures references once during manual dev: `curl ... snapshot_fixture` then
`curl ... screenshot --out .devloop/refs/<fixture>__<journey>.png`. Subsequent
automated runs compare silently and only surface visuals when something actually
changed.

Implementation: wgpu surface readback. The app already uses a wgpu `Surface`
(see `midas-render/src/renderer.rs`). Readback via a COPY_SRC-enabled surface
texture into a staging buffer, then `image` crate to encode PNG. iced 0.14
uses its own wgpu context; we need to reach in. Likely path: a custom
`iced::window::screenshot` call if 0.14 exposes one; otherwise a custom wgpu
command encoder passed through the chart widget's draw path. **Flag as risk** —
first thing to prototype.

---

## Build order

Eight steps. Step 1 is infrastructure only; Step 2 delivers the first usable
thing. Step 5 is the biggest win and should land within one sitting of completing
Step 4. Step 9 is the validation milestone, not new infra.

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

**Use it**: Max can now `curl` ping the running app from Claude sessions.

### 3. Event log + `wait_for_event` + `wait_for_idle` (~2 hours)

- `src/dev_harness/event_log.rs`: JSONL appender, truncates on startup.
- Wrap `TickerState::apply` — or add a post-call hook on the app-level `apply_ticker_msg`
  path (look at `app/ticker_wiring.rs`) — to serialize `(TickerMsg, Vec<TickerEffect>)`.
- Track last-Message-processed `Instant` in `MidasApp`; expose via harness for
  `wait_for_idle`.
- `wait_for_event`: spawn a tail task that reads new JSONL lines and matches on
  `msg.variant == event_type`.

**Use it**: Claude can now drive the app manually, then read the event log to
verify what happened.

### 4. `dump_state` (~1 hour)

- Serialize `MidasApp` state projection: `{"ticker_states": {symbol:
  TickerState}, "active_chart_id": ..., "charts": [...] }`.
- Support `path: "ticker_states.AAPL.live_bracket.entry.price"` via a small
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

### 6. Screenshot (~2–4 hours, possibly more)

- `src/dev_harness/screenshot.rs`. Reach into iced's wgpu context (risky — may
  require an iced upstream patch or a `wgpu::Surface` copy hack). Prototype
  early; if blocked, fall back to calling `PowerShell` to take a window
  screenshot by window handle — uglier but unblocks.
- Pixel-diff infra: `image` crate, RMS delta, write diff PNG with red
  highlights on changed regions.
- Reference-image storage convention.

**Use it**: visual regression tracking.

### 7. `inject_ticker_msg` (~1 hour)

- Deserialize `TickerMsg` from JSON (already serde-derives thanks to existing
  persistence work — verify).
- Route through same path as real messages. Effects must fire.

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
6. `dump_state ticker_states.AAPL.live_bracket` — verify 3 legs, correct prices.
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
- **Feature flag hygiene**. `dev_harness` types must not leak into release
  builds. `midas-devloop-proto` dep is feature-gated on `midas-app`, not
  unconditional. CI grep for `devloop` in the release build as a belt-and-suspenders
  check.
- **iced update ordering**. `Message::DevHarness` must be processed on the same
  thread that owns `MidasApp`. The TCP listener is on a tokio task; use an
  mpsc channel into iced's subscription to marshal commands back to the UI
  thread. Same pattern as `broker_bridge.rs`.
- **Screenshot readback is the risk.** iced 0.14's wgpu integration may not
  cleanly expose the surface texture. Fall back to a Win32
  `PrintWindow`/`BitBlt` by HWND if needed — the app runs only on Windows.
- **Chart coordinates are per-chart, not global**. `click_price` needs the
  chart's window-relative offset (pane-grid position), not just the camera
  projection. Look up via `ChartPanel` lookup in `MidasApp`.

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
bash: curl ... '{"cmd":"dump_state","path":"ticker_states.AAPL.live_bracket"}'
[... Claude reads dump, decides to fix code, iterates ...]
bash: curl ... '{"cmd":"shutdown"}'
```

The loop is literally bash + curl. No CLI to maintain. No phase harness. No
headless orchestration. Max presses Enter, Claude drives.
