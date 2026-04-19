# Research: IB API Quirks + Limits

*Source: parallel research agent, 2026-04-18. Anything uncertain tagged `[unverified]`.*

## TL;DR — Top 5 quirks that MUST be modeled

1. **`execDetails` can arrive before (or without) `orderStatus`.** Fast market fills frequently skip intermediate states. Officially documented. Simulators that emit clean Submitted → Filled are lying.
2. **50 msg/sec cap is enforced by disconnect**, not 429-style backoff. Exceed → error **100** + socket drop. Burst tolerance is tiny.
3. **Historical data is two independent pacing regimes** (60/10min *and* 6-in-2s per contract/exchange/ticktype *and* 15s identical-request cooldown), enforceable simultaneously. BID_ASK counts double.
4. **Daily restart** (~11:45 PM ET weekdays, plus Saturday night weekly reset) disconnects clients. Well-behaved clients must reconnect, re-subscribe market data, re-sync order state.
5. **Farm-status messages (1100/1101/1102, 2103-2108) arrive as `error()` callbacks with `orderId=-1`** and gate when market/historical requests are safe. 1101 means "reconnected but data was lost — re-subscribe."

---

## 1. Pacing rules and rate limits

| Rule | Value | Violation behavior |
|---|---|---|
| TWS API messages | **50/sec per client connection**, enforced | Error **100** + **connection dropped**. Hard limit. |
| Historical — window | **60 requests per rolling 10 min** | Error 162 `[unverified code, verified behavior]` |
| Historical — burst | **6 identical (contract/exchange/tickType) in 2 sec** | Same error |
| Historical — identical repeat | 15 sec cooldown | Same error |
| BID_ASK historical | **Counted 2x** against the 60/10min budget | — |
| Concurrent API connections | 32 per TWS/Gateway (clientId 0-31) | Error **326** "client id already in use" |

All budgets share the 50/s ceiling but historical has its own per-window caps on top.

**Paper vs live**: pacing rules identical. Paper fills are simulated and more optimistic; some order types (VWAP, RFQ, Pegged-to-Market, Auction) don't simulate.

## 2. Market data line limits

- **Default: 100 concurrent streaming L1 lines.** Formula: `MAX(commissions/8, (equity × 100)/1,000,000, 100)`, recomputed monthly.
- **Depth of book (L2)**: min 3, max 60 simultaneous.
- **Tick-by-tick (US)**: max **5** simultaneous. 1 request per instrument per 15 seconds.
- **Overflow**: error **10090/10197** `[unverified exact code]` — explicit, not silent.
- **Snapshot mode** (`reqMktData` with `snapshot=True`): does not consume a streaming line; $0.01/snapshot after 100 free/month.

## 3. Order event ordering quirks

**The critical quirk** (officially documented): *"There are not guaranteed to be orderStatus callbacks for every change in order status. For example with market orders when the order is accepted and executes immediately, there commonly will not be any corresponding orderStatus callbacks."* — [order_submission.html](https://interactivebrokers.github.io/tws-api/order_submission.html).

Concretely, sim must support:

- `execDetails` arriving **before any** `orderStatus`.
- `orderStatus(Submitted)` sometimes **never** arriving; `PreSubmitted` → first fill → `Filled` directly.
- Duplicate `orderStatus` messages for the same state `[unverified but widely reported]`.
- `commissionReport` arriving **after** `execDetails`, independent callback.
- `openOrder` always carries an `orderStatus` in the same message on initial reply, but subsequent status changes come standalone.

**Parent-child brackets**:
- Children enter `PreSubmitted` / `Inactive` while parent works. After parent fills, children → `Submitted`.
- Parent `Filled` and child state transition are **independent events, not atomically ordered**. Sim should emit with realistic small jitter.
- When one child fills, OCA cancels the other: `orderStatus(Cancelled)` on the remaining child.

## 4. Connection lifecycle quirks

- **No protocol-level heartbeat**. TWS detects dead clients through absence of data-stream activity — ~20s dropout detection window `[unverified]`. Clients without active subscriptions can linger longer.
- **Silent timeout** is the norm on network dropout; no explicit `goodbye`.
- **Reconnect semantics**: orders survive on IB side (permId stable). **Market data subscriptions do NOT** — must re-issue every `reqMktData`, `reqMktDepth`, `reqRealTimeBars`. Open orders re-enumerable via `reqOpenOrders`.
- **1100 → 1101 → 1102** is the official sequence:
  - 1100 "connectivity lost"
  - 1101 "restored, data lost (resubscribe)"
  - 1102 "restored, data maintained (no action)"
- **Daily restart**: ~11:45 PM ET weekdays. Weekly Saturday-night reset requires manual 2FA re-auth Sunday unless using IBC/IBeam.

## 5. Error codes worth modeling

| Code | Meaning | Handling |
|---|---|---|
| 100 | Max msg rate exceeded | Throttle. Reconnect follows. |
| 103 | Duplicate orderId | Bug — OrderId tracking broken. |
| 104 | Can't modify filled order | Race. Notify + refresh. |
| 110 | Price doesn't conform to min tick | Notify, retry snapped. |
| 162 | Historical pacing violation `[unverified code]` | Back off, retry with jitter. |
| 200 | No security definition | Notify + stop. |
| 201 | Order rejected | Notify + stop. |
| 202 | Order cancelled | Informational. |
| 326 | clientId in use | Connection management bug. |
| 354 | Not subscribed to market data | Fall back via `reqMarketDataType(3)`. |
| 10147 | OrderId not yet transmitted | Race. Notify + retry. |
| 1100/1101/1102 | Farm connectivity | Infrastructure — pause/resume/resubscribe. |
| 2103-2108, 2158 | Data farm status | Informational; gate data requests on 2104/2106. |

**Code ranges**:
- **100-500**: client/order errors (real errors).
- **1000s**: system/connectivity (informational via `error()`).
- **2000s**: warnings/farm status (`error()` with `orderId=-1`).
- **10000+**: advanced orders / algo.

Warnings and errors aren't separated by field — classification is by code range.

## 6. Time-related quirks

- **Historical bar timestamps**: "time zone chosen in TWS on the login screen" — Operator/Exchange/UTC. Format `yyyyMMdd  HH:mm:ss` (two spaces) with timezone suffix in newer versions; or epoch seconds if `formatDate=2`.
- **End datetime** format for `reqHistoricalData`: `yyyyMMdd-HH:mm:ss` (with dash). Empty = "now".
- **Duration strings**: `{n} S|D|W|M|Y`.
- **Bar alignment**: IB aligns to exchange clock boundaries for standard sizes. Sub-minute bars align to wall-clock `[unverified]`.

## 7. Account subscription quirks

- **`reqAccountUpdates` vs `reqPositions`**:
  - `reqAccountUpdates` — one account at a time, re-sent on change. Only one subscription.
  - `reqPositions` — all accounts interleaved, followed by `positionEnd` sentinel.
- **Initial snapshot on subscribe** for both. Incremental updates follow.
- **Max 2 `reqAccountSummary`** subscriptions concurrently.

---

## Fidelity tiers

**T1 — MUST model for any useful simulator:**
- 50 msg/sec → error 100 → disconnect
- `execDetails` without preceding `orderStatus` on fast market fills
- `orderStatus` not emitted for every state transition
- 1100/1101/1102 farm-status callbacks with `orderId=-1`
- Daily restart disconnect + market-data re-subscription requirement
- Historical pacing: 60/10min + 6-in-2s + 15s identical cooldown
- Error codes 100, 103, 200, 201, 202, 326, 354, 10147
- `commissionReport` arriving independently of (and after) `execDetails`
- Default 100 streaming L1 lines with overflow rejection

**T2 — for serious parity testing:**
- Bracket parent-fill / child-activation with non-atomic event ordering
- BID_ASK historical 2x counting
- Duplicate `orderStatus` messages for the same state
- `reqPositions` initial snapshot + `positionEnd` semantics
- `reqMarketDataType` live/frozen/delayed/delayed-frozen switching
- Contract qualification (`reqContractDetails`) latency + `contractDetailsEnd`
- 2103/2104/2105/2106/2108/2158 farm up/down cycling during session
- Snapshot-mode market data not consuming a line
- Tick-by-tick 5-symbol cap + 15s-per-instrument cooldown

**T3 — edge-case coverage:**
- Saturday weekly reset requiring Sunday manual re-auth
- FA/FA2 multi-account fan-out
- Order Efficiency Ratio (OER) soft warnings
- Conditional orders
- Timestamp format mode `formatDate=1` vs `=2` differences
- Extended-hours `outsideRth=True` fill behavior vs tick-attribute flags

## Sources

- [Historical Data Limitations](https://interactivebrokers.github.io/tws-api/historical_limitations.html)
- [Message Codes](https://interactivebrokers.github.io/tws-api/message_codes.html)
- [Placing Orders](https://interactivebrokers.github.io/tws-api/order_submission.html)
- [Connectivity](https://interactivebrokers.github.io/tws-api/connection.html)
- [ib_insync issue #469](https://github.com/erdewit/ib_insync/issues/469)
- Local: `D:\GitHub\HandOfMidas\research\provider-ib.md`, `plan\broker\01-architecture.md`
