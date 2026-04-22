# Live sim ticks + free bracket placement

Two independent fixes.

## Issue 1 — Live sim prices are static

### Root cause

`TestBrokerConfig::default()` has `tick_interval_ms: 0` (crates/midas-broker/src/test_broker/mod.rs:125). The broker engine polls every 10ms, but `poll_callbacks()` (mod.rs:1102) only emits ticks when `tick_interval_ms > 0`. `midas-app` uses `BrokerConfig::default()` unchanged (app.rs:1842), so the sim never ticks.

Even with ticks emitted, `get_or_seed_price_inner` (mod.rs:286) returns a frozen seed price — there is no random walk. So the fix has two parts.

### Changes

1. `crates/midas-broker/src/test_broker/mod.rs`
   - Add a tiny xorshift64 RNG field to `TestBrokerInner` (seeded from first subscribe timestamp).
   - In `poll_callbacks` tick block: draw a drift `~ U[-drift_bps, +drift_bps]` of the previous price, apply to `market_prices[symbol]`, and also call the same fill/trigger checks `set_market_price` runs (so limit/stop orders behave in the sim). Default drift: 10 bps / tick.
   - Add `tick_drift_bps: f64` to `TestBrokerConfig` (default 10.0).
   - Leave the `tick_interval_ms: 0` default alone — other tests rely on no auto-ticks.
2. `desktop/win/crates/midas-app/src/app.rs:1842`
   - Construct `BrokerConfig { test_broker: TestBrokerConfig { tick_interval_ms: 500, tick_drift_bps: 10.0, ..default() }, ..default() }` for the `Sim` branch.

### Risk

Only the desktop-app sim branch gets live ticks. Existing test-broker unit tests still see `tick_interval_ms: 0` and stay deterministic.

## Issue 2 — Brackets must not be clamped on drag/place; flag instead of block

### What to rip out

- `desktop/win/crates/midas-chart/src/interaction/mod.rs:1611-1635` — `clamp_bracket_leg_price` is the hard blocker during drag. Make it a pass-through (keep `snap_to_tick`).
- `desktop/win/crates/midas-chart/src/widget/bracket_tool/mod.rs:230-282` — `enforce_constraints` silently swaps TP/SL. Change to identity `(tp, sl)`. Remove the 6 swap/sort tests (they assert a behavior we are deleting).
- `desktop/win/crates/midas-app/src/order_panel/mod.rs:676-704` — `normalize_bracket` mirrors TP/SL on wrong side. Keep the `price <= 0 → None` degeneracy check; delete the mirror.

### What to add (classification + visuals)

The flag logic is already half-present: `OrderBracket.wrong_side_warning` (entry vs market). For TP/SL wrong-side we compute on the fly inside the decorator builders because chart has `bracket.side`, `entry.line.price`, and `leg.line.price` all in scope. No new `TickerState` field needed.

1. `desktop/win/crates/midas-chart/src/widget/order_bracket/decorators.rs`
   - In `tp_decorator_group` (line 307) and `sl_decorator_group` (line 424): compute `wrong_side = is_leg_on_wrong_side(side, leg_role, leg.price, entry.price)`. When true, tint the badge fill amber (`BRACKET_WARNING_COLOR`) and use a dashed line style for the leg line.
   - In `entry_decorator_group` (line 158): already takes `wrong_side_warning` — tint amber when set. (Today it is wired into state but not rendered; finish the wire.)
2. `desktop/win/crates/midas-chart/src/widget/order_bracket/mod.rs`
   - Add `const BRACKET_WARNING_COLOR: Color = ...` amber (same value the entry label plan referenced).
   - Add pure helper `fn is_leg_on_wrong_side(side: BracketSide, role: LegRole, leg_price: f64, entry_price: f64) -> bool`.

### Tests

- Update/remove `enforce_constraints` swap tests in `bracket_tool/tests.rs` lines 162-529 (6 tests expecting a swap — rewrite to assert pass-through).
- Update `normalize_bracket` callers' tests in `order_panel/tests.rs` if any lock in the mirror.
- Add: drag-leg test that pushes TP past entry on a Long, asserts price is accepted (no clamp), and asserts the decorator's `wrong_side` path is taken.
- Add: unit tests for `is_leg_on_wrong_side`.

### Files touched

- crates/midas-broker/src/test_broker/mod.rs
- desktop/win/crates/midas-app/src/app.rs
- desktop/win/crates/midas-chart/src/interaction/mod.rs
- desktop/win/crates/midas-chart/src/widget/bracket_tool/mod.rs
- desktop/win/crates/midas-chart/src/widget/bracket_tool/tests.rs
- desktop/win/crates/midas-chart/src/widget/order_bracket/mod.rs
- desktop/win/crates/midas-chart/src/widget/order_bracket/decorators.rs
- desktop/win/crates/midas-app/src/order_panel/mod.rs
