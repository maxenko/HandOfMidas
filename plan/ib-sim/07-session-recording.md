# Stage 07 — Session Recording

*Capture real IB paper-gateway sessions into a deterministic, replayable format. The corpus is the ground truth for both regression testing and synthetic-model calibration.*

**Depends on**: 02 (protocol — for wire byte capture)
**Blocks**: 09 (integration — replay sessions used in CI fixtures)
**Parallel-safe with**: 03, 04, 05, 06

## Scope

Two distinct recording modes:

1. **Wire-level** (`.tws.pcap`) — raw TCP byte stream to/from a real IB gateway. Reproduces the exact wire behavior, including quirks we didn't think to model. For low-level regression testing.
2. **Domain-level** (`.dbn` + `.scenario.yaml`) — decoded market data in Databento format + a scenario script that describes client actions. For high-level replay against the synthetic engine.

Both formats are stored in `fixtures/sessions/` and version-controlled via Git LFS for larger files.

## Wire-level capture

### Architecture

```
┌──────────┐     ┌──────────────────────────┐     ┌─────────────┐
│ ibapi    │────▶│ midas-ib-sim (proxy mode)│────▶│ real IB TWS │
│ client   │◀────│                          │◀────│ gateway     │
└──────────┘     └──────────────────────────┘     └─────────────┘
                              │
                              ▼
                      [recording to disk]
                      tws_session_YYYYMMDD_HHMMSS.pcap
                      tws_session_YYYYMMDD_HHMMSS.dbn
```

**Proxy mode**: `midas-ib-sim-server --proxy-to tws.paper:7497 --record out/session`

- Accepts client connection on our port
- Opens an upstream connection to real IB
- Forwards bytes verbatim in both directions
- **Records every frame** to two files:
  - `.pcap` — raw bytes with timestamp + direction (in/out)
  - `.dbn` — decoded market data messages only (for replay engine ingestion)

### Capture format

The `.pcap` here is our own format (not libpcap) for TWS framing:

```rust
#[repr(C)]
pub struct TwsPcapHeader {
    pub magic: [u8; 4],          // "TWSC"
    pub version: u16,             // 1
    pub server_version_neg: u16,  // negotiated version
    pub start_ts_nanos: i128,     // wall clock at capture start
}

#[repr(C)]
pub struct TwsPcapRecord {
    pub ts_nanos_since_start: u64,
    pub direction: u8,            // 0 = client→sim, 1 = sim→client
    pub flags: u8,                // reserved
    pub len: u32,
    // followed by `len` bytes of raw wire data
}
```

Append-only; zstd-compressed at rest. Cheap to produce (tokio tee), cheap to consume.

### Replay mode

`midas-ib-sim-server --replay session.pcap`

Replays wire bytes deterministically:
- Emits `sim→client` bytes at their recorded `ts_nanos_since_start` on virtual time
- **Validates** `client→sim` bytes against the recorded bytes (configurable: strict / best-effort / ignore)
- In strict mode, any mismatch logs + halts — useful for testing that a client update still produces identical requests
- In best-effort mode, bytes are consumed and their existence is checked, but exact equality isn't enforced — useful for replaying a scenario against a client that has been updated

### Anonymization

Captured sessions contain account numbers, perm IDs, exec IDs, and P&L data. Before committing to git, every capture must pass the anonymization pipeline.

- `midas-ib-sim anonymize session.pcap --out anon.pcap`
- Replaces every occurrence of configured patterns with stable synthetic values:
  - Account codes (e.g. `DU1234567` → `DU0000001`)
  - Perm IDs (reassigned from a synthetic counter, preserving uniqueness)
  - Exec IDs (synthetic hashes of the original)
  - Order IDs (only if `--strip-order-ids` set; default preserves for replay determinism)
  - Cash balances, equity, realized/unrealized P&L → rounded to whole dollars then scaled to a synthetic range
- **Deterministic**: same real account → same synthetic code across all files (keyed by a repo-committed salt so two anonymizations of the same capture produce identical output)
- Anonymization config lives at `fixtures/sessions/anonymize.config.yaml` — PR-reviewed when patterns change

### Data lifecycle policy

**Classifications**:

| Path | Classification | Allowed locations | Retention |
|------|----------------|-------------------|-----------|
| `fixtures/sessions/raw/` | **Sensitive** — contains real account data | Local disk only; `.gitignore`'d | Purge after 30 days (auto) |
| `fixtures/sessions/*.pcap` (not in raw/) | **Anonymized** — committed to git | Git repo, Git LFS for >10 MB | Indefinite |
| `~/.local/share/midas-ib-sim/captures/` | Sensitive (same as raw/) | Local only | Purge after 30 days (auto) |
| Network transmission of raw captures | **Forbidden** | Never | N/A |

**Enforcement**:

1. **Pre-commit hook** (installed via `tools/install-hooks.sh`): scans staged files for patterns that look like un-anonymized account codes (regex for `DU\d{7}` where `\d` is any digit, excluding the range `DU0000000–DU0000999` which is reserved for synthetic). Rejects commit if matched. Hook bypass is logged (`--no-verify` usage logged to `tools/hook-bypasses.log` and PR-reviewed).
2. **CI check**: separate CI job greps the committed tree for the same patterns, fails the build if found. Catches commits that slipped past the hook (different dev machine, `--no-verify`, etc.).
3. **Auto-purge**: `tools/sessions-purge.sh` runs weekly via cron (user opt-in during setup); deletes `raw/` files older than 30 days. The user runs this; the tool doesn't touch it without consent.

**Remediation runbook** (if a raw capture is accidentally committed):

- **Step 1 — immediate**: revert the commit via `git revert`. Force-push is NOT authorized by this runbook (would require a user decision).
- **Step 2 — contain**: even after revert, the data is in git history. Assess blast radius: was the commit pushed? For how long? To what audiences?
- **Step 3 — if not pushed**: `git reset --hard <good commit>` to drop it from local history before push; this is the one case where destructive git is acceptable, because the data has not left the machine.
- **Step 4 — if pushed**: coordinate a full history rewrite (`git filter-repo` or `bfg`) — requires all collaborators' buy-in and a coordinated force-push window. This is a weekend operation, not a casual fix.
- **Step 5 — rotate**: if account codes leaked to a public repo, rotate the account (real impact) per IB's account-change process.
- **Step 6 — post-mortem**: update the anonymization config to catch the missed pattern; update the pre-commit hook; add a regression test.

The runbook lives at `tools/SECURITY-INCIDENT.md` and is linked from the repo README.

**Authorization to leave the machine**: raw captures may never be uploaded to cloud storage, sent via email, posted in chat (Slack/Discord/etc.), or shared in support tickets. If support requires a capture, it must be anonymized first. This is a policy statement, not a technical control — auditable via the purge log if someone copies out of `raw/`.

`fixtures/sessions/` only contains anonymized captures. A `.gitignore`'d `fixtures/sessions/raw/` directory for local-only captures.

## Domain-level capture

### Producing `.dbn` from live

While proxying, decode every market-data event (TICK_PRICE, TICK_SIZE, HISTORICAL_DATA, etc.) and write to `.dbn` using `dbn::Encoder`. This produces a file the `ReplayEngine` (Stage 03) can consume directly.

### Producing `.scenario.yaml` from live

The proxy watches client-issued commands (PLACE_ORDER, REQ_MKT_DATA, etc.) and emits a scenario template:

```yaml
version: 1
name: "Captured session 2026-04-18 09:30-10:00"
seed: 42
clock: virtual
source_dbn: "captured_20260418_093000.dbn"

events:
  - at: 00:00:00.250
    do: subscribe_market_data
    args: { symbol: AAPL, subscription: streaming_l1 }
  - at: 00:00:03.110
    do: accept_order
    args: { order_kind: limit, side: buy, quantity: 100, limit_price: 174.50 }
  # ...
```

Not a perfect scenario — it's a starting point the human curator shapes into a test. Captures are the raw material; canonical scenarios are the edited artifact.

## Recording tooling

### CLI

```bash
# Live capture against IB paper
midas-ib-sim-server --proxy-to 127.0.0.1:7497 --record out/session_XYZ

# List recordings
midas-ib-sim recordings list fixtures/sessions/

# Inspect
midas-ib-sim recordings show fixtures/sessions/session_XYZ.pcap --summary
midas-ib-sim recordings show fixtures/sessions/session_XYZ.pcap --frames 0..100

# Convert to scenario template
midas-ib-sim recordings to-scenario fixtures/sessions/session_XYZ.pcap \
    --out plan/ib-sim/fixtures/scenarios/derived/session_XYZ.yaml

# Validate a replay against the captured session
midas-ib-sim replay fixtures/sessions/session_XYZ.pcap \
    --strict \
    --client-bin target/debug/midas-app

# Anonymize
midas-ib-sim anonymize fixtures/sessions/raw/session_XYZ.pcap \
    --out fixtures/sessions/session_XYZ.pcap
```

### Library API

```rust
pub struct Recorder {
    pcap_writer: TwsPcapWriter,
    dbn_writer: DbnEncoder,
    start: Instant,
}

impl Recorder {
    pub fn record_client_to_sim(&mut self, bytes: &[u8]) { /* ... */ }
    pub fn record_sim_to_client(&mut self, bytes: &[u8]) { /* ... */ }
    pub fn record_decoded(&mut self, msg: &OutgoingMsg) { /* routes MD to dbn */ }
}

pub struct Replayer {
    pcap_reader: TwsPcapReader,
    mode: ReplayMode,
}

pub enum ReplayMode { Strict, BestEffort, IgnoreClient }
```

## Fixture management

**Shipped with repo (in git)**:
- `fixtures/sessions/smoke_anonymized.pcap` — 5-minute session, handshake + a few orders
- `fixtures/sessions/fast_market_anonymized.pcap` — a volatile period with realistic bursts
- `fixtures/sessions/bracket_flow_anonymized.pcap` — full bracket lifecycle against real IB

**Git LFS (larger files)**:
- Full-day recordings for regression testing

**Never in git**:
- Raw (un-anonymized) captures — `fixtures/sessions/raw/` in `.gitignore`
- Personal account data

## Regression testing with recordings

Each shipped session has an integration test:

```rust
#[tokio::test]
async fn session_smoke_replay_is_deterministic() {
    let sim = Sim::start_in_process_replay("fixtures/sessions/smoke_anonymized.pcap");
    let client = rust_ibapi::Client::connect_async(sim.addr(), 0).await?;

    let mut recorded_outputs = Vec::new();
    while let Some(evt) = client.next_event().await {
        recorded_outputs.push(evt);
    }

    assert_snapshot!("smoke_session_outputs", recorded_outputs);
}
```

Uses `insta` for snapshot testing — first run writes the snapshot; subsequent runs compare byte-exactly.

## Session-derived calibration

An offline tool that analyzes a recorded `.dbn` and emits calibrated synthetic-model parameters:

```bash
midas-ib-sim calibrate fixtures/sessions/fast_market_anonymized.dbn \
    --symbol AAPL \
    --out plan/ib-sim/fixtures/presets/aapl_fast_market.yaml
```

Output is a YAML preset file the synthetic generator can load. This closes the loop between real data and synthetic model.

Calibration routines:
- GARCH(1,1) MLE fit on log-returns (~50 LOC using `statrs`)
- Roll half-spread estimator from observed bid-ask bounce
- Hawkes intensity fit from arrival-time spacing
- U-shape multipliers from per-half-hour volume ratios

## Parallelism within this stage

| Sub-team | Scope | LOC |
|----------|-------|-----|
| **A** | `.pcap` format + writer + reader + replay engine | ~500 |
| **B** | `.dbn` integration (reader wraps Databento crate) | ~200 |
| **C** | Proxy mode + recording CLI + anonymization | ~400 |
| **D** | Calibration routines + preset YAML emitter | ~500 |

## Rollback signals

- Captured session replays produce different outputs across runs → non-determinism; find the `Instant::now()` or `SystemTime::now()` leak.
- `.pcap` format version changes invalidate existing fixtures → versioning discipline broken; freeze v1 and add migration.
- Anonymization tool misses a pattern and leaks personal data to fixtures → add a regex test suite over the anon pass.

## Kill criteria

- **Proxy mode adds > 5ms latency to the round-trip** → bytes are being double-buffered; use `tokio::io::copy_bidirectional` directly.
- **Replay cannot reproduce a known bug from a captured session** → pcap layer is losing information; expand the record format, don't hack around it.

## Deliverables

- Proxy mode working against real IB paper for at least one 30-minute session
- 3 anonymized shipped fixtures in `fixtures/sessions/`
- Calibration tool produces working preset YAMLs for SPY / AAPL / small-cap
- `cargo test -p midas-ib-sim --test session_replay` exercises every shipped fixture
