# Stage 02 — Protocol Layer

*Wire codec, framing, handshake, and the ~40 message types that make `rust-ibapi` a happy client.*

**Depends on**: 01 (crate scaffolding + module boundaries)
**Blocks**: 03, 04, 05, 06, 07, 09

## Scope

Implement the byte-exact TWS wire protocol for the message subset in [research/tws-wire-protocol.md](research/tws-wire-protocol.md). Everything above this layer (engine, market data, orders) consumes typed `IncomingMsg` / `OutgoingMsg` enums — no byte-wrangling at any higher level.

## Design shape

### Framing — `protocol/framing.rs`

Post-handshake messages are `[u32 BE length][payload]`. Payload is NUL-delimited ASCII fields.

Implement `tokio_util::codec::{Decoder, Encoder}`:

```rust
pub struct TwsCodec {
    state: CodecState, // PreHandshake | Framed
}

impl Decoder for TwsCodec {
    type Item = RawFrame;
    type Error = ProtocolError;
    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<RawFrame>, ProtocolError> { ... }
}

pub struct RawFrame {
    pub fields: Vec<Bytes>,  // NUL-delimited, already split
}
```

`RawFrame` is the lowest-level typed envelope. Higher-level `IncomingMsg::parse(RawFrame)` does the field-by-field decode per message type.

### Field codec — `protocol/messages/fields.rs`

The NUL-delimited field layer is uniform but has sentinel values and per-message versioning:

```rust
pub const UNSET_INT: i32 = i32::MAX;
pub const UNSET_LONG: i64 = i64::MAX;
pub const UNSET_DOUBLE: f64 = f64::MAX; // 1.7976931348623157E308

pub trait FieldRead {
    fn read_i32(&mut self) -> Result<i32, ProtocolError>;
    fn read_i64(&mut self) -> Result<i64, ProtocolError>;
    fn read_f64(&mut self) -> Result<f64, ProtocolError>;
    fn read_string(&mut self) -> Result<String, ProtocolError>;
    fn read_bool(&mut self) -> Result<bool, ProtocolError>;
    fn read_opt_f64(&mut self) -> Result<Option<f64>, ProtocolError>; // None if UNSET sentinel or empty
    fn read_opt_i32(&mut self) -> Result<Option<i32>, ProtocolError>;
    // ... symmetric write methods
}
```

Implement on `FieldReader<'a>` that wraps `&'a [Bytes]` with a cursor. Write side is a `Vec<u8>` that `push`es fields with trailing `\0`.

### Handshake — `protocol/handshake.rs`

```rust
pub async fn server_handshake<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    my_version: ServerVersion, // e.g. 176
) -> Result<NegotiatedSession, HandshakeError> {
    // 1. Expect exactly "API\0" (4 bytes, no length prefix)
    let mut prefix = [0u8; 4];
    stream.read_exact(&mut prefix).await?;
    if &prefix != b"API\0" { return Err(HandshakeError::BadPrefix); }

    // 2. Expect length-prefixed "v{min}..{max}"
    let frame = read_length_prefixed(stream).await?;
    let (client_min, client_max) = parse_version_range(&frame)?;

    // 3. Pick version — max(client_min, my_version.min()) .. min(client_max, my_version.max())
    let chosen = negotiate(client_min, client_max, my_version)?;

    // 4. Reply with "<version>\0<connection_time>\0"
    let now = chrono::Utc::now().format("%Y%m%d %H:%M:%S %Z").to_string();
    write_length_prefixed(stream, format!("{chosen}\0{now}\0").as_bytes()).await?;

    Ok(NegotiatedSession { version: chosen })
}
```

Version negotiation target: **advertise support for `176..221`**. `rust-ibapi`'s current `MIN_VERSION=201` means it sends `v201..221`; advertising up to 221 gives us forward headroom if the crate bumps its `MIN_VERSION` higher in the future without forcing an immediate sim release. Core text framing is unchanged across 201..221 — we never emit protobuf for any message. The per-message inner `version` fields are what actually gate feature presence, not the negotiated server version.

**Version-drift strategy**: CI has a scheduled job (weekly) that pulls the current `rust-ibapi` HEAD and runs `handshake_e2e` against our sim. If the job goes red, it means `rust-ibapi` bumped its `MIN_VERSION` or `MAX_VERSION` past our advertised range, or added a per-field requirement at a newer server version. Trigger: open an issue, widen the range in `ServerVersion::MIN/MAX`, patch any missing per-field gates by cross-referencing the `rust-ibapi` PR that bumped it.

### Message enum — `protocol/messages/{incoming,outgoing}.rs`

```rust
#[derive(Debug, Clone)]
pub enum IncomingMsg {
    StartApi { client_id: i32, optional_caps: Option<String> },
    ReqCurrentTime,
    ReqIds { num_ids: i32 },
    ReqContractData { req_id: ReqId, contract: ContractSpec },
    ReqMktData { req_id: ReqId, contract: ContractSpec, generic_ticks: String, snapshot: bool, regulatory_snapshot: bool, opts: Vec<TagValue> },
    CancelMktData { req_id: ReqId },
    PlaceOrder { order_id: OrderId, contract: ContractSpec, order: OrderSpec },
    CancelOrder { order_id: OrderId, manual_order_cancel_time: Option<String> },
    ReqOpenOrders,
    ReqAccountData { subscribe: bool, acct_code: String },
    ReqExecutions { req_id: ReqId, filter: ExecutionFilter },
    ReqHistoricalData { req_id: ReqId, contract: ContractSpec, end_date_time: String, duration: String, bar_size: String, what_to_show: String, use_rth: bool, format_date: i32, keep_up_to_date: bool, chart_opts: Vec<TagValue> },
    ReqRealTimeBars { req_id: ReqId, contract: ContractSpec, bar_size: i32, what_to_show: String, use_rth: bool, opts: Vec<TagValue> },
    ReqMarketDataType { data_type: MarketDataType },
    ReqPositions,
    ReqAccountSummary { req_id: ReqId, group: String, tags: String },
    ReqGlobalCancel,
}

#[derive(Debug, Clone)]
pub enum OutgoingMsg {
    ErrMsg { req_id: ReqId, code: i32, message: String, advanced_order_reject_json: Option<String> },
    ManagedAccts { accounts: String /* comma-sep */ },
    NextValidId { order_id: OrderId },
    CurrentTime { epoch_secs: i64 },
    TickPrice { req_id: ReqId, tick_type: TickType, price: f64, size: Option<i64>, attribs: TickAttribs },
    TickSize { req_id: ReqId, tick_type: TickType, size: i64 },
    TickString { req_id: ReqId, tick_type: TickType, value: String },
    TickGeneric { req_id: ReqId, tick_type: TickType, value: f64 },
    MarketDataType { req_id: ReqId, data_type: MarketDataType },
    ContractData { req_id: ReqId, details: ContractDetails },
    ContractDataEnd { req_id: ReqId },
    OpenOrder { order_id: OrderId, contract: ContractSpec, order: OrderSpec, order_state: OrderState },
    OpenOrderEnd,
    OrderStatus { order_id: OrderId, status: String, filled: f64, remaining: f64, avg_fill_price: f64, perm_id: i64, parent_id: OrderId, last_fill_price: f64, client_id: i32, why_held: String, mkt_cap_price: f64 },
    ExecutionData { req_id: ReqId, contract: ContractSpec, execution: Execution },
    ExecutionDataEnd { req_id: ReqId },
    CommissionReport { report: CommissionReport },
    AcctValue { key: String, value: String, currency: String, acct_code: String },
    PortfolioValue { contract: ContractSpec, position: f64, market_price: f64, market_value: f64, avg_cost: f64, unrealized_pnl: f64, realized_pnl: f64, acct_code: String },
    AcctDownloadEnd { acct_code: String },
    Position { acct_code: String, contract: ContractSpec, position: f64, avg_cost: f64 },
    PositionEnd,
    AccountSummary { req_id: ReqId, acct_code: String, tag: String, value: String, currency: String },
    HistoricalData { req_id: ReqId, start: String, end: String, bars: Vec<Bar> },
    RealTimeBar { req_id: ReqId, timestamp: i64, open: f64, high: f64, low: f64, close: f64, volume: i64, wap: f64, count: i32 },
}
```

### Parsing strategy

Each message implements:

```rust
impl IncomingMsg {
    pub fn parse(frame: RawFrame, server_version: ServerVersion) -> Result<IncomingMsg, ProtocolError> {
        let mut r = FieldReader::new(&frame.fields);
        let msg_id: i32 = r.read_i32()?;
        match msg_id {
            5 => Self::parse_place_order(&mut r, server_version),
            71 => Self::parse_start_api(&mut r, server_version),
            // ...
            _ => Err(ProtocolError::UnsupportedMsgId(msg_id)),
        }
    }
}
```

Per-message inner version gates field presence. Example for `PLACE_ORDER`:

```rust
fn parse_place_order(r: &mut FieldReader, sv: ServerVersion) -> Result<IncomingMsg, ProtocolError> {
    let inner_version = r.read_i32()?;
    let order_id = r.read_i32()?;
    // ... 60+ fields, some gated by inner_version OR server_version
    if sv >= ServerVersion::TRAILING_PERCENT {
        let trailing_percent = r.read_opt_f64()?;
    }
    // ...
}
```

The per-field version gates come from `rust-ibapi`'s source — we mirror them exactly.

## Test strategy

### Unit tests per message type

For each message, at least:

1. **Roundtrip**: build `IncomingMsg::Foo { ... }`, encode to bytes, decode, assert equal.
2. **Golden fixture**: captured bytes from `rust-ibapi`'s test suite, parse, assert the struct shape matches.
3. **Field-boundary**: trailing empty fields, unset sentinels, overflow numbers.

### Integration tests (tests/handshake_e2e.rs)

1. Client connects, sends `API\0` + version range, receives version + time.
2. Client sends `START_API(client_id=0)`, receives `MANAGED_ACCTS` + `NEXT_VALID_ID` + farm status bulletins.
3. Sim sends unsolicited 2104/2106/2158 bulletins.
4. `REQ_CURRENT_TIME` round-trip.

### Property tests (proptest)

- Arbitrary `IncomingMsg` → encode → decode → equal (modulo known lossy roundtrips)
- Arbitrary bytes → decode → encode → bytes equal (structured fuzzing)

### Fuzzing target (`cargo-fuzz`)

Wire codecs are a classic fuzzing target — parsers for binary protocols routinely hide memory-safety or state-corruption bugs that proptest won't surface because proptest generates structurally plausible inputs, not adversarial byte streams.

Add `fuzz/fuzz_targets/decode_frame.rs`:

```rust
#![no_main]
use libfuzzer_sys::fuzz_target;
use midas_ib_sim::protocol::{TwsCodec, ServerVersion};

fuzz_target!(|data: &[u8]| {
    let mut codec = TwsCodec::new(ServerVersion::V201);
    let mut buf = bytes::BytesMut::from(data);
    // decode should never panic, no matter what bytes arrive
    let _ = tokio_util::codec::Decoder::decode(&mut codec, &mut buf);
});
```

Run nightly in CI for ≥1 hour. Findings become regression tests in the proptest suite.

## Wire-byte fixture corpus

Ingest `rust-ibapi`'s test fixtures into `fixtures/wire/` for regression coverage. Verify our decoder accepts every byte sequence they use as input, and our encoder produces byte-for-byte matches of their expected outputs.

## Parallelism within this stage

Three sub-teams can work in parallel after the `FieldReader`/`FieldWriter` primitives land:

| Sub-team | Scope | LOC estimate |
|----------|-------|--------------|
| **Team A** (framing + handshake) | `framing.rs`, `handshake.rs`, `fields.rs`, `codec state machine` | ~800 |
| **Team B** (incoming messages) | `messages/incoming.rs` — 17 message parsers | ~1200 |
| **Team C** (outgoing messages) | `messages/outgoing.rs` — 24 message encoders | ~1400 |

Integration tests (`tests/handshake_e2e.rs`) land after all three merge.

## Rollback signals

- A single message parser exceeds 200 LOC → field layout is being hand-unrolled; introduce a declarative helper macro or accept the size.
- `ProtocolError` variants exceed 20 → error type is leaking parsing details; collapse into fewer structured variants.
- Incoming/outgoing enums gain cross-dependencies → move shared types to `messages/types.rs`.
- Handshake needs more than one state to represent → it's actually a multi-step flow, not a codec switch; formalize as a state machine.

## Kill criteria

- **Can't negotiate with `rust-ibapi` client after 1 week of work** → version mismatch or field layout bug; halt and review wire fixtures.
- **Byte-for-byte mismatch against recorded real-IB traffic in >5% of messages** → per-message versioning gates are wrong; halt and re-derive from `rust-ibapi` source.

## Deliverables

- `cargo test -p midas-ib-sim --test handshake_e2e` green — real `rust-ibapi::Client` connects to our sim
- `cargo test -p midas-ib-sim` runs all roundtrip + golden + proptest suites green
- 100% code coverage on `fields.rs` (it's the lowest layer; a bug here is expensive)
- Wire-byte corpus in `fixtures/wire/` — at least 50 captured messages across the subset
