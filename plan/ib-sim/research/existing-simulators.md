# Research: Existing TWS Simulators + Adjacent Patterns

*Source: parallel research agent, 2026-04-18.*

## TL;DR

1. **No mature open-source TWS wire-protocol simulator exists.** Every "IB mock" found on GitHub mocks the Python `ib_insync` / `ibapi` *library API* (method-level stubs), not the TCP protocol. Nobody has reverse-engineered the TWS socket protocol into a testable fake gateway. Building one in Rust is defensible.
2. **FIX-world simulators are the real state of the art.** QuickFIX test servers, `fix-simulator`, exchange matching-engine mocks (`libfixengine`, CME iLink mocks) are what serious trading teams use for parity testing. Design patterns transfer: scripted scenarios, replay from pcap, latency injection.
3. **Biggest lesson from post-mortems:** simulators that model "a matching engine" tend to be more realistic than those built to "pass our test suite." Optimize for protocol byte-parity and reject-path coverage, not happy-path order fills.

## 1. Landscape — IB-specific mocks

| Project | Lang | Level | Scope | Replay | Fault inj. | Last commit | Stars | Notes |
|---|---|---|---|---|---|---|---|---|
| [`ib_insync`](https://github.com/erdewit/ib_insync) test suite | Python | Library | Both | No | No | 2024 (archived) | 2.8k | Uses live paper gateway, no mock. Author acknowledged this as a weakness. |
| [`mock-ib-gateway`](https://github.com/tradingsimulator/mock-ib-gateway) | Python | Library | Orders | No | Minimal | 2019 | ~40 | Abandoned. Stubs `IBApi.EClient` methods, returns canned fills. |
| [`ibapi-mock`](https://github.com/jeff-99/ibapi-mock) | Python | Library | Market data only | CSV replay | No | 2020 | ~15 | Subclass-based mocking of `EClient`. |
| [`ib-gateway-docker`](https://github.com/UnusualAlpha/ib-gateway-docker) | shell | Neither | N/A | N/A | N/A | 2024 | 700+ | Wraps real gateway in Docker — often confused with a mock; it isn't. |
| `twsmock` | — | — | — | — | — | — | — | Name appears in forum posts; no live repo found. |
| `IBKR-Tradingbot-mock` forks | Python | Library | Orders | No | No | mixed | <10 each | Toy-grade `unittest.mock` wrappers. |
| [`rust-ibapi` tests](https://github.com/wboayue/rust-ibapi) | Rust | Library | Both | Fixture strings | No | 2025 | 150+ | **Closest to what we want** — has captured wire bytes used in unit tests via a `MessageBus` mock. Not a standalone simulator, but the byte fixtures are reusable. |
| [`IBJts` sample code](https://interactivebrokers.github.io/) | Java/C++ | — | N/A | N/A | N/A | Current | — | IB's own test samples assume real gateway. |

Key finding: **every IB "mock" is library-level.** They monkey-patch `EClient` methods. None speaks the TWS binary framing, version handshake, or the `\0`-delimited field protocol. `rust-ibapi` is the only codebase with usable captured wire fixtures.

## 2. Commercial / broker-agnostic reference points

- **IB Paper Trading / Gateway** — real gateway connecting to IB's simulated matching engine. High fidelity (same protocol) but network-bound, non-deterministic, rate-limited, and IB resets accounts periodically. Cannot be used in CI.
- **QuantConnect / LEAN** ([github.com/QuantConnect/Lean](https://github.com/QuantConnect/Lean)) — has a `PaperBrokerage` and `BacktestingBrokerage`. Models fills at next-tick close with configurable slippage; does *not* model IB-specific error codes, rate limits, or partial-fill granularity. C#, 10k+ stars.
- **Backtrader** ([backtrader.com](https://www.backtrader.com/)) — `BackBroker` simulates with user-chosen fill model. Zero broker-protocol fidelity; it's a portfolio accountant, not a TWS sim.
- **Alpaca paper API** — live REST/WebSocket endpoint; closest commercial analog to IB paper. Open: protocol is public and simple, unlike TWS.
- **Rithmic Test / CQG Demo** — gated behind vendor NDAs; not usable as reference.

None reach protocol-parity. Industry convention is "test against real paper gateway and pray."

## 3. Adjacent patterns — FIX and crypto

- **QuickFIX acceptor in test mode** ([quickfixengine.org](https://www.quickfixengine.org/)) — canonical pattern: a scriptable acceptor that consumes an `.scen` file of expected messages and responds per script. Deterministic, replayable, widely copied.
- **`fix-simulator`** — Java, reads scenarios, injects RejectMessage / SessionLogout faults.
- **Binance testnet** — separate host, same WS protocol. Deterministic for a given seed account but orderbook is adversarial.
- **CME iLink mock servers** (commercial, e.g., Exegy, Vela) — go to extraordinary lengths: per-message latency distributions, intentional sequence gaps, throttle rejects. The gold standard.
- **Nasdaq ITCH/OUCH replayers** (LOBSTER, `itch-tools`) — byte-for-byte replay of the real exchange feed from `.pcap.gz` captures.

**Takeaway for our sim:** adopt QuickFIX's scenario-script idea (YAML or s-expr) + CME-vendor's explicit reject taxonomy.

## 4. Recorded-data replay — format landscape

- **LOBSTER** (TU Berlin / [lobsterdata.com](https://lobsterdata.com/)) — CSV, two files per symbol. De facto academic standard for LOB research.
- **NYSE TAQ / Databento** — Parquet + their own binary. Databento's [`dbn`](https://github.com/databento/dbn) format is well-documented, Rust-native, and the best modern choice: schema'd, zstd-compressed, streaming-friendly.
- **Apache Arrow IPC / Parquet** — generic, column-oriented. Good for analytics, awkward for strict temporal replay.
- **pcap** of the real TWS socket — if we record from a live paper gateway, this is our ground truth.

**Recommendation:** consume Databento `dbn` for historical ticks (Rust crate exists, MBO granularity), and record our own `.pcap` of TWS paper sessions as a protocol corpus. Emit synthetic streams as `dbn` too — free interop with Polars / DuckDB / arrow-rs.

## 5. Synthetic tick generation — literature

- **Hawkes processes** for order-arrival clustering: Bacry, Mastromatteo, Muzy — ["Hawkes processes in finance"](https://arxiv.org/abs/1502.04592) (2015). Canonical reference.
- **Queue-reactive model** — Huang, Lehalle, Rosenbaum, [arXiv:1312.0563](https://arxiv.org/abs/1312.0563). Order book dynamics as function of queue imbalance.
- **Stylized-facts GANs** — Wiese et al., [arXiv:1907.06673](https://arxiv.org/abs/1907.06673). Overkill for a test double.
- **GARCH(1,1)** for volatility clustering — ubiquitous; [`arch`](https://github.com/bashtage/arch) Python package, trivially portable.
- **QuantLib** — has Heston / SABR but aimed at derivatives pricing, not tick generation.

**Recommendation for MVP:** two-layer model — GBM for mid-price drift + GARCH for vol clustering + Hawkes arrivals for trade timing + fixed bid-ask spread with bounce. Minimum that produces plausible microstructure. Upgrade to queue-reactive only if realistic L2 is needed.

## 6. Anti-patterns (lessons learned)

1. **"Passes sim, breaks in prod" from optimistic fills.** Backtrader / LEAN fill at next bar open, zero slippage by default — strategies tuned against this lose money live. **Fix:** default fill model is pessimistic (touch+cross + configurable latency); partial fills / rejects / cancellations from day one.
2. **Mocking the library, not the protocol** (every Python IB mock). Tests pass, bugs in framing / sequence / handshake escape. **Fix:** speak the TWS wire protocol. Library mocks are a trap.
3. **Ignoring the reject taxonomy.** IB's error space (2100 warnings, 200/201/202 order rejects, pacing violations) is where real integration bugs hide. **Fix:** enumerate reject codes + make each injectable by scenario.
4. **Wall-clock coupling.** `sleep(tick_interval)` can't run faster than real-time, defeating CI use. **Fix:** virtual clock from day one; real-time is a debug affordance, not the default.
5. **Non-determinism via HashMap iteration / thread scheduling.** Several LEAN issues trace to this. **Fix:** `BTreeMap`, fixed seeds, single-threaded event loop behind the protocol boundary.

## Closest reference to copy

**QuickFIX acceptor + `rust-ibapi` wire fixtures.** Use QuickFIX's scenario-script design, implement on top of TWS framing, seed with captured byte fixtures from `rust-ibapi` test suite. Avoid the Python-mock lineage entirely.
