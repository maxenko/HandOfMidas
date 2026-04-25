# Group B — IB Pre-Flight

Three concrete bugs in code that runs against the IB sim today. They are present today (the sim's deterministic ordering hides them) and will surface against real IB. Fix before Phase-1 paper trading starts.

Three slices, all independent.

---

## Slice B1 — Gate `Ready` on first `FarmStatus::Ok(MKT)`

**Goal:** Honour the `MarketDataSource` trait contract that says `Ready` means "connected AND all farms up AND `nextValidId` received". Today the third clause is enforced; the second is not.

**Depends on:** None.

### Files to modify

- `crates/midas-broker/src/ib/market_data.rs` — `connect()` body at line 109-143
- `crates/midas-broker-core/src/provider.rs:154-158` — trait doc-comment update reflecting the gate
- `crates/midas-broker/tests/ib_sim_isomorphism.rs` — replace the existing TODO stub with a real test

### Current code (verbatim, `ib/market_data.rs:109-143`)

```rust
pub async fn connect(&self) -> Result<(), MarketDataError> {
    let _ = self.conn_state_tx.send(ConnectionState::Connecting);
    // ... (ibapi::Client::connect, store client) ...
    let _ = self
        .conn_state_tx
        .send(ConnectionState::Connected { server_version });
    if let Some(c) = self.client.read().await.clone() {
        let id_fut = async { c.next_valid_order_id().await ... };
        if let Ok(id) = with_ib_timeout(ib_timeout, "next_valid_order_id", id_fut).await {
            let _ = self.ordering_ready_tx.send(Some(id));
        }
        // BUG — sends Ready before farm-up
        let _ = self.conn_state_tx.send(ConnectionState::Ready);
    }
    Ok(())
}
```

### Target shape

The actual types (verified against `crates/midas-broker-core/src/market_data/farm.rs`):

```rust
pub struct FarmStatus { pub code: FarmCode, pub connected: bool, pub detail: String }
pub enum FarmCode { MarketDataFarmOk, MarketDataFarmInactive, MarketDataFarmBroken, ... }
```

The "MKT farm up" event is `FarmStatus { code: FarmCode::MarketDataFarmOk, connected: true, .. }`.

**Subscription ordering is critical.** `farm_status_tx.subscribe()` only receives messages sent AFTER the subscription. If the gateway emits the farm-up while we're awaiting `next_valid_order_id`, we miss it and deadlock until the timeout fires. Subscribe BEFORE the `next_valid_order_id` call:

```rust
// Subscribe BEFORE next_valid_order_id so we can't miss a fast farm-up.
let mut farm_rx = self.farm_status_tx.subscribe();

if let Some(c) = self.client.read().await.clone() {
    let id_fut = async { c.next_valid_order_id().await ... };
    if let Ok(id) = with_ib_timeout(ib_timeout, "next_valid_order_id", id_fut).await {
        let _ = self.ordering_ready_tx.send(Some(id));

        // Gate Ready on first MKT farm-up — trait contract M-23.
        let farm_up_fut = async {
            loop {
                match farm_rx.recv().await {
                    Ok(s) if s.code == FarmCode::MarketDataFarmOk && s.connected => {
                        return Ok(());
                    }
                    Ok(_) => continue,    // ignore other farm transitions
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => {
                        return Err(MarketDataError::Other(
                            "farm-status channel closed before MKT farm-up".into(),
                        ));
                    }
                }
            }
        };
        with_ib_timeout(ib_timeout, "farm_up_mkt", farm_up_fut).await?;
        let _ = self.conn_state_tx.send(ConnectionState::Ready);
    }
}
```

### Key implementation details

- **Subscribe BEFORE `next_valid_order_id`.** As noted above, subscribing after means a fast gateway can emit farm-up between `next_valid_order_id` returning and the loop arming, and we'd hang until timeout. The subscription receiver buffers up to its capacity (per `tokio::sync::broadcast` semantics) so the inverse race — subscribe first, then drain backlog after `next_valid_order_id` — is safe.
- **Handle `Lagged` non-fatally.** A slow consumer can lag the broadcast; don't conflate that with a real disconnect. Drop the lag, keep looping. Real connection loss surfaces as `Closed`.
- **Match on `code` field, NOT enum-variant pattern.** `FarmStatus` is a struct, not an enum — `matches!(ev, FarmStatus::Ok(...))` would not compile. Use field equality.
- **Timeout policy.** Reuse `ib_timeout`. If the farm-up never arrives, the connect call returns the timeout error — caller observes a failed connect, not a stuck `Connecting`.
- **Don't mark `Ready` if `nextValidId` succeeded but farm-up timed out.** The current `if let Ok(id) = …` swallows the nextValidId error; farm-up follows the same pattern intentionally — if either fails, we stay in `Connected` and surface the error.

### Testing

- **New deterministic test in `ib_sim_isomorphism.rs`:**
  ```rust
  #[tokio::test(start_paused = true)]
  async fn ready_waits_for_farm_up_mkt() {
      // Sim configured to delay FarmStatus::Ok(Mkt) by 500ms.
      // Spawn IbMarketData::connect, observe ConnectionState transitions.
      // Assert: Connecting → Connected{...} → (no Ready for 500ms) → Ready.
      // The exact event-injection knob comes from midas-ib-sim's
      // existing test surface; the sim already publishes farm status
      // immediately on construction — extend it with a delay knob if missing.
  }
  ```
- **Existing IB tests** must continue to pass. The sim publishes farm-up nearly instantly so existing assertions on Ready state should not regress.

### Done when

- `cargo test -p midas-broker --test ib_sim_isomorphism` green, including the new test.
- `cargo test --workspace` (root) green.
- `cargo clippy --workspace -- -D warnings` clean.
- Trait doc on `MarketDataSource::connection_state` updated to reference the implementation guarantee (one-line doc-comment patch).

### Risks & mitigations

- **Risk:** The test relies on a sim feature (delayed farm-up emission) that doesn't exist yet. **Mitigation:** Check `crates/midas-ib-sim/src/` for an existing knob; if absent, the slice grows to ~80 LoC including the sim hook. Keep it `#[cfg(feature = "test_inject")]`-gated.
- **Risk:** Farm-up never arrives in some IB-Gateway configurations (paper account with no MKT farm subscription). **Mitigation:** This is the correct behaviour — reject Ready in that scenario, surface the timeout. Document the failure mode in the trait doc.

### Rollback signal

If the new test passes against the sim but real IB connects routinely time out at the farm-up gate, the gate is too strict — the real IB-gateway state may emit a different `FarmStatus` variant (e.g., `Inactive` for MKT when no subscription present). Loosen to "any `Ok(Mkt)` OR `Inactive(Mkt)` after `nextValidId`".

---

## Slice B2 — Router disconnect policy

**Goal:** Define and implement what `MarketDataRouter` does when the upstream `MarketDataSource` transitions to `Disconnected` mid-stream. Today the publisher silently exits and `SymbolHub`s linger.

**Depends on:** None.

### Files to modify

- `crates/midas-market-data/src/router/publisher.rs` — `RecvError::Closed` arm at line 76-82
- `crates/midas-market-data/src/router/actor.rs` — control-actor message loop; add `UpstreamClosed { symbol }` handler
- `crates/midas-market-data/src/router/state.rs` — drop hub from `per_symbol` map cleanly
- `crates/midas-market-data/src/lib.rs` — module-level doc-comment describing the policy
- `crates/midas-broker-core/src/market_data/event.rs` — `EndReason::Disconnected` already exists per research; no type changes
- `crates/midas-market-data/tests/router_behavior.rs` — new test exercising disconnect

### Policy (per design decision D3 in `00-index.md`)

**Scope clarification:** This slice handles **full upstream close** (`broadcast::error::RecvError::Closed` — the upstream sender was dropped). It does NOT handle farm transitions (`FarmStatus { code: MarketDataFarmInactive, .. }`). Farm transitions remain a side-channel signal on the existing `farm_status_tx` broadcast — consumers can subscribe for data-quality awareness without the router tearing anything down. Tearing down on every farm blip would cause massive resubscription churn during routine IB-gateway hiccups.

When the publisher observes `RecvError::Closed` on its upstream stream:

1. The publisher's last action is to send a control-actor message `UpstreamClosed { symbol, reason }` (new variant). The `reason` is `EndReason::Disconnected` for an unexpected close; future variants can distinguish other close causes.
2. The control actor handler:
   a. Drops the hub from `state.per_symbol`.
   b. Before the hub is dropped, the actor publishes the appropriate `EndReason` event on each of the hub's broadcast lanes (ticks, RT-bars, quote-watch). Receivers observe it as the last message before the channel closes.
   c. Emits a structured warn log:
      ```rust
      tracing::warn!(
          target: "midas_market_data::router",
          symbol = %hub.symbol,
          subscriber_count = hub.refcount(),
          hub_uptime_ms = hub.uptime().as_millis(),
          reason = ?reason,
          "upstream closed; tearing down hub"
      );
      ```
      (`subscriber_count` and `hub_uptime` accessors may need to be added; one-line each.)
3. Consumers (chart widget, watchlist, `TickerState`) observe the `EndReason::Disconnected` event and decide whether to re-subscribe — that decision is NOT in the router's scope.
4. On reconnect, the next subscription request from any consumer triggers a fresh `subscribe_quotes` upstream and a fresh hub spawn — the existing first-subscription path handles this without any change. Emit `tracing::info!(target: "midas_market_data::router", symbol = %s, "upstream reopened; new hub");` for diagnostic symmetry.

### Key implementation details

- **`EndReason::Disconnected` already exists** at `crates/midas-broker-core/src/market_data/event.rs:108-111` (verified by research). No type changes; we just start emitting it.
- **The `UpstreamClosed` control message is small.** Single field, one handler arm. Avoid the temptation to add `UpstreamLagged` or other variants in the same slice — scope creep.
- **The publisher does not need to wait for the actor to acknowledge.** Send-and-forget; the actor is single-threaded and will process the message next tick.
- **`SubscriptionHandle` refcounts behave identically.** Dropping the hub triggers `Guard::drop` chains as today; the new code path is additive, not a replacement.

### Testing

- **New test in `router_behavior.rs`:**
  ```rust
  #[tokio::test(start_paused = true)]
  async fn upstream_disconnect_emits_end_reason_then_closes() {
      // Set up a router with a fake MarketDataSource whose
      // subscribe_quotes returns a stream we control.
      // Subscribe two consumers (different SymbolKey paths if available).
      // Drop the upstream stream; observe each consumer receives
      // an EndReason::Disconnected event, then the channel closes.
      // Assert: state.per_symbol no longer contains the hub.
      // Assert: a follow-up subscribe re-spawns a fresh hub (verify
      // by injecting a tick post-reconnect).
  }
  ```
- **No regression** in existing `router_behavior.rs` happy-path or refcount tests.

### Done when

- `cargo test -p midas-market-data` green, including the new test.
- `cargo test --workspace` (root) green.
- `cargo clippy --workspace -- -D warnings` clean.
- `lib.rs` carries a 5-10 line doc comment "Disconnect policy" section describing the tear-down behaviour.

### Risks & mitigations

- **Risk:** Emitting `EndReason::Disconnected` on a closed broadcast channel panics. **Mitigation:** `tokio::sync::broadcast::Sender::send` returns `Err` on no-receivers (does not panic). Use `let _ = …`.
- **Risk:** A consumer holds a `SubscriptionHandle` AND a separate clone of the broadcast `Receiver` directly (bypassing the handle's `recv` method). **Mitigation:** This is the existing API contract — `SubscriptionHandle::recv` is the public surface. If any internal code holds a raw receiver, it observes `Closed` exactly as before. The `Disconnected` event flows through the existing `recv` path.
- **Risk:** Test flake if the disconnect happens during the subscribe path's setup. **Mitigation:** Test uses `start_paused = true` and explicitly advances time between steps.

### Rollback signal

If the new test passes but a real disconnect leaks `SymbolHub`s in production (memory grows), there's a missed code path — likely a control-actor message dropped under backpressure. Investigate the actor's mailbox depth before adjusting.

---

## Slice B3 — Defence-in-depth live-trading guard

**Goal:** Catch programmatic `IbMarketDataConfig` mutation that bypasses TOML validation. Today the only live-port check is in `BrokerConfig::validate()`, which is only called during file-load.

**Depends on:** None.

### Files to modify

- `crates/midas-broker/src/ib/config.rs` — add `pub allow_live: bool` field to `IbMarketDataConfig` (default `false`)
- `crates/midas-broker/src/config.rs` — `BrokerConfig` already has `allow_live`; thread it into `IbMarketDataConfig` at the construction site (find via grep; likely a `From<&BrokerConfig>` impl or inline construction in adapter setup)
- `crates/midas-broker/src/ib/market_data.rs` — at the top of `connect()`, before any I/O, add the guard
- `crates/midas-broker/src/ib/market_data.rs` (or `ib/config.rs`) — new test asserting the guard

### Target code (top of `connect()`)

```rust
pub async fn connect(&self) -> Result<(), MarketDataError> {
    if self.config.port == 4001 && !self.config.allow_live {
        return Err(MarketDataError::LiveTradingNotConfirmed);
    }
    let _ = self.conn_state_tx.send(ConnectionState::Connecting);
    // ... rest unchanged ...
}
```

### Key implementation details

- **Add a typed error variant, not a `String`.** `MarketDataError::LiveTradingNotConfirmed` (no payload) lives in `crates/midas-broker-core/src/market_data/error.rs`. This is one of the rare cases where adding a variant is justified — the call site is going to switch on it (UI may want to show "set allow_live=true" guidance).
- **Don't `assert!`/`panic!`.** A misconfigured port is a recoverable misconfiguration, not a programmer error. Return the typed error.
- **`allow_live` field on `IbMarketDataConfig` defaults to `false`.** Programmatic construction with `..Default::default()` (e.g., the existing `paper(client_id)` constructor) carries the safe default. The TOML path explicitly sets it from `BrokerConfig::connection.allow_live`.
- **Existing TOML guard stays.** Defence-in-depth means both layers fire on misconfig — TOML rejects at load, adapter rejects at connect. They are not redundant; they catch different attack/mistake surfaces.

### Testing

- **Direct-construction test:**
  ```rust
  #[tokio::test]
  async fn connect_refuses_live_port_without_allow_live() {
      let cfg = IbMarketDataConfig {
          port: 4001,
          allow_live: false,
          ..IbMarketDataConfig::paper(7)
      };
      let mkt = IbMarketData::new(cfg);
      let err = mkt.connect().await.unwrap_err();
      assert!(matches!(err, MarketDataError::LiveTradingNotConfirmed));
  }
  ```
- **Programmatic-mutation test:**
  ```rust
  #[tokio::test]
  async fn connect_refuses_post_construction_port_swap() {
      let mut cfg = IbMarketDataConfig::paper(7);   // port 7497, safe
      cfg.port = 4001;                              // simulate programmatic mistake
      let mkt = IbMarketData::new(cfg);
      assert!(matches!(mkt.connect().await.unwrap_err(),
                        MarketDataError::LiveTradingNotConfirmed));
  }
  ```
- **Allow-live-true smoke:** confirm `IbMarketDataConfig { port: 4001, allow_live: true, .. }` does NOT trigger the guard and proceeds (the connect itself will fail without a real IB-gateway, but it should fail with a different error — connection refused, not the guard).
- **Existing TOML-load test** at `config.rs:278-297` keeps passing.

### Done when

- `cargo test -p midas-broker` green, including 3 new tests.
- `cargo test --workspace` (root) green.
- `cargo clippy --workspace -- -D warnings` clean.
- `MarketDataError::LiveTradingNotConfirmed` has a `Display` impl with actionable text (mentions setting `allow_live = true`).

### Risks & mitigations

- **Risk:** Adding a new `MarketDataError` variant breaks downstream `match` exhaustiveness. **Mitigation:** Search for `match … MarketDataError::` across both workspaces; add the new arm or use `_ => …` if the existing match is already non-exhaustive.
- **Risk:** TOML deserialization for `IbMarketDataConfig` doesn't carry `allow_live` because it's split from `BrokerConfig`. **Mitigation:** Verify the construction site (find via `IbMarketDataConfig {` grep). If `IbMarketDataConfig` is built from `BrokerConfig` at adapter startup, copy `connection.allow_live` over there. The field on `IbMarketDataConfig` itself does not need to be `Deserialize` — it's an internal post-construction value.

### Rollback signal

If a legitimate test trips the guard (port 4001 but `allow_live = false`), the test's fixture is wrong — fix the fixture by setting `allow_live = true` on the test's BrokerConfig OR change its port to 7497 (paper). **Do NOT revert the guard** — that would re-open the security hole this slice exists to close. Rollback for a defence-in-depth addition is "fix the call site", not "remove the check".
