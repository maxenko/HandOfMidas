# Parity Gate — user action required

*The final acceptance step for the IB Simulator arc. Requires your IB paper-trading account + a working TWS / IB Gateway install.*

## Status

- ✅ **Implementation**: complete (Waves 0–4 all merged; 2091 tests passing; 0 Critical / 0 High scrutiny findings remaining)
- ⏸ **Parity gate**: waiting on you — no code remaining for Claude to write without real-IB traffic to diff against
- ⏸ **M3 spike** (captured session for stylized-facts validation): waiting on you — same IB session can produce this

## What you need

1. Active IB paper-trading account (paper is fine — we never touch `live_allowed`)
2. TWS or IB Gateway installed and logged in on a Windows box
3. One ~45-minute session during US market hours (09:30–16:00 ET) for both captures at once
4. Python 3 or a Windows equivalent for the capture CLI (technically just the sim binary — no Python needed)

## Two captures, one session

### Capture 1: M3 spike

**Purpose**: the synthetic generator's stylized-facts tests run against a real IB session to confirm cross-engine parity, not just internal consistency.

**Command** (run on the Windows box where TWS is logged in):

```powershell
# Make sure TWS paper gateway is accepting connections on 7497 (default)
# Start the sim in proxy mode — it listens on 7498, forwards to real IB on 7497, records everything
cargo run --release -p midas-ib-sim --bin midas-ib-sim-server -- `
    --port 7498 `
    --proxy-to 127.0.0.1:7497 `
    --record data\m3_spike
```

**Then, in a separate terminal**, connect *any* IB client to port **7498** (not 7497). That client subscribes to:

- AAPL (streaming L1)
- SPY (streaming L1)
- One small-cap of your choice (streaming L1) — this is the "Illiquid" preset target

Leave it running for ≥ 30 minutes. Sim records both the pcap (wire bytes) and a `.dbn` (decoded market data).

Stop with Ctrl-C.

**Output files** (before anonymization — `.gitignore`d):

- `data/m3_spike.tws.pcap` — full wire capture
- `data/m3_spike.dbn` — decoded market data

### Capture 2: Error-code validation

**Purpose**: Stage 05 quirks has 4 error codes marked `[unverified]` in `crates/midas-ib-sim/src/quirks/error_codes.rs`. Capturing one real instance of each confirms the constants are correct.

Still in the proxy capture (same session), trigger:

1. **Line-cap overflow** — your client subscribes 101+ symbols rapidly. Expected error: **10197**.
2. **Historical pacing violation** — fire 61 `reqHistoricalData` requests in 10min. Expected error: **162**.
3. **Msg-rate violation** — send 51 requests in 1 second. Expected error: **100** + disconnect.
4. **Client-id conflict** — open a second client with the same `clientId`. Expected error: **326**.

Each fires into the same pcap capture. No need to anonymize between triggers.

### Capture 3: Parity gate (12 rows)

Run each of these against **real IB** (port 7497), then the same against **our sim** (port 7498), and diff the event streams. The test harness is already in `desktop/win/tests/app_sim_e2e.rs` — extend it with 12 parametrized cases or run manually via devloop commands.

| # | Test | Real IB | Sim | Parity? |
|---|------|---------|-----|---------|
| 1 | Connect + MANAGED_ACCTS + NEXT_VALID_ID | | | |
| 2 | Farm-status bulletins within 500ms | | | |
| 3 | reqCurrentTime round-trip | | | |
| 4 | reqContractData for AAPL returns conId | | | |
| 5 | reqMktData streaming, 60s of ticks | | | |
| 6 | Market order fills, OrderStatus + Execution arrive | | | |
| 7 | Limit order rests, cancels cleanly | | | |
| 8 | Bracket: parent + TP + SL full lifecycle | | | |
| 9 | reqHistoricalData returns 1-day of minute bars | | | |
| 10 | reqPositions returns snapshot | | | |
| 11 | 50 msg/sec burst → error 100 + disconnect | | | N/A-real (can't trigger on demand) |
| 12 | 101st reqMktData → error 10197 | | | N/A-real (requires exact state) |

## Anonymize before committing

Before any pcap/dbn lands in git:

```powershell
cargo run -p midas-ib-sim --bin midas-ib-sim -- `
    anonymize data\m3_spike.tws.pcap `
    --out fixtures\sessions\m3_spike.anon.tws.pcap

cargo run -p midas-ib-sim --bin midas-ib-sim -- `
    anonymize data\m3_spike.dbn `
    --out fixtures\sessions\m3_spike.anon.dbn
```

Then verify against the pre-commit hook:

```powershell
bash tools\pre-commit-anonymize.sh fixtures\sessions\
```

Must exit 0 (no un-anonymized patterns).

## Updating the error-code constants

Open `crates/midas-ib-sim/src/quirks/error_codes.rs`. For each `[unverified]` constant whose code matched reality:

- Flip the comment from `// [unverified]` to `// VERIFIED (m3_spike session, <date>)`.

For any that didn't match (unlikely but possible):

- Update the constant value to what real IB emitted.
- Flip the comment.

Run `cargo test -p midas-ib-sim --test quirks_e2e` to confirm the fixtures still pass with the new constants.

## Reporting back to Claude

Once the captures are anonymized and committed, ping me with:

- "Parity gate ran: N/12 rows matched" (ideally 10/12 or better; the 2 marked N/A-real are OK to skip)
- Branch name of your commit
- Any error codes that turned out different from what Stage 05 assumed

I'll then run the final M9 production-readiness review with captured data on hand.

## Fallback if this is too much

Skip it all. The arc ships as "validated against internal scenarios + synthetic data; real-IB parity verified by spot-check during M9." The sim is already usable for off-market development. The parity gate is the highest-quality-bar pass; it's not a blocker for first use.
