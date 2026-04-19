# Stage 05 — Quirk Modeling

*IB's rate limits, line caps, farm-status broadcasts, and connection-lifecycle quirks. Without these, a client that tests green against our sim will hit error 100 and get disconnected in production.*

**Depends on**: 02 (protocol)
**Blocks**: 06 (scenarios depend on quirks being injectable), 09 (integration exposes quirk knobs)
**Parallel-safe with**: 03, 04, 07

## Scope

Implement the T1 quirks from [research/ib-quirks-and-limits.md](research/ib-quirks-and-limits.md) as **always-on** engine behavior. T2 quirks behind feature flags. T3 deferred.

## T1 quirks (always on)

### 1. 50 msg/sec rate limit → error 100 → disconnect

```rust
pub struct MsgRateLimiter {
    clock: Arc<dyn Clock>,
    tokens: TokenBucket, // refill 50/sec, capacity 50
    sessions: BTreeMap<SessionId, PerSessionRate>,
}

pub struct PerSessionRate {
    pub bucket: TokenBucket,
    pub violation_count: u32,
}

impl MsgRateLimiter {
    pub fn check(&mut self, session: SessionId) -> Result<(), QuirkViolation> {
        let per = self.sessions.entry(session).or_default();
        if !per.bucket.take() {
            per.violation_count += 1;
            return Err(QuirkViolation::RateLimit {
                code: 100,
                message: "Max rate of messages per second has been exceeded.".into(),
                action: ViolationAction::DisconnectAfterError,
            });
        }
        Ok(())
    }
}
```

- Token bucket: 50 tokens, refill 50/sec.
- **On violation**: emit `ErrMsg { code: 100, message: "Max rate ..." }` then close the session socket after a short delay (real IB closes ~50ms after the error).
- Burst tolerance matches real IB — the token bucket allows up to 50 messages in a single millisecond, then strictly 50/sec thereafter.

### 2. 100 streaming L1 line cap

```rust
pub struct LineLimiter {
    pub streaming_lines: BTreeMap<SessionId, BTreeSet<ReqId>>,
    pub max_lines_per_session: usize, // 100 default
}

impl LineLimiter {
    pub fn reserve(&mut self, session: SessionId, req_id: ReqId) -> Result<(), QuirkViolation> {
        let lines = self.streaming_lines.entry(session).or_default();
        if lines.len() >= self.max_lines_per_session {
            return Err(QuirkViolation::LineLimit {
                code: error_codes::LINE_CAP_OVERFLOW,
                message: error_codes::message(error_codes::LINE_CAP_OVERFLOW).into(),
                action: ViolationAction::RejectRequest,
            });
        }
        lines.insert(req_id);
        Ok(())
    }
    pub fn release(&mut self, session: SessionId, req_id: ReqId) { /* ... */ }
}
```

All error codes are routed through a single `error_codes.rs` table (see §Error code table below) so that when the pre-release capture session reveals the actual code, the change is a single constant edit, not a scatter of literal replacements across the codebase.

- **Snapshot mode exempt**: `reqMktData` with `snapshot=true` does not consume a line.
- **Tick-by-tick cap**: separate limiter for tick-by-tick subscriptions (5 simultaneous, 15s per-instrument cooldown).

### 3. Historical pacing (three independent regimes)

```rust
pub struct HistoricalPacing {
    pub per_session: BTreeMap<SessionId, HistoricalSessionState>,
}

pub struct HistoricalSessionState {
    pub window_60_10min: SlidingWindow<60, Duration>, // 60 requests per 10min
    pub burst_6_2sec: SlidingWindow<6, Duration>,     // 6 identical req in 2s
    pub identical_cooldown: BTreeMap<RequestKey, VirtualInstant>, // 15s cooldown
}

pub fn check(&mut self, session: SessionId, req: &HistoricalReq) -> Result<(), QuirkViolation> {
    let now = self.clock.now();
    let state = self.per_session.entry(session).or_default();
    let key = req.fingerprint(); // (contract, exchange, tick_type, bar_size)

    // Cost: BID_ASK counts double
    let cost = if req.what_to_show == "BID_ASK" { 2 } else { 1 };

    // Regime 1: 60 per 10min
    if state.window_60_10min.count_since(now - Duration::from_secs(600)) + cost > 60 {
        return pacing_violation(162, "Historical data pacing violation");
    }

    // Regime 2: 6 identical in 2s
    let identical_in_2s = state.window_60_10min.iter()
        .filter(|e| e.key == key && e.ts > now - Duration::from_secs(2))
        .count();
    if identical_in_2s >= 6 {
        return pacing_violation(162, "Historical data pacing violation (burst)");
    }

    // Regime 3: 15s cooldown on identical
    if let Some(last_ts) = state.identical_cooldown.get(&key) {
        if now - *last_ts < Duration::from_secs(15) {
            return pacing_violation(162, "Historical data pacing violation (cooldown)");
        }
    }

    state.window_60_10min.record(now, key.clone(), cost);
    state.identical_cooldown.insert(key, now);
    Ok(())
}
```

### 4. Farm-status bulletins

On session startup, emit:

```
err(-1, 2104, "Market data farm connection is OK:usfarm")
err(-1, 2106, "HMDS data farm connection is OK:ushmds")
err(-1, 2158, "Sec-def data farm connection is OK:secdefil")
```

**Client feature gating depends on these** — some libraries (ib_insync) won't fire `connectedEvent` until they arrive. The sim emits them unsolicited ~100ms after `START_API`.

### 5. Connection lifecycle (1100/1101/1102)

```rust
pub enum ConnEvent {
    FarmLost,      // → emit 1100 "Connectivity between IB and TWS has been lost"
    FarmRestoredNoData, // → emit 1101 "Connectivity restored, data lost" (client re-subscribes)
    FarmRestoredData,   // → emit 1102 "Connectivity restored" (client does not re-subscribe)
}
```

These are only emitted via scenario injection (not spontaneously). The handler re-drops all active market data subscriptions on 1101 — forcing the client to re-request to actually receive ticks.

### 6. `execDetails` / `orderStatus` / `commissionReport` non-atomic ordering

Implemented in Stage 04. Listed here for completeness as a T1 quirk.

### 7. Daily restart disconnect

Simulated as a scheduled `ConnEvent::DailyRestart` event at 11:45 PM ET virtual time. Closes all sessions with ErrMsg code 1300 (or similar informational). Clients must reconnect; orders survive (parked in engine state); market-data subs drop.

## T2 quirks (feature-flagged)

### 8. Duplicate OrderStatus emissions

Some real-IB paths emit the same `OrderStatus(Submitted, filled=0, remaining=100)` twice. Behind flag `quirks.duplicate_order_status_rate = 0.05` (5% of status changes).

### 9. `reqMarketDataType` live / frozen / delayed / delayed-frozen

```rust
pub enum MarketDataType {
    Live = 1,
    Frozen = 2,
    Delayed = 3,
    DelayedFrozen = 4,
}
```

- **Live**: normal tick stream
- **Frozen**: last-seen snapshot, no updates (simulates off-hours)
- **Delayed**: 15-min lagged tick stream (subtract 15 min from virtual time)
- **Delayed-frozen**: delayed + no updates

### 10. BID_ASK historical double-counts

Implemented above in the pacing limiter.

### 11. `reqPositions` initial snapshot + `positionEnd`

Implemented in Stage 04 account handler.

### 12. Contract qualification latency

`ReqContractData` doesn't respond immediately — emit `ContractData` + `ContractDataEnd` after a jittered delay (50–200ms). Feature flag: `quirks.contract_details_latency = true`.

### 13. 2103/2105/2108 farm cycling

Every ~30 minutes virtual time, cycle a farm down and back up:

```
2103 "Market data farm connection is broken:usfarm"
[short delay]
2104 "Market data farm connection is OK:usfarm"
```

Tests that the client correctly pauses data requests during outages.

### 14. Tick-by-tick 5-symbol cap + per-instrument 15s cooldown

Separate `TickByTickLimiter` — same pattern as line limiter but with stricter caps.

### 15. Snapshot mode free budget

Snapshot requests counted separately with a budget of 100 free per virtual day. After 100, charge $0.01 each (emit warning but allow). Tracked in `SessionMetrics`.

## T3 quirks (deferred)

- Saturday weekly reset with manual 2FA requirement
- FA/FA2 multi-account fan-out
- OER soft warnings
- Conditional orders (price/time/volume/margin triggers)
- Timestamp `formatDate` mode switching
- Extended-hours tick attributes

These aren't modeled. If a test needs them, it must use real IB paper.

## Quirk configuration

YAML config (loaded from CLI flag or scenario file):

```yaml
quirks:
  msg_rate:
    limit_per_sec: 50
    violation_action: disconnect
  line_limit:
    max_l1_lines: 100
    max_tbt: 5
    tbt_cooldown_sec: 15
  historical_pacing:
    window_60_10min: 60
    burst_6_2sec: 6
    identical_cooldown_sec: 15
    bidask_double_count: true
  farm_status:
    emit_on_connect: [2104, 2106, 2158]
    periodic_cycling: true
  fills:
    duplicate_order_status_rate: 0.0
    fill_pattern_distribution: {clean: 0.4, fast_market: 0.5, partial_drift: 0.1}
  contract_latency_ms: [50, 200]
```

Defaults = T1 only. T2 opt-in per flag.

## Parallelism within this stage

| Sub-team | Scope | LOC |
|----------|-------|-----|
| **A** | Msg rate limiter + token bucket + disconnect path | ~250 |
| **B** | Line limiter (L1 + tick-by-tick) | ~250 |
| **C** | Historical pacing (3 regimes, sliding windows) | ~400 |
| **D** | Farm status emitter + connection lifecycle events | ~300 |
| **E** | Config loader + feature flags + metrics | ~200 |

All can develop in parallel once the `QuirkGuard` trait + error types land (~half day).

## Testing

- Per-quirk unit tests with virtual clock
- Integration tests: scenario scripts that trigger each T1 quirk and verify client-observable behavior (error codes, disconnects)
- Golden fixtures: captured real-IB sessions where quirks fired, replay and compare

## Rollback signals

- A quirk requires knowing engine internals (peek at order state from the rate limiter) → quirk is not a pure guard; refactor so the engine calls the guard at boundaries.
- Config schema balloons past 30 keys → move less-used flags to scenario YAML.
- Sliding window data structures consume > 1 MB per session → use approximate counting.

## Kill criteria

- **Rate limiter adds > 10µs per message to the hot path** → profile and replace with a simpler ring buffer; we're measuring 50/sec, not 50,000,000.
- **Farm cycling causes flaky tests** → cycling must only fire when explicitly scripted, never spontaneously in tests.

## Error code table (`quirks/error_codes.rs`)

Every error code the sim emits is declared in one file:

```rust
// Pre-release verified from real IB paper captures (see validation gate below).
// Each constant has a // VERIFIED or // [unverified] comment; the latter
// must be resolved before the T1 quirks ship.

pub const MSG_RATE_EXCEEDED: i32 = 100;            // VERIFIED
pub const DUPLICATE_ORDER_ID: i32 = 103;           // VERIFIED
pub const CANT_MODIFY_FILLED: i32 = 104;           // VERIFIED
pub const PRICE_NOT_MIN_TICK: i32 = 110;           // VERIFIED
pub const HISTORICAL_PACING: i32 = 162;            // [unverified]
pub const NO_SECURITY_DEF: i32 = 200;              // VERIFIED
pub const ORDER_REJECTED: i32 = 201;               // VERIFIED
pub const ORDER_CANCELLED: i32 = 202;              // VERIFIED
pub const CLIENT_ID_IN_USE: i32 = 326;             // VERIFIED
pub const MD_NOT_SUBSCRIBED: i32 = 354;            // VERIFIED
pub const ORDER_NOT_YET_TRANSMITTED: i32 = 10147;  // VERIFIED
pub const LINE_CAP_OVERFLOW: i32 = 10197;          // [unverified]
pub const FARM_LOST: i32 = 1100;                   // VERIFIED
pub const FARM_RESTORED_NO_DATA: i32 = 1101;       // VERIFIED
pub const FARM_RESTORED_DATA: i32 = 1102;          // VERIFIED
pub const TWS_DAILY_RESTART: i32 = 1300;           // [unverified]
pub const MD_FARM_OK_USFARM: i32 = 2104;           // VERIFIED
pub const HMDS_FARM_OK_USHMDS: i32 = 2106;         // VERIFIED
pub const SEC_DEF_FARM_OK: i32 = 2158;             // VERIFIED

pub fn message(code: i32) -> &'static str {
    match code {
        MSG_RATE_EXCEEDED => "Max rate of messages per second has been exceeded.",
        LINE_CAP_OVERFLOW => "Max number of tickers has been reached",
        HISTORICAL_PACING => "Historical Market Data Service error message: Historical data request pacing violation",
        MD_FARM_OK_USFARM => "Market data farm connection is OK:usfarm",
        HMDS_FARM_OK_USHMDS => "HMDS data farm connection is OK:ushmds",
        SEC_DEF_FARM_OK => "Sec-def data farm connection is OK:secdefil",
        // ...
        _ => "Unknown error code",
    }
}
```

All sim-emitted errors go through these constants, never bare numeric literals. This makes the pre-release verification a single-file diff: the engineer running the capture session updates comments from `[unverified]` to `VERIFIED` (and corrects values if wrong), and every call site gets the fix for free.

## Real-IB error code validation (pre-release gate)

Several error codes are `[unverified]` from the research phase (notably `10197` for line-cap overflow and `162` for historical pacing violation). Before marking T1 quirks "done," each `[unverified]` code must be captured from a real IB paper session:

| Quirk | Code (planned) | Verification |
|-------|----------------|--------------|
| Line-cap overflow | 10197 | Subscribe 101 symbols against real IB paper; capture the actual error frame; record to `fixtures/wire/error_codes/line_cap_overflow.bin` |
| Historical pacing | 162 | Fire 61 historical requests within 10min against real IB paper; capture; record |
| Msg-rate violation | 100 | Spam 51 requests in 1 second against real IB paper; capture; record |
| Client-id conflict | 326 | Two clients with same ID; capture; record |
| Market data not subscribed | 354 | Request unsubscribed MD; capture; record |

Each capture becomes a wire fixture. The sim's emission is diffed against it byte-for-byte in `quirks_e2e`. Any discrepancy (wrong code, wrong message text, different `orderId` convention) is flagged before sim ships.

**Process**: one engineer spends a ~half-day session on a paper account triggering each quirk and capturing via Stage 07's proxy mode. Fixtures land in git; anyone can re-validate by replaying them.

## Deliverables

- All T1 quirks implemented + tested
- T2 quirks behind feature flags, opt-in
- `cargo test -p midas-ib-sim --test quirks_e2e` green — every T1 quirk has an E2E test that confirms client-observable behavior matches real IB
- Real-IB error-code captures in `fixtures/wire/error_codes/` (pre-release gate above)
- `docs/quirks.md` — operator reference listing every modeled quirk with the verified error code and trigger
