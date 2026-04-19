# Plan: MidasApp god-object split — proof-of-pattern (Toast slice)

*Architecture audit P1 — first slice, written after 5 research + 3 critique agents.*

## Status

| Slice | Status |
|---|---|
| 0 — Toast controller | **shipped** (commits `f4015da`, `9614b4b`, follow-up review fixes) |
| 2 — WindowGeometry controller | **shipped** (commits `f338700`, `2395864`); 4 fields + 4 Message variants moved into `crate::window_geometry`. Round-trips through `midas_core::config::WindowConfig` (7 fields). Pattern proven for "multi-field + persistence + iced runtime task spawn" beyond Toast's degenerate single-Option case. |
| 1A — `Message::Chart` wrapper introduced | **shipped** (commit `468fd30`); chart_widget decoupled from Message variants, 132-LOC fan-out → 1 line |
| 1B batch 1 — Delete `ChartPan`/`ChartZoom`/`ChartZoomY` | **shipped** (commit `e8f06c1`); 3 variants gone, dev_harness migrated |
| 1B batch 2 — Delete level/placing/setting variants (11) | **shipped** (commit `bf5a807`); `Crosshair`, `CreateLevel`, `DragLevel`, `SelectLevel`, `DeselectLevel`, `DeleteSelectedLevel`, `CancelPlacing`, `PlacingCursorMoved`, `SetTimelineBorderRatio`, `SetVolumeScale`, `RightClickLevel` |
| 1B batch 3 — Delete bracket variants (9) | **shipped** (commit `9204054`). Bodies extracted into `handle_chart_bracket_*` methods on `MidasApp`; `dispatch_chart_action` calls them directly. Dispatcher routing for brackets shrinks from 11 entries to 2 (only `BracketContextCancel`/`Dismiss` from the popup widget remain). |
| **Slice 1 (audit P2 #4) — COMPLETE** | All 23 chart-action `Message::Chart*` variants deleted. Top-level `Message` enum: 134 → ~110 variants. |
| **View-models migration (audit P1) — COMPLETE** | 12 slices, ~30 unit tests. Account / Watchlist / Chart pane / Order panel / Status bar / Toolbar all read from `*Vm` projections built once via `MidasApp::*_vm()` builders. Commits `d58c75d`, `4cbad6e`, `deb9e2b`, `cac474b`, `78f114b`, `012a800`. **This is the new floor for the controller split**: every cross-cutting controller can now consume a VM rather than reach into MidasApp internals, which removes one of the original "Watchlist is too coupled" objections. |
| 3 — Watchlist controller | **gated** (see "Next steps" below). Pattern-scaling review still applies: needs `SharedServices` + `Controller` trait first. View-models work has narrowed the scope but not removed the gate. |

Slice 0 ratio came in at **3.98×** (438 LOC new / 110 LOC removed) — just under the 4× kill threshold the plan set. Pattern works for trivial controllers (single-instance state, no shared deps); it is **not yet proven** for cross-cutting controllers like Watchlist.

## Post-slice-0 review verdict

Three review agents (code-scrutiny, bug-hunt, pattern-scaling) ran against the slice. Highlights:

- **No correctness bugs.** `consume_toast_effects`'s recursive `self.update(*boxed)` is bounded — `state.take()` defuses any Toast-cycle on the second hop.
- **`Effect::Spawn` deleted as YAGNI.** Was added for symmetry with no caller; removed in the follow-up. Effect enum down to 1 variant, the `FireParentMsg` back-edge.
- **`clear()` removed.** Escape now routes through `ToastMsg::Dismiss` so the "all mutation via `update()`" invariant holds without exception.
- **`state()` test-only** (`#[cfg(test)]`).
- **Pre-existing UX bug surfaced** (not a regression): `ToastMsg::Show` unconditionally replaces an existing toast, dropping any pending action button. Same as old code; tracked separately, not blocking.
- **Integration test deferred.** A round-trip test through `dispatch_toast` + `FireParentMsg` requires constructing `MidasApp`, which is itself the object the audit is trying to split. Adding a test harness is its own slice. Documented as a known coverage gap.

## Slice 1 recommendation: NOT another controller

The pattern-scaling agent's verdict was unambiguous: extracting Watchlist next will hit problems Toast didn't and likely fire the kill criterion. Concrete reasons:

- Watchlist's drag emits ~3 cross-domain mutations per drop (`MovePane`, `RebindChart`, `RouteLink`). The controller becomes a thin pass-through; the parent's `consume_*` interpreter grows by exactly the LOC the struct shrinks. Net architectural lever ≈ 0.
- `FireParentMsg(Box<Message>)` becomes the routing strategy, not the back-edge it is for Toast — every cross-domain mutation goes through the parent re-dispatch. That's god-state with extra Box allocation.
- Plan's "introduce `SharedServices` on second caller" is exactly wrong: slice 1 IS the second caller and the abstraction would be designed under deadline pressure.

**Better next move**: take the audit's *other* P2 finding — collapse `Message::Chart*` (30 variants, 132-LOC `action_to_message`) into a single `Message::Chart(ChartId, ChartAction)` wrapper. This:

- Deletes ~30 enum variants and the entire fan-out function
- Introduces zero new abstraction; no `SharedServices` debate
- Directly attacks the "Message enum is the real god, not the struct" diagnosis from the audit research
- Decisive, measurable shrink with no pattern question to validate

**If a second controller still wanted as a sanity check** before tackling Watchlist, do `WindowGeometry` (4 OS-window fields, no view, no shared deps) — explicitly mechanical, fast, validates the trait-ification of dispatch+interpret on a slice that can't fail.

**Before any real cross-cutting controller (Watchlist, Account, Chart):**
1. Introduce `SharedServices` struct (start with `link_routing`, `drag`, `market_cache`)
2. Introduce `Controller` trait + a single generic interpreter so `consume_*` boilerplate doesn't multiply by 12

These two abstractions need to land *before* slice 1's first cross-cutting controller, not during it.



## Overview

Split the 76-field `MidasApp` god struct into per-domain controllers, each owning a slice of state and exposing `update(msg) -> Vec<Effect>` + `view(&self) -> Element<SubMsg>`. The pattern matches our existing `TickerState::apply -> Vec<TickerEffect>` (already shipped, hundreds of tests) — **deliberately the SAME shape**, not a new one, so the codebase has one mental model rather than two.

This plan ships **Slice 0: Toast**. Toast was chosen over the bucket-scheme agent's Window recommendation because Window has zero shared deps and no view — it would prove the *easy* half of the pattern. Toast has ~120 LOC across handler + view + state + the interesting `Box<Message>` re-dispatch case. It exercises the questions a future Watchlist slice will hit (cross-cutting state read, view composition, parent-routed effects) without taking on drag/drop in the first slice.

## Research summary

Five parallel research agents covered:

- **Watchlist surface inventory** — debunked the audit's "Watchlist is half-drawn" claim. `WatchlistMsg` doesn't exist; handler reaches into 14 `self.*` fields and 8 cross-domain methods; drag/drop is genuinely cross-cutting (`DragMouseUp` mutates `workspace.panes` + `charts`). Watchlist is *not* the easy first slice.
- **TickerState pattern study** — confirmed `apply -> Vec<Effect>` works for narrow per-instance state. Hidden costs: `SubmitToBroker` interpretation has 39 LOC of translation in the handler; effect handlers reach back into `self.tickers`. Generalization risk for multi-instance shared state (drag, link).
- **Iced 0.14 sub-controller idioms** — Halloy is the production reference. Sub-update returns `(Task<SubMsg>, Option<Event>)`; shared state stays parent-owned, passed `&mut`; subscriptions stay centralized. The `Component` trait was removed in 0.13.
- **Message bucket scheme** — proposed 12-bucket split of the 134 variants. Recommended Window first (smallest blast radius). Critical observation: "the bucket split helps message routing, but the real architectural lever is moving the four shared stores [TickerState, AnnotationStore, OrderBlotter/PositionStore, Link bus] behind explicit `&mut`-borrowing service handles."
- **Real-world god-struct splits** (Halloy, cosmic-files, rerun) — most common mistake is **bottom-up god-struct re-formation**. cosmic-files has 7300-LOC `app.rs` *despite* per-tab controllers. hecrj (iced maintainer) explicit: split by domain, not layer. The Message enum is the real god, not the struct.

Three critique agents flagged:
- **Factual**: should explicitly use `midas_core::config::WindowConfig` (later slice); LOC math overstated.
- **Gaps**: shutdown race; subscription should live on controller; needs 2-commit rollback; concrete fitness function; Toast is a better pattern-proof than Window because it has a view.
- **Design**: Window is the *wrong* first slice (avoids real coupling); the TickerState/Halloy divergence is THE call-out and we should unify on `Vec<Effect>`; defer of `SharedServices` is debt; rename `WindowChrome → WindowGeometry`; add a kill criterion.

## Design decisions

### Decision 1: which slice goes first

**Context**: audit suggested Watchlist. Bucket-scheme agent suggested Window. Design critique pushed for something with a view that exercises real cross-cutting.

**Recommendation**: **Toast**, for these reasons:
- Single-instance state (`Option<ToastState>`) — no per-id routing complexity to muddy the slice
- Has a real `view_toast_overlay` that exercises `Element::map(Message::Toast)` composition
- `ToastActionClicked` re-dispatches an arbitrary `Box<Message>` — the most interesting cross-controller effect we'll see, and Toast already handles it cleanly (no broker / no link / no workspace coupling)
- Auto-dismiss via `Tick` exercises the "subscription reads sub-state" pattern centrally
- ~3 message variants, ~1 state field, ~120 LOC across handler + view + struct — small enough to ship safely, large enough to be a real pattern proof
- Failure to extract Toast = pattern fundamentally doesn't fit; failure is loud and recoverable in one PR

**Explicitly dropped**:
- Window-geometry slice — too easy. Saved as Slice 1 (or later) with a clearer name (`WindowGeometry`, owning only the 4 OS-window fields).
- Watchlist — has 14-field cross-cutting + drag/drop. Saved for after pattern proof.
- Account / Chart — biggest payoff, biggest risk.

**Confidence**: high.

### Decision 2: sub-update signature

**Context**: TickerState returns `Vec<TickerEffect>`. Halloy returns `(Task<Msg>, Option<Event>)`. We have to pick one shape — having two is the concrete failure mode (cosmic-files / Halloy). Design critique was emphatic.

**Recommendation**: **`fn update(&mut self, msg: SubMsg) -> Vec<Effect>`**, matching TickerState exactly.

`Effect` carries everything:
- `Effect::Spawn(Task<SubMsg>)` — for async work (controller-local Tasks)
- `Effect::FireParentMsg(Box<Message>)` — for cross-controller dispatch (Toast's case)
- Typed events the parent interprets — domain-specific per controller

**Why `Vec` not `Option`**: Halloy's `Option<Event>` cannot express "this update produced two effects" (e.g., `WindowMoved` → both `MarkConfigDirty` + `MonitorChanged`). `Vec` handles 0/1/N uniformly. Empty `Vec` is ergonomic and zero-allocation cheap.

**Why no `Task<SubMsg>` return**: it'd require the parent to know the wrapping (`.map(Message::Toast)`) at every call site. By making Tasks an Effect variant the controller never knows the parent's Message type.

**Confidence**: high. Cost = the Halloy research is now reference-only, not a blueprint.

### Decision 3: shared state

**Context**: Toast doesn't need shared state. But the design pattern has to scale.

**Recommendation**: **No `SharedServices` in slice 0.** Toast's only shared concern is `ToastEffect::FireParentMsg(Box<Message>)` for ActionClicked — that's already an Effect. When slice 1 (whichever) hits its first real shared dep, introduce `SharedServices` with that one field, and migrate Toast (still requires no shared deps) for free.

This is intentionally NOT the design critique's recommendation. Reasoning: introducing `SharedServices` with zero callers is YAGNI. Introducing it with one caller is YAGNI. Two callers = real abstraction. Toast is the zero-call case.

**Confidence**: medium-high. Risk: slice 1 designs `SharedServices` while elbow-deep in implementation. Mitigation: kill criterion (Decision 5) catches this — if slice 1 takes >1 week, we revisit the pattern.

### Decision 4: subscription ownership

**Context**: `Tick` (1Hz) drives Toast auto-dismiss. Currently the central `subscription()` produces `Tick`; the central handler reads `self.toast`.

**Recommendation**: **Subscriptions stay centralized for slice 0** — Toast subscribes to nothing of its own. The `Tick`-driven auto-dismiss continues to work via the existing path: `Tick` arrives → `handle_tick_ticker_msg` → it now calls `self.toast_ctrl.tick(now)` instead of inspecting `self.toast` directly.

Halloy keeps subs centralized; we follow. If a future slice (e.g., Watchlist with drag) really needs its own subscription, define `SubController::subscription(&self) -> Subscription<SubMsg>` then and the parent does `Subscription::batch(...)`. Don't preemptively invent.

**Confidence**: high.

### Decision 5: kill criterion

If slice 0 ships and:
- Total new LOC (controller + tests + wiring) > 4× the LOC moved out of `MidasApp`, OR
- The controller's public API has > (Effect variants + 1) public methods beyond `new`/`update`/`view`, OR
- The integration on the parent side requires reaching back into the controller's private state

…then the per-controller pattern is the wrong decomposition for this codebase. Stop after slice 0; revisit with a different shape (e.g., extract by lifecycle phase, or just trim the Message enum in place via newtypes).

**Concrete numeric**: Toast moves ~120 LOC out of `app.rs` + `handlers.rs` + `views.rs`. Budget for the controller is ~480 LOC of new code (4× ratio). Beyond that, abandon.

## Implementation plan

### Slice 0: Toast controller

**Goal**: prove the controller pattern with a real subsystem (state + view + cross-cutting effect) at minimum LOC.

**Depends on**: nothing.

**Files to create**:
- `crates/midas-app/src/toast/mod.rs` — `pub struct ToastController`, `pub enum ToastMsg`, `pub enum ToastEffect`, `pub struct ToastState` (moved from app.rs), `pub struct ToastAction` (moved from app.rs), `impl ToastController { fn new, update, view, tick }`
- `crates/midas-app/src/toast/tests.rs` — characterization tests

**Files to modify**:
- `crates/midas-app/src/app.rs`:
  - Remove the `ToastState` and `ToastAction` struct definitions (move to `toast/mod.rs`)
  - Replace `pub toast: Option<ToastState>` with `pub toasts: ToastController`
  - Replace `Message::ShowToast`, `Message::DismissToast`, `Message::ToastActionClicked` with single `Message::Toast(ToastMsg)`
  - Update dispatcher arm: `Message::Toast(m) => self.dispatch_toast(m)`
- `crates/midas-app/src/app/handlers.rs`:
  - Delete `handle_toast_msg` (3 arms gone)
  - Add `dispatch_toast(&mut self, msg: ToastMsg) -> Task<Message>` — calls `self.toasts.update(msg)`, interprets `Vec<ToastEffect>` (translates `Effect::FireParentMsg(boxed) → self.update(*boxed)`, `Effect::Spawn(task) → task.map(Message::Toast)`)
  - In `handle_tick_ticker_msg` (the Tick handler), replace `self.toast`-inspection auto-dismiss with `self.toasts.tick(now)` — returns same `Vec<ToastEffect>` interpreted the same way
  - In every other site that fires `Message::ShowToast { ... }`, change to `Message::Toast(ToastMsg::Show { message, action })`
- `crates/midas-app/src/app/views.rs`:
  - `view_toast_overlay` either (a) moves to `ToastController::view` returning `Element<ToastMsg>` and the call site does `.map(Message::Toast)`, or (b) becomes a thin wrapper that calls into the controller. **Recommendation: (a)** — that's the whole point of the slice.
- All emit-sites of `Message::ShowToast` / `Message::DismissToast`: search-and-replace to `Message::Toast(ToastMsg::Show {...})` etc.

**Key implementation details**:

- `ToastController::update(&mut self, msg: ToastMsg) -> Vec<ToastEffect>`:
  - `ToastMsg::Show { message, action }` → set `self.state = Some(...)`, return `vec![]`
  - `ToastMsg::Dismiss` → clear `self.state`, return `vec![]`
  - `ToastMsg::ActionClicked` → take state, if action present, return `vec![ToastEffect::FireParentMsg(action.on_click)]`
- `ToastController::tick(&mut self, now: Instant) -> Vec<ToastEffect>`:
  - if state exists and elapsed > timeout, clear it, return `vec![]`
  - otherwise return `vec![]`
- `ToastEffect`:
  ```rust
  pub enum ToastEffect {
      /// Async task spawned by the controller. Parent maps to top-level Message.
      Spawn(iced::Task<ToastMsg>),
      /// Fire an arbitrary parent message — the only path Toast has to talk
      /// to other controllers (used by ActionClicked re-dispatch).
      FireParentMsg(Box<crate::app::Message>),
  }
  ```
- `ToastController::view(&self) -> Option<Element<'_, ToastMsg>>` — returns `None` if no toast; the call site decides whether to render in the stack.

**Two-commit shape** (per gaps critique #6):
1. **Commit A**: introduce `toast/` module + tests + the `ToastController` type. NOT YET wired into MidasApp. `cargo test` passes.
2. **Commit B**: wire it in (delete old struct/handler/view code, add controller field, replace Message variants, redirect emit sites). `cargo test` passes.
   
Revert by `git revert <B>`. Slice 0 ships as PR with both commits.

**Testing**:
- Pure unit tests on `ToastController` (no `MidasApp`). Cover:
  - `new()` produces empty state
  - `update(ToastMsg::Show{...})` sets state, returns `vec![]`
  - `update(ToastMsg::Dismiss)` clears state, returns `vec![]`
  - `update(ToastMsg::ActionClicked)` with action present → returns `vec![FireParentMsg(boxed)]`, clears state
  - `update(ToastMsg::ActionClicked)` with no action → no effect, clears state
  - `tick(now)` past timeout clears state
  - `tick(now)` before timeout preserves state
  - `view()` returns `None` when empty, `Some(_)` when present
- **Variant-count fitness function** (per gaps critique #7) — a `compile_fail` doctest or `const _: () = assert!(...)` that pins:
  ```rust
  // Forces re-evaluation if anyone adds an Effect variant — the Effect enum
  // is the most-likely place for hidden god-state to creep in. Bump
  // deliberately when justified; don't auto-bump.
  const _: () = assert!(std::mem::variant_count::<ToastEffect>() <= 2);
  ```
- **Integration test** (per gaps critique #3) — small test that constructs a minimal MidasApp, fires `Message::Toast(ToastMsg::Show { ... })`, asserts toast appears, fires `Message::Toast(ToastMsg::ActionClicked)`, asserts the embedded message was dispatched. This covers the `dispatch_toast` translation path.

**Done when** (per gaps critique #3):
- `cargo test --workspace` green (currently 1205 passing — must remain green)
- `cargo clippy --workspace --all-targets --features dev_harness -- -D warnings` green
- `cargo doc --workspace --no-deps` builds (controller introduces `pub` types — all need `///` per project rule)
- Devloop fixture replay still works: `tools/devloop-smoke.sh` exits 0
- Saved `data/config.toml` is byte-identical (Toast is session-only state — no persistence change should leak)
- `MidasApp` struct: `toast: Option<ToastState>` field gone, `toasts: ToastController` field added (-1 +1 = 0 net, but the *type* is now opaque)
- `Message` enum: `ShowToast`, `DismissToast`, `ToastActionClicked` gone, `Toast(ToastMsg)` added (-3 +1 = -2 variants)
- `app/handlers.rs`: `handle_toast_msg` gone (~26 LOC)
- `app/views.rs`: `view_toast_overlay` gone (~70 LOC); replaced by 3-line `.map(Message::Toast)` wrapper at the call site
- LOC delta on `MidasApp` files (`app.rs` + `app/*.rs`): -120 ± 20
- New `toast/` module: target ≤480 LOC including tests (kill-criterion budget)

### Dependency summary

Slice 0 is independent. Outcomes feed into the slice-1 decision:
- If pattern feels ergonomic → next: `WindowGeometry` (4 OS-window fields, audit's #1 finding sub-piece) for 1-day mechanical practice
- If pattern feels heavy → kill criterion fires, document "decomposition pattern not adopted; trim Message variants in place instead"

## Risks & unknowns

- **Risk: `Box<Message>` back-reference is gross.** ToastController has to carry `Box<Message>` (or via `ToastAction`) to re-dispatch on ActionClicked. This is a back-edge from sub to parent type. Mitigation: it already exists today (`ToastAction.on_click: Box<Message>` lives in app.rs); we're not making it worse, just relocating. Future cleanup might introduce a generic `ActionToken<M>` if a second controller needs the same shape, but slice 0 doesn't.
- **Risk: every `ShowToast` emit site has to change.** Search-and-replace; find them all via `grep -rn "Message::ShowToast"`. Compiler catches misses.
- **Risk: `ToastMsg` shape becomes the new design tax for every emit site.** The new variant `Message::Toast(ToastMsg::Show { ... })` is more verbose than the old `Message::ShowToast { ... }`. Acceptable tax; mitigated by inline `impl From<ToastMsg> for Message` if it gets old.
- **Risk: integration test is the load-bearing one.** Pure unit tests don't catch wiring bugs in `dispatch_toast`. The integration test (single test on minimal `MidasApp`) is the canary. If we can't get a minimal MidasApp standing in tests, that itself is a finding (testability of the god struct).
- **Unknown: emit-site count.** `grep -c "Message::ShowToast"` will produce the actual blast-radius number. Plan assumes <20 sites; if it's >50, slice 0 grows beyond budget and the kill criterion gets closer to firing.

## Testing strategy

- **Pure unit tests**: 7–9 covering every `ToastMsg` variant + `tick()` boundary + `view()` empty/present.
- **Variant-count fitness**: `const _: () = assert!(variant_count::<ToastEffect>() <= 2);`
- **Integration test**: 1 test exercises `dispatch_toast` with `FireParentMsg` re-dispatch end-to-end.
- **Existing tests** (1205) re-run unchanged.
- **Manual smoke**: launch app, trigger any toast (e.g., GATR snap to fire `TickerEffect::Toast` path), confirm appears + auto-dismisses.

## Non-goals / Out of scope

- Splitting any other bucket. Slice 0 is exploratory; subsequent slices depend on outcome.
- Introducing `SharedServices`. Slice 0 doesn't need it.
- View-models (audit P1 #3). Independent.
- Collapsing `Message::Chart*` (audit P2 #4). Independent.
- Subscriptions per controller. Halloy proves centralized works; revisit if a slice genuinely needs it.
- `Arc<Mutex<_>>`. Not idiomatic in iced 0.14.
- Forcing slice 1 to follow the pattern. Slice 0 outcome decides.

## Review notes

The design critique correctly identified that this plan is more "trying the pattern" than "executing the pattern". That's deliberate — the cost of one slice we might revert is much lower than the cost of committing to a 12-slice refactor before learning whether the pattern fits. The kill criterion (Decision 5) is the explicit acknowledgment.

If slice 0 succeeds, the audit's TL;DR ranking (Watchlist → Account → …) holds. If it fails, the TL;DR has to be re-visited with a different decomposition strategy.

## Next steps

Three slices ahead, in order. Each gates the next.

### A. `SharedServices` struct (gates everything else)

**Goal**: a single `&mut SharedServices` borrow that controllers can take to read/write the app-wide stores they need, without re-deriving access policies per controller.

**Members** (start narrow; expand as a second controller demands a third member):
- `link_routing: &mut LinkBus` — symbol/timeframe link propagation
- `market_cache: &mut MarketDataCache` — price snapshots
- `annotation_store: &mut AnnotationStore` — order brackets + levels

**Construction**: `MidasApp::shared_services(&mut self) -> SharedServices<'_>` packs the borrows into a struct. The borrow checker enforces that no two controllers can write the same store simultaneously — the design intent.

**What it doesn't include**: `tickers`, `charts`, `workspace`. Those are owned by the controllers that *will* exist (TickerStore controller, Chart-list controller, Workspace controller); having the parent loan them out via `SharedServices` is exactly the god-pattern the split is supposed to break.

**Done when**: a stub controller (any new feature, even trivial) constructs `SharedServices`, calls one method on `link_routing` through it, and the change compiles. No production controller migrates yet.

### B. `Controller` trait + generic interpreter

**Goal**: kill the `consume_*_effects` boilerplate. Today every controller needs its own `dispatch_<name>` method on `MidasApp` that interprets `Vec<Effect>`. Twelve controllers = twelve dispatchers, each ~30 LOC of `match effect { … }`.

**Shape**:

```rust
pub trait Controller {
    type Msg;
    type Effect;
    fn update(&mut self, msg: Self::Msg, services: &mut SharedServices) -> Vec<Self::Effect>;
}

// On MidasApp:
fn dispatch<C: Controller, F>(&mut self, controller: C, msg: C::Msg, interpret: F)
where F: Fn(&mut Self, C::Effect) -> Task<Message>;
```

The trait gives controllers a uniform contract; the generic dispatcher handles the routing scaffold. Each effect type still needs its parent-side interpreter (the `interpret` closure), but the *dispatch loop* is written once.

**Done when**: Toast and WindowGeometry both implement `Controller`, share the generic dispatcher, and the per-controller `dispatch_toast`/`dispatch_window` methods on `MidasApp` collapse into trait calls. Existing tests stay green.

### C. Watchlist controller (the original gated slice)

With A + B in place, Watchlist becomes:
1. `WatchlistController` owns `watchlists: BTreeMap<WatchlistId, WatchlistPanel>` + `selected_symbol`-bridging logic.
2. `update(WatchlistMsg, &mut SharedServices) -> Vec<WatchlistEffect>` handles all 14 of the `self.*` reads the current view does, plus drag/drop's three cross-domain mutations (which now route through `services.link_routing` etc., not `Box<Message>`).
3. View consumes the existing `WatchlistBodyVm` (already shipped) — no refactor of the view function.
4. Parent's `consume_watchlist_effects` interprets `MovePane` / `RebindChart` / `RouteLink` against its own state.

**Kill criterion (carried over from Slice 0)**: ratio of (LOC added in controller + parent interpreter) ÷ (LOC removed from `MidasApp`) must be < 4×. The view-models work has already shifted ~150 LOC of inline projection out of the view; if the controller migration adds another 600 LOC of machinery to remove 150, the abstraction is paying its cost in the wrong direction and the slice should be reverted.

### Out of scope for this plan

- Account / Order / Chart-list / Workspace controllers — those are post-Watchlist slices, sized after we see what the actual cost of `SharedServices` + `Controller` looks like in production.
- Splitting the `Message` enum into per-controller `Msg` types globally. Only Watchlist's slice needs `WatchlistMsg`; the rest stay flat until proven otherwise.
