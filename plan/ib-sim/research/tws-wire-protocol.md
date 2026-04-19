# Research: TWS Wire Protocol

*Source: parallel research agent, 2026-04-18.*

## TL;DR

- **Protocol is text framed, NUL-delimited, length-prefixed**. Post-handshake: `[u32 BE length][NUL-separated ASCII fields]`. Newer server versions (v201+) add protobuf for specific message types keyed by `msg_id > 200`.
- **Handshake is trivial**: client sends literal bytes `API\0` then a length-prefixed ASCII version range `v{min}..{max}` (e.g. `v100..176`). Server replies with chosen version + timestamp. Client sends `START_API` with client ID.
- **No production-grade open-source IB simulator exists** in any language. Building one has real value.
- **Minimum viable subset is ~25 message types** (10 out, 15 in) for shallow parity; **~40 types** for deep parity (covers historical, positions, executions, brackets — what Hand of Midas actually touches).
- **Advertise `v176..201`, implement text framing only**. Avoids protobuf entirely. Note: `rust-ibapi` now requires `MIN_VERSION>=201`; we advertise a range that overlaps but speak text — it's unchanged for core messages at v201.

## Minimum viable message set (deep parity)

**Outgoing (client → sim)** from [rust-ibapi messages.rs](https://github.com/wboayue/rust-ibapi) + [tws_c_api twsapi.h](https://github.com/GerHobbelt/tws_c_api/blob/master/twsapi.h):

| ID | Name |
|----|------|
| 1 | REQ_MKT_DATA |
| 2 | CANCEL_MKT_DATA |
| 3 | PLACE_ORDER |
| 4 | CANCEL_ORDER |
| 5 | REQ_OPEN_ORDERS |
| 6 | REQ_ACCOUNT_DATA |
| 7 | REQ_EXECUTIONS |
| 8 | REQ_IDS (NEXT_VALID_ID) |
| 9 | REQ_CONTRACT_DATA |
| 20 | REQ_HISTORICAL_DATA |
| 49 | REQ_CURRENT_TIME |
| 50 | REQ_REAL_TIME_BARS |
| 58 | REQ_GLOBAL_CANCEL (optional) |
| 59 | REQ_MARKET_DATA_TYPE |
| 61 | REQ_POSITIONS |
| 62 | REQ_ACCOUNT_SUMMARY |
| 71 | START_API |

**Incoming (sim → client)**:

| ID | Name |
|----|------|
| 1 | TICK_PRICE |
| 2 | TICK_SIZE |
| 3 | ORDER_STATUS |
| 4 | ERR_MSG |
| 5 | OPEN_ORDER |
| 6 | ACCT_VALUE |
| 7 | PORTFOLIO_VALUE |
| 9 | NEXT_VALID_ID |
| 10 | CONTRACT_DATA |
| 11 | EXECUTION_DATA |
| 15 | MANAGED_ACCTS |
| 17 | HISTORICAL_DATA |
| 45 | TICK_GENERIC |
| 46 | TICK_STRING |
| 49 | CURRENT_TIME |
| 50 | REAL_TIME_BARS |
| 52 | CONTRACT_DATA_END |
| 53 | OPEN_ORDER_END |
| 54 | ACCT_DOWNLOAD_END |
| 55 | EXECUTION_DATA_END |
| 58 | MARKET_DATA_TYPE |
| 59 | COMMISSION_REPORT |
| 61 | POSITION |
| 63 | ACCOUNT_SUMMARY |

**~17 out / ~24 in = ~41 types** for deep parity.

## Wire format + handshake essentials

1. Client opens TCP to 7497 (TWS paper) / 7496 (TWS live) / 4002 (Gateway paper) / 4001 (Gateway live).
2. Client sends raw bytes `API\0` (4 bytes, no length prefix).
3. Client sends length-prefixed ASCII `v{min}..{max}` — e.g. `\0\0\0\x09v100..176`. The `..` is literal.
4. Server replies with `<server_version>\0<connection_time_string>\0`.
5. Client sends `START_API` (id 71): `71\02\0<client_id>\0<optional_capabilities>\0`.
6. Server begins unsolicited messages: `MANAGED_ACCTS` (15), `NEXT_VALID_ID` (9), plus informational `ERR_MSG` codes 2104 ("Market data farm connection is OK"), 2106 ("HMDS data farm connection is OK"), 2158. **Clients treat these as health markers — sim MUST emit them.**

Every post-handshake frame: `[u32 BE length][payload]`. Payload is NUL-delimited ASCII. Outgoing messages start with `<msg_id>\0<version>\0...`; inner `version` is a per-message API version (1..N) the client picks based on negotiated server version. Unset sentinels: `2147483647` for int, `1.7976931348623157E308` for double, `9223372036854775807` for long.

## Version strategy

- **Pre-V100 (< 100)**: legacy, no `API\0` prefix. Skip.
- **V100+ (100–200)**: `API\0` handshake + length-prefixed NUL-delimited text. All messages are text.
- **V201+**: selected messages move to protobuf; payload becomes `<be_u32: msg_id + 200><protobuf bytes>`. Text still works for older message IDs.

**Recommendation: advertise range that overlaps 176 and 201, speak pure text.** `rust-ibapi`'s current `MIN_VERSION` ≥201, so the sim must advertise ≥201 to negotiate; text framing is unchanged for all our target messages at v201.

## Existing simulators

Nothing usable on the server side. Closest architectural reference: [pg-wire-mock](https://github.com/The-DevOps-Daily/pg-wire-mock) (PostgreSQL wire mock). `rust-ibapi` test fixtures are the only wire-byte corpus.

## rust-ibapi specifics

- Advertises `v{PROTOBUF}..{UPDATE_CONFIG}` = `v201..221`. Requires sim to negotiate ≥201.
- On `Client::connect()`: handshake → `START_API` → typically issues `REQ_IDS` + `REQ_CURRENT_TIME`.
- Encodes `UNSET_DOUBLE` / `UNSET_INTEGER` / `UNSET_LONG` sentinels exactly as IB does — parser must recognise them.
- Both sync and async clients speak the same wire protocol.

## Gotchas

1. **2104/2106/2158 "errors" aren't errors.** Client feature gating depends on emission.
2. **Per-message inner version field.** The outer `msg_id` is stable, but each message has its own internal version (often 6, 8, 12) that gates field presence. Wrong version = silent field misalignment.
3. **Order IDs must be monotonic and unique per client ID.** Clients cache them.
4. **`NEXT_VALID_ID` is the "ready" signal.** Many clients won't send `PLACE_ORDER` until they receive it. Emit unsolicited within ~50ms of `START_API`.
5. **Trailing empty fields.** Protocol tolerates/requires trailing NULs for optional fields. Off-by-one breaks parsers silently.
6. **Bulletins, account updates, positions require subscribe/ack handshake.** `REQ_ACCOUNT_UPDATES` (6) is stateful — only one subscription at a time per connection; must send `ACCT_DOWNLOAD_END` (54).
7. **Historical-data pacing violations.** Real IB rate-limits with error code 162.
8. **No authoritative protocol spec exists.** Only source of truth is the official Java/C++/Python client source.

## Recommendation

**Deep parity, ~40 messages, text framing only.** Build order:
1. Handshake + `START_API` + unsolicited `MANAGED_ACCTS` + `NEXT_VALID_ID` + 2104/2106 bulletins
2. `REQ_CURRENT_TIME` round-trip (smoke test)
3. `REQ_CONTRACT_DATA` (enables symbol resolution)
4. `REQ_MKT_DATA` → tick stream
5. `PLACE_ORDER` → `OPEN_ORDER` + `ORDER_STATUS` + synthetic fill → `EXECUTION_DATA` + `COMMISSION_REPORT`
6. `REQ_HISTORICAL_DATA`
7. Account/positions

## Key sources

- [tws-api/connection.html](https://interactivebrokers.github.io/tws-api/connection.html)
- [tws-api/message_codes.html](https://interactivebrokers.github.io/tws-api/message_codes.html)
- [rust-ibapi GitHub](https://github.com/wboayue/rust-ibapi)
- [tws_c_api header](https://github.com/GerHobbelt/tws_c_api/blob/master/twsapi.h) (message IDs)
- [ib_async docs](https://ib-api-reloaded.github.io/ib_async/) (cleanest field-level reference)
- [IBKR ProtoBuf Reference](https://www.interactivebrokers.com/campus/ibkr-api-page/protobuf-reference/) (for future v201+ protobuf)
- [TWS API 2025 Release Notes](https://www.ibkrguides.com/releasenotes/prod-2025.htm)
