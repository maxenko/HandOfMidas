# Research: Market Microstructure Models for Synthetic Tick Generation

*Source: parallel research agent, 2026-04-18.*

## TL;DR

- **Volatility clustering, bid-ask bounce, and intraday U-shape are the three "stylised facts" the sim MUST show** — without them, latency and fill-assumption bugs hide in testing.
- **Start with Roll bounce + GARCH(1,1) + piecewise-constant intensity by time-of-day**, driving arrival times from a Hawkes-lite (exponentially decaying kernel). ~500 LOC of Rust, covers ~80% of the realism gap.
- **Don't build an order book.** If the app consumes L1 (bid/ask/last/size), synthesise only those four streams. Full LOB dynamics are a rabbit hole.
- **Hybrid > pure synthetic.** Replay one captured IB session per symbol, then perturb with synthetic jumps/gaps to stress edge cases.
- **Validate with 4 tests**: return autocorrelation near zero, squared-return autocorrelation > 0.1 at lag 1, return distribution kurtosis > 4, volume U-shape visible intraday.

## Tier 1 / 2 / 3 Property Table

| Tier | Property | Why it matters for a UI/trading app |
|---|---|---|
| **1** | Volatility clustering | Risk sizing, stop-loss placement, chart "feel" |
| **1** | Bid-ask bounce on last-trade | Last-price UI jitter; strategies that read `last` misbehave without it |
| **1** | Heavy-tailed returns (kurtosis > 3) | Risk UIs and alerts need to fire on fat tails |
| **1** | Intraday U-shape (volume + spread) | Open/close logic, VWAP widgets, spread badges |
| **2** | Tick clustering (Hawkes-like bursts) | Throughput testing; subscription fan-out under load |
| **2** | Return mean-reversion at 1-5 tick lag | Arising from bounce; strategies see false signals without it |
| **2** | Overnight gaps / open jumps | Gap-detection UI, order-carry-over logic |
| **2** | Volume-volatility correlation (Clark 1973) | Heatmap/activity widgets |
| **3** | Long-memory in volatility (FIGARCH) | Academic; not visible in intraday UI testing |
| **3** | Leverage effect (asymmetric vol response) | Relevant for risk models, not UI |
| **3** | Full LOB depth dynamics | Skip unless building an execution sim |
| **3** | Exact microstructure noise decomposition | Overkill |

## Recommended Starting Model: "Roll-GARCH-U"

~500 LOC Rust, three composable layers:

1. **Arrival clock** — Non-homogeneous Poisson with λ(t) = λ_base × u_volume(t) × (1 + excitement), where `excitement` decays exponentially after each trade (half-life ~2s). Hawkes-lite.
2. **Price process** — Log-return `r_t = σ_t × ε_t + J_t`, where σ²_t follows GARCH(1,1) and J_t is a rare jump (probability ~1e-5 per tick). ε_t drawn from Student-t with ν=4 for fat tails.
3. **Observed price** — Efficient price `m_t` plus Roll bounce: `last_t = m_t ± s(t)/2` with direction from a biased coin. Bid = m − s/2, Ask = m + s/2; spread s(t) from the U-shape table.

Volume per trade: log-normal with mean scaled by u_volume(t). Volume-volatility correlation for free via the shared intensity channel.

## Parameter Sourcing

| Approach | When |
|---|---|
| **Published rules-of-thumb** | Default. GARCH (ω=1e-6, α=0.08, β=0.9) from Andersen et al. (2003). Half-spread for SPY ≈ $0.005, AAPL ≈ $0.01, small-caps $0.02–$0.10. U-shape from Hasbrouck (2007) ch. 8. |
| **LOBSTER / NYSE TAQ** | If per-symbol calibration wanted. LOBSTER offers free NASDAQ samples; TAQ is paid. Fit GARCH via MLE (~50 LOC with `statrs`). |
| **Per-symbol presets** | Ship 3: `Liquid` (SPY-like), `MidCap` (AAPL-like), `Illiquid` (wide spread, sparse arrivals, high jumps). |
| **Calibration from captured IB** | When a recorded session exists: fit σ_base, λ_base, s from that session, keep GARCH/U-shape defaults. |

## Replay vs Synthetic Decision Tree

- **Deterministic reproduction of a known bug** → Session replay
- **Stress-testing throughput / UI jitter** → Pure synthetic (crank λ_base, enable burst mode)
- **Rare events (halts, gaps, flash crashes)** → Hybrid: replay base + inject synthetic jumps/halts at scripted timestamps
- **Regression suite** → Fixtures (seeded synthetic, committed as JSONL)

**Industry practice:** Two Sigma, Jane Street, HRT mostly replay captured data (Kissell 2013 ch. 12); synthetic generators are for load testing, UI dev, and agent-based research (JPMorgan's ABIDES — arXiv:1904.12066 — is the reference open-source LOB sim). For a UI-focused tool, lean hybrid.

## Validation Tests

Run after every generator change; fail the build if any regresses:

1. **Return autocorrelation** `|ρ(r_t, r_t+1)| < 0.05` beyond lag 2 (bounce allowed at lag 1).
2. **Squared-return autocorrelation** `ρ(r²_t, r²_t+1) > 0.1` (volatility clustering present).
3. **Kurtosis** of 1-minute returns `> 4` (heavy tails).
4. **Ljung-Box on r²** rejects iid at p < 0.01.
5. **Intraday volume U-shape**: first and last 30-min bars each > 1.5× midday mean.
6. **Roll spread estimator** recovers configured half-spread within 10%.

Use `statrs` + a 50-LOC stats module — no external deps.

## Exposed Knobs

`base_volatility`, `base_intensity` (trades/sec), `half_spread`, `jump_rate`, `jump_size_mean`, `garch_persistence` (α+β, default 0.98), `hawkes_excitement` (0–1), `u_shape_strength` (0=flat, 1=full U), `momentum_bias` (-1 mean-reverting to +1 trending), `rng_seed`.

## Scope-Creep Traps

- **Order book depth / queue position** — Only if app consumes L2. IB default is L1.
- **Full multivariate Hawkes** (buy/sell cross-excitation) — 1-D Hawkes with random sign is plenty.
- **Informed-trader models (Glosten-Milgrom, Kyle)** — Invisible in a UI test.
- **Realistic options chain dynamics** — Defer until options trading is scoped.
- **FIGARCH / long-memory volatility** — Undetectable in a UI session.
- **Tick-size / sub-penny rules** — Round to symbol's tick, done.
- **Exchange-specific quirks** (auction imbalance, reg-SHO) — Out of scope for IB L1 sim.
- **Realistic latency simulation** — Separate concern; model in transport layer, not tick generator.

## Key References

- Roll (1984) "A Simple Implicit Measure of the Bid/Ask Spread" JoF 39(4). DOI:10.1111/j.1540-6261.1984.tb03897.x
- Bollerslev (1986) "Generalized Autoregressive Conditional Heteroskedasticity" JoE 31(3).
- Bacry, Mastromatteo, Muzy (2015) "Hawkes Processes in Finance" arXiv:1502.04592
- Hasbrouck (2007) *Empirical Market Microstructure*, Oxford UP.
- O'Hara (1995) *Market Microstructure Theory*, Blackwell.
- Byrd, Hybinette, Balch (2019) "ABIDES: Towards High-Fidelity Market Simulation" arXiv:1904.12066
- Cont (2001) "Empirical properties of asset returns: stylized facts and statistical issues" *Quant Finance* 1(2). DOI:10.1080/713665670
