# Stage 03 — Market Data Engine

*Synthetic Roll-GARCH-U generator + Databento `dbn` replay + hybrid mode. Produces tick events that the protocol layer encodes as `TICK_PRICE` / `TICK_SIZE` / `TICK_STRING` / `REAL_TIME_BARS` / `HISTORICAL_DATA`.*

**Depends on**: 01 (scaffold), 08 (clock)
**Blocks**: 09 (integration)
**Parallel-safe with**: 04, 05, 06, 07

## Scope

For every subscribed `(SessionId, ReqId, ContractSpec)` subscription, drive a tick stream that:

- Respects virtual time (deterministic in CI, real-time in dev)
- Produces realistic microstructure (see [research/microstructure-models.md](research/microstructure-models.md))
- Supports three sources: `synthetic`, `replay`, `hybrid`
- Emits the right event types per subscription mode (`tick-by-tick`, `streaming L1`, `real-time 5s bars`, `historical`)

## Public API

```rust
pub trait MarketDataEngine: Send {
    fn subscribe(&mut self, key: SubKey, mode: SubMode) -> Result<(), MarketDataError>;
    fn unsubscribe(&mut self, key: SubKey);
    fn step(&mut self, now: VirtualInstant) -> Vec<MarketEmission>;
    fn snapshot(&self, symbol: &SymbolKey) -> Option<Snapshot>;
}

pub struct SubKey { pub session: SessionId, pub req_id: ReqId, pub symbol: SymbolKey }
pub enum SubMode { StreamingL1, TickByTick, RealtimeBars5s, Historical(HistoricalReq) }

// MarketEmission is defined in crate::engine (Stage 01 §Central types).
// Re-declared here for reference; the authoritative type lives in engine/types.rs.
pub enum MarketEmission {
    TickPrice { key: SubKey, tick: TickType, price: f64, size: Option<i64>, attribs: TickAttribs },
    TickSize { key: SubKey, tick: TickType, size: i64 },
    TickString { key: SubKey, tick: TickType, value: String },
    TickGeneric { key: SubKey, tick: TickType, value: f64 },
    Bar { key: SubKey, bar: Bar5s },
    HistoricalBatch { key: SubKey, bars: Vec<Bar>, is_complete: bool },
}
```

Three implementors — `SyntheticEngine`, `ReplayEngine`, `HybridEngine` — fronted by a `MarketDataEngine` dispatcher selected by CLI flag.

## Synthetic engine — Roll-GARCH-U

Per-symbol state:

```rust
pub struct SymbolState {
    // Price process
    pub mid_price: f64,                // efficient price m_t
    pub log_mid: f64,                  // ln(m_t) for log-normal GARCH
    pub garch: GarchState,             // σ²_t, σ²_{t-1}, r_{t-1}
    pub last_tick: VirtualInstant,

    // Arrival process
    pub lambda_base: f64,              // baseline trades/sec
    pub excitement: f64,               // Hawkes kick, decays exponentially
    pub next_arrival: VirtualInstant,

    // Observed price
    pub half_spread: f64,              // s/2 from U-shape table * preset
    pub last_side: Side,               // for bounce direction bias

    // Volume
    pub volume_mean_log: f64,          // log-normal volume per trade

    // Knobs
    pub preset: SymbolPreset,          // Liquid | MidCap | Illiquid
    pub rng: SmallRng,
}
```

Per-tick step (called each time virtual time advances past `next_arrival`):

```rust
fn generate_tick(&mut self, now: VirtualInstant) -> Vec<MarketEmission> {
    // 1. Update σ²_t from GARCH(1,1)
    let vol = self.garch.step(self.rng.gen::<f64>());

    // 2. Sample ε_t from Student-t(ν=4)
    let eps = sample_student_t(&mut self.rng, 4.0);

    // 3. Rare jump
    let jump = if self.rng.gen::<f64>() < self.jump_rate { sample_jump_size(&mut self.rng) } else { 0.0 };

    // 4. Log-return
    let r = vol.sqrt() * eps + jump;
    self.log_mid += r;
    self.mid_price = self.log_mid.exp();

    // 5. Observed last with Roll bounce
    let spread = self.half_spread * u_shape_multiplier(now);
    let bias = self.momentum_bias + (self.last_side.as_f64() * -0.3);
    let side = if self.rng.gen::<f64>() < 0.5 + bias { Side::Buy } else { Side::Sell };
    let last_price = self.mid_price + side.as_f64() * spread;
    let bid = self.mid_price - spread;
    let ask = self.mid_price + spread;

    // 6. Volume
    let volume = sample_lognormal(&mut self.rng, self.volume_mean_log * u_shape_multiplier(now), 0.6);

    // 7. Update Hawkes excitement
    let dt = (now - self.last_tick).as_secs_f64();
    self.excitement *= (-dt / HAWKES_HALF_LIFE.as_secs_f64() * 2.0_f64.ln()).exp();
    self.excitement += 1.0;

    // 8. Schedule next arrival from non-homogeneous Poisson
    let lambda = self.lambda_base * u_shape_multiplier(now) * (1.0 + self.excitement);
    let dt_next = sample_exponential(&mut self.rng, lambda);
    self.next_arrival = now + Duration::from_secs_f64(dt_next);
    self.last_tick = now;
    self.last_side = side;

    // 9. Emit ticks (subset depending on subscription mode)
    vec![
        MarketEmission::TickPrice { key: .., tick: TickType::Last, price: last_price, size: Some(volume as i64) },
        MarketEmission::TickPrice { key: .., tick: TickType::Bid, price: bid, size: None },
        MarketEmission::TickPrice { key: .., tick: TickType::Ask, price: ask, size: None },
        MarketEmission::TickSize { key: .., tick: TickType::Volume, size: cumulative_volume },
    ]
}
```

Constants from the research (see [microstructure-models.md](research/microstructure-models.md)):
- `GARCH(ω=1e-6, α=0.08, β=0.9)` — defined over a **1-second sampling grid** (see time-basis note below)
- Student-t ν=4 for innovations
- `HAWKES_HALF_LIFE = 2 seconds`
- U-shape table: 13 half-hour multipliers, 09:30–16:00 ET

### Time-basis discipline: GARCH on fixed grid, not per-arrival

GARCH parameters are defined over a *fixed sampling interval* (e.g., 1-second log-returns). If you mix GARCH with a per-event Hawkes arrival clock naively, the effective persistence `α+β` drifts with `λ(t)` — stylized-fact validation test 2 (squared-return autocorrelation) will silently fail when `λ_base` is changed. This is a well-known pitfall in ACD-GARCH literature (Engle & Russell 1998).

**The fix** — decouple the GARCH step from tick arrivals:

```rust
pub struct SymbolState {
    pub garch: GarchState,
    pub garch_next_step: VirtualInstant,     // advances on a fixed 1-second grid
    // ...
}

fn generate_tick(&mut self, now: VirtualInstant) -> Vec<MarketEmission> {
    // 1. Advance GARCH along its own fixed grid, independent of ticks.
    //    eps_grid and eps_tick are drawn INDEPENDENTLY by design — see §Innovation-stream note.
    while self.garch_next_step <= now {
        let eps_grid = sample_student_t(&mut self.grid_rng, 4.0);
        self.garch.step(eps_grid); // updates σ²_t at 1-second cadence
        self.garch_next_step += GARCH_GRID_INTERVAL;  // = 1 second
    }

    // 2. Per-tick innovation: scale the grid sigma by sqrt(dt).
    let dt = (now - self.last_tick).as_secs_f64();
    let sigma_per_tick = self.garch.sigma().sqrt() * dt.sqrt();
    let eps_tick = sample_student_t(&mut self.tick_rng, 4.0);
    let r = sigma_per_tick * eps_tick + jump;

    // ... rest of the tick generation as before
}
```

This keeps GARCH's persistence invariant regardless of arrival intensity. Two independent RNG streams (`grid_rng` vs. `tick_rng`), both seeded from the symbol + master seed at session start, so changing `λ_base` doesn't shift the GARCH realization.

### Innovation-stream note (independence is deliberate)

`eps_grid` (used once per second to advance GARCH) and `eps_tick` (used per-arrival for tick returns) are **independent draws**, not the canonical Brownian-bridge form that would scale the *same* innovation across grid and tick timescales. This is a deliberate design choice:

- **What the canonical form does**: `r_tick = sigma_grid * sqrt(dt) * eps_bridge` where `eps_bridge` is a single draw whose per-tick increments sum (in distribution) to the grid-level innovation. This exactly matches GARCH moments at both scales.
- **Why we don't do that**: the tick arrival times are non-equidistant and driven by Hawkes intensity. A proper Brownian bridge between grid points conditioned on arrival times is mathematically clean but adds ~200 LOC of conditional-distribution sampling for a benefit that doesn't surface in UI testing.
- **What we accept**: 1-minute aggregated tick returns have slightly higher kurtosis than a hypothetical bridge-based model would produce (independent-sum of Student-t draws has fatter tails than a bridged draw). Validation test #3 (kurtosis > 4) still passes with margin; if it ever failed for a model refinement, we'd revisit.
- **The role of GARCH on the grid**: to maintain `α+β` persistence in the *grid-level* return series. That's what the stylized-facts test #2 measures (squared-return autocorrelation). It is NOT to produce tick-level returns that marginalize back to the grid. We only need the former.

Put differently: GARCH on the grid is the *variance process*, and per-tick innovations are *samples from that variance*. They're independently sampled Student-t draws whose variance at any instant matches the grid process. That's the right shape for our goal (correct volatility clustering in aggregated returns); it's not the right shape for a derivatives-pricing model that needs moment consistency across scales — but we're not a derivatives-pricing model.

**Validation hook**: add a stylized-facts test that varies `λ_base` across 3 values (0.5, 5.0, 50.0 trades/sec) and asserts `α+β` estimated from the resulting 1-second return series matches the configured persistence within ±0.03. Catches regressions to the per-arrival coupling.

### Presets

```rust
pub enum SymbolPreset {
    Liquid,    // SPY-like: half_spread=0.005, lambda_base=5.0/s, jump_rate=1e-5
    MidCap,    // AAPL-like: half_spread=0.01, lambda_base=2.0/s, jump_rate=2e-5
    Illiquid,  // small-cap: half_spread=0.05, lambda_base=0.2/s, jump_rate=1e-4
}
```

Per-symbol overrides via CLI or scenario YAML.

### Validation tests (CI-blocking)

From [microstructure-models.md](research/microstructure-models.md) — run on 1-hour synthetic sessions with fixed seeds:

1. `|ρ(r_t, r_{t+1})| < 0.05` beyond lag 2
2. `ρ(r²_t, r²_{t+1}) > 0.1` (vol clustering)
3. Kurtosis of 1-min returns `> 4`
4. Ljung-Box on r² rejects iid at p<0.01
5. Intraday U-shape: first and last 30-min bars > 1.5× midday mean
6. Roll spread estimator recovers configured half-spread within 10%

Implemented as `cargo test --package midas-ib-sim --test stylized_facts`.

## Replay engine

Reads a `.dbn` file (Databento format) and replays ticks at virtual time:

```rust
pub struct ReplayEngine {
    reader: DbnReader,
    subs: BTreeMap<SubKey, Option<f64>>, // last emitted price per sub
    clock: Arc<dyn Clock>,
    next_record: Option<DbnRecord>,
}

impl MarketDataEngine for ReplayEngine {
    fn step(&mut self, now: VirtualInstant) -> Vec<MarketEmission> {
        let mut out = Vec::new();
        while let Some(rec) = self.next_record.as_ref() {
            if rec.ts_event > now { break; }
            // Emit for every matching subscription
            for (key, _last) in &self.subs {
                if key.symbol == rec.symbol() {
                    out.extend(rec.to_emissions(key));
                }
            }
            self.next_record = self.reader.next_record().ok().flatten();
        }
        out
    }
}
```

### DBN record types consumed

- `TbboMsg` (tick-by-tick bid/ask, `MboMsg` for L3 if we want it later — we don't)
- `TradeMsg` (executed trades — becomes `TickType::Last`)
- `OhlcvMsg` (minute/daily bars — becomes `HISTORICAL_DATA` batches)

Databento's `dbn` crate handles the binary decode; we map to `MarketEmission` types.

### Session recording hook

When running with `--record path/`, capture the sim's outbound market-data events to a `.dbn` file using the `dbn::Encoder`. Round-trippable — the recorded session is replayable verbatim.

## Hybrid engine

Replays a base session and injects synthetic perturbations at scripted timestamps:

```rust
pub struct HybridEngine {
    base: ReplayEngine,
    perturbations: Vec<Perturbation>, // from scenario YAML
}

pub enum Perturbation {
    InjectJump { at: VirtualInstant, symbol: SymbolKey, magnitude_pct: f64 },
    InjectGap { at: VirtualInstant, symbol: SymbolKey, from: f64, to: f64 },
    InjectHalt { at: VirtualInstant, symbol: SymbolKey, duration: Duration },
    BurstMode { from: VirtualInstant, to: VirtualInstant, multiplier: f64 }, // crank lambda_base × N
}
```

Perturbations are pure post-processing — they mutate the emission stream rather than the underlying price process. Simple, composable, deterministic.

## Historical data response

`REQ_HISTORICAL_DATA` fetches a closed range. Two cases:

1. **Synthetic historical** — fast-forward a fresh `SyntheticEngine` from a seeded past timestamp, emit all bars as one `HistoricalData` message. Seed is `hash(symbol, duration, bar_size, rng_seed)` for determinism.
2. **Replay historical** — query the dbn index for bars in `[start, end]`, batch-emit.

`keep_up_to_date=true` transitions to real-time bar streaming after the historical batch completes.

## Parallelism within this stage

Four sub-teams after `MarketDataEngine` trait + `SubKey` + `MarketEmission` types land (~1 day):

| Sub-team | Scope |
|----------|-------|
| **A** | `generator/` — GARCH, Hawkes, Roll, U-shape (~600 LOC) |
| **B** | `replay/` — dbn reader integration, emission mapping (~400 LOC) |
| **C** | Hybrid engine + perturbations + scenario YAML schema (~300 LOC) |
| **D** | Validation harness — stylized facts tests + fixture generation (~400 LOC) |

All four can merge independently; the dispatcher in `market_data/mod.rs` picks implementor by config.

## Rollback signals

- Synthetic path emits stylized-fact regressions (any validation test fails) → parameter drift; hold at last-known-good and bisect.
- Replay engine drops records during fast-forward → back-pressure bug; buffer in bounded channel, don't block the step loop.
- Hybrid perturbation mutates state beyond its scope (affects later non-perturbed emissions) → perturbations must be pure functions of (base emission, state), never mutate the base engine.

## Kill criteria

- **Validation suite can't hit all 6 stylized-fact thresholds after 2 weeks of tuning** → model is mis-specified; reconsider Cont's stylized facts list, may need to add leverage effect or switch to Heston-style stochastic vol.
- **Single symbol step() exceeds 10µs at 60fps (enough to stall the engine loop at 100 symbols)** → inner loop allocation; profile and remove.

## Deliverables

- `cargo test -p midas-ib-sim --test stylized_facts` green
- `cargo bench -p midas-ib-sim market_data` — single-symbol step < 10µs, 100-symbol dispatch < 1ms
- End-to-end smoke: `rust-ibapi` client connects, `reqMktData(AAPL)`, receives coherent tick stream for 60 seconds of virtual time, prices stay within plausible bounds
- At least 3 canned session fixtures in `fixtures/sessions/` (SPY, AAPL, small-cap)
